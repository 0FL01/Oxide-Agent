# Goal: Web UI Tasks Slice Refactor

Date started: 2026-06-05
Status: active
Codex goal: `/goal Implement docs/goals/2026-06-05-web-ui-tasks-slice-refactor.md until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update this document after each meaningful verification, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.`
Source spec: User request and recon of `crates/oxide-agent-web-ui/src/tasks.rs` at 4,291 lines.
Goal doc owner: Codex
Last updated: 2026-06-05

## Objective

Reduce the maintenance burden of `crates/oxide-agent-web-ui/src/tasks.rs` by slicing it into locally understandable `tasks/` submodules while preserving the current web chat UI behavior, task lifecycle flows, SSE streaming semantics, tool/activity rendering, delivered-file rendering, attachment handling, profile/effort behavior, and CSS class contracts.

Done when `crate::tasks::TaskConsole` remains the stable public entry point, `tasks.rs` becomes a small facade/module hub, the planned submodules compile under wasm, and every Completion Audit item is verified by its listed evidence.

## Scope

In scope:
- `crates/oxide-agent-web-ui/src/tasks.rs` and new focused files under `crates/oxide-agent-web-ui/src/tasks/`.
- Web UI task/session workspace, composer, task cards, activity drawer, tool cards, delivered-file helpers, stream glue, version grouping, payload helpers, and tests currently embedded in `tasks.rs`.
- This goal document and checkpoint progress evidence.

Out of scope:
- Changing backend web routes, DTOs, contracts, task lifecycle semantics, SSE event formats, storage, auth, CSRF behavior, or provider/tool output formats.
- Changing CSS class names or visual behavior unless explicitly required to preserve compilation after a mechanical move.
- Adding new crates, state-management frameworks, routing frameworks, macro systems, card registries, builders, or generic UI abstraction layers.
- Refactoring unrelated web UI modules (`api`, `auth`, `sessions`, `sse`, `markdown`, `routes`, `app`, `components`) except for import adjustments required by the split.
- Direct Google Gemini provider work or unrelated transport/core/runtime refactors.

## Missing Inputs

- None required.

## Repository Context

- `TaskConsole` is the only external public entry point from this file and is consumed by `crates/oxide-agent-web-ui/src/components.rs:3` and `crates/oxide-agent-web-ui/src/components.rs:20`.
- `tasks.rs` is compiled only for wasm through `crates/oxide-agent-web-ui/src/main.rs:17`; native `cargo test -p oxide-agent-web-ui` does not fully validate task UI compilation.
- `SessionWorkspace` currently owns session load, task list load, submit/resume/cancel/profile flows, and composer state in `crates/oxide-agent-web-ui/src/tasks.rs:304`.
- SSE glue currently wraps `spawn_task_stream` through `StreamUiSignals` at `crates/oxide-agent-web-ui/src/tasks.rs:1122`; stale stream guarding depends on `streaming_task_id` behavior in `crates/oxide-agent-web-ui/src/sse.rs:492`.
- Task cards and task edit/version UI start at `crates/oxide-agent-web-ui/src/tasks.rs:1216`; task version fallback semantics come from `crates/oxide-agent-web-contracts/src/tasks.rs:251`.
- Profile and effort sentinels `__default__` and `__none__` are defined at `crates/oxide-agent-web-ui/src/tasks.rs:24` and must keep their API mapping.
- Activity grouping and tool dispatch are centralized at `crates/oxide-agent-web-ui/src/tasks.rs:1838` and `crates/oxide-agent-web-ui/src/tasks.rs:1908`.
- Delivered-file linkification, previews, and related tests live around `crates/oxide-agent-web-ui/src/tasks.rs:1553`, `crates/oxide-agent-web-ui/src/tasks.rs:3170`, and `crates/oxide-agent-web-ui/src/tasks.rs:4233`.
- CSS coupling is strong for task cards and composer: `crates/oxide-agent-web-ui/src/styles.css:781` and `crates/oxide-agent-web-ui/src/styles.css:2564`.
- `crates/oxide-agent-web-ui/Cargo.toml:11` forbids `unwrap_used` and warns on `too_many_lines`; no new dependencies are needed.

## Completion Audit

- G1: `tasks.rs` is reduced to a facade/module hub
  - Source: User request to plan/refactor the 4,291-line `tasks.rs`.
  - Acceptance: `crates/oxide-agent-web-ui/src/tasks.rs` exposes `pub fn TaskConsole` or re-exports it from a focused module, declares focused child modules, and no longer contains the bulk of workspace/card/tool/helper implementation.
  - Evidence required: final `wc -l crates/oxide-agent-web-ui/src/tasks.rs`, file-list review, and wasm `cargo check`.
  - Status: pending
  - Evidence collected:

- G2: Mechanical slices preserve workspace and task lifecycle behavior
  - Source: Recon of create-session/upload/create-task flow, existing-session submit/resume/cancel flow, and edit-version flow.
  - Acceptance: Create session -> upload attachments -> create task -> navigate remains intact; existing-session upload -> resume/create remains intact; cancel updates active task/stream/session summary; edit version creates a task version and starts streaming the new task.
  - Evidence required: focused diff review plus `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`.
  - Status: pending
  - Evidence collected:

- G3: Composer/profile/effort/attachment behavior remains unchanged
  - Source: Recon of composer state, file paste/drag/drop/upload helpers, profile sentinels, and effort persistence.
  - Acceptance: Pending attachments, pasted image filtering, drag/drop handling, profile selection mapping, missing-profile fallback option, and default effort load/persist semantics are preserved.
  - Evidence required: diff review, existing native tests for profile fallback/linkification, and wasm `cargo check`.
  - Status: pending
  - Evidence collected:

- G4: Activity drawer, task cards, tool cards, and delivered files remain behavior-preserving
  - Source: Recon of activity grouping, tool result matching, tool card dispatch, delivered-file previews/linkification, and markdown rendering.
  - Acceptance: Activity timeline filters/grouping, tool call/result pairing, specialized tool-card branches, reasoning/todos/context cards, delivered-file linkification/previews, and CSS class names remain compatible.
  - Evidence required: focused diff review by slice, `cargo test -p oxide-agent-web-ui`, and wasm `cargo check`.
  - Status: pending
  - Evidence collected:

- G5: Pure helper/testable logic is isolated before high-volume UI moves
  - Source: Recon recommendation to avoid mixing behavior changes with large Leptos view moves.
  - Acceptance: Low-risk pure helpers for version grouping/session summary conversion/submit errors/delivered-file linkification/payload parsing are moved or exposed in focused modules before moving large components.
  - Evidence required: checkpoint diff review and native tests where available.
  - Status: verified
  - Evidence collected: 2026-06-05 checkpoint 1 moved helper groups into `crates/oxide-agent-web-ui/src/tasks/{delivered_files,payload,profile,state,versions}.rs`; `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`, `cargo test -p oxide-agent-web-ui`, and `cargo test -p oxide-agent-web-ui --target wasm32-unknown-unknown --no-run` passed.

- Q1: UI-only blast radius is preserved
  - Source: User requested refactor of `tasks.rs`; AGENTS guardrails require smallest maintainable change.
  - Acceptance: No backend/core/runtime/contracts/provider behavior changes; only web-ui task split files and this goal document change, except necessary import adjustments.
  - Evidence required: `git diff --name-only` and `git diff --stat` review.
  - Status: pending
  - Evidence collected: 2026-06-05 checkpoint 1 changed only `crates/oxide-agent-web-ui/src/tasks.rs`, new `crates/oxide-agent-web-ui/src/tasks/*.rs` helper modules, and this goal document.

- Q2: No over-engineering or new dependencies
  - Source: AGENTS implementation bias and recon conclusion.
  - Acceptance: No `Cargo.toml` changes; no new crates/frameworks; no broad generic component registry/builder; modules stay boring and local.
  - Evidence required: `Cargo.toml` diff review and implementation diff review.
  - Status: pending
  - Evidence collected: 2026-06-05 checkpoint 1 added no dependencies and made no `Cargo.toml` changes.

- V1: Web UI validation passes for meaningful checkpoints
  - Source: Repo validation practice for web UI and wasm-only compilation of `tasks.rs`.
  - Acceptance: Relevant validation commands pass after each meaningful checkpoint, or exact blockers are recorded with the smallest external action needed.
  - Evidence required: `cargo fmt`, `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`, `cargo clippy -p oxide-agent-web-ui --target wasm32-unknown-unknown`, `cargo test -p oxide-agent-web-ui`, and `git diff --check` before final completion.
  - Status: pending
  - Evidence collected: 2026-06-05 checkpoint 1 passed `cargo fmt`, `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`, `cargo test -p oxide-agent-web-ui`, `cargo test -p oxide-agent-web-ui --target wasm32-unknown-unknown --no-run`, and `git diff --check`.

- N1: No hidden visual redesign
  - Source: Refactor request is about slicing/maintainability, not UI changes.
  - Must preserve: Existing visible strings, task/card/composer/activity CSS classes, collapsed/expanded defaults, preview priorities, and specialized parser behavior unless explicitly approved later.
  - Evidence required: diff review and behavior checklist in progress log.
  - Status: pending
  - Evidence collected: 2026-06-05 checkpoint 1 was a mechanical helper move; no CSS files changed.

## Implementation Plan

1. Extract low-risk pure helpers first
   - Audit IDs: G1, G5, Q1, Q2, V1, N1.
   - Expected changes: create the `tasks/` module directory and move pure helper groups such as version grouping, session/task summary helpers, submit error mapping, delivered-file linkification tests, and small payload/event helpers where dependencies are minimal.
   - Validation: `cargo fmt`; `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`; `cargo test -p oxide-agent-web-ui`; `git diff --check`.
   - Exit condition: pure helpers compile from focused modules, tests still pass, and no UI behavior changes are introduced.

2. Extract streaming and composer slices
   - Audit IDs: G2, G3, Q1, Q2, V1, N1.
   - Expected changes: move `StreamUiSignals`/`start_task_stream` into `tasks/streaming.rs`; move pending attachments, profile/effort selects, attachment lists, browser file helpers, and composer utility functions into `tasks/composer.rs`.
   - Validation: wasm check and focused diff review for `WaitingForUserInput`, stale stream guard, profile sentinel mapping, and effort persistence.
   - Exit condition: `SessionWorkspace` still submits/resumes/cancels and compiles using the extracted modules.

3. Extract delivered-file, task-card, and edit-form slices
   - Audit IDs: G2, G4, G5, Q1, V1, N1.
   - Expected changes: move delivered-file models/rendering/linkification to `tasks/delivered_files.rs`; move `TaskCard`, user-message rendering, resume-message extraction, and `TaskInputEditForm` to `tasks/task_card.rs`.
   - Validation: native tests for delivered-file linkification; wasm check; diff review for edit-version stream startup.
   - Exit condition: task cards, version switching, editing, final response copy, delivered files, and resume messages remain equivalent.

4. Extract activity and tool-card slices
   - Audit IDs: G4, Q1, Q2, V1, N1.
   - Expected changes: move drawer/status/timing/grouping/context/todos cards to `tasks/activity.rs`; move tool-card dispatcher, specialized cards, visual primitives, and sub-agent/todo/search parsers to `tasks/tool_cards.rs` or a small local sub-split if needed.
   - Validation: wasm check, clippy, and behavior checklist for tool call/result pairing, specialized branches, reasoning/todos/context cards, and CSS classes.
   - Exit condition: activity UI compiles with the same event filtering, grouping, and visual behavior.

5. Extract workspace facade and final audit
   - Audit IDs: G1-G5, Q1-Q2, V1, N1.
   - Expected changes: move `WelcomeView` and `SessionWorkspace` into `tasks/workspace.rs`; keep `crates/oxide-agent-web-ui/src/tasks.rs` as a compact module hub/re-export for `TaskConsole`; update this goal document with evidence.
   - Validation: full validation contract and file-list/diff audit.
   - Exit condition: every Completion Audit item is verified and the refactor is ready for final checkpoint commit.

## Validation Contract

- Format/static checks:
  - `cargo fmt`
  - `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`
  - `cargo clippy -p oxide-agent-web-ui --target wasm32-unknown-unknown`
  - `git diff --check`
- Tests:
  - `cargo test -p oxide-agent-web-ui`
- Optional build artifact when the final diff is non-trivial:
  - `env -u NO_COLOR trunk build --release`
- Done when: every Completion Audit item is verified, `TaskConsole` remains the stable external entry point, no out-of-scope files/behaviors changed, and `tasks.rs` is reduced to a compact facade.

## Decisions

- 2026-06-05: Use `docs/goals/2026-06-05-web-ui-tasks-slice-refactor.md` because the repo stores durable goal docs under `docs/goals/`.
- 2026-06-05: Keep the refactor UI-only and behavior-preserving; backend routes, contracts, SSE formats, and CSS semantics are out of scope.
- 2026-06-05: Do not introduce new dependencies or generic UI frameworks. This is a mechanical module split, not a redesign.
- 2026-06-05: First implementation step is extracting low-risk pure helpers before moving large Leptos components, because it creates the module directory with the smallest behavior risk.

## Progress Log

- 2026-06-05: Goal document created from `tasks.rs` recon.
  - Changed: Added this goal contract and checkpoint plan.
  - Evidence: Existing docs convention found under `docs/goals/`; `TaskConsole` external usage confirmed in `components.rs`; wasm-only module gating confirmed in `main.rs`; major `tasks.rs` responsibilities mapped by line range.
  - Commands: `git status --short`; `git log --oneline -5`.
  - Audit IDs updated: none.
  - Next: Checkpoint 1 — extract low-risk pure helpers first.

- 2026-06-05: Checkpoint 1 completed.
  - Changed: Added focused helper modules for delivered files, payload helpers, profile/effort mapping, task/session state helpers, and version grouping; `tasks.rs` now declares these child modules and dropped from 4,291 to 3,903 lines.
  - Evidence: Diff review found no backend, contract, CSS, or dependency changes; helper tests were moved with their helper modules.
  - Commands: `cargo fmt`; `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`; `cargo test -p oxide-agent-web-ui`; `cargo test -p oxide-agent-web-ui --target wasm32-unknown-unknown --no-run`; `git diff --check`.
  - Audit IDs updated: G5 verified; Q1, Q2, V1, N1 evidence collected.
  - Next: Checkpoint 2 — extract streaming and composer slices.

## Risks and Blockers

- Leptos view type friction during component moves.
  - Impact: Compile failures or accidental boxing/visibility churn.
  - Evidence: Existing code mixes `impl IntoView`, `AnyView`, `view!`, `For`, `Children`, and conditional branches.
  - Mitigation: Move pure helpers first; move components in small groups; use `pub(super)` and local `AnyView` only where the existing code already needs it.
  - Audit IDs affected: G1, G4, V1.

- Accidental behavior changes while splitting session/task flows.
  - Impact: Regressions in create/resume/cancel/edit-version workflows.
  - Evidence: `SessionWorkspace` currently interleaves API calls, signal updates, stream startup, drawer state, and task summary updates.
  - Mitigation: Keep moves mechanical; preserve call order; validate wasm after each meaningful checkpoint; review `WaitingForUserInput` and `streaming_task_id` handling explicitly.
  - Audit IDs affected: G2, V1, N1.

- Tool payload parsing depends on loose JSON shapes.
  - Impact: Specialized cards can lose previews/status/meta if helper moves alter key lookup order.
  - Evidence: Tool rendering uses payload keys such as `name`, `id`, `input_preview`, `output_preview`, `structured_payload`, stdout/stderr shapes, sub-agent statuses, and todo arrays.
  - Mitigation: Move payload helpers separately; keep specialized parsing local unless a helper is already trivial and behavior-preserving.
  - Audit IDs affected: G4, N1.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
