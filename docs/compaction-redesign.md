# Compaction Redesign — Verified Contracts and Target Architecture

Date: 2026-06-22
Status: Phase 0 verification skeleton
Goal doc: `docs/goals/2026-06-22-compaction-dcp-redesign.md`

## 1. П0.5 Verification — Storage Serialization

### AgentMemory struct (verified `memory.rs:662-673`)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMemory {
    messages: Vec<AgentMessage>,
    pub todos: TodoList,
    token_count: usize,
    max_tokens: usize,
    #[serde(default)]
    last_api_usage: Option<TokenUsage>,
}
```

### Serialization path (verified `storage/sqlx/mod.rs:165-200`)

- `save_agent_memory_scope`: `serde_json::to_value(memory)` → `INSERT ... ON CONFLICT DO UPDATE` into `agent_memory_snapshots.memory` (JSONB column).
- `load_agent_memory_scope`: reads the JSONB column → `serde_json::from_value::<AgentMemory>(row_value)`.
- `schema_version` column is hardcoded `1` — no migration framework exists.

### Checkpoint trait (verified `session.rs:118-123`)

```rust
#[async_trait]
pub trait AgentMemoryCheckpoint: Send + Sync {
    async fn persist(&self, memory: &AgentMemory) -> Result<()>;
}
```

- Takes a full `&AgentMemory` clone.
- Debounced background persistence with hash-based dedup (`memory_checkpoint_hash` → `serde_json::to_vec(memory)` → `DefaultHasher`).
- Adding a `#[serde(default)]` field to `AgentMemory` is backward-safe: old JSON without the field deserializes to `default()`. Hash changes → one forced checkpoint write on first persist after migration (no data loss).

### Backward compatibility conclusion

- Adding `CompactionState` with `#[serde(default)]` to `AgentMemory` is safe.
- Existing persisted sessions without compaction state will deserialize with `CompactionState::default()` (empty — no blocks, identity render).
- No SQLx schema migration needed — JSONB column stores the full struct.
- Storage facade changes: **not required** — `save/load` already serialize the entire `AgentMemory` struct.

## 2. П0.5 Verification — Provider Typed Error Contract

### LlmError enum (verified `llm/error.rs:5-59`)

```rust
pub enum LlmError {
    ApiError { status: Option<u16>, message: String, provider: Option<String>, model: Option<String> },
    EmptyResponse(String),
    NetworkError(String),
    RequestBuilder(String),
    JsonError(String),
    MissingConfig(String),
    RateLimit { wait_secs: Option<u64>, message: String },
    RepairableHistory(String),
    Unknown { message: String, provider: Option<String>, model: Option<String> },
}
```

### Error classification (verified `llm/support/backoff.rs:64-119`)

- `is_rate_limit_error`: typed pattern match on `RateLimit` variant and `ApiError { status: Some(429) }` — no string matching.
- `is_retryable_error`: delegates to `get_retry_delay` which uses typed pattern matching on HTTP status codes.
- **No typed context-overflow detection exists.** `llm_error_suggests_context_overflow` (verified `llm_calls.rs:923-936`) uses `error.to_string().to_ascii_lowercase()` + substring matching against 7 hardcoded phrases — this is the П0 violation to fix.

### Context overflow reality

Providers return context-overflow as `ApiError { status: Some(400), message: "<provider-specific text>" }`. There is no structured error type field — the error type is embedded in the message string. Different providers use different phrasings:
- OpenAI: `"context_length_exceeded"` in the JSON body
- Anthropic: `"prompt is too long"` in the message
- Others: various free-text

### Target: typed classification without string matching on LLM output

The redesign must not rely on string matching over provider error messages. Two viable approaches:

1. **Add a `ContextOverflow` variant to `LlmError`**: Provider implementations parse their structured error responses and return `ContextOverflow` when the HTTP 400 body contains a known context-length error code. Each provider maps its own error body to this variant — no cross-provider string matching.
2. **Add an `is_context_overflow()` method on `LlmError`** that checks `ApiError { status: Some(400) }` + provider-specific structured body parsing (not `to_string()` substring matching).

**Decision**: Approach 1 — add `ContextOverflow { provider, model, message }` variant. Provider implementations already parse their HTTP error responses; they can classify context-overflow at the source. The runner checks `matches!(error, LlmError::ContextOverflow { .. })` — no string matching in runner code.

### Provider implementations to update

Each provider in `llm/providers/` must detect context-overflow from its native error response and return `LlmError::ContextOverflow`. The provider already has the raw response body and can parse provider-specific error codes.

## 3. П0.5 Verification — Event Consumer Contract

### AgentEvent compaction variants (verified `progress.rs:207-279`)

- `RuntimeCompactionStarted { reason, phase, backend, provider, route, token_before, history_items_before }`
- `RuntimeCompactionCompleted { reason, phase, backend, provider, route, token_before, token_after, history_items_before, history_items_after, generation, repair_applied }`
- `RuntimeCompactionFailed { reason, phase, backend, provider, route, error }`
- `RuntimeCompactionSkipped { reason, phase, skipped_reason }` — defined, handled in `ProgressState`, mapped in transports, but **never emitted**.
- `RepeatedCompactionWarning { kind, count }`

### ProgressState consumers (verified `progress.rs:897-985`)

- `last_compaction_status`: surfaced in `ProgressState`, rendered by Telegram `progress_render.rs:290-303` and web `server/types.rs:522-523`.
- `repeated_compaction_warning`: surfaced similarly.

### Transport mappings

- Web: `web_transport.rs:45-49` maps variants to stable strings (`compaction_started`, `compaction_completed`, `compaction_failed`, `compaction_skipped`, `repeated_compaction_warning`). Payload construction in `compaction_event_parts` / `compaction_completed_parts`.
- Web contracts: `TaskEventKind::RuntimeCompactionSkipped` in `oxide-agent-web-contracts/src/events.rs:43`.
- Web UI: `oxide-agent-web-ui/src/tasks/activity.rs:546,598` renders compaction events.
- Telegram: `progress_render.rs:410-415` maps `BudgetState` to labels; `progress_render.rs:594` tests compaction status rendering.
- Milestone: `execution.rs:178` emits `Milestone { name: "pre_run_compaction_done" }`. Web E2E tracks `pre_run_compaction_done_ms`.

### Event redesign implications

- Event variants carry `CompactionReason`, `CompactionPhase`, `CompactionBackend` — all of which may change or be replaced in the new system.
- `CompactionBackend` is single-variant (`LocalLlmSummary`) — will be replaced or removed.
- `generation` field in `RuntimeCompactionCompleted` maps to the old generation counter — will be replaced by block graph state.
- **Transport-facing contract**: the stable string names (`compaction_started`, etc.) and the JSON payload shape consumed by web UI must remain backward-compatible or be explicitly migrated.
- **Decision**: Redesign event payloads to carry block-id, token-before/after, and reason. Transport string names stay stable; payload schema evolves. Web UI rendering adapts to new fields.

## 4. П0.5 Verification — Tool History Repair Contract

### repair_agent_message_history_runtime (verified `recovery.rs:38-43`)

- `repair_agent_message_history_with_policy(messages, preserve_terminal_open_batch: bool)`
- Runtime policy (`true`): preserves the terminal open tool batch (assistant tool calls without matching tool results — the in-flight batch).
- Validates: tool-call/result pairing, orphaned tool results, partial tool batches.
- `replace_compacted_history` rejects histories needing repair (`InvalidToolHistory` error).

### Renderer implication

The compacted renderer must produce tool-call-valid histories:
- No orphaned tool results (tool result without preceding tool call).
- No partial completed tool batches (assistant tool call with some but not all matching tool results).
- Terminal open tool batch preserved.
- If a compaction block covers a range that includes tool calls but not their results (or vice versa), the block boundary must be adjusted or the orphaned side must be pruned.

This is a hard constraint on block graph selection: **block boundaries must not split tool-call/result pairs.**

## 5. П0.5 Verification — Runner → Provider Boundary

### Current flow (verified `runner/types.rs:128-165`, `runner/mod.rs:101-124`, `runner/token_snapshots.rs:93-95`)

1. `AgentRunnerContext.messages: &'a mut Vec<Message>` — the model-facing message buffer.
2. `refresh_messages_from_memory(ctx)`: `*ctx.messages = convert_memory_to_messages(ctx.agent.memory().get_messages())` — direct 1:1 conversion from raw `AgentMessage` to `llm::Message`.
3. `chat_with_tools_single_attempt(ctx.system_prompt, ctx.date_suffix, ctx.messages, ...)` sends `ctx.messages` to the provider.
4. Compaction currently mutates `AgentMemory` then calls `refresh_messages_from_memory` to rebuild `ctx.messages`.

### Target boundary

`refresh_messages_from_memory` becomes `refresh_rendered_messages`:
```text
raw_messages = ctx.agent.memory().get_messages()
compaction_state = ctx.agent.memory().compaction_state()
*ctx.messages = CompactionRenderer::render(raw_messages, compaction_state, policy)
```

The renderer is the **only** point where raw messages become model-facing messages. The compaction engine only mutates `CompactionState`. Raw `AgentMemory::messages` is never replaced/destroyed by compaction.

## 6. Target Architecture — Component Boundaries

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         AgentMemory (persisted)                             │
│  ┌─────────────────────┐  ┌─────────────────────────────────────────────┐  │
│  │  raw_messages:      │  │  compaction_state: CompactionState           │  │
│  │  Vec<AgentMessage>  │  │  #[serde(default)]                            │  │
│  │  (never destroyed)  │  │  blocks, refs, strategies, policy snapshot   │  │
│  └─────────────────────┘  └─────────────────────────────────────────────┘  │
│  todos, token_count, max_tokens, last_api_usage                             │
└─────────────────────────────────────────────────────────────────────────────┘
           │                                    │
           ▼                                    ▼
┌─────────────────────┐           ┌──────────────────────────┐
│ CompactionRenderer  │◄──────────│ CompactionEngine          │
│ render(raw, state)  │           │ (only mutation authority  │
│ → Vec<Message>      │           │  for CompactionState)     │
└─────────────────────┘           └──────────────────────────┘
           │                                    ▲
           ▼                                    │
┌─────────────────────┐           ┌──────────────────────────┐
│ Runner LLM call     │           │ ContextAdmission          │
│ (ctx.messages =     │           │ (gate before any hot-     │
│  rendered context)  │           │  memory mutation)         │
└─────────────────────┘           └──────────────────────────┘
```

### Mutability rules

- **Raw messages**: only mutated by `add_message`, `replace_messages` (from non-compaction paths), `clear`. Compaction **never** touches raw messages.
- **CompactionState**: only mutated by `CompactionEngine`. Engine is called by all trigger paths.
- **Rendered messages**: produced fresh before every LLM call by `CompactionRenderer`. Never stored, never mutated.
- **ContextAdmission**: gates external/tool payloads before they reach `add_message` or any raw-memory mutation. Decides inline/archive/pause.

### Trigger matrix → engine entry points

| Trigger | Initiator | Engine method |
|---------|-----------|---------------|
| Context admission | `ContextAdmission` | `engine.admit_payload(payload, budget) → AdmissionDecision` |
| Pre-LLM budget | runner render gate | `engine.compact_for_budget(raw, state, budget) → CompactionResult` |
| Agent compress | `compress` tool | `engine.apply_compression(selection, summary_parts, state) → BlockResult` |
| User/manual | transport | `engine.compact_on_demand(raw, state, reason) → CompactionResult` |
| Model downshift | route failover | `engine.compact_for_budget(raw, state, smaller_budget)` |
| Typed overflow | provider `ContextOverflow` | `engine.emergency_shrink(raw, state, route_window) → CompactionResult` |

All methods produce a `CompactionState` transition. The renderer then renders from the updated state.

## 7. П0.5 Verification — Pending Checks for Implementation

The following need runtime verification during implementation phases:

- **Provider structured error bodies**: each provider's HTTP 400 response body format for context-overflow. Must be verified by inspecting actual provider error response parsing code (not just error messages).
- **Block boundary + tool-call safety**: property tests must verify that no block selection splits a tool-call/result pair.
- **Storage round-trip with CompactionState**: serialize `AgentMemory` with non-empty `CompactionState`, deserialize, verify state integrity.
- **Transport event payload backward compat**: web UI rendering of new event fields.

These are deferred to their respective implementation phases where they can be verified against actual code, not assumptions.
