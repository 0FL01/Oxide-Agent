# Compaction Architecture

## 0. As-Built Module Map

The compaction subsystem lives in `crates/oxide-agent-core/src/agent/compaction/`
(16 files, ~6900 lines). Public exports are declared in `compaction/mod.rs`:

| File | Purpose | Key exports |
|------|---------|-------------|
| `mod.rs` | Module hub + re-exports | all `pub use` below |
| `admission.rs` | Stateless payload admission gate | `ContextAdmission`, `AdmissionDecision`, `AdmissionBudget`, `AdmissionBlocker`, `ManifestSpec`, `PayloadDescriptor`, `PayloadKind`, `EmergencySummarizer`, `split_into_chunks`, `summarize_in_chunks` |
| `archive.rs` | Externalized payload reference type | `ArchiveRef` |
| `auto_select.rs` | Automatic compression range selection | (internal) `select_automatic_compression_range`, `target_history_tokens_for_messages` |
| `block.rs` | Block graph: `CompressionBlock`, `CompressionSelection`, `SummaryPart` | `CompressionBlock`, `CompressionSelection`, `SummaryPart` |
| `budget.rs` | Token estimation and request budget | `count_tokens_cached`, `estimate_request_budget` |
| `controller.rs` | Orchestrator: selects range, generates LLM summary, applies engine | `CompactionController`, `CompactionControllerError`, `EngineCompactionResult`, `EngineCompactionSkipped` |
| `engine.rs` | Sole mutation authority for `CompactionState` | `CompactionEngine`, `CompactionError` |
| `local_llm_summary.rs` | `CompactSummaryBackend` impl using a side-LLM | `LocalLlmSummary` |
| `prompt.rs` | Compaction summary system/user prompts | `local_compaction_system_prompt`, `build_local_compaction_user_message` |
| `refs.rs` | Stable visible refs (`mNNNN`, `bN`) | `MessageRef`, `BlockRef` |
| `renderer.rs` | Renders raw messages + compaction state → model-facing `Vec<Message>` | `CompactionRenderer` |
| `state.rs` | Persisted compaction overlay state | `CompactionState` |
| `strategy.rs` | Stateless dedup/purge-error rendering strategies | `RenderPolicy`, `compute_superseded_tool_results`, `compute_purge_error_inputs` |
| `task.rs` | Summary backend contract types | `CompactSummaryBackend`, `CompactSummaryRequest`, `CompactSummaryResult`, `CompactSummaryError` |
| `types.rs` | Enums, policy, budget types | `CompactionReason`, `CompactionPhase`, `CompactionBackend`, `CompactionTrigger`, `CompactionPolicy`, `CompactionRetention`, `CompactionScope`, `BudgetState`, `BudgetEstimate`, `HotMemoryBudget` |

Runner orchestration slices:
- `agent/runner/runtime_compaction.rs` — all automatic trigger entry points → `compact_via_engine`.
- `agent/runner/tools.rs` — `compress` tool → `CompactionEngine::apply_compression`; tool output admission.
- `agent/executor/compaction.rs` — transport-triggered manual compaction → `compact_via_engine`.

## 1. Storage Serialization (verified)

### AgentMemory struct (verified `memory.rs:611-626`)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMemory {
    messages: Vec<AgentMessage>,
    /// Task list for the agent
    pub todos: TodoList,
    /// Estimated tokens currently represented by hot memory messages.
    token_count: usize,
    max_tokens: usize,
    /// Last request-scoped token usage reported by the LLM API.
    #[serde(default)]
    last_api_usage: Option<TokenUsage>,
    /// Compaction overlay state — tracks blocks, refs, and strategy decisions.
    /// The renderer uses this to produce compacted model-facing context.
    /// Old checkpoints without this field deserialize to `CompactionState::default()`.
    #[serde(default)]
    compaction_state: CompactionState,
}
```

The `compaction_state` field was added as part of the redesign (Phase 1–2).
It carries the full block graph, message/block refs, and strategy decisions.

### Serialization path (verified `storage/sqlx/mod.rs:165-230`)

- `save_agent_memory_scope` (`sqlx/mod.rs:165`): `serde_json::to_value(memory)` → `INSERT ... ON CONFLICT DO UPDATE` into `agent_memory_snapshots.memory` (JSONB column). The `WHERE ... IS DISTINCT FROM` clause skips writes when the serialized value is unchanged.
- `load_agent_memory_scope` (`sqlx/mod.rs:202`): reads the JSONB column → `serde_json::from_value::<AgentMemory>(row_value)`.
- `schema_version` column is hardcoded `1` — no migration framework exists.

### Checkpoint trait (verified `session.rs:120-122`)

```rust
#[async_trait]
pub trait AgentMemoryCheckpoint: Send + Sync {
    async fn persist(&self, memory: &AgentMemory) -> Result<()>;
}
```

- Takes a full `&AgentMemory` clone.
- Debounced background persistence with hash-based dedup (`memory_checkpoint_hash` → `serde_json::to_vec(memory)` → `DefaultHasher`, `session.rs:156`).

### Backward compatibility (verified by tests)

- Adding `CompactionState` with `#[serde(default)]` to `AgentMemory` is safe and verified.
- `compaction_state_defaults_on_new` (`memory.rs:988`): new `AgentMemory` has empty compaction state.
- `old_json_without_compaction_state_deserializes` (`memory.rs:1035`): old checkpoint JSON without `compaction_state` field deserializes to `CompactionState::default()` (empty — no blocks, identity render).
- `compaction_state_resets_on_clear` (`memory.rs:994`), `compaction_state_resets_on_replace_messages` (`memory.rs:1002`), `compaction_state_resets_on_repair` (`memory.rs:1010`): state resets on memory mutations outside compaction.
- No SQLx schema migration needed — JSONB column stores the full struct.
- Storage facade changes: **not required** — `save/load` already serialize the entire `AgentMemory` struct.

## 2. Provider Typed Error Contract (verified)

### LlmError enum (verified `llm/error.rs:5-70`)

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
    ContextOverflow { message: String, provider: Option<String>, model: Option<String> },
    Unknown { message: String, provider: Option<String>, model: Option<String> },
}
```

The `ContextOverflow` variant (`error.rs:52`) was added in Phase 5. It is the
typed replacement for the old string-matching classifier.

### Error classification (verified `llm/support/backoff.rs:24-88`)

- `is_rate_limit_error` (`backoff.rs:69`): typed pattern match on `RateLimit` variant and `ApiError { status: Some(429) }` — no string matching.
- `is_retryable_error` (`backoff.rs:64`): delegates to `get_retry_delay` which uses typed pattern matching on HTTP status codes.
- `ContextOverflow` is classified as non-retryable by backoff logic (no `get_retry_delay` arm matches it). Context-limit retry is handled explicitly by the runner via compaction + retry.

### Context overflow classification (verified `llm/error.rs:153-220`)

The old П0 violation — `llm_error_suggests_context_overflow` using
`error.to_string().to_ascii_lowercase()` + substring matching in the runner —
has been **removed**. The function no longer exists in production code; the
only reference is a historical mention in a doc comment (`error.rs:160`).

**As-built approach (deviation from original Approach 1):**

The original design proposed Approach 1: each provider implementation parses
its own HTTP error response and returns `ContextOverflow` at the source — no
cross-provider string matching. The actual implementation took a different path:

Classification is **centralized** in `LlmError::try_classify_context_overflow`
(`error.rs:179`), called once in the runner (`llm_calls.rs:576`). The method:

1. Checks the typed HTTP status: `ApiError { status: Some(400|413), .. }` or `ApiError { status: None, .. }` → candidate.
2. Inspects the message string for known overflow indicators (`INDICATORS` array: "context length", "context window", "too many tokens", "token limit", "maximum context", "prompt is too long", "context overflow").
3. Converts matching `ApiError` to `ContextOverflow`; leaves all others unchanged.

This still uses substring matching, but over **HTTP API error response
bodies**, not over LLM generative output. The code comment (`error.rs:160`)
explicitly distinguishes: "This is HTTP API error response parsing, not
heuristic over LLM output." The П0 ban on string matching targets LLM output
and generative steps — provider error bodies are structured HTTP responses
where the status code gates the message inspection.

The runner uses the typed check (`llm_calls.rs:576-578`):

```rust
let error = error.try_classify_context_overflow();
if error.is_context_overflow() && metadata.attempt == 1 {
    let retried = self.run_runtime_context_limit_compaction(ctx, state, metadata.route).await?;
    if retried { return Ok(AttemptOutcome::RetrySameRoute); }
}
```

`is_context_overflow()` (`error.rs:153`) is a typed `matches!` — no string matching in runner code.

**Verified by tests** (`error.rs:293-375`): `is_context_overflow_typed_match`, `try_classify_context_overflow_400_with_indicator`, `try_classify_context_overflow_413_with_indicator`, `try_classify_context_overflow_none_status_with_indicator`, plus non-classification tests for other statuses and non-API variants.

## 3. Event Consumer Contract (verified)

### AgentEvent compaction variants (verified `progress.rs:208-279`)

- `RuntimeCompactionStarted { reason, phase, backend, provider, route, token_before, history_items_before }` (`progress.rs:208`)
- `RuntimeCompactionCompleted { reason, phase, backend, provider, route, token_before, token_after, history_items_before, history_items_after, generation, repair_applied }` (`progress.rs:225`)
- `RuntimeCompactionFailed { reason, phase, backend, provider, route, error }` (`progress.rs:250`)
- `RuntimeCompactionSkipped { reason, phase, skipped_reason }` (`progress.rs:265`)
- `RepeatedCompactionWarning { kind, count }` (`progress.rs:274`)

### Correction from Phase 0: RuntimeCompactionSkipped is now emitted

The Phase 0 skeleton stated `RuntimeCompactionSkipped` was "defined, handled in `ProgressState`, mapped in transports, but **never emitted**." This is no longer true. The event is now emitted from both compaction paths:

- `runtime_compaction.rs:378` — runner engine path emits `Skipped` when `compact_via_engine` returns `EngineCompactionResult::Skipped`.
- `executor/compaction.rs:222` — executor manual path emits `Skipped` similarly.

**Verified by test**: `runtime_compaction.rs:589` asserts `RuntimeCompactionSkipped` is emitted when the tail fits without compaction.

### Event payload schema: unchanged from original

The Phase 0 skeleton proposed: "Redesign event payloads to carry block-id, token-before/after, and reason. Transport string names stay stable; payload schema evolves."

The **actual** implementation did not redesign the payload schema. The fields remain the same as the original `AgentEvent` variants — no `block-id` was added. The `generation: u32` field in `RuntimeCompactionCompleted` (`progress.rs:245`) was retained, not replaced by block graph state. `CompactionBackend` remains single-variant `LocalLlmSummary` (`types.rs:156`).

The improvement was in **emission semantics** only: `RuntimeCompactionSkipped` now actually fires, and duplicate event emitters were unified into shared `emit_runtime_compaction_started/completed/failed/skipped` helpers.

### ProgressState consumers (verified `progress.rs:897-985`)

- `last_compaction_status`: surfaced in `ProgressState`, rendered by Telegram `progress_render.rs:290-303` and web `server/types.rs:537`.
- `repeated_compaction_warning`: surfaced in `ProgressState` (`server/types.rs:537`).

### Transport mappings

- **Web**: `web_transport.rs:45-49` maps variants to stable strings (`compaction_started`, `compaction_completed`, `compaction_failed`, `compaction_skipped`, `repeated_compaction_warning`). Payload construction in `compaction_completed_parts` (`web_transport.rs:1227`).
- **Web contracts**: `TaskEventKind::RuntimeCompaction{Started,Completed,Failed,Skipped}` in `oxide-agent-web-contracts/src/events.rs:40-43`.
- **Web UI**: `oxide-agent-web-ui/src/tasks/activity.rs:543-599` renders compaction events with labels (`compacting`, `compacted`, `compaction failed`, `compaction skipped`, `repeated compaction`).
- **Telegram**: `progress_render.rs:410-415` maps `BudgetState` to labels (healthy/warning/compact soon/over limit); `progress_render.rs:290-303` renders `last_compaction_status`.
- **Milestone**: `execution.rs:178` emits `Milestone { name: "pre_run_compaction_done" }`. Web E2E tracks `pre_run_compaction_done_ms` (`server/types.rs:583`, `task_executor.rs:918`).

## 4. Tool History Repair Contract (verified)

### Repair functions (verified `agent/recovery.rs:32-52`)

```rust
pub fn repair_agent_message_history(messages: &mut Vec<AgentMessage>)
    // → repair_agent_message_history_with_policy(messages, false)

pub fn repair_agent_message_history_runtime(messages: &mut Vec<AgentMessage>)
    // → repair_agent_message_history_with_policy(messages, true)

pub fn repair_agent_message_history_for_provider(messages: &mut Vec<AgentMessage>, strict_tool_history: bool)
    // → repair_agent_message_history_with_policy(messages, !strict_tool_history)
```

The file moved from `runner/recovery.rs` to `agent/recovery.rs`. The runtime
policy (`true`) preserves the terminal open tool batch (assistant tool calls
without matching tool results — the in-flight batch). The `for_provider`
variant was added to handle provider-specific strictness: when
`strict_tool_history` is true, the preserve-terminal policy is **inverted**
(strict providers require complete batches).

Internal: `repair_agent_message_history_with_policy(messages, preserve_terminal_open_batch: bool)` (`recovery.rs:149`).

### replace_compacted_history: deleted

The Phase 0 skeleton referenced `replace_compacted_history` which rejected
histories needing repair (`InvalidToolHistory` error). This method was
**deleted** in Phase 8 as part of removing the old replacement pipeline. The
replacement path is now `AgentMemory::replace_messages` (`memory.rs:789`),
which calls `repair_history_after_mutation` (`memory.rs:797`) internally —
repair happens automatically on any message replacement, and
`compaction_state` is reset.

### Block boundary tool-call safety: enforced

The hard constraint — **block boundaries must not split tool-call/result pairs** — is enforced by the engine. `auto_select.rs:492-500` asserts:

```rust
"tool batch must not be split: range [{start_idx}, {end_idx}], batch [{batch_start}, {batch_end}]"
```

The automatic selection algorithm collects tool batches (assistant tool calls + their `ToolResult`s) atomically — the tail boundary never splits a batch.

**Verified by tests**: `block_render_includes_full_tool_batch` (`renderer.rs:468`) shows a block covering a full tool batch (call+result) renders correctly with no orphaned results or dangling calls. `block_render_multiple_non_overlapping_blocks` shows multiple blocks render without breaking tool-call pairing.

## 5. Runner → Provider Boundary (verified)

### As-built flow

1. `AgentRunnerContext.messages: &'a mut Vec<Message>` — the model-facing message buffer.
2. `refresh_messages_from_memory(ctx)` (`token_snapshots.rs:93`): `*ctx.messages = ctx.agent.memory().rendered_messages()` — delegates to the renderer.
3. `AgentMemory::rendered_messages()` (`memory.rs:858`): `CompactionRenderer::render(&self.messages, &self.compaction_state, &RenderPolicy::default())` — the **single boundary** where raw messages become model-facing messages.
4. `chat_with_tools_single_attempt(ctx.system_prompt, ctx.date_suffix, ctx.messages, ...)` sends `ctx.messages` to the provider.
5. Compaction mutates `CompactionState` only (via `CompactionEngine`); `refresh_messages_from_memory` then rebuilds `ctx.messages` from the updated state.

The function name `refresh_messages_from_memory` was retained (not renamed to `refresh_rendered_messages` as the Phase 0 skeleton proposed), but its body is the target implementation — it delegates to `rendered_messages()` which calls the renderer.

### Legacy entry point

`AgentRunner::convert_memory_to_messages` (`mod.rs:105`) is the legacy 1:1 conversion entry point. It delegates to `CompactionRenderer::render` with empty compaction state and default policy, producing identity-equivalent output. Production code should use `AgentMemory::rendered_messages()` instead.

### Rendered context metrics

- `rendered_messages()` (`memory.rs:858`): full `Vec<Message>` from raw + state.
- `rendered_token_count()` (`memory.rs:872`): full model-facing estimate for rendered output, including content, reasoning, tool-call correlation IDs, tool names, and serialized tool calls. It may be smaller than raw context after blocks become active.
- `rendered_item_count()` (`memory.rs:894`): item count of rendered output.
- `compaction_state()` / `compaction_state_mut()` (`memory.rs:900-907`): read/write access to the overlay state.

When `CompactionState` is empty, rendered messages and item count are identity-equivalent to raw memory. The rendered token estimate can exceed the legacy raw text-only counter because it includes provider-facing tool metadata.

## 6. As-Built Architecture — Component Boundaries

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         AgentMemory (persisted)                             │
│  ┌─────────────────────┐  ┌─────────────────────────────────────────────┐  │
│  │  messages:          │  │  compaction_state: CompactionState           │  │
│  │  Vec<AgentMessage>  │  │  #[serde(default)]                            │  │
│  │  (never destroyed   │  │  blocks, refs, strategy decisions             │  │
│  │   by compaction)    │  │                                               │  │
│  └─────────────────────┘  └─────────────────────────────────────────────┘  │
│  todos, token_count, max_tokens, last_api_usage                             │
└─────────────────────────────────────────────────────────────────────────────┘
           │                                    │
           ▼                                    ▼
┌─────────────────────┐           ┌──────────────────────────┐
│ CompactionRenderer  │◄──────────│ CompactionEngine          │
│ render(raw, state,  │           │ apply_compression() —     │
│   policy)           │           │ sole mutation authority   │
│ → Vec<Message>      │           │ for CompactionState       │
└─────────────────────┘           └──────────────────────────┘
           │                                    ▲
           ▼                                    │
┌─────────────────────    ┌───────────────┐    │
│ Runner LLM call     │    │ Compaction     │───┘
│ (ctx.messages =     │    │ Controller     │
│  rendered context)  │    │ compact_via_   │
└─────────────────────┘    │ engine()       │
                             └───────────────┘
                                     ▲
            ┌────────────────────────┴──────────────────────┐
            │ Automatic pre-sampling/context-limit and      │
            │ transport operator triggers                   │
            └───────────────────────────────────────────────┘

Agent `compress` tool ── selection + summary ──► CompactionEngine

┌──────────────────────┐
│ ContextAdmission     │  (stateless gate, not through engine)
│ evaluate(descriptor, │  → Inline / Manifest / ControlledPause
│   budget)            │  runs before add_message on all ingress paths
└──────────────────────┘
```

### Mutability rules

- **Raw messages**: only mutated by `add_message`, `replace_messages` (from non-compaction paths), `clear`. Compaction **never** touches raw messages. `replace_messages` and `clear` reset `compaction_state` to default.
- **CompactionState**: only mutated by `CompactionEngine::apply_compression`. The engine is called by all compaction trigger paths through `CompactionController::compact_via_engine` or directly.
- **Rendered messages**: produced fresh before every LLM call by `CompactionRenderer::render` via `AgentMemory::rendered_messages()`. Never stored, never mutated.
- **ContextAdmission**: gates external/tool payloads before they reach `add_message`. Returns `Inline`, `Manifest`, or `ControlledPause` — does not mutate state itself; the caller acts on the decision.

### Engine API (verified)

`CompactionEngine` (`engine.rs:58`) has a single public method:

```rust
pub fn apply_compression(
    state: &mut CompactionState,
    selection: &CompressionSelection,
    summary: &[SummaryPart],
) -> Result<BlockResult, CompactionError>
```

`CompactionController` (`controller.rs:82`) orchestrates: selects a compressible range, generates an LLM summary via `CompactSummaryBackend`, and calls `apply_compression`:

```rust
pub async fn compact_via_engine(
    &self,
    memory: &mut AgentMemory,
    route: &ModelInfo,
    task: &str,
    tools: &[ToolDefinition],
    system_prompt: &str,
    reason: CompactionReason,
    phase: CompactionPhase,
) -> Result<EngineCompactionResult, CompactionControllerError>
```

### Enum variants (verified `types.rs:117-170`)

```rust
pub enum CompactionTrigger { PreRun, PreIteration, Manual }

pub enum CompactionReason { PreTurn, MidTurn, Manual, ContextLimit }

pub enum CompactionPhase { PreSampling, MidTurn, Manual }

pub enum CompactionBackend { LocalLlmSummary }  // single-variant, retained

pub enum BudgetState { Healthy, Warning, ShouldCompact, OverLimit }
```

### Trigger matrix → engine entry points (as-built)

| Trigger | Initiator | Path | Reason / Phase |
|---------|-----------|------|----------------|
| Context admission | `ContextAdmission::evaluate` (stateless gate) | `AdmissionDecision` (Inline/Manifest/ControlledPause) — **not through engine**, gate runs before `add_message` | — |
| Pre-LLM budget | `maybe_run_runtime_pre_sampling_compaction` (`runtime_compaction.rs:40`) | `run_engine_compaction` → `compact_via_engine` | `PreTurn`/`MidTurn` / `PreSampling` |
| Agent compress | `compress` tool → `apply_compress_through_engine` (`tools.rs:474`) | `CompactionEngine::apply_compression` directly (`tools.rs:506`) | — |
| User/manual (transport) | `compact_current_context` (`executor/compaction.rs:14`) | `compact_via_engine` directly (`executor/compaction.rs:50`) | `Manual` / `Manual` |
| Typed overflow | `error.is_context_overflow()` (`llm_calls.rs:578`) | `run_runtime_context_limit_compaction` → `run_engine_compaction` | `ContextLimit` / `MidTurn` |

The runner is the sole owner of the automatic threshold and evaluates the full rendered request against the active model context window. The controller performs no second budget decision; it only selects, summarizes, and applies. `HotContextHealthHook` is warning-only. All successful compaction paths produce a `CompactionState` transition via `CompactionEngine::apply_compression`, then the renderer rebuilds model-facing messages.

**Deviation from Phase 0 design**: the original trigger matrix proposed separate engine methods (`admit_payload`, `compact_for_budget`, `compact_on_demand`, `emergency_shrink`). None of these exist. The actual API is simpler: one engine method (`apply_compression`) + one controller orchestrator (`compact_via_engine`) + one stateless admission gate (`ContextAdmission::evaluate`).

## 7. Verified Checks

All checks from the Phase 0 skeleton have been resolved:

| Check (Phase 0) | Status | Evidence |
|-----------------|--------|----------|
| Provider structured error bodies | **Resolved** (with deviation) | Classification centralized in `try_classify_context_overflow` (`error.rs:179`), not per-provider. Substring matching over HTTP error bodies gated by typed status code (400/413/None). See §2. |
| Block boundary + tool-call safety | **Verified** | `auto_select.rs:492-500` asserts batches are not split; `block_render_includes_full_tool_batch` (`renderer.rs:468`) proves rendering. No formal proptest, but unit tests cover the constraint. |
| Storage round-trip with CompactionState | **Verified** | `old_json_without_compaction_state_deserializes` (`memory.rs:1035`) proves backward-compatible deserialization. |
| Transport event payload backward compat | **Verified** | Stable string names (`web_transport.rs:45-49`), web contracts (`events.rs:40-43`), web UI rendering (`activity.rs:543-599`), Telegram (`progress_render.rs:290-415`). No payload schema change. |

## 8. Rendering Strategies (dedup / purge-error)

The renderer applies stateless strategies during rendering, not stored in `CompactionState`. Implemented in `strategy.rs`:

### RenderPolicy (verified `strategy.rs:22-38`)

```rust
pub struct RenderPolicy {
    pub protected_tools: Vec<String>,
    pub turn_protection: usize,      // default: 3
    pub purge_error_age_turns: usize, // default: 5
}
```

Centralizes rendering-time strategy parameters. All call sites use
`RenderPolicy::default()` consistently — no scattered independent defaults.

### Dedup: superseded tool results (`strategy.rs:106`)

`compute_superseded_tool_results` identifies tool results that are superseded
by later results for the same operation:
- Same-path `read_file` superseded by a later `read_file` of the same path.
- `read_file` superseded by `write_file` or `apply_file_edit` to the same path (write/edit intervention).
- General same-argument tool calls superseded by later identical calls.
- Protected tools bypass dedup.
- Recent turns are protected by `turn_protection` boundary.

### Purge-error: old errored tool inputs (`strategy.rs`)

`compute_purge_error_inputs` identifies old errored tool calls marked
`pruned_artifact` for purging. Recent errors are protected by
`purge_error_age_turns`.

Both strategies are **stateless** (П0: don't store what can be computed),
applied during rendering, and never remove messages or break tool-call/result
pairing — they only modify content/arguments of existing messages.

## 9. ContextAdmission (verified `admission.rs`)

### Stateless admission gate

`ContextAdmission` (`admission.rs:195`) is a stateless struct with one method:

```rust
pub fn evaluate(descriptor: &PayloadDescriptor, budget: &AdmissionBudget) -> AdmissionDecision
```

### Decision types (`admission.rs:116-185`)

```rust
pub enum AdmissionDecision {
    Inline,                                    // payload fits inline
    Manifest(ManifestSpec),                    // bounded manifest + externalized payload
    ControlledPause(AdmissionBlocker),         // safe continuation impossible
}

pub enum AdmissionBlocker {
    PayloadExceedsContextWindow { payload_tokens, route_window },
    NoBudgetForManifest { available_tokens, manifest_tokens },
}
```

`ManifestSpec` (`admission.rs:133`) carries:
- `manifest_content`: bounded manifest for model-visible message (~1500 chars).
- `externalized_payload`: `ExternalizedPayload` with lossless `inline_fallback` (raw content stored, not counted in `token_count`, not rendered to model).

### Untrusted data marking

Manifest content is explicitly marked `[Externalized content — untrusted data]`
so prompt-injection text from files/tools cannot become model instructions.
The manifest provides a retrieval hint (`Use read_file with offset/limit
parameters to retrieve specific sections`) for retrievable payloads.

**Verified by test**: `manifest_marks_content_as_untrusted` (`admission.rs:771`).

### Ingress paths (all three gated)

| Path | Call site | Budget source |
|------|-----------|---------------|
| Tool output | `apply_runtime_tool_output` (`tools.rs:349`) → `ContextAdmission::evaluate` (`tools.rs:399`) | `compute_admission_budget(ctx)` (`tools.rs:528`) — route context window |
| Runtime context | `apply_pending_runtime_context` (`execution.rs:199`) → `ContextAdmission::evaluate` (`execution.rs:228`) | `compute_admission_budget(ctx)` — route context window from `ctx.config.model_routes` |
| New task | `prime_session_for_execution` (`executor/execution.rs:285`) → `ContextAdmission::evaluate` (`executor/execution.rs:319`) | `memory.max_tokens()` as context window estimate (no route info at executor time — pre-LLM budget trigger re-checks with accurate numbers) |

**Verified by tests**: `new_task_admission_inline_for_normal_input` (`executor/tests/basics.rs:805`), `new_task_admission_manifest_for_oversized_input` (`executor/tests/basics.rs:829`), `payload_exceeds_entire_window_pause` (`admission.rs:828`).

## 10. Emergency Chunked Summarization (verified `admission.rs:372-465`)

### Infrastructure

```rust
pub trait EmergencySummarizer: Send + Sync { ... }

pub enum SummarizeError { Unavailable, Failed(String) }

pub fn split_into_chunks(content: &str, chunk_chars: usize) -> Vec<String>
pub fn summarize_in_chunks(...) -> Result<ChunkSummaryResult, SummarizeError>
```

`split_into_chunks` (`admission.rs:413`) is paragraph-boundary-aware.
`summarize_in_chunks` (`admission.rs:465`) runs chunk-by-chunk summarization
+ a summary-of-summaries block. On any failure, it degrades to
`SummarizeError` — the caller falls back to manifest-only.

### Current production path

The current admission flow uses **manifest-only** (the safe degradation). The
`EmergencySummarizer` trait and chunking infrastructure are tested and ready
for LLM-backed implementation, but manifest-only is the correct fallback when
summarization is unavailable. Architecture is complete; the production path is
the safe degradation.
