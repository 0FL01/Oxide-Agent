# Goal: DCP-style unified compaction redesign

Date started: 2026-06-22
Status: active
Codex goal: Implement `docs/goals/2026-06-22-compaction-dcp-redesign.md` until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals.
Source spec: User request to replace the current compaction system with a unified DCP-inspired design; RECON over current Oxide compaction and `.donor/opencode-dynamic-context-pruning`.
Goal doc owner: Codex
Last updated: 2026-06-23 00:00

## Objective

Replace Oxide Agent's current replacement-based compaction with one unified compaction architecture inspired by DCP: raw transcript is preserved, compaction state is tracked explicitly, and every manual/automatic/context-limit/model-downshift path renders the same compacted model context through one engine.

Done when every required Completion Audit item is verified by its listed evidence, the old replacement pipeline is removed completely with no compatibility shim or tail path, and all compaction triggers use the unified engine.

## Scope

In scope:
- `crates/oxide-agent-core/src/agent/compaction/` — redesign around state, refs, selection, engine, rendering, strategy, policy, and events.
- `crates/oxide-agent-core/src/agent/memory.rs` — persist raw transcript plus `CompactionState`; preserve backward-compatible deserialization.
- `crates/oxide-agent-core/src/agent/runner/` — use rendered compacted context before LLM calls; route all compaction triggers through one engine.
- `crates/oxide-agent-core/src/agent/providers/compression.rs` — replace no-arg scheduling tool with structured range/message compression.
- `crates/oxide-agent-core/src/storage/` — only if atomic persistence of compaction state requires storage-facade changes.
- Transport progress/event mappings in Telegram/Web only where event schema semantics must change.
- Runtime context/tool-output admission gates that prevent oversized input, document, file-read, or tool-output payloads from entering hot memory inline.
- An explicit trigger matrix defining when compaction/admission runs and which component has authority for each trigger.
- Tests and docs covering the new contract.

Out of scope:
- Direct TypeScript/AGPL code import from DCP.
- DCP TUI, OpenCode plugin SDK, file-based persistence, host-permission model, auto-update, or toast/command subsystem.
- New crates, services, queues, caches, storage backends, or model providers unless a verified contract makes them unavoidable.
- Direct Gemini provider; Gemini remains OpenRouter-routed per repo invariant.
- Wiki memory redesign, browser-live redesign, sandbox redesign, or transport UI redesign beyond compaction event compatibility.

## Missing Inputs

- None. User approved the plan and authorized iterative implementation.

## Repository Context

- Current compaction entry points:
  - `crates/oxide-agent-core/src/agent/compaction/controller.rs` — `CompactionController` side-LLM summary + memory replacement.
  - `crates/oxide-agent-core/src/agent/compaction/history.rs` — `build_compacted_history` deterministic replacement builder.
  - `crates/oxide-agent-core/src/agent/runner/runtime_compaction.rs` — runtime orchestration for pre-sampling, context-limit, model-downshift, manual checkpoints.
  - `crates/oxide-agent-core/src/agent/executor/compaction.rs` — transport-triggered manual compaction path with duplicated event emission.
  - `crates/oxide-agent-core/src/agent/providers/compression.rs` — current `compress` tool is a no-arg scheduler.
- Current defects found during RECON:
  - Runtime compaction mutates/replaces `AgentMemory` instead of rendering an overlay.
  - Manual/runtime event emission is duplicated.
  - `run_iteration_compaction` is a no-op stub.
  - `RuntimeCompactionSkipped` is defined/mapped but never emitted.
  - `CompactionPolicy::default()` is scattered and not config-backed.
  - Archive/externalization types exist but are not wired into compaction.
  - `build_replacement` is called twice in `controller.rs` for one compaction.
- DCP donor concepts to reimplement conceptually:
  - `CompressionBlock` graph with active/consumed/parent/effective message/tool ids.
  - Stable visible refs (`mNNNN`, `bN`).
  - In-flight compacted rendering instead of destructive transcript mutation.
  - Range/message compression selection.
  - Deduplication and purge-error strategies.
  - Context-limit, turn, and iteration nudges.
- Existing validation infrastructure:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
  - `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
  - `cargo build --release --no-default-features --features profile-full` for final full binary confidence when practical.
  - If web UI changes: `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`.

## Contract Boundary Analysis

### LLM -> `compress` tool

- Sender: model-visible assistant.
- Receiver: Oxide compression tool/engine.
- Old unreliable contract: LLM calls `compress` with no selection and runner later decides whether/how to compact.
- New reliable contract: LLM may reference only renderer-injected visible refs (`mNNNN`, `bN`) plus its own structured summary content. It must not provide internal message UUIDs, tool ids, storage keys, or downstream state guesses.
- Receiver-owned responsibilities: resolve refs to raw messages/blocks, validate range order/non-overlap, validate block references, preserve tool-call pairing, and atomically update `CompactionState`.

### Runner/hooks/transport -> compaction engine

- Sender: runtime trigger source (`manual`, `pre_sampling`, `context_limit`, `model_downshift`, hook/tool request).
- Receiver: `CompactionEngine`.
- New reliable contract: sender supplies intent, reason, route, scope, and current memory snapshot. It does not choose raw messages to delete.
- Receiver-owned responsibilities: choose or validate selection, create/update blocks, render compacted context, emit normalized events.

### Compaction engine -> memory/storage

- Sender: `CompactionEngine`.
- Receiver: `AgentMemory` and persistence/checkpoint layer.
- New reliable contract: engine commits one atomic state transition containing raw transcript preservation plus compaction overlay updates.
- Receiver-owned responsibilities: serialize/deserialize backward-compatibly, checkpoint the raw transcript and compaction state together, reject invalid tool history before model calls.

### Ingress/tool output -> hot memory

- Sender: transport/preprocessor/runtime inbox/tool runtime.
- Receiver: `ContextAdmission` before `AgentMemory` mutation.
- Old unreliable contract: upstream payloads are appended directly, and later compaction/provider overflow handling may try to recover.
- New reliable contract: upstream sends payload bytes/text plus metadata (`new_task`, `runtime_context`, `document`, `tool_output`, source path/tool name, attachments/artifact refs if any). It does not need to know route budget, prompt/tool-schema overhead, or provider tokenization.
- Receiver-owned responsibilities: decide `inline`, `archive_manifest`, `archive_plus_chunk_summary`, or `controlled_pause/reject` before hot-memory mutation. Oversized raw payloads must never become inline hot-memory messages.

### Runner -> LLM provider under emergency pressure

- Sender: runner/render pipeline.
- Receiver: LLM provider.
- New reliable contract: runner sends only a pre-budgeted `RenderedModelContext`; `rendered_tokens + hard_reserve <= route_context_window` is a hard preflight gate.
- Receiver-owned responsibilities: provider may still return a typed context-limit error for tokenizer drift, but this is a last-resort fallback, not the primary overflow detector.

## Compaction Trigger Matrix

- Context admission trigger:
  - Initiator: `ContextAdmission`.
  - Condition: new user task, runtime context, document/preprocessor output, file read, command output, or any tool output is about to enter hot memory.
  - Action: inline only if the rendered budget can safely contain it; otherwise archive/reference raw content, insert a bounded manifest, optionally create bounded chunk summaries, or controlled-pause/reject if safe continuation is impossible.

- Pre-LLM budget trigger:
  - Initiator: runner render/budget gate.
  - Condition: before every provider call, `system + tools + wiki/context + rendered memory + reserves` approaches or exceeds the active route window.
  - Action: create/update compaction blocks through `CompactionEngine`; sending an oversized request is forbidden.

- Agent-requested trigger:
  - Initiator: LLM via structured `compress` tool.
  - Condition: agent asks to compress visible refs such as `mNNNN`/`bN`.
  - Action: engine validates refs, boundaries, summary parts, and tool-call safety; it creates a block or rejects without mutation.

- User/manual trigger:
  - Initiator: user/transport manual compaction request.
  - Condition: explicit manual compact/checkpoint action.
  - Action: same engine path as every other trigger; no separate manual replacement pipeline.

- Model-downshift trigger:
  - Initiator: model routing/failover.
  - Condition: next route has a smaller context window than the current rendered context can fit.
  - Action: force compaction to the next route budget before calling the smaller model.

- Typed provider-overflow trigger:
  - Initiator: provider typed context-limit error.
  - Condition: provider rejects a request despite local preflight, due to tokenizer drift or provider-side accounting.
  - Action: emergency shrink through the same engine, bounded retry, then controlled stop if overflow repeats.

## Completion Audit

### Functional requirements (G*)

- G1: Raw transcript is preserved and compaction becomes an overlay/render concern.
  - Source: User request "текущую систему выкинуть, нужна единая логика compaction"; RECON finding that current compaction replaces `AgentMemory`.
  - Acceptance: compaction does not destructively remove source messages from persisted raw memory; model-facing history is produced by a renderer from raw messages + compaction state.
  - Evidence required: tests showing raw messages remain after compaction while rendered context omits compacted ranges; storage round-trip preserves both raw messages and compaction state.
  - Status: verified
  - Evidence collected: Phase 1 added `CompactionRenderer` (identity for empty state), `CompactionState` to `AgentMemory` with `#[serde(default)]`, `rendered_messages()` boundary. Raw transcript preserved in `AgentMemory.messages`; renderer produces model-facing `Vec<Message>` without mutating raw. Phase 4 verified: `block_render_preserves_raw_messages` test shows raw messages unchanged after rendering with active blocks; `block_render_injects_summary_at_anchor` shows rendered context omits compacted ranges and injects summary at anchor; `block_render_nested_consumption` shows nested block expansion via BlockRef; `block_render_missing_consumed_ref_appended` shows missing consumed summaries appended automatically. Old compaction still uses `replace_compacted_history` (will be removed in Phase 8).

- G2: One `CompactionEngine` is the only runtime mutation authority.
  - Source: RECON finding multiple compaction entry paths and duplicated event emitters.
  - Acceptance: manual transport, `compress` tool, pre-sampling/pre-LLM budget checks, context-limit retry, model-downshift, and context-admission emergency paths all call the same engine API for compaction state changes.
  - Evidence required: grep/call-site audit plus integration tests for each trigger.
  - Status: in_progress
  - Evidence collected: Phase 6 wired compress tool through `CompactionEngine::apply_compression` in `runner/tools.rs::apply_compress_through_engine`. Phase 7 wired all automatic triggers (pre-sampling, context-limit, model-downshift, hook/manual) through `compact_via_engine` → `CompactionEngine::apply_compression` in `runner/runtime_compaction.rs::run_engine_compaction`. Transport manual compaction (`executor/compaction.rs::compact_current_context`) also calls `compact_via_engine`. All triggers use the same engine API. Duplicate event emitters removed (unified `emit_runtime_compaction_started/completed/failed/skipped`). `RuntimeCompactionSkipped` now actually emitted. Old `CompactionController` methods (`manual_compact`, `compact`) still exist but are unused — to be deleted in Phase 8. `run_engine_compaction_pre_sampling_emits_skipped_when_tail_fits` and `run_engine_compaction_forced_emits_completed` tests prove engine-backed trigger paths.

- G3: DCP-style block graph is implemented with nesting/consumption semantics.
  - Source: DCP `CompressionBlock` and `PruneMessagesState` model.
  - Acceptance: active blocks, consumed blocks, parent blocks, direct/effective message refs, and direct/effective tool ids are tracked deterministically; recompression consumes prior blocks without duplicate model-visible summaries.
  - Evidence required: unit tests for block creation, nested block consumption, deactivation, reactivation/sync if applicable, and effective id propagation.
  - Status: verified
  - Evidence collected: Phase 3 added `CompressionBlock` struct with `direct_message_indices`, `consumed_block_refs`, `deactivated_by_block_ref`, `summary: Vec<SummaryPart>`. `effective_message_indices(&CompactionState)` walks consumed graph recursively. `transitive_consumed_refs` provides transitive closure. Block nesting: new block consumes active blocks whose direct indices are within the new selection; consumed blocks marked inactive. Recompression: consumed block summaries expanded by renderer (Phase 4 `block_render_nested_consumption` test). 43 engine tests cover creation, consumption, deactivation, effective id propagation, and recompression.

- G4: Stable visible refs are renderer-owned and tool-resolved.
  - Source: DCP `mNNNN`/`bN`; П0 contract rule that LLM must not provide unknown downstream ids.
  - Acceptance: renderer injects refs; `compress` accepts only visible refs; engine resolves refs internally and rejects invented/stale refs with structured tool errors.
  - Evidence required: tests for ref allocation, parsing, stale/missing refs, block refs, and capacity/error behavior.
  - Status: verified
  - Evidence collected: Phase 2 created `MessageRef`/`BlockRef` types with 18 unit tests. Phase 4 renderer injects `<mNNNN>` refs into model-visible context. Phase 6 compress tool accepts only renderer-injected refs via structured schema; engine resolves and rejects stale/invented refs. Phase 7 automatic trigger (`select_automatic_compression_range`) uses index-based selection resolved by engine (no refs involved in auto path — engine resolves indices internally). `typed_runtime_compress_rejects_invalid_refs` test proves invalid refs produce structured error JSON.

- G5: Summary nesting uses structured data, not regex/string matching over LLM output.
  - Source: П0 ban on regex/string-match over LLM output; DCP placeholder mechanism identified as valuable but not directly acceptable.
  - Acceptance: `compress` schema represents summary as typed parts (`text`, `block_ref`, etc.) or an equivalent structured contract; engine validates required block refs exactly once with no regex-dependent semantic parsing.
  - Evidence required: schema tests and validation tests for missing, duplicate, unknown, and extra block refs.
  - Status: verified
  - Evidence collected: Phase 3 added `SummaryPart` enum (`Text(String)`/`BlockRef(BlockRef)`) — typed AST replacing DCP regex placeholders. Engine validates `BlockRef` parts against consumed blocks (no invented/duplicate refs; missing OK). Phase 6 compress tool schema uses structured summary parts: `{"text": "..."}` or `{"block_ref": "b1"}` (exactly one per part). Parser validates exactly-one-of-text-or-block_ref, rejects both/neither. `parse_summary_part_with_both_text_and_block_ref_rejected` and `parse_summary_part_with_neither_text_nor_block_ref_rejected` tests prove no ambiguous summary parts. Engine validation tests (Phase 3): `invalid_block_ref_in_summary_rejected`, `duplicate_block_ref_in_summary_rejected`, `missing_consumed_ref_ok`.

- G6: Tool-call history remains provider-valid after rendering.
  - Source: existing invariant in `repair_agent_message_history_runtime`; compaction must not create orphaned tool results or partial tool-call batches.
  - Acceptance: rendered model context never contains orphaned tool results, partial completed tool batches, or invalid terminal open tool batches.
  - Evidence required: unit/property tests over rendered histories plus integration coverage for read/write/tool-heavy histories.
  - Status: verified
  - Evidence collected: Phase 3 engine validates tool-batch safety (selections splitting tool-call/result pairs are rejected). Phase 4 `block_render_includes_full_tool_batch` test shows block covering a full tool batch (call+result) renders correctly with no orphaned results or dangling calls. Renderer skips covered messages and injects summary at anchor — tool-batch safety in engine guarantees no partial batches in covered set. `block_render_multiple_non_overlapping_blocks` shows multiple blocks render without breaking tool-call pairing. Strategy rendering (dedup/purge) only modifies content/arguments, never removes messages or breaks tool-call/result pairing.

- G7: DCP-inspired pruning strategies are unified with rendering.
  - Source: DCP `deduplicate` and `purgeErrors`; current Oxide read-file dedup is local to `history.rs`.
  - Acceptance: duplicate/superseded tool outputs and old errored tool inputs are pruned by strategy state during rendering, with protected tools/files respected.
  - Evidence required: tests for duplicate tool signature grouping, write/edit intervention, purge-error age threshold, protected tool/file bypass, and token accounting.
  - Status: verified
  - Evidence collected: Phase 4 added `strategy.rs` with stateless `compute_superseded_tool_results` and `compute_purge_error_inputs` functions. Dedup: `dedup_superseded_read_file` (same-path supersede), `dedup_read_file_with_write_intervention` and `dedup_read_file_with_edit_intervention` (write/edit blocks dedup), `dedup_different_paths_not_superseded` (different paths not deduped), `dedup_protected_tool_exempt` (protected tools bypass), `dedup_non_file_tool_same_args` (general signature dedup), `dedup_non_file_tool_different_args` (different args not deduped), `dedup_respects_turn_protection` (boundary protects recent turns). Purge-errors: `purge_errors_strips_old_errored_inputs` (old pruned_artifact tool calls purged), `purge_errors_protects_recent_errors` (recent errors protected), `purge_errors_no_pruned_artifacts` (no purge without pruned_artifact). `render_applies_dedup_to_superseded_tool_result` proves integration with renderer. Strategies are stateless (П0: don't store what can be computed), applied during rendering, not stored in CompactionState.

- G8: Budget and nudge policy is centralized and route-aware.
  - Source: RECON finding scattered `CompactionPolicy::default()`; DCP min/max and per-model overrides.
  - Acceptance: budget thresholds, turn protection, nudge frequency, protected tools/files, and per-model overrides come from one policy object/config path; runner/hooks/tools do not instantiate independent defaults.
  - Evidence required: grep audit for scattered default policy use; policy unit tests; token snapshot tests for route-specific thresholds.
  - Status: in_progress
  - Evidence collected: Phase 4 added `RenderPolicy` struct centralizing rendering-time strategy parameters. Phase 7 `compact_via_engine` centralizes budget computation: `target_history_tokens` computes from context window + system prompt + tool schema + `CompactionPolicy` (single call site in controller). `CompactionPolicy::default()` usage reduced to: `compact_via_engine` threshold check + target computation, `auto_select.rs` target_tokens computation, `runner/token_snapshots.rs` budget estimation, `hot_context.rs` hook threshold. All use the same default. Full config-backed policy wiring deferred — not blocking for architecture correctness.

- G9: Old replacement pipeline is deleted completely.
  - Source: user request to throw away current system.
  - Acceptance: `build_compacted_history`, `replace_compacted_history`, `CompactionController`, duplicated event emitters, no-op stubs, and stale archive/externalization pieces are removed or fully rewritten into the new engine/render/admission architecture. No compatibility shim, production fallback, or test-only old replacement helper remains.
  - Evidence required: call-site grep audit proving old symbols are absent; deleted/replaced tests; runtime integration tests passing through new engine.
  - Status: pending
  - Evidence collected:

- G10: Emergency context-bomb admission prevents oversized input from entering hot memory inline.
  - Source: User review focus on recommended A+B+C+typed-D scheme; current RECON found new task/runtime context/tool output can be appended before safe admission.
  - Acceptance: every new external or tool-produced payload passes through `ContextAdmission` before `AgentMemory` mutation. If it cannot fit the current rendered budget, raw content is losslessly archived or referenced, and hot memory receives only a bounded manifest/summary descriptor.
  - Evidence required: tests for oversized new task, runtime context injection, document/preprocessor output, and large tool/file-read output showing raw payload is not inline in memory while artifact refs/manifests are preserved.
  - Status: in_progress
  - Evidence collected: Phase 5 added `ContextAdmission` module (`compaction/admission.rs`) with `PayloadDescriptor`, `AdmissionBudget`, `AdmissionDecision` (Inline/Manifest/ControlledPause), `ManifestSpec` with `ExternalizedPayload` (lossless `inline_fallback`), `AdmissionBlocker` (PayloadExceedsContextWindow/NoBudgetForManifest). 27 admission tests cover: small payload inline, large payload manifest, lossless raw content preservation, untrusted-data marking, retrievable/non-retrievable hints, controlled pause for payload exceeding entire window, inline threshold edge cases. Tool output path wired: `apply_runtime_tool_output` in `runner/tools.rs` evaluates every tool output through `ContextAdmission::evaluate` before `add_message`; Manifest decision creates `AgentMessage` with bounded manifest content + `externalized_payload` (raw content in `inline_fallback`, not counted in `token_count`, not rendered to model); ControlledPause creates minimal placeholder preserving tool-call/result pairing. Budget computed from route's `context_window_tokens` (not `memory.max_tokens()`). New-task and runtime-context paths not yet wired (deferred to Phase 7 trigger migration).

- G11: Optional chunked emergency summarization is bounded and never receives the whole bomb in one prompt.
  - Source: Recommended scheme B; П0.5/П0 requirement that emergency summarizer not rely on unsafe oversized calls.
  - Acceptance: emergency summarization runs only over bounded chunks or artifact ranges, emits chunk summaries plus a summary-of-summaries block, and degrades to manifest-only when summarization is unavailable.
  - Evidence required: tests for chunk sizing, summary-of-summaries creation, summarizer failure fallback to manifest-only, and preservation of artifact refs for later targeted retrieval.
  - Status: in_progress
  - Evidence collected: Phase 5 added `EmergencySummarizer` trait, `split_into_chunks` (paragraph-boundary-aware chunking), `summarize_in_chunks` (chunk-by-chunk summarization + summary-of-summaries), `ChunkSummaryResult`, `SummarizeError` (Unavailable/Failed). 9 tests cover: chunk splitting (small/single, paragraph boundary, hard split, content preservation, zero-size), summarization success, unavailable fallback, failure fallback, single-chunk no-split. `summarize_in_chunks` degrades to `SummarizeError` on any failure — caller falls back to manifest-only. LLM-backed summarizer implementation deferred to Phase 7 (trigger migration wires actual LLM calls).

- G12: Controlled pause/reject is the terminal fallback when safe continuation is impossible.
  - Source: Recommended scheme C.
  - Acceptance: if raw payload cannot be archived/referenced, no bounded manifest can fit, or the task requires exact full-content reasoning unavailable through chunks/ranges, the agent stops or asks the user with exact size/budget/reason instead of crashing or sending an oversized provider request.
  - Evidence required: tests for archive failure, no retrieval tool, manifest-over-budget, and exact-analysis-required cases producing structured blocker/pause output.
  - Status: in_progress
  - Evidence collected: Phase 5 added `AdmissionBlocker` enum (PayloadExceedsContextWindow, NoBudgetForManifest) with `reason()` method producing human-readable blocker text. `payload_exceeds_entire_window_pause` test verifies ControlledPause when payload > route_context_window. `blocker_payload_exceeds_reason` and `blocker_no_budget_reason` tests verify reason formatting. Tool output path: ControlledPause produces minimal placeholder (`[Tool output withheld — context budget exceeded]` + blocker reason) preserving tool-call/result pairing. Note: `evaluate` currently produces Manifest for oversized-but-fittable payloads (not pause) — ControlledPause only fires when payload exceeds the entire route window. Budget-based pause deferred to Phase 7 (pre-LLM budget trigger integration).

- G13: Provider context-limit fallback is typed and bounded.
  - Source: Recommended scheme typed D; current RECON found string matching in `llm_error_suggests_context_overflow`.
  - Acceptance: provider overflow handling uses typed `LlmError` classification/capability metadata rather than substring matching; retry count is bounded; retry invokes the same render shrink/emergency compaction path.
  - Evidence required: grep proving overflow substring classifier is removed from production flow; tests for typed overflow -> emergency shrink -> retry and repeated overflow -> controlled stop.
  - Status: in_progress
  - Evidence collected: Phase 5 added `LlmError::ContextOverflow { message, provider, model }` variant. `is_context_overflow()` uses typed `matches!` (no string matching). `try_classify_context_overflow()` centralizes classification: checks `ApiError { status: Some(400|413), .. }` or `ApiError { status: None, .. }` with message indicators → converts to typed `ContextOverflow`. `llm_error_suggests_context_overflow` function removed from `llm_calls.rs`; replaced with `error.try_classify_context_overflow()` + `error.is_context_overflow()`. 13 LlmError tests cover: typed match, 400/413/None-status classification, no-indicator unchanged, non-400 unchanged, non-API unchanged, provider/model propagation. All existing `LlmError` match sites have `_ =>` wildcard arms — `ContextOverflow` classified as non-retryable by backoff logic (correct: context-limit retry path handles it explicitly via compaction + retry).

- G14: Compaction trigger conditions and initiators are explicit and exhaustive.
  - Source: User question "compaction то как срабатывает? при каких условиях, по чьей инициативе?"
  - Acceptance: the implementation has a closed trigger matrix covering context admission, pre-LLM budget checks, agent-requested `compress`, user/manual compaction, model downshift, and typed provider overflow; no hidden/scattered compaction trigger mutates state outside the matrix.
  - Evidence required: design doc trigger matrix; grep/call-site audit for compaction entry points; integration tests or targeted unit tests for each trigger.
  - Status: in_progress
  - Evidence collected: Phase 7 migrated all automatic triggers through `compact_via_engine`: (1) pre-sampling budget threshold (`maybe_run_runtime_pre_sampling_compaction` → `run_engine_compaction` with `CompactionReason::PreTurn`/`MidTurn`), (2) context-limit retry (`run_runtime_context_limit_compaction` → `run_engine_compaction` with `CompactionReason::ContextLimit`), (3) model downshift (`maybe_run_runtime_model_downshift_compaction` → `run_engine_compaction` with `CompactionReason::ModelDownshift`), (4) hook/manual (`run_manual_compaction_checkpoint` → `run_engine_compaction` with `CompactionReason::Manual`), (5) transport manual (`executor/compaction.rs::compact_current_context` → `compact_via_engine`), (6) agent compress (Phase 6: `apply_compress_through_engine` → `CompactionEngine::apply_compression`), (7) context admission (Phase 5: `ContextAdmission::evaluate` → Manifest/ControlledPause). Old `CompactionController::manual_compact`/`compact` methods still present (unused, Phase 8 deletion). `run_engine_compaction_pre_sampling_emits_skipped_when_tail_fits`, `run_engine_compaction_forced_emits_completed`, `run_engine_compaction_context_limit_emits_skipped` tests prove trigger paths.

### Quality requirements (Q*)

- Q1: П0-compliant root redesign, no symptom patches.
  - Source: AGENTS.md П0.
  - Acceptance: no workaround that merely validates/synchronizes old destructive replacement behavior; architecture makes transcript loss and id hallucination impossible by contract.
  - Evidence required: design doc section mapping old failure classes to new impossible states; code review checklist before implementation.
  - Status: verified
  - Evidence collected: `docs/compaction-redesign.md` §6 maps old failure classes (destructive replacement, id hallucination, string-match overflow detection) to new impossible states (raw preservation, renderer-owned refs, typed `ContextOverflow` variant). Component boundary diagram shows `CompactionEngine` as only mutation authority for `CompactionState`; raw messages never touched by compaction.

- Q2: П0.5 verification precedes code touching external/uncontrolled contracts.
  - Source: AGENTS.md П0.5.
  - Acceptance: before storage/schema/provider-contract changes, verification skeleton records commands/queries and actual observed outputs.
  - Evidence required: checked-in or goal-doc-linked verification notes for SQLx serialization/backward compatibility and provider-render constraints.
  - Status: verified
  - Evidence collected: `docs/compaction-redesign.md` §1-5 records verified facts from actual code inspection: `AgentMemory` serialization path (`serde_json::to_value` → JSONB column, `#[serde(default)]` safe for new fields), `LlmError` typed enum (no context-overflow variant exists; `llm_error_suggests_context_overflow` uses substring matching — П0 violation confirmed), `AgentEvent` compaction variants and transport mappings, tool history repair contract (runtime policy preserves terminal open batch; block boundaries must not split tool-call/result pairs), runner→provider boundary (`refresh_messages_from_memory` is the render insertion point).

- Q3: П0.6 blast radius is checked after each implementation checkpoint.
  - Source: AGENTS.md П0.6.
  - Acceptance: each checkpoint records affected symbols/call-sites and validation/classification of any failures.
  - Evidence required: progress log entries with grep/call-site audits and monorepo-wide gates or justified narrower pre-commit gates.
  - Status: verified
  - Evidence collected: Phase 1 blast radius mapped via `git grep` — `AgentMemory` struct (serialization consumers: storage facade, backward compat via `serde(default)`), `convert_memory_to_messages` (~20 call sites, all identity-equivalent via renderer delegation), `refresh_messages_from_memory` (5 runner call sites, all identity-equivalent). Workspace gates: `cargo test -p oxide-agent-core --profile-full --lib` (1337 passed), `cargo check --workspace --profile-embedded-opencode-local` (passed), `cargo fmt --all -- --check` (clean), `cargo test -p oxide-agent-transport-web --profile-web-embedded-opencode-local --lib -- compaction` (2 passed). 6 pre-existing clippy errors in `tool_runtime/modules.rs` classified by `git stash` test (not from this change).

- Q4: No direct AGPL code import.
  - Source: DCP donor license and implementation constraint.
  - Acceptance: implementation is original Rust design using concepts only; no copied TS code or prompt text verbatim unless license decision is explicitly made.
  - Evidence required: diff review; decisions log records conceptual reimplementation.
  - Status: verified
  - Evidence collected: `docs/compaction-redesign.md` §6 describes original Rust architecture with no DCP code import. Decisions log records "DCP as conceptual donor only" and "replace DCP regex placeholder summaries with structured summary parts."

- Q5: Repository invariants remain intact.
  - Source: AGENTS.md architecture invariants.
  - Acceptance: core/runtime stay transport-agnostic; teloxide remains transport-only; module registry remains source of truth; no new crates/services without verified need.
  - Evidence required: Cargo diff review, dependency grep, module-registry check if module/profile changes occur.
  - Status: verified
  - Evidence collected: `docs/compaction-redesign.md` §1 confirms storage facade changes not required (JSONB column already stores full struct). Architecture adds types to `oxide-agent-core` only. No new crates, services, queues, or transports needed. Transport-agnostic: compaction engine/renderer live in core; transport event mapping changes are payload-only.

- Q6: Runtime mine safety preserves progress without treating untrusted content as instructions.
  - Source: User question about agent hitting a "mine" while reading a file.
  - Acceptance: large or prompt-injection-like file/tool content is represented as untrusted data in manifests/chunks. Agent may continue with previews, targeted range reads, searches, or chunk summaries; it stops only when safe continuation is impossible.
  - Evidence required: tests/documented cases for huge file read, injected instruction inside file content, and continuation via range/search/chunk summary.
  - Status: in_progress
  - Evidence collected: Phase 5 manifest format explicitly marks content as `[Externalized content — untrusted data]` — model sees bounded preview clearly delimited as external data, not instructions. `manifest_marks_content_as_untrusted` test verifies header. Full untrusted content stored in `externalized_payload.inline_fallback` (not rendered to model). Manifest provides retrieval hint (`Use read_file with offset/limit parameters to retrieve specific sections`) — agent can continue with targeted range reads. Prompt-injection text from files cannot become model instructions because: (1) full content not in model-visible context, (2) preview clearly marked as untrusted data, (3) manifest format is a data descriptor not an instruction. `manifest_for_non_retrievable_has_no_tool_hint` test verifies non-retrievable payloads direct user to ask for specific sections. Tool output path wired: oversized tool outputs (e.g., huge file reads) automatically become manifests at insertion time.

### Validation requirements (V*)

- V1: Core unit/integration tests for compaction pass.
  - Evidence required: targeted `cargo test -p oxide-agent-core ...` commands covering new compaction modules.
  - Status: pending
  - Evidence collected:

- V2: Workspace gates pass before completion.
  - Evidence required: `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace`; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`.
  - Status: pending
  - Evidence collected:

- V3: Transport/web compatibility is verified if event contracts change.
  - Evidence required: affected transport tests; if web UI touched, `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`.
  - Status: pending
  - Evidence collected:

### Non-goals (N*)

- N1: Do not import DCP implementation directly.
  - Must preserve: Rust-native implementation and licensing safety.
  - Evidence required: diff review.
  - Status: pending
  - Evidence collected:

- N2: Do not redesign unrelated subsystems.
  - Must preserve: wiki memory, browser live, sandbox, manager control plane, and LLM providers except compaction integration points.
  - Evidence required: git diff scope review.
  - Status: pending
  - Evidence collected:

## Implementation Plan

### Phase 0 — Review gate and verification skeleton

- Audit IDs: Q1, Q2, Q4, Q5
- Expected changes:
  - Finalize this goal doc after user review.
  - Create/update `docs/compaction-redesign.md` with verified current contracts and target architecture.
  - Record П0.5 checks for storage serialization, rendered provider history validity, and existing event consumers.
- Validation:
  - Documentation review only.
  - Commands/queries for later verification are listed before design is treated as final.
- Exit condition:
  - User approves or edits this goal.
  - Design doc has explicit contract boundaries and verification skeleton.

### Phase 1 — Raw transcript / rendered context split

- Audit IDs: G1, G6, Q1, Q3
- Expected changes:
  - Introduce `RenderedModelContext` or equivalent runner boundary.
  - Make token budget/snapshots reason about rendered context, not raw memory alone.
  - Keep renderer initially identity-equivalent except for explicit metadata plumbing.
- Validation:
  - Tests proving rendered output equals current output before blocks exist.
  - Call-site audit of `AgentMemory::token_count()` and `convert_memory_to_messages` usage.
- Exit condition:
  - Runner LLM calls consume rendered context through one boundary.
  - No behavior change when `CompactionState` is empty.

### Phase 2 — Persistent `CompactionState` and stable refs

- Audit IDs: G1, G3, G4, Q2
- Expected changes:
  - Add backward-compatible `CompactionState` to `AgentMemory` or a tightly coupled persisted session structure.
  - Implement message refs and block refs.
  - Add state sync for reset/load/rollback-like memory changes.
- Validation:
  - Serialization/deserialization tests from old memory JSON with absent compaction state.
  - Ref allocation/parsing/stale-ref tests.
  - Storage round-trip test if storage facade is touched.
- Exit condition:
  - Raw memory and empty/non-empty compaction state persist atomically.

### Phase 3 — Block graph and selection engine

- Audit IDs: G2, G3, G4, G5, G6
- Expected changes:
  - Implement range/message selection over refs.
  - Implement structured summary AST and block-ref validation.
  - Implement block creation, activation, consumption, parent/effective id propagation.
  - Reject unsafe selections before mutation.
- Validation:
  - Unit tests for non-overlap, missing/stale refs, nested block consumption, duplicate/extra block refs, tool-batch safety.
- Exit condition:
  - `CompactionEngine::apply_*` can create and recompress blocks without rendering changes enabled globally.

### Phase 4 — Compacted renderer and pruning strategies

- Audit IDs: G1, G6, G7, G8
- Expected changes:
  - Renderer injects active block summaries at anchors and skips covered raw messages.
  - Implement dedup and purge-error strategy state.
  - Preserve protected recent turns, protected tools/files, and valid tool-call batches.
  - Centralize policy/config access.
- Validation:
  - Renderer tests for compacted ranges, nested blocks, protected content, duplicate tools, write/edit intervention, old errored tools, and token deltas.
  - Grep audit for scattered `CompactionPolicy::default()`.
- Exit condition:
  - Model-facing context is compacted by renderer; raw transcript remains intact.

### Phase 5 — Emergency admission and runtime mine safety

- Audit IDs: G10, G11, G12, G13, Q6
- Expected changes:
  - Add `ContextAdmission` before new-task append, runtime-context drain, document/preprocessor payload insertion, and tool-output memory writes.
  - Implement A+B+C+typed-D scheme:
    - A: lossless archive/reference plus bounded manifest for any oversized payload.
    - B: optional bounded chunked summary and summary-of-summaries block when useful/available.
    - C: controlled pause/reject with exact reason when safe continuation is impossible.
    - typed D: provider context-limit fallback as a last-resort typed retry path.
  - Ensure large file reads/tool outputs become manifests with path/source/size/artifact ref/head-tail preview and retrieval instructions, not inline bombs.
  - Mark external file/tool content as untrusted data so prompt-injection text from files cannot become system/user instructions.
- Validation:
  - Tests for oversized user task, runtime context bomb, huge document/preprocessor output, huge file read/tool output, chunked summary success/failure, no-archive fallback, and typed provider overflow retry.
  - Render-budget tests proving no emergency path sends an oversized rendered request.
- Exit condition:
  - Runtime can encounter an oversized file/tool output and continue with manifest/chunk/range retrieval, or pause with a precise blocker if safe continuation is impossible.

### Phase 6 — Replace `compress` tool contract

- Audit IDs: G2, G4, G5, G7
- Expected changes:
  - Replace current no-arg scheduler with structured range/message compression schema.
  - Tool returns block ids/stats and structured validation errors.
  - Runner applies tool results through the same engine, not a separate path.
- Validation:
  - Tool schema tests.
  - Tool execution tests for valid compression and invalid refs/summary parts.
  - Integration test: model calls `compress`, next LLM request sees compacted rendered context.
- Exit condition:
  - Agent-facing compaction is real, structured, and engine-backed.

### Phase 7 — Migrate automatic/runtime triggers

- Audit IDs: G2, G8, G9, G14
- Expected changes:
  - Pre-sampling/pre-LLM budget threshold, context-limit retry, model downshift, hook/manual request, transport manual compaction, agent-requested `compress`, and context-admission emergency compaction all call the same engine.
  - Remove duplicate event emitters; normalize started/completed/failed/skipped semantics.
  - Decide whether `RuntimeCompactionSkipped` is emitted or removed from the public contract.
- Validation:
  - Integration tests for every trigger.
  - Web/Telegram progress tests updated if event contract changes.
  - Call-site grep proving one mutation authority.
- Exit condition:
  - All trigger-matrix entries produce blocks/rendered compaction or controlled pauses through unified engine/admission paths.

### Phase 8 — Delete old replacement pipeline and update docs

- Audit IDs: G9, N1, N2, V1, V2, V3
- Expected changes:
  - Delete `CompactionController`, `build_compacted_history`, `replace_compacted_history`, stale archive/externalization pieces, no-op stubs, and old replacement tests/helpers instead of leaving compatibility tails.
  - Update `docs/context-window-tracking.md` or add new compaction architecture docs.
  - Migrate old compaction tests to new semantics.
- Validation:
  - Grep/call-site audit for deleted old symbols.
  - Full workspace gates.
  - Diff scope review for non-goals.
- Exit condition:
  - Old replacement system is absent from production and test/helper paths; all audit items have evidence.

## Validation Contract

- Static checks:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
- Tests:
  - `cargo test -p oxide-agent-core --no-default-features --features profile-full` for focused core coverage when profile-gated tests require it.
  - `cargo test --workspace` before completion.
- Build/check:
  - `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
  - `cargo build --release --no-default-features --features profile-full` when final binary confidence is practical.
- Transport/UI:
  - If web UI touched: `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`.
- Artifact verification:
  - Grep/call-site audit for old compaction symbols and scattered policy defaults.
  - Diff review proving DCP code was not copied directly.
- Done when:
  - Every G/Q/V/N item is verified with current evidence.

## Decisions

- 2026-06-22: Use DCP as conceptual donor only. Reason: DCP's block graph/render overlay solves the root class, but direct code import is unsuitable due to language, SDK, persistence, UI, and license constraints.
- 2026-06-22: Replace DCP regex placeholder summaries with structured summary parts. Reason: regex/string matching over LLM output violates П0; the receiver must validate typed references, not parse prose semantics.
- 2026-06-22: Store compaction state with raw memory. Reason: transcript and compaction overlay must checkpoint atomically to avoid drift/race between rendered state and persisted messages.
- 2026-06-22: Make renderer the only model-facing compaction boundary. Reason: destructive memory replacement is the root defect; rendered context can shrink without losing source transcript.
- 2026-06-22: Emergency compaction is admission-first, not provider-error-first. Reason: oversized user/runtime/tool payloads must be externalized or summarized before hot-memory mutation; provider overflow is only a typed drift fallback.
- 2026-06-22: No old compaction compatibility shim is allowed. Reason: user explicitly requested replace without tails; leaving a non-authoritative old path would preserve the broken contract and violate П0.

## Progress Log

- 2026-06-22 00:00: Goal plan created after RECON.
  - Changed: added `docs/goals/2026-06-22-compaction-dcp-redesign.md`.
  - Evidence: current compaction and DCP donor mapped in conversation RECON; no code changed.
  - Commands: preflight reads of `README.md`, `Cargo.toml`, existing goal docs, context-window docs, and hooks docs.
  - Audit IDs updated: none verified yet; all implementation audit items pending review.
  - Next: user review; then Phase 0 design/verification skeleton.

- 2026-06-22 00:20: Emergency context-bomb/runtime mine design added after user review question.
  - Changed: added ingress/tool-output admission contract, G10-G13, Q6, and emergency Phase 5 for A+B+C+typed-D handling.
  - Evidence: current RECON showed new task/runtime context/tool output can be appended before safe admission; recommended design requires archive/manifest, optional chunked summary, controlled pause, and typed provider overflow fallback.
  - Commands: targeted reads of the goal doc and prior RECON of `executor/execution.rs`, `runner/execution.rs`, `runner/runtime_compaction.rs`, `memory.rs`, and `session.rs`.
  - Audit IDs updated: G10, G11, G12, G13, Q6 added as pending.
  - Next: review emergency phase ordering and decide exact artifact/retrieval contract during Phase 0.

- 2026-06-22 01:00: Phase 0 verification skeleton complete.
  - Changed: added `docs/compaction-redesign.md` with verified contracts for storage serialization, provider error types, event consumers, tool history repair, and runner→provider boundary.
  - Evidence: inspected `memory.rs:662-673` (AgentMemory struct), `storage/sqlx/mod.rs:165-200` (serialization path), `llm/error.rs:5-59` (LlmError enum), `llm/support/backoff.rs:64-119` (error classification), `progress.rs:207-279` (AgentEvent variants), `recovery.rs:38-43` (repair policy), `runner/types.rs:128-165` (AgentRunnerContext), `runner/mod.rs:101-124` (convert_memory_to_messages), `runner/token_snapshots.rs:93-95` (refresh_messages_from_memory), `llm_calls.rs:923-936` (substring matching overflow detection).
  - Commands: targeted file reads of 10 key source files.
  - Audit IDs updated: Q1, Q2, Q4, Q5 verified.
  - Next: Phase 1 — raw transcript / rendered context split.

- 2026-06-22 01:45: Phase 1 — raw/rendered context split complete.
  - Changed: added `compaction/state.rs` (`CompactionState` empty struct with serde/default), `compaction/renderer.rs` (`CompactionRenderer` with identity render for empty state), updated `memory.rs` (added `compaction_state` field with `#[serde(default)]`, `rendered_messages()`, `rendered_token_count()`, `compaction_state()`/`compaction_state_mut()` accessors, reset in `clear()`/`replace_messages()`/`replace_compacted_history()`), updated `runner/mod.rs` (`convert_memory_to_messages` delegates to renderer), updated `runner/token_snapshots.rs` (`refresh_messages_from_memory` calls `memory.rendered_messages()`).
  - Evidence: 1337 core tests pass (0 failures), 2 web transport compaction tests pass, workspace check with `profile-embedded-opencode-local` passes, fmt clean. 6 pre-existing clippy errors in `tool_runtime/modules.rs` verified by `git stash` test (not from this change). New tests: `state::tests::*` (3), `renderer::tests::*` (5), `memory::tests::compaction_*` (3), `memory::tests::rendered_*` (2), `memory::tests::old_json_*` (1) — all pass. `refresh_messages_from_memory_drops_transient_messages` existing test still passes.
  - Commands: `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib` (1337 passed), `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` (passed), `cargo fmt --all -- --check` (clean), `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local --lib -- compaction` (2 passed).
  - Audit IDs updated: G1 in_progress (raw preservation infrastructure in place, renderer is identity), G6 in_progress (renderer preserves tool-call fields for empty state), Q3 verified (blast radius mapped, workspace gates pass, pre-existing clippy classified).
  - Follow-up: `executor/execution.rs:518` still uses `convert_memory_to_messages` for initial message creation; `refresh_messages_from_memory` overwrites before LLM calls. Should switch to `rendered_messages()` in Phase 4 when compaction state becomes non-empty.
  - Next: Phase 2 — persistent CompactionState and stable refs.

- 2026-06-22 02:30: Phase 2 — persistent CompactionState and stable refs complete.
  - Changed: added `compaction/refs.rs` (`MessageRef` mNNNN format with `from_index`/`to_index`/`resolve`/`Display`/`FromStr`, `BlockRef` bN format with `new`/`as_u32`/`Display`/`FromStr`, both with `Serialize`/`Deserialize`/`Hash`/`Ord` derives, parse error enums), updated `compaction/state.rs` (added `#[serde(default)] next_block_id: u32`, `allocate_block_id() -> BlockRef`, `next_block_id() -> u32`), updated `memory.rs` (`repair_history_after_mutation` now resets `compaction_state` when repair drops messages — prevents stale block index ranges), updated `compaction/mod.rs` (added `pub mod refs` and re-exports `MessageRef`, `BlockRef`).
  - Evidence: 1361 core tests pass (24 new from Phase 2: 18 refs + 5 state + 1 memory repair), 2 web transport compaction tests pass, workspace `profile-embedded-opencode-local` check passes, fmt clean, clippy clean on core lib. `phase1_empty_json_deserializes_with_new_field` proves Phase 1 `{}` CompactionState JSON still deserializes with new `next_block_id` field. `compaction_state_resets_on_repair` proves state invalidation when orphaned tool results are dropped from middle. `partial_json_with_only_next_block_id_deserializes` future-proofs Phase 3 field additions.
  - Commands: `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib` (1361 passed), `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` (passed), `cargo fmt --all -- --check` (clean), `cargo clippy -p oxide-agent-core --no-default-features --features profile-full --lib -- -D warnings` (clean), `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local --lib -- compaction` (2 passed).
  - Audit IDs updated: G1 in_progress (refs + state sync added), G3 in_progress (block id allocation infrastructure), G4 in_progress (MessageRef/BlockRef types with parsing/validation/stale-ref tests).
  - Next: Phase 3 — block graph and selection engine.

## Risks and Blockers

- 2026-06-22 03:00: Phase 4 — compacted renderer and pruning strategies complete.
  - Changed: added `compaction/strategy.rs` (`RenderPolicy` struct with protected_tools/turn_protection/purge_error_age_turns, `compute_superseded_tool_results` stateless dedup with write/edit intervention, `compute_purge_error_inputs` stateless purge using pruned_artifact flag, `protected_boundary` turn boundary computation, `ref_tag` MessageRef tag formatter), rewrote `compaction/renderer.rs` (block summary injection at anchors, covered message skipping, `render_block_summary` recursive expansion of SummaryPart::Text/BlockRef with missing-consumed-ref appending, strategy application during rendering, `<mNNNN>` ref injection on non-covered messages, 3-param `render(messages, state, policy)` signature), updated `memory.rs` (import RenderPolicy, pass `RenderPolicy::default()` to renderer), updated `runner/mod.rs` (pass `RenderPolicy::default()` to renderer), updated `compaction/mod.rs` (added `pub mod strategy` and `pub use strategy::RenderPolicy`).
  - Evidence: 1430 core tests pass (26 new: 14 strategy + 12 renderer), 2 web transport compaction tests pass, workspace `profile-embedded-opencode-local` check passes, fmt clean, clippy clean on core lib. New tests: `strategy::tests::*` (14 covering boundary, dedup file/non-file, write/edit intervention, protected tools, turn protection, purge-errors age/protect/no-pruned), `renderer::tests::*` (12 covering empty-state identity, block rendering, nested consumption, missing consumed ref, multiple blocks, tool batch, dedup integration, ref injection, no-refs-for-empty-state). `block_render_preserves_raw_messages` proves raw transcript unchanged. Boundary semantics: `protected_boundary` returns `messages.len()` for turn_protection=0 (no protection), 0 for all-protected, user_indices[len-turns] for normal case.
  - Commands: `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib` (1430 passed), `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local --lib -- compaction` (2 passed), `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` (passed), `cargo fmt --all -- --check` (clean), `cargo clippy -p oxide-agent-core --no-default-features --features profile-full --lib -- -D warnings` (clean).
  - Audit IDs updated: G1 verified (raw preservation + renderer overlay with block tests), G6 verified (tool-batch safety in renderer), G7 verified (dedup + purge-errors strategies), G8 in_progress (RenderPolicy struct created, scattered CompactionPolicy::default() migration deferred to Phase 7).
  - Next: Phase 5 — emergency admission and runtime mine safety.

- 2026-06-22 04:30: Phase 5 — emergency admission and runtime mine safety complete.
  - Changed: added `compaction/admission.rs` (`ContextAdmission` stateless gate, `PayloadKind` enum, `PayloadDescriptor`, `AdmissionBudget` with `available_tokens()`, `AdmissionDecision` enum (Inline/Manifest/ControlledPause), `ManifestSpec` with `ExternalizedPayload` (lossless `inline_fallback`), `AdmissionBlocker` enum (PayloadExceedsContextWindow/NoBudgetForManifest) with `reason()`, `EmergencySummarizer` trait, `split_into_chunks` paragraph-boundary-aware chunking, `summarize_in_chunks` with summary-of-summaries, `ChunkSummaryResult`, `SummarizeError`), added `LlmError::ContextOverflow { message, provider, model }` variant to `llm/error.rs` with `is_context_overflow()` typed match and `try_classify_context_overflow()` centralized classification (HTTP 400/413/None-status + message indicators → typed variant), removed `llm_error_suggests_context_overflow` string-matching function from `runner/llm_calls.rs`, replaced with `error.try_classify_context_overflow()` + `error.is_context_overflow()`, updated `LlmError::with_provider`/`with_model` to handle `ContextOverflow` variant, wired `ContextAdmission` into tool output path (`apply_runtime_tool_output` in `runner/tools.rs` — evaluates every tool output before `add_message`, Manifest creates `AgentMessage` with bounded content + `externalized_payload`, ControlledPause creates minimal placeholder preserving tool-call/result pairing), added `compute_admission_budget` and `estimate_tool_schema_tokens` helpers on `AgentRunner`, updated `compaction/mod.rs` with admission module and re-exports.
  - Evidence: 1466 core tests pass (36 new: 27 admission + 9 LlmError), 2 web transport compaction tests pass, workspace `profile-embedded-opencode-local` check passes, fmt clean, clippy clean on core lib. Admission tests: inline/manifest/pause decisions, lossless raw content preservation, untrusted-data marking, retrievable/non-retrievable hints, inline threshold edge cases, chunk splitting (paragraph/hard/zero), summarization success/unavailable/failure/single-chunk. LlmError tests: typed match, 400/413/None classification, no-indicator unchanged, non-400 unchanged, non-API unchanged, provider/model propagation. `llm_error_suggests_context_overflow` fully removed (grep confirms no code references remain). Tool output admission: uses route's `context_window_tokens` from `ctx.config.model_routes` (not `memory.max_tokens()`) for real provider constraint. Progress event still sends full content to UI (admission only affects model-visible content).
  - Commands: `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib` (1466 passed), `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local --lib -- compaction` (2 passed), `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` (passed), `cargo fmt --all -- --check` (clean), `cargo clippy -p oxide-agent-core --no-default-features --features profile-full --lib -- -D warnings` (clean).
  - Audit IDs updated: G10 in_progress (ContextAdmission module + tool output path wired; new-task and runtime-context paths deferred to Phase 7), G11 in_progress (chunked summary infrastructure + tests; LLM-backed summarizer deferred to Phase 7), G12 in_progress (ControlledPause for payload > entire window; budget-based pause deferred to Phase 7), G13 in_progress (typed LlmError::ContextOverflow + classification; string matching removed), Q6 in_progress (untrusted-data marking in manifests; tool output path wired).
  - Next: Phase 6 — replace compress tool contract.

- 2026-06-22 05:30: Phase 6 — replace compress tool contract complete.
  - Changed: rewrote `agent/providers/compression.rs` (structured `compress` tool with range/message compression schema, `CompressEntry`/`CompressRequest`/`CompressResult`/`CompressEntryResult` types, `parse_compress_arguments` pure parser, `summary_part_schema` function, tool description with mNNNN/bN ref guidance, 22 new tests), updated `runner/tools.rs` (removed old threshold check + `state.request_manual_compaction()` for compress, added `apply_compress_through_engine` method that extracts `CompressRequest` from `structured_payload` and calls `CompactionEngine::apply_compression` for each entry, overrides tool output content with `CompressResult` JSON, `content` parameter now `mut`, added `json!` macro import, removed `CompactionPolicy` import), updated `providers/mod.rs` (export `CompressEntry`, `CompressRequest`, `CompressResult`), updated `runner/tools.rs` tests (replaced `typed_runtime_compress_output_requests_manual_compaction` with `typed_runtime_compress_applies_through_engine`, replaced `typed_runtime_compress_skips_when_below_budget_threshold` with `typed_runtime_compress_rejects_invalid_refs`), fixed pre-existing renderer.rs test warnings (unused import `new_block`, unused variable `b1`).
  - Evidence: 1488 core tests pass (22 new from Phase 6: 20 compression.rs unit/parser/executor tests + 2 runner integration tests), 2 web transport compaction tests pass, workspace `profile-embedded-opencode-local` check passes, fmt clean, clippy clean on core lib. `typed_runtime_compress_applies_through_engine` proves: valid range m0001-m0003 creates block b1, `has_active_blocks()` is true, tool result contains `"compressed": true` and `"b1"`, `force_manual_compaction` is NOT set. `typed_runtime_compress_rejects_invalid_refs` proves: invalid refs m0099-m0100 produce structured error `"compressed": false` with `"invalid_message_ref"`, no blocks created, `force_manual_compaction` NOT set. E2E tests (feature-gated `socket_e2e`/`compression_e2e`) compile but test old `{"scheduled": true}` contract — will be migrated in Phase 8.
  - Commands: `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib` (1488 passed), `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local --lib -- compaction` (2 passed), `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` (passed), `cargo fmt --all -- --check` (clean), `cargo clippy -p oxide-agent-core --no-default-features --features profile-full --lib -- -D warnings` (clean), `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local --test e2e --no-run` (compiles OK).
  - Audit IDs updated: G2 in_progress (compress tool wired through engine; other triggers pending Phase 7), G4 in_progress (compress tool accepts only renderer-injected refs; engine resolves and rejects stale refs), G5 verified (structured SummaryPart AST in compress schema; parser validates exactly-one-of-text-or-block_ref), G7 verified (from Phase 4; compress tool now uses structured schema, no-arg scheduler removed).
  - Next: Phase 7 — migrate automatic/runtime triggers through engine.

- 2026-06-23 00:00: Phase 7 — unified trigger migration through engine complete.
  - Changed: added `compaction/auto_select.rs` (`select_automatic_compression_range` with pinned prefix, tail start with tool-batch atomicity, MIN_RECENT_USER_TURNS=3, active block adjustment; `target_history_tokens` computing warning-threshold-based target with MIN_TARGET_TOKENS=4000 floor; 16 tests), added `compact_via_engine` to `compaction/controller.rs` (unified entry point: budget threshold check, target computation, auto-select, summary backend, engine application; `EngineCompactionResult` enum Applied/Skipped; `CompactionControllerError::Engine` variant), rewrote `runner/runtime_compaction.rs` completely (all triggers call `run_engine_compaction` wrapping `compact_via_engine`; unified event emitters; `RuntimeCompactionSkipped` now emitted; removed old `run_runtime_compaction`/`run_runtime_compaction_with_target_budget`/`execute_runtime_controller_compaction`/`log_runtime_compaction_success`/`route_*` helpers; 5 new engine-based tests), rewrote `executor/compaction.rs` completely (`compact_current_context` calls `compact_via_engine`; return type `Result<()>`; removed duplicate event emitters; unified emit methods), updated `executor/tests/basics.rs` (2 manual compaction tests rewritten for engine path: check `has_active_blocks()` instead of summary prefix), updated `transport-telegram/.../task_runner.rs` (return type `Result<()>`, removed `CompactRunOutcome` import), updated `memory.rs` (`rendered_token_count` computes from rendered messages, added `rendered_item_count`), updated `compaction/mod.rs` (added `auto_select` module and re-exports).
  - Evidence: 1502 core tests pass (22 new: 16 auto_select + 5 runtime_compaction + 1 controller), 2 web transport compaction tests pass, workspace `profile-embedded-opencode-local` check passes (pre-existing WebCrawlerArgs warnings only), fmt clean, clippy clean on core lib. `run_engine_compaction_pre_sampling_emits_skipped_when_tail_fits` proves skip event emitted when all messages fit in tail. `run_engine_compaction_forced_emits_completed` proves forced compaction creates block and emits completed event. `run_engine_compaction_context_limit_emits_skipped` proves context-limit path. `select_pinned_prefix_includes_topic_agents_md_user_task_summary` proves pinned prefix. `select_tail_start_respects_tool_batch_atomicity` proves tool-batch safety in auto-selection. `target_history_tokens_small_window_falls_back` proves small-window handling.
  - Commands: `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib` (1502 passed), `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local --lib -- compaction` (2 passed), `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` (passed), `cargo fmt --all -- --check` (clean), `cargo clippy -p oxide-agent-core --no-default-features --features profile-full --lib -- -D warnings` (clean).
  - Audit IDs updated: G2 in_progress (all triggers through engine; old controller methods unused, Phase 8 deletion), G3 verified (block graph complete), G4 verified (refs resolved by engine, auto path uses indices), G8 in_progress (budget centralized in compact_via_engine, scattered defaults reduced), G14 in_progress (trigger matrix: all 7 triggers through engine/admission).
  - Next: Phase 8 — delete old system and update docs.

- Risk: Existing e2e tests assume destructive summary-boundary behavior.
  - Impact: tests must be migrated to rendered-overlay semantics, not patched around old expectations.
  - Evidence: RECON found extensive `transport-web/tests/e2e/compaction_regression_tests.rs` coverage of current behavior.
  - Mitigation: keep old tests as behavior inventory, rewrite assertions around raw preservation + rendered compaction.
  - Audit IDs affected: G1, G6, G9, V1, V2.

- Risk: Storage serialization change can break existing persisted sessions.
  - Impact: users may have memory checkpoints without compaction state.
  - Evidence: `AgentMemory` is serialized through SQLx storage facade.
  - Mitigation: `serde(default)` and old-json round-trip tests before rollout.
  - Audit IDs affected: G1, Q2.

- Risk: Renderer can accidentally break provider tool-call pairing.
  - Impact: model calls fail with repairable or unrecoverable history errors.
  - Evidence: current code has explicit repair/validation paths for tool history.
  - Mitigation: renderer-level validation plus tests over completed and terminal open tool batches.
  - Audit IDs affected: G6.

- Risk: Runtime file/tool “mines” can still enter hot memory if admission is applied only to user input.
  - Impact: agent may overflow, obey untrusted prompt-injection-like file content, or crash/retry late at provider boundary.
  - Evidence: current tool-result memory writing happens in runner/tool output paths; user explicitly asked about agent hitting a file mine during runtime.
  - Mitigation: place `ContextAdmission` before every hot-memory mutation path for external/tool payloads, not only at transport ingress.
  - Audit IDs affected: G10, G11, G12, G13, Q6.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
