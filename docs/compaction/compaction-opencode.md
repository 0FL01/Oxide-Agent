# OpenCode Context Compression Reference

**Version**: 1.0  
**Last Updated**: 2026-03-18  
**Scope**: Agent context management and compression mechanisms

---

## Table of Contents

1. [Overview](#overview)
2. [Compaction (LLM Summarization)](#compaction-llm-summarization)
3. [Pruning (Tool Output Removal)](#pruning-tool-output-removal)
4. [Truncation (Output Size Limits)](#truncation-output-size-limits)
5. [Configuration](#configuration)
6. [API Reference](#api-reference)
7. [Examples](#examples)

---

## Overview

OpenCode manages context window through three complementary mechanisms:

| Mechanism      | Purpose                            | Method                  | LLM Required |
| -------------- | ---------------------------------- | ----------------------- | ------------ |
| **Compaction** | Summarize old conversation history | LLM-based summarization | Yes          |
| **Pruning**    | Remove old tool outputs            | Token-based removal     | No           |
| **Truncation** | Limit individual tool outputs      | Size-based cutting      | No           |

### Context Flow

```
User Message → Process → Check Overflow → [Compact if needed] → LLM
                    ↓
              Tool Output → [Truncate if >50KB] → Store to file
                    ↓
              Session End → [Prune old tools] → Replace with placeholder
```

---

## Compaction (LLM Summarization)

### When It Triggers

Compaction activates when:

```
total_tokens >= (model_context_limit - reserved_buffer)

Where:
- total_tokens = input + output + cache_read + cache_write
- reserved_buffer = 20,000 tokens (configurable)
```

### Summary Format

The LLM generates a structured summary following this template:

```markdown
## Goal

[User's objectives]

## Instructions

- [Key instructions given]

## Discoveries

[Notable learnings]

## Accomplished

[Completed work]

- [In progress]
- [Remaining]

## Relevant files / directories

- path/to/file.ts
- path/to/dir/
```

### What Happens to Messages

- **Not deleted** from database
- **Excluded** from future LLM context via `toModelMessages()`
- **Replaced** by single summary message with `summary: true` flag

### Manual Invocation

**CLI**:

```bash
# Via slash command
/compact
/summarize

# Via keybinding (default)
<leader>c
```

**Agent Tool**:

```typescript
{
  "tool": "compact",
  "params": {}
}
```

**REST API**:

```bash
POST /session/{session_id}/summarize
Content-Type: application/json

{
  "providerID": "anthropic",
  "modelID": "claude-3-5-sonnet-20241022",
  "auto": true
}
```

---

## Pruning (Tool Output Removal)

### Algorithm

Pruning protects recent context while removing old tool outputs:

```
Constants:
- PRUNE_MINIMUM = 20,000 tokens (minimum to prune)
- PRUNE_PROTECT = 40,000 tokens (protected window)
- PRUNE_PROTECTED_TOOLS = ["skill"] (never pruned)

Process:
1. Iterate messages backwards from most recent
2. Skip last 2 conversation turns
3. Stop at first summary message
4. Accumulate tool output tokens
5. After 40K protected, mark older as "compacted"
```

### Result Format

Compacted tool outputs are replaced with:

```
[Old tool result content cleared]
```

### Protected Content

The following is **never pruned**:

- Last 2 conversation turns
- First 40K tokens of tool outputs
- Skill tool outputs
- Messages with `summary: true`

---

## Truncation (Output Size Limits)

### Limits

| Limit | Default        | Configurable |
| ----- | -------------- | ------------ |
| Lines | 2,000          | Yes          |
| Bytes | 50 KB (51,200) | Yes          |

### Behavior

When output exceeds limits:

1. **Full content saved** to:

   ```
   {data_dir}/tool-output/tool_{uuid}
   ```

2. **Truncated preview returned** with notice:

   ```
   ...{count} lines truncated...

   Hint: Full output saved to {path}
   ```

3. **Auto-cleanup**: Files deleted after 7 days

### Direction Options

- `head` (default): Keep beginning, truncate end
- `tail`: Keep end, truncate beginning

### Usage Example

```typescript
// Truncate to first 100 lines
const result = await Truncate.output(content, {
  maxLines: 100,
  direction: "head"
})

// Result object
{
  content: "...truncated preview...",
  truncated: true,
  outputPath: "tool-output/tool_abc123"
}
```

---

## Configuration

### Configuration File

**File**: `opencode.json`

```json
{
  "compaction": {
    "auto": true,
    "prune": true,
    "reserved": 20000
  }
}
```

### Options

| Option                | Type    | Default | Description                             |
| --------------------- | ------- | ------- | --------------------------------------- |
| `compaction.auto`     | boolean | `true`  | Enable automatic compaction on overflow |
| `compaction.prune`    | boolean | `true`  | Enable pruning of old tool outputs      |
| `compaction.reserved` | number  | `20000` | Token buffer for compaction safety      |

### CLI Flags

```bash
# Disable auto-compaction
opencode --no-auto-compact

# Disable pruning
opencode --no-prune
```

### Environment Variables

```bash
export OPENCODE_DISABLE_AUTOCOMPACT=1
export OPENCODE_DISABLE_PRUNE=1
```

---

## API Reference

### SessionCompaction

#### isOverflow

```typescript
function isOverflow(input: {
  tokens: {
    input: number
    output: number
    reasoning: number
    cache: { read: number; write: number }
  }
  model: {
    limit: { input: number }
    maxOutputTokens: number
  }
}): Promise<boolean>
```

#### create

```typescript
function create(input: {
  sessionID: string
  agent: string
  model: { providerID: string; modelID: string }
  auto: boolean
  overflow?: boolean
}): Promise<void>
```

#### process

```typescript
function process(input: {
  parentID: string
  messages: MessageV2.WithParts[]
  sessionID: string
  abort: AbortSignal
  auto: boolean
  overflow?: boolean
}): Promise<"continue" | "compact" | "stop">
```

#### prune

```typescript
function prune(input: { sessionID: string }): Promise<void>
```

### Truncate

#### output

```typescript
function output(
  content: string,
  options: {
    maxLines?: number // default: 2000
    maxBytes?: number // default: 51200
    direction?: "head" | "tail" // default: "head"
  },
  agent?: Agent,
): Promise<{
  content: string
  truncated: boolean
  outputPath?: string
}>
```

---

## Examples

### Example 1: Automatic Compaction

**Scenario**: Long conversation exceeds context limit

```
Context: 200K tokens
Model limit: 128K tokens
Buffer: 20K tokens
Usable: 108K tokens

Result: Compaction triggers automatically
```

**What happens**:

1. System detects `200K >= 108K`
2. Creates compaction task
3. LLM summarizes conversation history
4. Old messages excluded from context
5. Summary message added

### Example 2: Manual Compaction

**User action**:

```
/compact
```

**Result**:

```
[Summary message added by compaction agent]

## Goal
Refactor authentication module

## Instructions
- Keep backward compatibility
- Add JWT refresh tokens

## Discoveries
- Current auth uses session cookies
- JWT library already in deps

## Accomplished
✅ Analyzed current auth flow
🔄 Implementing JWT middleware
⏳ Update tests

## Relevant files
- src/auth/middleware.ts
- src/auth/jwt.ts
```

### Example 3: Pruning in Action

**Scenario**: Session with many tool calls

```
Message 1: User asks for file search
  → Tool: grep (output: 500 tokens)

Message 2: User asks to read files
  → Tool: read (output: 2000 tokens)

... 50 more messages with tools ...

Message 53: Current conversation
  → Protected from pruning

Result: Messages 1-40 tool outputs pruned
```

### Example 4: Large Output Truncation

**Tool call**:

```typescript
{
  "tool": "bash",
  "params": {
    "cmd": "find . -type f -exec cat {} \;"
  }
}
```

**Result**:

```
...2,000 lines truncated (showing first 2,000 of 15,000 lines)...

Hint: Full output saved to tool-output/tool_abc123
Use the Task tool to have explore agent process this file
```

---

## Troubleshooting

### Context Still Too Large After Compaction

If compaction cannot reduce context enough:

```
Error: "Conversation history too large to compact - exceeds model context limit"
```

**Solutions**:

1. Start new session
2. Reduce `compaction.reserved` buffer
3. Manually remove old messages

### Pruning Not Working

Check configuration:

```json
{
  "compaction": {
    "prune": true // Must be true or omitted
  }
}
```

Check environment variable:

```bash
echo $OPENCODE_DISABLE_PRUNE  # Should be empty
```

### Truncated Output Missing

Truncated files are auto-deleted after 7 days. Check:

- File still exists in `{data_dir}/tool-output/`
- Path in truncation message

---

## See Also

- Model context limits vary by provider
- Cache tokens count toward context limit
- Skill tool outputs are never pruned (preserve critical info)
