# Context Window Tracking Reference

**Autonomous reference document for OpenCode's context window tracking system**

**Focus:** GLM models via zai provider

**Version:** OpenCode (as of commit analysis, March 2026)

---

## Table of Contents

1. [Overview](#overview)
2. [Architecture](#architecture)
3. [Data Structures](#data-structures)
4. [Token Counting](#token-counting)
5. [Overflow Detection](#overflow-detection)
6. [Provider-Specific Handling](#provider-specific-handling)
7. [Code References](#code-references)

---

## Overview

OpenCode tracks token usage to prevent context window overflow and trigger automatic compaction when approaching limits. For GLM models (GLM-4.6, GLM-4.7, GLM-5) provided by the **zai** provider (also referenced as **zhipuai**), the system uses:

- **OpenAI-compatible SDK:** `@ai-sdk/openai-compatible`
- **Token source:** Provider response (primary), local estimation (secondary, limited use)
- **Overflow strategy:** Proactive checking (critical for zai, may silently truncate)

---

## Architecture

```
Provider Response (with usage data)
    ↓
SessionProcessor.process() handles 'finish-step'
    ↓
Session.getUsage() normalizes token counts
    ↓
MessageV2.Assistant.tokens updated
    ↓
SessionCompaction.isOverflow() checks limits
    ↓
[if overflow] → SessionCompaction.process() compacts history
```

### Key Components

| Component         | File                                          | Purpose                                       |
| ----------------- | --------------------------------------------- | --------------------------------------------- |
| SessionProcessor  | `packages/opencode/src/session/processor.ts`  | Handles provider streaming events             |
| Session           | `packages/opencode/src/session/index.ts`      | Core session management, usage normalization  |
| MessageV2         | `packages/opencode/src/session/message-v2.ts` | Message schemas including tokens              |
| SessionCompaction | `packages/opencode/src/session/compaction.ts` | Overflow detection and compaction             |
| ProviderTransform | `packages/opencode/src/provider/transform.ts` | Provider-specific request/response transforms |
| ModelsDev         | `packages/opencode/src/provider/models.ts`    | Model metadata including limits               |
| Token             | `packages/opencode/src/util/token.ts`         | Local token estimation utility                |
| ProviderError     | `packages/opencode/src/provider/error.ts`     | Error parsing (including overflow errors)     |

---

## Data Structures

### Message Token Schema

**Location:** `packages/opencode/src/session/message-v2.ts:427-436`

```typescript
tokens: {
  total: number | undefined,      // Sum of all tokens (computed for Anthropic/Bedrock)
  input: number,                  // Input tokens used
  output: number,                 // Output tokens used
  reasoning: number,              // Reasoning tokens (thinking models)
  cache: {
    read: number,                 // Cache read tokens (prompt caching)
    write: number,                // Cache write tokens (prompt caching)
  }
}
```

**Also in:** `MessageV2.StepFinishPart` (lines 251-260)

### Model Limit Schema

**Location:** `packages/opencode/src/provider/models.ts:27-31`

```typescript
limit: z.object({
  context: z.number(), // Total context window size
  input: z.number().optional(), // Input limit (if separate from context)
  output: z.number(), // Output limit
})
```

**Source:** Loaded from `models.dev` API via `ModelsDev.get()`

---

## Token Counting

### Primary Source: Provider Response

**Function:** `Session.getUsage()`

**Location:** `packages/opencode/src/session/index.ts:784-861`

**How it works:**

```typescript
// Read usage from provider response
const inputTokens = usage.inputTokens ?? 0
const outputTokens = usage.outputTokens ?? 0
const reasoningTokens = usage.reasoningTokens ?? 0

// Cache tokens (for providers with prompt caching)
const cacheReadTokens = usage.cache_read_input_tokens ?? usage.prompt_tokens_details?.cached_tokens ?? 0

// Compute total for specific providers
if (providerID === "anthropic" || providerID === "bedrock") {
  // Anthropic: inputTokens excludes cached tokens
  total = inputTokens + outputTokens + cacheReadTokens + cacheWriteTokens
} else {
  // Others: total comes from provider or is computed differently
}
```

**Provider differences handled:**

- **Anthropic/Bedrock:** `total` computed manually (excludes cache from input)
- **Others:** `total` may come directly from provider

### Secondary Source: Local Estimation

**Function:** `Token.estimate()`

**Location:** `packages/opencode/src/util/token.ts:4-6`

```typescript
const CHARS_PER_TOKEN = 4

export function estimate(input: string) {
  return Math.max(0, Math.round((input || "").length / 4))
}
```

**When used:**

- Estimating tool output size for pruning (`compaction.ts:80`)
- **NOT** used for:
  - Pre-send estimation to providers
  - User message token counting
  - Media file token estimation

**Note:** This is a rough approximation (4 chars per token). Real tokenization is provider-specific.

---

## Overflow Detection

### Proactive Checking

**Function:** `SessionCompaction.isOverflow()`

**Location:** `packages/opencode/src/session/compaction.ts:32-48`

```typescript
const COMPACTION_BUFFER = 20_000 // Safety buffer

export async function isOverflow({ tokens, model }) {
  // Get total token count
  const count = tokens.total || tokens.input + tokens.output + tokens.cache.read + tokens.cache.write

  // Calculate available space
  const maxOutput = ProviderTransform.maxOutputTokens(model)
  const reserved = config.compaction?.reserved ?? Math.min(COMPACTION_BUFFER, maxOutput)

  const usable = model.limit.input
    ? model.limit.input - reserved // If input limit defined
    : model.limit.context - maxOutput // Otherwise: context - max output

  // Check overflow
  return count >= usable
}
```

**How it works:**

1. Checks tokens from **last completed assistant message**
2. Uses 20K token buffer (configurable)
3. Respects `model.limit.input` if defined, otherwise uses `model.limit.context - maxOutputTokens`
4. Returns `true` if compaction needed

### Reactive Checking

**Function:** `ProviderError.parseStreamError()`

**Location:** `packages/opencode/src/provider/error.ts`

Parses provider error messages for context overflow patterns:

- Anthropic: `/exceeds the context window/i`
- OpenAI: `/context window exceeds limit/i`
- MiniMax: `/content length exceeds limit/i`
- Google, xAI, OpenRouter, etc.

**Important for zai:**

```
// Line 33: z.ai can accept overflow silently
// Needs token-count/context-window checks
```

zai may **silently truncate** context without returning an error, making proactive checking **critical**.

---

## Provider-Specific Handling

### zai / zhipuai (GLM Models)

**Provider ID:** `"zai"` or `"zhipuai"`

**SDK:** `@ai-sdk/openai-compatible`

#### Thinking Mode Configuration

**Location:** `packages/opencode/src/provider/transform.ts:713-718`

```typescript
if (["zai", "zhipuai"].includes(input.model.providerID) && input.model.api.npm === "@ai-sdk/openai-compatible") {
  result["thinking"] = {
    type: "enabled",
    clear_thinking: false, // Preserve reasoning in response
  }
}
```

#### Temperature Settings

**Location:** `packages/opencode/src/provider/transform.ts:297-298`

```typescript
if (input.model.api.id === "glm-4.6") {
  result["temperature"] = 1.0
}
if (input.model.api.id === "glm-4.7") {
  result["temperature"] = 1.0
}
```

#### Critical Considerations

| Aspect             | General          | zai/GLM                                |
| ------------------ | ---------------- | -------------------------------------- |
| SDK                | Varies           | `@ai-sdk/openai-compatible`            |
| Thinking mode      | Model-dependent  | **Enabled by default**                 |
| Overflow errors    | Provider returns | **May not return** (silent truncation) |
| Proactive checking | Important        | **Critical**                           |

---

## Token Update Flow

### Step-by-Step

**Trigger:** Provider responds with usage data

**Location:** `packages/opencode/src/session/processor.ts:244-262`

```
1. SessionProcessor.process() receives 'finish-step' event
   ↓
2. Calls Session.getUsage() to normalize token counts
   ↓
3. Updates assistantMessage.tokens with computed values
   ↓
4. Calls Session.updatePart() with StepFinishPart (contains tokens)
   ↓
5. Calls Session.updateMessage() to persist message
   ↓
6. Calls SessionCompaction.isOverflow() to check limits
   ↓
7. [if overflow] → Triggers compaction process
```

---

## Compaction Process

**Function:** `SessionCompaction.process()`

**Location:** `packages/opencode/src/session/compaction.ts:84-269`

When overflow detected:

1. **Find last user message** in conversation
2. **Invoke compaction agent** to generate summary
3. **Generate detailed summary** including:
   - Goal
   - Instructions
   - Discoveries
   - Accomplished work
   - Relevant files/directories
4. **Replace old messages** with compaction parts
5. **Replay** if needed (continues from compacted point)

### Pruning

**Location:** `packages/opencode/src/session/compaction.ts:68-132`

```typescript
const PRUNE_PROTECT = 40_000 // Protect last 40K tool output tokens
const PRUNE_MINIMUM = 20_000 // Minimum to prune
const PRUNE_PROTECTED_TOOLS = ["skill"] // Never prune these tools
```

**Process:**

- Goes backward through messages (skip last 2 turns)
- Finds completed tool calls
- Accumulates tool output tokens until 40K protected
- Marks earlier tool outputs as compacted
- Minimum prune threshold: 20K tokens

---

## Model Limits

### Where Limits Come From

**Location:** `packages/opencode/src/provider/models.ts`

**Source:** `models.dev` API (refreshed every hour)

**Example GLM limits** (from i18n files):

- GLM-5: Part of Go subscription ($10/mo, 5-hour limits)
- GLM-4.6, GLM-4.7: Various context limits

**Loading:**

```typescript
// models.ts:30-54
export async function get() {
  const result = await Data() // Loads from cache, snapshot, or API
  return result as Record<string, Provider>
}

// Auto-refresh every hour
setInterval(
  async () => {
    await ModelsDev.refresh()
  },
  60 * 1000 * 60,
).unref()
```

### Limit Properties

| Property        | Type    | Description               |
| --------------- | ------- | ------------------------- |
| `limit.context` | number  | Total context window size |
| `limit.input`   | number? | Input limit (if separate) |
| `limit.output`  | number  | Maximum output tokens     |

---

## Configuration

### Compaction Buffer

**Default:** 20,000 tokens

**Location:** `packages/opencode/src/session/compaction.ts:30`

**Configurable:** Via `config.compaction.reserved`

```typescript
const COMPACTION_BUFFER = 20_000

const reserved = config.compaction?.reserved ?? Math.min(COMPACTION_BUFFER, maxOutput)
```

### Auto Compaction

**Default:** Enabled

**Location:** `packages/opencode/src/session/compaction.ts:36-37`

```typescript
if (config.compaction?.auto === false) return false
```

### Auto Pruning

**Default:** Enabled

**Location:** `packages/opencode/src/session/compaction.ts:73-74`

```typescript
if (config.compaction?.prune === false) return
```

---

## Code References

### Key Functions

| Function             | File                    | Lines   | Purpose                   |
| -------------------- | ----------------------- | ------- | ------------------------- |
| `isOverflow()`       | `session/compaction.ts` | 32-48   | Check if context exceeded |
| `process()`          | `session/compaction.ts` | 84-269  | Execute compaction        |
| `prune()`            | `session/compaction.ts` | 68-132  | Prune old tool outputs    |
| `getUsage()`         | `session/index.ts`      | 784-861 | Normalize token counts    |
| `estimate()`         | `util/token.ts`         | 4-6     | Local token estimation    |
| `maxOutputTokens()`  | `provider/transform.ts` | ~250    | Get max output from model |
| `parseStreamError()` | `provider/error.ts`     | 125-161 | Parse provider errors     |

### Key Types/Schemas

| Type                    | File                    | Lines   | Purpose              |
| ----------------------- | ----------------------- | ------- | -------------------- |
| `Model.limit`           | `provider/models.ts`    | 27-31   | Model limits schema  |
| `Assistant.tokens`      | `session/message-v2.ts` | 427-436 | Token storage schema |
| `StepFinishPart.tokens` | `session/message-v2.ts` | 251-260 | Per-step tokens      |

### Provider-Specific Code

| Provider     | File                    | Lines   | Special handling     |
| ------------ | ----------------------- | ------- | -------------------- |
| zai, zhipuai | `provider/transform.ts` | 713-718 | Thinking mode        |
| GLM models   | `provider/transform.ts` | 297-298 | Temperature          |
| GLM-4.6      | `provider/transform.ts` | 708     | `chat_template_args` |

---

## Summary for Implementation

### For GLM Models (zai Provider)

1. **Use OpenAI-compatible SDK** (`@ai-sdk/openai-compatible`)
2. **Enable thinking mode** with `clear_thinking: false`
3. **Set temperature** to 1.0 for GLM-4.6/4.7
4. **Implement proactive overflow checking** (zai may truncate silently)
5. **Track tokens from provider response** (input, output, reasoning, cache)
6. **Use 20K token buffer** before hitting context limit
7. **Implement compaction** when approaching limit (summary-based history reduction)

### Token Counting Best Practices

1. **Primary:** Use provider's `usage` data (inputTokens, outputTokens, reasoningTokens)
2. **Secondary:** Local estimation only for non-critical operations (e.g., pruning estimates)
3. **Cache tokens:** Handle separately if provider supports prompt caching
4. **Total computation:** May need manual calculation depending on provider

### Overflow Detection Strategy

1. **Proactive:** Check last assistant message tokens against usable limit
2. **Reactive:** Parse provider errors for overflow patterns
3. **Buffer:** Maintain 20K token buffer (configurable)
4. **Compaction:** Generate summary of old messages to preserve context

---

## File Path Summary

All paths relative to repository root `/home/stfu/ai/agent-frameworks/opencode/`

```
packages/opencode/src/
├── session/
│   ├── index.ts              (Session.getUsage: lines 784-861)
│   ├── compaction.ts         (SessionCompaction: lines 1-329)
│   ├── message-v2.ts         (MessageV2 schemas: lines 19-914)
│   └── processor.ts          (SessionProcessor: finish-step handler)
├── provider/
│   ├── models.ts             (ModelsDev: lines 1-133)
│   ├── transform.ts          (ProviderTransform: lines 1-980)
│   └── error.ts              (ProviderError: lines 1-200)
└── util/
    └── token.ts              (Token.estimate: lines 1-8)
```

---

## Notes

- **GLM-5** is mentioned in Go subscription materials ($10/mo, generous 5-hour limits)
- **GLM models** are OpenAI-compatible through zai/zhipuai
- **Thinking mode** is enabled by default for these models
- **Context limits** vary by model and are loaded from models.dev API
- **Silent truncation** by zai makes proactive checking critical

---

**Document generated:** March 19, 2026
**OpenCode repository:** `/home/stfu/ai/agent-frameworks/opencode/`
**Focus:** GLM models via zai provider
**Purpose:** Portable reference for context window tracking implementation
