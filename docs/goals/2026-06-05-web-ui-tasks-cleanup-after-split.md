# Goal: Web UI Tasks Cleanup After Split

Date started: 2026-06-05
Status: active
Codex goal: `/goal Implement docs/goals/2026-06-05-web-ui-tasks-cleanup-after-split.md until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update this document after each meaningful verification, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.`
Source spec: User-selected RECON findings after `docs/goals/2026-06-05-web-ui-tasks-slice-refactor.md` completion.
Goal doc owner: Codex
Last updated: 2026-06-05

## Objective

Clean up the focused post-split `crates/oxide-agent-web-ui/src/tasks/` modules by applying low-risk pure helper fixes, removing duplicated composer event-handler logic, and eliminating the remaining task-card clippy suppressions without changing task lifecycle, streaming, attachment, composer, or rendering behavior.

Done when every required Completion Audit item is verified by its listed evidence, `cargo clippy` passes without the targeted task-card `allow(clippy::...)` suppressions, and all listed validation commands pass.

## Scope

In scope:
- `crates/oxide-agent-web-ui/src/tasks/state.rs`
- `crates/oxide-agent-web-ui/src/tasks/workspace.rs`
- `crates/oxide-agent-web-ui/src/tasks/composer.rs`
- `crates/oxide-agent-web-ui/src/tasks/delivered_files.rs`
- `crates/oxide-agent-web-ui/src/tasks/tool_cards.rs`
- `crates/oxide-agent-web-ui/src/tasks/task_card.rs`
- This goal document and checkpoint evidence.

Out of scope:
- Backend routes, DTOs, storage, auth, CSRF, SSE protocol, and task lifecycle semantics.
- CSS cleanup, selector renames, visual redesign, or cascade consolidation.
- Host-testability restructuring for wasm-gated `tasks` modules.
- Large state-manager rewrites, new crates, new generic form abstractions, builders, registries, or framework changes.
- Changing create-session/upload/create-task order, existing-session resume-or-create behavior, cancel behavior, or edit-version streaming behavior.

## Missing Inputs

- None required.

## Repository Context

- The completed split left `crates/oxide-agent-web-ui/src/tasks.rs` as a small facade and moved workspace code to `crates/oxide-agent-web-ui/src/tasks/workspace.rs`.
- `latest_task(Vec<TaskSummary>)` currently forces a task-list clone at `crates/oxide-agent-web-ui/src/tasks/state.rs:60` and `crates/oxide-agent-web-ui/src/tasks/workspace.rs:389`.
- `task_submit_error_message` is UI copy in `crates/oxide-agent-web-ui/src/tasks/state.rs:88`, called from `crates/oxide-agent-web-ui/src/tasks/workspace.rs:435`.
- `DeliveredFileLink` exposes module-wide fields at `crates/oxide-agent-web-ui/src/tasks/delivered_files.rs:8` even though construction and field access stay inside the module.
- `first_line` truncates with a byte slice at `crates/oxide-agent-web-ui/src/tasks/tool_cards.rs:963`, which is unsafe for non-ASCII text.
- Composer drag/drop, textarea resize, paste, and Ctrl+Enter handlers are duplicated between welcome and session composers at `crates/oxide-agent-web-ui/src/tasks/workspace.rs:183` and `crates/oxide-agent-web-ui/src/tasks/workspace.rs:704`.
- `TaskCard` and `TaskInputEditForm` carried targeted clippy suppressions after the mechanical split; checkpoint 3 removes them through local component/props cleanup.
- `crates/oxide-agent-web-ui/src/main.rs:17` keeps web UI modules wasm-gated; wasm `cargo check` remains mandatory.

## Completion Audit

- G1: Low-risk pure helper cleanup is complete
  - Source: User-selected RECON findings for `latest_task`, `task_submit_error_message`, `DeliveredFileLink`, and `first_line`.
  - Acceptance: `latest_task` no longer requires cloning the full task list; UI submit-error copy no longer lives in `state.rs`; `DeliveredFileLink` fields are not broader than needed; `first_line` is Unicode-safe and keeps the same preview intent.
  - Evidence required: focused diff review, wasm `cargo check`, wasm `cargo clippy`, and native `cargo test -p oxide-agent-web-ui`.
  - Status: verified
  - Evidence collected: 2026-06-05 checkpoint 1 changed `latest_task` to take `&[TaskSummary]` and clone only the selected latest task, moved submit-error UI copy from `state.rs` to `workspace.rs`, made `DeliveredFileLink` fields private, and replaced byte-index `first_line` truncation with character-boundary-safe truncation. Wasm check, wasm clippy, native tests, wasm test build, and diff checks passed.

- G2: Composer duplicate event-handler logic is extracted without flow changes
  - Source: User-selected RECON findings for duplicated welcome/session composer handlers.
  - Acceptance: Welcome and session composers reuse small helpers from `composer.rs` or local focused helpers for drag/drop, textarea resize, pasted images, and Ctrl+Enter submit; submit/resume/create/cancel control flow in `workspace.rs` is not rewritten.
  - Evidence required: focused diff review proving lifecycle branches are untouched, wasm `cargo check`, wasm `cargo clippy`, native tests, and release `trunk build`.
  - Status: verified
  - Evidence collected: 2026-06-05 checkpoint 2 added focused composer helpers for drag state, drop attachment import, textarea input/resize, pasted image attachments, and Ctrl+Enter form submission; both welcome/session composer call sites now call those helpers while task create/resume/cancel branches stayed untouched. Wasm check, wasm clippy, native tests, wasm test build, release `trunk build`, and diff checks passed.

- G3: Task-card clippy suppressions are removed through small component/props cleanup
  - Source: User-selected RECON findings for `TaskCard` and `TaskInputEditForm` suppressions.
  - Acceptance: The targeted `#[allow(clippy::too_many_arguments, clippy::too_many_lines)]` on `TaskCard` and `#[allow(clippy::too_many_arguments)]` on `TaskInputEditForm` are removed or narrowed away by small local splits/props grouping; task version navigation, clipboard actions, edit-version creation, drawer close, selected-version update, and stream startup behavior remain unchanged.
  - Evidence required: focused diff review, wasm `cargo clippy -p oxide-agent-web-ui --target wasm32-unknown-unknown`, native tests, and release `trunk build`.
  - Status: verified
  - Evidence collected: 2026-06-05 checkpoint 3 removed the targeted `allow(clippy::...)` annotations by introducing local `TaskCardModel`/`TaskCardSignals`, smaller user/resume/assistant message components, and `TaskInputEditTarget`/`TaskInputEditSignals`; version navigation, clipboard actions, edit-version stream startup, selected-version update, drawer close, delivered-file rendering, and CSS classes stayed in the moved code paths. Wasm check, wasm clippy, native tests, wasm test build, release `trunk build`, and diff checks passed.

- Q1: Behavior-critical task flows are preserved
  - Source: Existing web UI task invariants from the completed slice refactor goal.
  - Acceptance: Create-session/upload/create-task, existing-session upload/resume-or-create, cancel, profile update, edit-version stream startup, `streaming_task_id` stale guard, profile sentinels, and CSS class names are not semantically changed.
  - Evidence required: diff review by checkpoint plus wasm check/clippy.
  - Status: pending
  - Evidence collected: 2026-06-05 checkpoint 1 touched only pure helper visibility/location and preview truncation; create/submit/resume/cancel/edit-version flow code stayed unchanged and wasm check/clippy passed. Checkpoint 2 touched only composer event handler bodies and preserved submit/resume/create/cancel lifecycle code. Checkpoint 3 reshaped only task-card component boundaries and preserved edit-version submission, selected-version update, drawer close, stream startup, copy/version controls, delivered-file rendering, and CSS classes.

- Q2: Cleanup stays simple and dependency-free
  - Source: Repository implementation bias and user focus on cleanup.
  - Acceptance: No new crates, no new services, no global state manager, no UI framework changes, and no broad abstractions are introduced.
  - Evidence required: `git diff -- Cargo.toml crates/oxide-agent-web-ui/Cargo.toml`, file list review, and diff review.
  - Status: pending
  - Evidence collected: 2026-06-05 checkpoint 1 added no dependencies and made no `Cargo.toml` changes. Checkpoint 2 added no dependencies and kept cleanup to small helper functions. Checkpoint 3 added no dependencies and kept cleanup local to task-card props/components.

- V1: Required validation passes
  - Source: Repository validation conventions for wasm-gated web UI.
  - Acceptance: Required commands pass for each checkpoint; release `trunk build` is run for component/CSS-affecting checkpoints and final verification.
  - Evidence required: command output summary recorded in Progress Log and Final Verification.
  - Status: pending
  - Evidence collected: 2026-06-05 checkpoint 1 passed `cargo fmt`, `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`, `cargo clippy -p oxide-agent-web-ui --target wasm32-unknown-unknown`, `cargo test -p oxide-agent-web-ui`, `cargo test -p oxide-agent-web-ui --target wasm32-unknown-unknown --no-run`, and `git diff --check`. Checkpoint 2 passed the same command set plus `env -u NO_COLOR trunk build --release` from `crates/oxide-agent-web-ui`. Checkpoint 3 passed the same checkpoint 2 command set.

- N1: Out-of-scope cleanup remains untouched
  - Source: User focus excludes testability-gap, CSS, and broad redesign work.
  - Must preserve: No CSS cleanup, selector renames, host-testability restructuring, backend changes, or broad workspace/task lifecycle rewrites are included in this goal.
  - Evidence required: file list review and diff review.
  - Status: pending
  - Evidence collected: 2026-06-05 checkpoint 1 changed only `crates/oxide-agent-web-ui/src/tasks/{state,workspace,delivered_files,tool_cards}.rs` and this goal document; no CSS, backend, host-testability, or broad lifecycle changes. Checkpoint 2 changed only `composer.rs`, `workspace.rs`, and this goal document. Checkpoint 3 changed only `task_card.rs`, `workspace.rs`, and this goal document.

## Implementation Plan

1. Low-risk pure cleanup first
   - Audit IDs: G1, Q1, Q2, V1, N1
   - Expected changes: Change `latest_task` to take a slice and clone only the selected summary; move or localize `task_submit_error_message` as UI copy; make `DeliveredFileLink` fields private; replace byte-index truncation in `first_line` with character-boundary-safe truncation.
   - Validation: `cargo fmt`; `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`; `cargo clippy -p oxide-agent-web-ui --target wasm32-unknown-unknown`; `cargo test -p oxide-agent-web-ui`; `cargo test -p oxide-agent-web-ui --target wasm32-unknown-unknown --no-run`; `git diff --check`.
   - Exit condition: Pure cleanup is committed separately with no task flow or component restructuring changes.

2. Extract duplicated composer event helpers
   - Audit IDs: G2, Q1, Q2, V1, N1
   - Expected changes: Add small helpers in `composer.rs` for shared drag/drop, textarea resize, paste image handling, and Ctrl+Enter submit behavior; update both composer call sites mechanically; keep submit/resume/create/cancel branches intact.
   - Validation: `cargo fmt`; `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`; `cargo clippy -p oxide-agent-web-ui --target wasm32-unknown-unknown`; `cargo test -p oxide-agent-web-ui`; `cargo test -p oxide-agent-web-ui --target wasm32-unknown-unknown --no-run`; `env -u NO_COLOR trunk build --release` from `crates/oxide-agent-web-ui`; `git diff --check`.
   - Exit condition: Duplicated handler bodies are removed or reduced to identical helper calls, with no sequencing changes in task submission/resume flows.

3. Remove task-card clippy suppressions
   - Audit IDs: G3, Q1, Q2, V1, N1
   - Expected changes: Split only small presentational sections or introduce local prop structs needed to satisfy clippy for `TaskCard` and `TaskInputEditForm`; preserve version navigation, copy/edit controls, delivered-file rendering, edit submission, and stream startup.
   - Validation: `cargo fmt`; `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`; `cargo clippy -p oxide-agent-web-ui --target wasm32-unknown-unknown`; `cargo test -p oxide-agent-web-ui`; `cargo test -p oxide-agent-web-ui --target wasm32-unknown-unknown --no-run`; `env -u NO_COLOR trunk build --release` from `crates/oxide-agent-web-ui`; `git diff --check`.
   - Exit condition: Targeted clippy suppressions are absent or no longer needed, clippy passes, and task-card behavior is unchanged by focused diff review.

4. Final audit and close goal
   - Audit IDs: G1, G2, G3, Q1, Q2, V1, N1
   - Expected changes: Update this goal document with final evidence and mark complete only if every audit item is verified.
   - Validation: Full validation command set, final file list review, and `git diff --check`.
   - Exit condition: Completion Audit and Final Verification are filled with current evidence, then committed.

## Validation Contract

- Static checks: `cargo fmt`; `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`; `cargo clippy -p oxide-agent-web-ui --target wasm32-unknown-unknown`; `git diff --check`.
- Tests: `cargo test -p oxide-agent-web-ui`; `cargo test -p oxide-agent-web-ui --target wasm32-unknown-unknown --no-run`.
- Runtime/build verification: `env -u NO_COLOR trunk build --release` from `crates/oxide-agent-web-ui` for component/CSS-affecting checkpoints and final verification.
- Artifact verification: review changed file list, confirm no `Cargo.toml` dependency changes, and confirm targeted clippy `allow` annotations are removed by checkpoint 3.
- Done when: every Completion Audit item is verified and this goal is marked complete with final evidence.

## Decisions

- 2026-06-05: Scope excludes host-testability restructuring and CSS cleanup because the user selected low-risk pure cleanup, composer duplication, and clippy suppressions as the focus for this goal.
- 2026-06-05: Composer cleanup must extract only small event helpers first; submit/resume/create/cancel lifecycle logic stays in `workspace.rs` unless a later diff proves a purely mechanical move.
- 2026-06-05: Task-card suppressions are intentionally later than pure/composer cleanup because removing them may require component/props reshaping.

## Progress Log

- 2026-06-05: Goal drafted.
  - Changed: Created `docs/goals/2026-06-05-web-ui-tasks-cleanup-after-split.md` from focused RECON findings.
  - Evidence: Scope, audit ledger, checkpoints, validation commands, non-goals, and first implementation step recorded.
  - Commands: `git status --short --branch`; goal-doc diff review; `git diff --check`.
  - Audit IDs updated: none; implementation not started.
  - Next: Checkpoint 1 — low-risk pure cleanup first.

- 2026-06-05: Checkpoint 1 completed.
  - Changed: Cleaned `latest_task`, submit-error UI copy location, `DeliveredFileLink` field visibility, and UTF-8-safe `first_line` truncation.
  - Evidence: Diff review showed no task lifecycle sequencing changes and no dependency/CSS/backend changes.
  - Commands: `cargo fmt`; `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`; `cargo clippy -p oxide-agent-web-ui --target wasm32-unknown-unknown`; `cargo test -p oxide-agent-web-ui`; `cargo test -p oxide-agent-web-ui --target wasm32-unknown-unknown --no-run`; `git diff --check`.
  - Audit IDs updated: G1 verified; Q1, Q2, V1, N1 evidence collected.
  - Next: Checkpoint 2 — extract duplicated composer event helpers.

- 2026-06-05: Checkpoint 2 completed.
  - Changed: Added shared composer helpers for drag/drop, textarea input resize, pasted image attachments, and Ctrl+Enter form submission; replaced duplicated welcome/session handler bodies with helper calls.
  - Evidence: Focused diff showed create-session, submit/resume, cancel, profile update, and stream startup branches were not rewritten; no dependency/CSS/backend changes.
  - Commands: `cargo fmt`; `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`; `cargo clippy -p oxide-agent-web-ui --target wasm32-unknown-unknown`; `cargo test -p oxide-agent-web-ui`; `cargo test -p oxide-agent-web-ui --target wasm32-unknown-unknown --no-run`; `env -u NO_COLOR trunk build --release`; `git diff --check`.
  - Audit IDs updated: G2 verified; Q1, Q2, V1, N1 evidence collected.
  - Next: Checkpoint 3 — remove task-card clippy suppressions.

- 2026-06-05: Checkpoint 3 completed.
  - Changed: Removed the targeted `TaskCard` and `TaskInputEditForm` clippy suppressions through local prop structs and smaller message/action/edit components.
  - Evidence: Focused diff preserved version navigation, copy/edit controls, delivered-file rendering, edit submission, selected-version update, drawer close, stream startup, visible strings, and CSS classes; no dependency/CSS/backend changes.
  - Commands: `cargo fmt`; `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`; `cargo clippy -p oxide-agent-web-ui --target wasm32-unknown-unknown`; `cargo test -p oxide-agent-web-ui`; `cargo test -p oxide-agent-web-ui --target wasm32-unknown-unknown --no-run`; `env -u NO_COLOR trunk build --release`; `git diff --check`.
  - Audit IDs updated: G3 verified; Q1, Q2, V1, N1 evidence collected.
  - Next: Checkpoint 4 — final audit and close goal.

## Risks and Blockers

- Task modules are wasm-gated.
  - Impact: Native tests do not compile or execute task UI modules directly.
  - Evidence: `crates/oxide-agent-web-ui/src/main.rs:17` gates web UI modules to wasm.
  - Mitigation or requested decision: Use wasm `cargo check`, wasm clippy, wasm test build, and release `trunk build`; do not expand scope into host-testability restructuring unless explicitly requested.
  - Audit IDs affected: V1

- Composer event cleanup can accidentally change task submission semantics.
  - Impact: Submit/resume/create/cancel behavior could regress if helper extraction grows beyond event handling.
  - Evidence: Duplicated event blocks live near `crates/oxide-agent-web-ui/src/tasks/workspace.rs:183` and `crates/oxide-agent-web-ui/src/tasks/workspace.rs:704`, while lifecycle code lives elsewhere in `workspace.rs`.
  - Mitigation or requested decision: Keep checkpoint 2 limited to event helpers and prove lifecycle branches are untouched by focused diff review.
  - Audit IDs affected: G2, Q1

- Removing task-card clippy suppressions required several small component boundaries.
  - Impact: Over-splitting or prop grouping could reduce readability if done too aggressively.
  - Evidence: Checkpoint 3 removed the targeted suppressions and validation passed.
  - Mitigation or requested decision: Keep later work to final audit; do not broaden into unrelated task-card redesign.
  - Audit IDs affected: G3, Q1, Q2

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
