# Context Compaction in Claude Code

## Overview

Claude Code implements automatic context compaction to enable infinite conversation length. The system compresses conversation history by summarizing older messages while preserving critical context.

---

## Mechanisms

### Two Compaction Modes

| Mode | Trigger | Description |
|------|---------|-------------|
| **Auto-compact** | Context reaches 80% threshold | Automatic summarization (toggleable via `/config`) |
| **Manual** | User runs `/compact` command | On-demand compression |

### Summarization Process

1. **API Call**: Conversation history sent to Claude's summarization API
2. **Content Stripping**: PDFs and images removed before API call (to prevent failures)
3. **Image Preservation**: Images kept in summarizer request for prompt cache reuse
4. **Summary Generation**: Full history replaced with condensed summary
5. **Circuit Breaker**: Stops after 3 consecutive auto-compaction failures

### What Gets Preserved vs Removed

**Preserved:**
- Session names and custom titles
- Plan mode state
- Memory files with last-modified timestamps
- Deferred tool schemas (via ToolSearch)
- Skill descriptions
- Images (for cache reuse)

**Removed/Stripped:**
- Heavy progress message payloads
- Large tool results (>50K chars → persisted to disk)
- PDF documents (before API call)
- Full conversation history (replaced with summary)

---

## Context Window Management

### Limits and Thresholds

| Setting | Value | Description |
|---------|-------|-------------|
| Warning threshold | 80% | Auto-compact warning (increased from 60%) |
| Blocking limit | ~98% | Effective context window ceiling |
| Max context (Opus 4.6) | 1M tokens | Max/Team/Enterprise plans |
| Skill budget | 2% of context | Scales with window size |
| Tool result limit | 50K chars | Persisted to disk if exceeded |

### Environment Variables

```bash
# Output token limits
CLAUDE_CODE_MAX_OUTPUT_TOKENS=64000       # Default for Opus 4.6
CLAUDE_CODE_MAX_OUTPUT_TOKENS=128000      # Upper bound for Opus/Sonnet 4.6

# Context window control
CLAUDE_CODE_DISABLE_1M_CONTEXT=1          # Disable 1M context support

# File reading limits
CLAUDE_CODE_FILE_READ_MAX_OUTPUT_TOKENS   # Override default
```

---

## Plugin Hooks

### PreCompact Hook

Executes **before** context compaction. Use to add critical information to preserve.

```json
{
  "PreCompact": [
    {
      "matcher": "*",
      "hooks": [
        {
          "type": "prompt",
          "prompt": "Add critical project context to preserve during compaction: current branch, pending tasks, important decisions, TODO items."
        }
      ]
    }
  ]
}
```

### PostCompact Hook

Executes **after** compaction completes (added in v2.1.76).

```json
{
  "PostCompact": [
    {
      "matcher": "*",
      "hooks": [
        {
          "type": "command",
          "command": "bash ${CLAUDE_PLUGIN_ROOT}/scripts/notify-compacted.sh"
        }
      ]
    }
  ]
}
```

### Available Hook Events

- `PreToolUse`, `PostToolUse`
- `Stop`, `SubagentStop`
- `SessionStart`, `SessionEnd`
- `UserPromptSubmit`
- `PreCompact`, `PostCompact`
- `Notification`

---

## Context Preservation Patterns

### SessionStart Hook with Additional Context

```bash
#!/bin/bash
cat << 'EOF'
{
  "hookSpecificOutput": {
    "hookEventName": "SessionStart",
    "additionalContext": "Critical context: Working on feature-X branch. Pending: database migration. Last compact: 2 hours ago."
  }
}
EOF
```

### Environment Variable Persistence

```bash
#!/bin/bash
cd "$CLAUDE_PROJECT_DIR" || exit 1

# Detect and persist project context
if [ -f "package.json" ]; then
  echo "export PROJECT_TYPE=nodejs" >> "$CLAUDE_ENV_FILE"
fi
```

---

## Best Practices for Context Efficiency

### Skill Development

**Three-Level Loading System:**

1. **Metadata** (name + description) - Always in context (~100 words)
2. **Core instructions** - Loaded when skill invoked
3. **References/Scripts** - Loaded as needed or executed without reading into context

**Avoid Duplication:**
- Information lives in SKILL.md OR references files, not both
- Keep SKILL.md lean - move detailed reference material to references/
- Scripts can be executed without reading into context window

### Memory Management

1. **Persistent state**: Use `.local.md` files
2. **Atomic updates**: Write complete state files atomically
3. **Timestamps**: Use last-modified to track freshness
4. **Auto-memory**: Claude automatically saves useful context (manage with `/memory`)

### MCP Tool Optimization

- Auto mode enabled by default
- Tools exceeding 10% of context window automatically deferred to `ToolSearch`
- Reduces context usage for users with many MCP tools

---

## Slash Commands for Context Management

| Command | Purpose |
|---------|---------|
| `/compact` | Manual context compaction |
| `/context` | Show context usage with optimization suggestions |
| `/memory` | Manage auto-memory files |
| `/clear` | Clear conversation history |

---

## Version History (Key Changes)

- **v2.1.76** - Added PostCompact hook
- **v2.1.74** - Fixed circuit breaker (stops after 3 failures)
- **v2.1.64** - Fixed token estimation over-counting
- **v2.1.57** - Reduced tool result limit from 100K to 50K chars
- **v2.1.48** - Improved compaction to preserve images for cache reuse
- **v2.1.28** - Auto-compact warning threshold: 60% → 80%
- **v0.2.45** - Automatic conversation compaction introduced

---

## Architecture Insights

### Memory Optimization

- Compaction clears internal caches
- Reduces memory when resuming large sessions (~100-150MB less peak memory)
- Strip heavy progress message payloads during compaction
- Memory leak fixes for long-running teammates

### Resume Behavior

- Compacted summaries reduce load time
- No preamble recap after resuming from compaction
- Session names preserved through compaction
- Plan mode state preserved

### Subagent Context

- Parent's full history no longer pinned for teammate's lifetime
- Skills invoked by subagents properly isolated after compaction
- Background agent completion notifications include output file paths

---

## References

- [Hook Development SKILL.md](plugins/plugin-dev/skills/hook-development/SKILL.md)
- [Plugin Development README](plugins/plugin-dev/README.md)
- CHANGELOG.md (search: "compact", "summarize", "context window")
