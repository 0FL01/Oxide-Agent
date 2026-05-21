# Goal: Codex-style Compaction

Date started: 2026-05-21
Status: complete
Codex goal: Implement prd/PRD.md: migrate Oxide-Agent to Codex-style runtime/session-level compaction by replacing the old active multi-stage CompactionService flow with a single CompactionController + LocalLlmSummary path, deterministic atomic history replacement, provider-agnostic behavior without OpenAI /responses/compact, compatibility with old persisted compaction data, updated progress UX, tests, docs, and rollback notes.

## Objective

Implement [prd/PRD.md](../../prd/PRD.md) end-to-end: compaction must become one runtime/session-level operation with `CompactionController`, default provider-agnostic `LocalLlmSummary`, deterministic compacted history construction, atomic session history replacement, and no active fallback to the old multi-stage `CompactionService` pipeline.

Stopping condition: all acceptance criteria in PRD section 17 are satisfied, validation commands pass, and documentation/config migration notes are updated.

## Scope

In scope:
- Replace active old compaction wiring in core runner, executor, delegation/sub-agent path, tools path, post-run response handling, and progress events.
- Add new compaction controller/task/history/local summary components.
- Add route-aware threshold logic and reason/phase metadata.
- Preserve tool call/tool result invariants, pinned context, todos, runtime context, and persisted old compaction data compatibility.
- Update Telegram/Web progress rendering and event names.
- Update `.env.example`, `README.md`, and relevant docs.
- Add/replace tests for history builder, replacement semantics, thresholds, context-limit retry, model downshift, migration fixtures, transports, and E2E regressions.

Out of scope:
- OpenAI `/responses/compact` or remote compaction adapter.
- Permanent dual active old/new compaction fallback.
- New memory subsystem or rewrite of durable Wiki memory.
- Automatic deletion of old R2 archive/payload objects.
- Unrelated runner/tool execution refactors.

## Repository Context

- Product PRD: [prd/PRD.md](../../prd/PRD.md).
- Core compaction modules: `crates/oxide-agent-core/src/agent/compaction/`.
- Runtime entrypoints: `crates/oxide-agent-core/src/agent/runner/execution.rs`, `runner/tools.rs`, `runner/responses.rs`, `runner/types.rs`.
- Executor entrypoints: `crates/oxide-agent-core/src/agent/executor.rs`, `executor/config.rs`, `executor/types.rs`, `executor/compaction.rs`.
- Sub-agent wiring: `crates/oxide-agent-core/src/agent/providers/delegation.rs`.
- State/persistence: `crates/oxide-agent-core/src/agent/memory.rs`, `session.rs`, `storage/r2_memory.rs`, `storage/compaction.rs`.
- Progress/UX: `crates/oxide-agent-core/src/agent/progress.rs`, `crates/oxide-agent-transport-telegram/src/bot/progress_render.rs`, `crates/oxide-agent-transport-web/src/web_transport.rs`.
- Config/docs: `crates/oxide-agent-core/src/config.rs`, `.env.example`, `README.md`.

Existing conventions to preserve:
- Core/runtime stay transport-agnostic.
- Transport crates depend on core/runtime, not the other way around.
- Use `thiserror` in library crates and `anyhow` in app/binary crates.
- Keep compatibility for persisted `AgentMemory` JSON and old R2 references.
- Prefer `cargo check` for fast verification, run `cargo fmt` and `cargo clippy` before finishing.

## Implementation Plan

1. Inventory and safety net: document all old active entrypoints, add initial tests for new history invariants and old-session compatibility before deleting behavior.
2. Data model and events: introduce `CompactionReason`, `CompactionPhase`, compact metadata/result types, and progress event payloads while keeping compile compatibility during migration.
3. History builder: implement detection/extraction for new and legacy summaries, deterministic `build_compacted_history`, pinned-context preservation, tool-pair validation, and atomic replacement semantics.
4. Local summary backend: implement provider-agnostic `LocalLlmSummary` using ordinary text generation, no tools, no JSON contract, configured route first and active/inherited route fallback.
5. Controller: add `CompactionController` with pre-sampling, mid-turn/context-limit, manual, and model-downshift paths; ensure failures before replacement are no-ops.
6. Wire runtime: replace runner/executor/delegation/tools/responses calls to old service with controller calls and refresh messages only after successful replacement.
7. Disable old active flow: stop constructing/passing `CompactionService`, stop emitting active prune/archive/externalize events, keep only serde/data compatibility shims.
8. UX/docs/tests: update Telegram/Web rendering, event names, `.env.example`, README/docs, and regression/E2E coverage.
9. Final cleanup: remove temporary migration switch after the new path is default and regression-safe.

## Validation Contract

- Formatting: `cargo fmt --all --check`
- Static build: `cargo check --workspace`
- Lint: `cargo clippy --workspace --all-targets --all-features`
- Focused tests while developing: `cargo test -p oxide-agent-core compaction`
- Transport regression tests: `cargo test -p oxide-agent-transport-telegram progress_render` and `cargo test -p oxide-agent-transport-web`
- E2E regression: `cargo test -p oxide-agent-transport-web --test e2e`
- Done when: all required checks pass, no active old/new dual compaction path remains, and PRD acceptance criteria are mapped to tests or documented manual checks.

## Decisions

- 2026-05-21: First implementation path is provider-agnostic `LocalLlmSummary`; OpenAI `/responses/compact` is explicitly out of scope for this goal.
- 2026-05-21: Use a repo-local goal document because the PRD is large and the goal must be resumable across Codex sessions.
- 2026-05-21: Treat old R2 archive/payload records as compatibility data only; first version must not create new archive/payload objects during compaction.
- 2026-05-21: Use one active compaction wiring path at a time during migration; no runtime fallback where old `CompactionService` and new `CompactionController` can both mutate history in one turn.
- 2026-05-21: Keep compacted summaries as `MessageRole::System` in memory. Provider-visible calls already fold system history into the top-level `system_prompt` via `support::history::fold_system_messages_into_prompt(...)`, so providers do not receive mid-history system messages.
- 2026-05-21: `OXIDE_CODEX_STYLE_COMPACTION` now defaults on. Setting it false is only a short-lived emergency disable for runner auto-compaction; production executor/delegation no longer re-enable the legacy staged pipeline.

## Progress Log

- 2026-05-21 20:05 +03: Created active Codex goal from `prd/PRD.md`. Read PRD and confirmed scope: replace old multi-stage compaction with Codex-style runtime/session-level compaction.
- 2026-05-21 20:05 +03: Initial `rg` inventory found active old compaction references in executor, runner, delegation, runner tools, runner responses, progress state, Telegram progress rendering, Web event mapping, compaction tests, config, `.env.example`, and README. Working tree was clean before creating this document.
- 2026-05-21 20:06 +03: Baseline `cargo check --workspace` passed before code changes.
- 2026-05-21 20:09 +03: Started Phase 1 foundation without runner wiring: added reason/phase/backend/metadata types, new `[OXIDE_COMPACTED_SUMMARY_V1]` summary constructor, and `compaction/history.rs` with current/legacy summary detectors plus a conservative deterministic history builder.
- 2026-05-21 20:11 +03: Validation passed after first code slice: `cargo test -p oxide-agent-core compaction::history` passed 4 tests; `cargo check --workspace` passed.
- 2026-05-21 20:19 +03: Added Phase 1/2 scaffolding without runtime wiring: `compaction/task.rs` backend contract, `local_llm_summary.rs` provider-agnostic plain-text backend, `controller.rs` with manual/context-limit controller entrypoints, local compact prompt builder, and `AgentMemory::replace_compacted_history` with pre-mutation tool-history validation.
- 2026-05-21 20:19 +03: Validation passed after controller/replacement slice: `cargo test -p oxide-agent-core compaction::` passed 109 tests; `cargo test -p oxide-agent-core replace_compacted_history` passed 2 tests; `cargo check --workspace` passed; `cargo fmt --all --check` passed.
- 2026-05-21 20:20 +03: Prepared migration switch/factory pieces for later wiring: added `AgentSettings::codex_style_compaction_enabled()` for `OXIDE_CODEX_STYLE_COMPACTION` and `CompactionController::local_llm(...)` to construct the default backend from existing compaction routes/timeouts. This does not activate the new path yet, so there is still no active dual-path mutation.
- 2026-05-21 20:20 +03: Validation passed after switch/factory slice: `cargo check -p oxide-agent-core`, `cargo test -p oxide-agent-core compaction::controller`, `cargo fmt --all --check`, and `cargo check --workspace` passed.
- 2026-05-21 20:29 +03: Wired manual compaction to the new controller behind `OXIDE_CODEX_STYLE_COMPACTION` / `oxide_codex_style_compaction`. When the flag is enabled, `AgentExecutor::compact_current_context(...)` uses `CompactionController::manual_compact(...)`, creates `[OXIDE_COMPACTED_SUMMARY_V1]`, persists a background memory checkpoint after success, and does not run old prune/archive/externalize stages for manual compaction. The old manual path remains selected when the flag is off.
- 2026-05-21 20:29 +03: Added executor regression test `manual_compaction_uses_codex_style_controller_when_flag_enabled`, proving the flag path uses plain text `chat_completion`, removes legacy `[COMPACTION_SUMMARY]`, creates exactly one new summary prefix, and reports no prune/archive side effects.
- 2026-05-21 20:29 +03: Validation passed after manual wiring slice: `cargo test -p oxide-agent-core manual_compaction_uses_codex_style_controller_when_flag_enabled`, `cargo test -p oxide-agent-core compaction::`, `cargo check --workspace`, and `cargo fmt --all --check` passed.
- 2026-05-21 20:35 +03: Added runtime/session-level compaction progress events: `RuntimeCompactionStarted`, `RuntimeCompactionCompleted`, `RuntimeCompactionFailed`, and `RuntimeCompactionSkipped`. Progress state now renders reason/phase/backend/provider/route/token/item metadata for the new path. Web transport maps runtime events to `compaction_started`, `compaction_completed`, `compaction_failed`, and `compaction_skipped`.
- 2026-05-21 20:35 +03: Updated flag-enabled manual compaction to emit only runtime compaction events, not legacy `CompactionStarted`/`CompactionCompleted`/`PruningApplied`. Extended executor regression coverage to assert the emitted event sequence is `runtime_started`, `runtime_completed`.
- 2026-05-21 20:35 +03: Validation passed after progress-event slice: `cargo test -p oxide-agent-core runtime_compaction_events_update_progress_state`, `cargo test -p oxide-agent-core manual_compaction_uses_codex_style_controller_when_flag_enabled`, `cargo test -p oxide-agent-core compaction::`, `cargo check --workspace`, and `cargo fmt --all --check` passed.
- 2026-05-21 20:44 +03: Wired the runner context to carry `CompactionController` and `codex_style_compaction_enabled`. With the flag enabled, runner manual compaction requests and context-overflow retry now use runtime controller compaction instead of `CompactionService`; context overflow uses reason `ContextLimit` and phase `MidTurn`.
- 2026-05-21 20:44 +03: Added runner regression `run_retries_after_context_overflow_with_runtime_context_limit_compaction`, proving overflow retry succeeds through `LocalLlmSummary`, emits runtime context-limit events, creates `[OXIDE_COMPACTED_SUMMARY_V1]`, removes legacy `[COMPACTION_SUMMARY]`, and emits no legacy `CompactionStarted` event on the new path.
- 2026-05-21 20:44 +03: Validation passed after runner context-limit slice: `cargo test -p oxide-agent-core run_retries_after_context_overflow_with_runtime_context_limit_compaction`, legacy overflow tests `run_overflow_retry_emits_manual_compaction_progress_in_order` and `run_retries_after_context_overflow_with_manual_compaction`, `cargo test -p oxide-agent-core compaction::`, `cargo check --workspace`, and `cargo fmt --all --check` passed.
- 2026-05-21 20:55 +03: Wired additional runtime paths behind `OXIDE_CODEX_STYLE_COMPACTION`: pre-sampling threshold compaction, context-limit controller dispatch, `compress` tool runtime controller path, and post-run legacy compaction suppression when the flag is enabled.
- 2026-05-21 20:55 +03: Added route-aware model-downshift compaction in failover selection. If the next selected route has a smaller `context_window_tokens` and the projected request no longer fits, runner emits/executes reason `ModelDownshift`, phase `ModelSwitch`, with a route-sized hot-history budget before sampling the fallback route.
- 2026-05-21 20:55 +03: Added regression `run_compacts_before_downshifting_to_smaller_model_route`, proving persistent primary 429 failover to a smaller backup route compacts first, folds the new summary into provider-visible `system_prompt`, completes on backup, and emits no legacy `CompactionStarted` / `PruningApplied` events.
- 2026-05-21 20:55 +03: Updated `.env.example` and README migration notes for `OXIDE_CODEX_STYLE_COMPACTION`, local summary routes, legacy compatibility knobs, no `/responses/compact`, no new R2 archive/payload objects, and new compaction event semantics.
- 2026-05-21 20:55 +03: Validation passed for focused runtime paths: `cargo test -p oxide-agent-core manual_compaction_uses_codex_style_controller_when_flag_enabled`, `runtime_compaction_events_update_progress_state`, `run_retries_after_context_overflow_with_runtime_context_limit_compaction`, `run_pre_sampling_uses_runtime_compaction_when_threshold_reached`, and `run_compacts_before_downshifting_to_smaller_model_route`.
- 2026-05-21 21:01 +03: Broader validation passed after docs/model-downshift/clippy cleanup: `cargo test -p oxide-agent-core compaction::`, `cargo check --workspace`, `cargo fmt --all --check`, and `cargo clippy --workspace --all-targets --all-features`.
- 2026-05-21 21:08 +03: Removed production executor/delegation ownership of `CompactionService`. `AgentExecutor` now constructs and passes only `CompactionController`; manual executor compaction always uses the runtime controller path; sub-agent delegation constructs `CompactionController::local_llm(...)` and forces Codex-style compaction in sub-agent runner config.
- 2026-05-21 21:08 +03: Changed Codex-style compaction default to enabled and updated `.env.example` / README to describe `OXIDE_CODEX_STYLE_COMPACTION=false` as an emergency auto-compaction disable, not a fallback to the legacy staged pipeline.
- 2026-05-21 21:08 +03: Validation passed for this cleanup slice: `cargo check -p oxide-agent-core`, `cargo test -p oxide-agent-core codex_style_compaction_defaults_on_and_allows_explicit_disable`, `cargo test -p oxide-agent-core manual_compaction_uses_codex_style_controller_when_flag_enabled`, and `cargo test -p oxide-agent-core --test sub_agent_delegation`.
- 2026-05-21 21:16 +03: Removed active runner fallbacks to the legacy staged pipeline. Production `AgentRunnerContext` no longer carries `CompactionService`; `compress` always uses runtime `CompactionController`; context-overflow retry no longer falls back to legacy when `OXIDE_CODEX_STYLE_COMPACTION=false`; post-run legacy cleanup is test-only compatibility. Old runner compaction helpers and service field are isolated behind `#[cfg(test)]`.
- 2026-05-21 21:16 +03: Validation passed for runner cleanup: `cargo check -p oxide-agent-core`, `cargo test -p oxide-agent-core run_pre_sampling_uses_runtime_compaction_when_threshold_reached`, `cargo test -p oxide-agent-core run_retries_after_context_overflow_with_runtime_context_limit_compaction`, `cargo test -p oxide-agent-core --lib`, `cargo check --workspace`, `cargo fmt --all --check`, and `cargo clippy -p oxide-agent-core --all-targets --all-features`.
- 2026-05-21 21:31 +03: Added transport/runtime compaction coverage. Web transport now has unit coverage for stable runtime compaction event names and for `collect_events(...)` recording `compaction_started` / `compaction_completed` without `pruning_applied`. Telegram progress rendering now has a runtime-event-specific status test. The web manual `compress` E2E assertion was strengthened to reject `pruning_applied` and require `[OXIDE_COMPACTED_SUMMARY_V1]` in session memory.
- 2026-05-21 21:31 +03: Added lazy migration fixture coverage for old `ArchiveReference` compatibility. The history builder now preserves legacy archive reference messages as compatibility pointers while filtering old `[COMPACTION_SUMMARY]` / breadcrumb summary messages and producing exactly one `[OXIDE_COMPACTED_SUMMARY_V1]`.
- 2026-05-21 21:31 +03: Validation passed for this slice: `cargo test -p oxide-agent-core compaction::history`, `cargo test -p oxide-agent-transport-web runtime_compaction_events_use_stable_web_event_names`, `cargo test -p oxide-agent-transport-web collect_events_records_runtime_compaction_without_pruning_event`, `cargo test -p oxide-agent-transport-telegram renders_runtime_compaction_status`, `cargo check --workspace`, `cargo fmt --all --check`, and `cargo clippy --workspace --all-targets --all-features`.
- 2026-05-21 21:31 +03: Attempted `cargo test -p oxide-agent-transport-web --features socket_e2e e2e_compress_tool_triggers_manual_compaction -- --nocapture`. In sandbox it failed to bind a local test server with `PermissionDenied`; the escalated rerun was interrupted by the user before completion. Treat socket E2E execution as not verified in this slice.
- 2026-05-21 21:37 +03: Closed a `compress` tool-call boundary risk in the new history builder. Runtime compaction now preserves the terminal open assistant tool-call batch when compaction runs before the tool result is recorded, preventing the subsequent `compress` result from becoming an orphaned tool message.
- 2026-05-21 21:37 +03: Added regression `preserves_terminal_open_tool_batch_for_compress_result_continuation`, proving the replacement history remains valid after appending the `compress` tool result and does not require runtime repair.
- 2026-05-21 21:37 +03: Validation passed after the tool-boundary fix: `cargo test -p oxide-agent-core preserves_terminal_open_tool_batch_for_compress_result_continuation`, `cargo test -p oxide-agent-core compaction::history`, `cargo test -p oxide-agent-core manual_compaction_uses_codex_style_controller_when_flag_enabled`, `cargo test -p oxide-agent-core run_retries_after_context_overflow_with_runtime_context_limit_compaction`, `cargo test -p oxide-agent-core run_pre_sampling_uses_runtime_compaction_when_threshold_reached`, `cargo test -p oxide-agent-core run_compacts_before_downshifting_to_smaller_model_route`, `cargo test -p oxide-agent-transport-web collect_events_records_runtime_compaction_without_pruning_event`, `cargo test -p oxide-agent-transport-telegram renders_runtime_compaction_status`, `cargo check --workspace`, `cargo fmt --all --check`, and `cargo clippy --workspace --all-targets --all-features`.
- 2026-05-21 21:41 +03: Re-ran the web socket E2E for manual `compress` compaction with local TCP binding. The first run exposed that the web compaction harness inherited the scripted ZAI tool-call provider as the compaction route, but `LocalLlmSummary` requires ordinary `chat_completion`. The harness now configures a dedicated compaction route to the narrator provider, matching the PRD's ordinary text-generation backend requirement.
- 2026-05-21 21:41 +03: Preserved `summary_updated: true` in the `compress` tool result JSON for API compatibility while keeping the new runtime metadata (`reason`, `phase`, `backend`, provider/route, token/item counts) and no old prune/archive counters.
- 2026-05-21 21:41 +03: Validation passed for the socket E2E slice: `cargo test -p oxide-agent-transport-web --features socket_e2e e2e_compress_tool_triggers_manual_compaction -- --nocapture`, `cargo test -p oxide-agent-core compaction::history`, `cargo test -p oxide-agent-core manual_compaction_uses_codex_style_controller_when_flag_enabled`, `cargo test -p oxide-agent-transport-web collect_events_records_runtime_compaction_without_pruning_event`, `cargo check --workspace`, `cargo fmt --all --check`, and `cargo clippy --workspace --all-targets --all-features`.
- 2026-05-21 21:46 +03: Added missing backend/controller acceptance coverage. `LocalLlmSummary` now has tests proving it uses ordinary plain-text `chat_completion`, trims output, rejects empty output, and fails without a usable route. `CompactionController` now has an explicit no-op failure test proving summary backend failure leaves memory messages and token accounting unchanged.
- 2026-05-21 21:46 +03: Validation passed for backend/controller coverage: `cargo test -p oxide-agent-core compaction::local_llm_summary`, `cargo test -p oxide-agent-core manual_compact_failure_does_not_mutate_memory`, `cargo test -p oxide-agent-core manual_compact_replaces_memory_with_one_prefixed_summary`, `cargo check --workspace`, `cargo fmt --all --check`, and `cargo clippy --workspace --all-targets --all-features`.
- 2026-05-21 21:49 +03: Added broader tool-heavy web socket E2E coverage. New test `e2e_compress_preserves_tool_heavy_batch_continuation` runs one assistant tool batch containing both `compress` and `write_todos`, proves runtime compaction emits started/completed events without `pruning_applied`, records both tool results, preserves valid tool-call history without repair, and keeps todos synchronized.
- 2026-05-21 21:49 +03: Validation passed for tool-heavy E2E slice: `cargo test -p oxide-agent-transport-web --features socket_e2e e2e_compress_preserves_tool_heavy_batch_continuation -- --nocapture`, `cargo test -p oxide-agent-transport-web --features socket_e2e e2e_compress_tool_triggers_manual_compaction -- --nocapture`, `cargo test -p oxide-agent-core compaction::history`, `cargo test -p oxide-agent-transport-web collect_events_records_runtime_compaction_without_pruning_event`, `cargo check --workspace`, `cargo fmt --all --check`, and `cargo clippy --workspace --all-targets --all-features`.
- 2026-05-21 21:52 +03: Removed the old classifier fast path from the active Codex-style pre-sampling branch. Runtime pre-sampling now dispatches to `maybe_run_runtime_pre_sampling_compaction(...)` before the legacy fresh-session `classify_hot_memory(...)` compatibility path, so the production Codex-style branch cannot silently use old staged summary classification.
- 2026-05-21 21:52 +03: Validation passed after the runner audit patch: `cargo test -p oxide-agent-core run_pre_sampling_uses_runtime_compaction_when_threshold_reached`, `cargo test -p oxide-agent-core run_retries_after_context_overflow_with_runtime_context_limit_compaction`, `cargo test -p oxide-agent-core run_compacts_before_downshifting_to_smaller_model_route`, `cargo check --workspace`, `cargo fmt --all --check`, and `cargo clippy --workspace --all-targets --all-features`.
- 2026-05-21 22:04 +03: Added direct history-builder coverage for `ApprovalReplay` preservation. This closes the pinned approval replay acceptance gap: compacted replacement history keeps exact approval replay instructions and tokens instead of relying only on the generic pinned-kind match.
- 2026-05-21 22:04 +03: Validation passed after approval replay/doc edits: `cargo test -p oxide-agent-core compaction::history`, `cargo fmt --all --check`, `cargo check --workspace`, and `cargo clippy --workspace --all-targets --all-features`.
- 2026-05-21 22:14 +03: Added direct controller coverage for two PRD acceptance points that were previously indirect: repeated manual compaction now proves it keeps exactly one `[OXIDE_COMPACTED_SUMMARY_V1]`, and manual compaction now proves `AgentMemory.todos` survives atomic history replacement unchanged.
- 2026-05-21 22:14 +03: Validation passed for this coverage slice: `cargo test -p oxide-agent-core repeated_manual_compact_keeps_one_prefixed_summary` and `cargo test -p oxide-agent-core compaction::controller`.
- 2026-05-21 22:19 +03: Re-ran the focused runtime/transport acceptance suite after the repeated-compaction/todos tests. Passing commands: `cargo test -p oxide-agent-core compaction::history`, `cargo test -p oxide-agent-core compaction::controller`, `cargo test -p oxide-agent-core run_pre_sampling_uses_runtime_compaction_when_threshold_reached`, `cargo test -p oxide-agent-core run_retries_after_context_overflow_with_runtime_context_limit_compaction`, `cargo test -p oxide-agent-core run_compacts_before_downshifting_to_smaller_model_route`, `cargo test -p oxide-agent-transport-web collect_events_records_runtime_compaction_without_pruning_event`, and `cargo test -p oxide-agent-transport-telegram renders_runtime_compaction_status`.
- 2026-05-21 22:19 +03: Re-ran final gates and E2E evidence. Passing commands: `cargo fmt --all --check`, `cargo check --workspace`, `cargo clippy --workspace --all-targets --all-features`, `cargo test -p oxide-agent-transport-web --features socket_e2e e2e_compress_tool_triggers_manual_compaction -- --nocapture`, and `cargo test -p oxide-agent-transport-web --features socket_e2e e2e_compress_preserves_tool_heavy_batch_continuation -- --nocapture`.
- 2026-05-21 22:33 +03: Closed the final default/compatibility-export gap. `AgentRunnerConfig::new/default` now selects Codex-style compaction by default; old runner compatibility tests that intentionally exercise `CompactionService` opt out explicitly with `.with_codex_style_compaction(false)`.
- 2026-05-21 22:33 +03: Marked legacy staged compaction modules/exports in code as compatibility-only, not production runtime fallback. `compaction/mod.rs`, `agent/mod.rs`, and `compaction/service.rs` now document that the active runtime path is `CompactionController` plus `LocalLlmSummary`, while old staged modules remain only for persisted-session compatibility and regression tests.
- 2026-05-21 22:33 +03: Validation passed after the default/compatibility cleanup: `cargo test -p oxide-agent-core runner_config_defaults_to_codex_style_compaction`, `cargo test -p oxide-agent-core run_applies_pre_run_compaction_before_first_llm_call`, `cargo test -p oxide-agent-core run_retries_after_context_overflow_with_manual_compaction`, `cargo test -p oxide-agent-core run_pre_sampling_uses_runtime_compaction_when_threshold_reached`, `cargo test -p oxide-agent-core compaction::`, `cargo fmt --all --check`, `cargo check --workspace`, and `cargo clippy --workspace --all-targets --all-features`.

## Acceptance Matrix

- Old active compaction flow disabled in production: executor/delegation construct only `CompactionController`; production runner context no longer carries `CompactionService`; `AgentRunnerConfig::new/default` selects Codex-style compaction; old runner checkpoint/post-run helpers are `#[cfg(test)]`; Codex-style pre-sampling bypasses the old classifier fast path.
- No active old/new dual path: manual executor compaction, runner `compress`, context-limit retry, pre-sampling, and model-downshift compaction all use the runtime controller path. `OXIDE_CODEX_STYLE_COMPACTION=false` is an emergency auto-compaction disable, not a legacy fallback.
- Default backend is `LocalLlmSummary`: controller factory uses `LocalLlmSummary`; tests cover route selection, plain-text `chat_completion`, trimming, empty output rejection, missing route rejection, and no-op mutation on backend failure.
- No OpenAI `/responses/compact`: implementation uses ordinary provider text generation through `chat_completion_for_model_info`; docs explicitly state `/responses/compact` is out of scope.
- Atomic replacement and repeated compaction shape: `AgentMemory::replace_compacted_history(...)` validates before mutation; controller tests prove failure is no-op, success creates one `[OXIDE_COMPACTED_SUMMARY_V1]`, and repeated compaction keeps one current summary; history tests filter previous current/legacy summaries.
- Old persisted data compatibility: legacy `[COMPACTION_SUMMARY]`, breadcrumb cards, structured summaries, and old `ArchiveReference` messages deserialize and migrate lazily; new compaction does not create new R2 archive/payload objects.
- Tool-heavy safety: history test preserves terminal open tool-call batches; socket E2E covers `compress`; socket E2E covers one assistant batch with `compress` + `write_todos` and validates no orphan repair.
- Context-limit and model-downshift: focused runner tests cover retry after context overflow and compaction before failover to a smaller route.
- Pinned context and todos: history builder preserves topic `AGENTS.md`, user task, runtime context, skill context, approval replay, infra status, and archive references; approval replay has direct history-builder coverage; controller coverage proves `AgentMemory.todos` survives compaction.
- Progress UX: core progress state, Telegram rendering, and Web event mapping cover runtime `compaction_started`, `compaction_completed`, `compaction_failed`, and `compaction_skipped`; new path does not emit `pruning_applied`.
- Docs/config/rollback: `.env.example`, README, source module docs, and this goal document describe the new default, emergency disable behavior, provider-agnostic backend, no `/responses/compact`, compatibility-only legacy modules, and old-data compatibility.

## Current Inventory

Old active flow entrypoints found so far:
- `CompactionService::prepare_for_run` in `crates/oxide-agent-core/src/agent/compaction/service.rs`. Retained for compatibility/regression tests, not production-wired.
- Executor construction in `crates/oxide-agent-core/src/agent/executor/config.rs`. Closed for production runtime on 2026-05-21 21:08 +03; executor now constructs only `CompactionController`.
- Executor manual compaction in `crates/oxide-agent-core/src/agent/executor/compaction.rs`. Closed for production runtime on 2026-05-21 21:08 +03; manual compaction always uses controller/runtime events.
- Runner pre-LLM and context-overflow compaction in `crates/oxide-agent-core/src/agent/runner/execution.rs`. Closed for production runtime on 2026-05-21 21:16 +03; active Codex-style pre-sampling/context-limit paths use `CompactionController`, while old checkpoint helpers are test-only.
- Runner tool-path compaction in `crates/oxide-agent-core/src/agent/runner/tools.rs`. Closed for production runtime on 2026-05-21 21:16 +03; `compress` uses runtime controller and keeps only compatibility JSON fields.
- Runner post-run compaction in `crates/oxide-agent-core/src/agent/runner/responses.rs`. Closed for production runtime on 2026-05-21 21:16 +03; legacy cleanup is `#[cfg(test)]`.
- Runner context service passing in `crates/oxide-agent-core/src/agent/runner/types.rs` and `executor/types.rs`. Closed for production context on 2026-05-21 21:16 +03; legacy service field is `#[cfg(test)]` only.
- Sub-agent compaction service in `crates/oxide-agent-core/src/agent/providers/delegation.rs`. Closed on 2026-05-21 21:08 +03; delegation now uses `CompactionController`.
- Old progress events/status in `crates/oxide-agent-core/src/agent/progress.rs`. New runtime events are active; old variants remain for compile/backward compatibility.
- Telegram/Web rendering of old progress events in `crates/oxide-agent-transport-telegram/src/bot/progress_render.rs` and `crates/oxide-agent-transport-web/src/web_transport.rs`. Runtime compaction rendering/event names are covered as of 2026-05-21 21:31 +03; legacy event names remain compatible.
- Old architecture tests under `crates/oxide-agent-core/src/agent/compaction/tests/`, `crates/oxide-agent-core/tests/compaction_lifecycle.rs`, and runner/progress transport tests. These remain as compatibility/regression coverage and should not be treated as active production wiring.

## Risks and Blockers

- Provider-visible role for compacted summary is resolved as `System` in memory plus provider-call system folding.
- Mid-turn safe boundary is covered for context-limit retry and `compress` tool paths, including terminal open tool-call batch preservation, web socket E2E coverage for manual `compress`, and a tool-heavy `compress` + `write_todos` batch.
- New compaction uses `AgentMemory::replace_compacted_history(...)`, which validates before mutation. Keep auditing accidental direct `replace_messages(...)` calls in future runner changes because that older helper can repair after mutation.
- Old compaction modules and public re-exports remain for persisted-data compatibility and regression tests. Final cleanup can further narrow exports only after legacy tests are migrated, but production wiring is already on the controller path.
- Old compaction symbols are still exported for integration tests and persisted-data compatibility, and are now marked in source docs as compatibility-only. They are not production runtime entrypoints; a future cleanup can narrow these exports after legacy migration tests are rewritten.

## Final Verification

Current implementation slice is build-, format-, and lint-clean after the default/compatibility cleanup. Focused core/transport tests and manual/tool-heavy web socket E2E passed in this session. The latest final command set also passed: `cargo fmt --all --check`, `cargo check --workspace`, `cargo clippy --workspace --all-targets --all-features`, `cargo test -p oxide-agent-core compaction::`, `cargo test -p oxide-agent-core runner_config_defaults_to_codex_style_compaction`, legacy runner compatibility tests with explicit opt-out, runtime runner regressions for pre-sampling/context-limit/model-downshift, transport progress/event tests, and both socket E2E compaction regressions.

Production wiring is on the `CompactionController` / `LocalLlmSummary` path. Old staged compaction remains compatibility-only for old persisted data and regression tests, not a runtime fallback.

## Completion Audit

- Old active compaction agent/summarizer flow removed or disabled from runtime: satisfied. Production executor/delegation construct `CompactionController`; production runner no longer carries `CompactionService`; old checkpoint/post-run helpers are test-only; `AgentRunnerConfig` defaults to Codex-style.
- No old/new dual compaction in one turn: satisfied. New controller path handles manual, pre-sampling, context-limit, `compress`, and model-downshift. Legacy staged compaction is only reachable in explicit compatibility tests with `.with_codex_style_compaction(false)`.
- `LocalLlmSummary` default/required backend: satisfied for production wiring. `CompactionController::local_llm(...)` is the executor/delegation/runtime factory path; tests cover plain text `chat_completion`, route selection, empty output, missing route, and no-op failure.
- No dependency on OpenAI `/responses/compact`: satisfied. New summary backend calls ordinary provider text generation via `chat_completion_for_model_info`; docs/config state `/responses/compact` is not used.
- Long sessions continue after compaction: satisfied by context-overflow retry, pre-sampling, model-downshift, manual `compress`, and tool-heavy socket E2E coverage.
- Repeated compaction creates no duplicate summaries: satisfied by direct controller test and history-builder legacy/current summary filtering.
- Old summaries are not copied as ordinary hot context: satisfied by history tests for legacy summary/breadcrumb/structured-summary filtering and old archive-ref lazy migration.
- Tool-heavy conversations preserve valid tool pairs: satisfied by terminal open tool-batch history test and socket E2E with `compress` + `write_todos`.
- Context overflow and model downshift reasons/phases: satisfied by focused runner regressions for `ContextLimit`/`MidTurn` and `ModelDownshift`/`ModelSwitch`.
- Pinned context, todos, approval, runtime state: satisfied by history-builder pinned-kind preservation tests plus direct approval replay and todos controller coverage.
- Old sessions with structured summaries/archive refs: satisfied by lazy migration fixture and old staged compatibility tests.
- No new R2 archive/payload objects by default compaction: satisfied by new controller/history path not depending on archive/payload sinks, and docs/config stating old R2 data is compatibility-only.
- Telegram/Web progress: satisfied by runtime progress state, Telegram rendering test, Web event-name/collection tests, and E2E assertions that new compaction does not emit `pruning_applied`.
- Relevant tests/docs/config/rollback: satisfied by focused core/transport/E2E commands above plus `.env.example`, README, source module docs, and this goal document.
