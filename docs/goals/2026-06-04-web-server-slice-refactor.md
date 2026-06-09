# Goal: Web Server Slice Refactor

Date started: 2026-06-04
Status: complete
Codex goal: Implement this document until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals.
Source spec: User request and recon of `crates/oxide-agent-transport-web/src/server/mod.rs`
Goal doc owner: Codex
Last updated: 2026-06-04 20:53 +0300

## Objective

Reduce the maintenance burden of `crates/oxide-agent-transport-web/src/server/mod.rs` by slicing it into locally understandable server modules and then deduplicating the confirmed repeated logic.

Done when the planned checkpoints are mechanically verified, public API/route behavior remains compatible, and every required Completion Audit item is verified by its listed evidence.

## Scope

In scope:
- `crates/oxide-agent-transport-web/src/server/mod.rs` and new/related files under `crates/oxide-agent-transport-web/src/server/`.
- Focused backend server tests and e2e helpers only if imports must be adjusted for the slice.
- Documentation updates in this goal document.

Out of scope:
- Changing REST paths, response DTOs, status codes, auth cookie semantics, CSRF behavior, task lifecycle behavior, SSE replay/stream behavior, or UI API paths.
- Replacing the axum router, auth model, web store, session registry, task executor, or SSE implementation.
- Adding new crates, services, persistence backends, queues, middleware frameworks, or generalized abstractions.
- Direct Google Gemini provider work or unrelated transport/core refactors.

## Repository Context

- `crates/oxide-agent-transport-web/src/lib.rs:47` exposes `pub mod server`; `crates/oxide-agent-transport-web/src/lib.rs:53` re-exports `pub use server::*`.
- `crates/oxide-agent-transport-web/src/server/mod.rs` is now a thin server module hub with health/public-config handlers, shared error helper, public exports, and `pub(crate)` re-exports for focused route slices.
- `crates/oxide-agent-transport-web/src/server/mod.rs:30` re-exports `types::*`; public `build_router` and `serve` must remain reachable from the crate root.
- Tests import many private handlers via `super::{...}` from the inline module, so handler moves require either import updates or `pub(crate)` re-exports.
- UI path coupling exists in `crates/oxide-agent-web-ui/src/api.rs` and `crates/oxide-agent-web-ui/src/sse.rs`; paths must not change during this refactor.
- Existing server submodules include `auth_helpers`, `auth_routes`, `auto_title`, `converters`, `model_routes`, `agent_profiles`, `settings_routes`, `session_routes`, `task_routes`, `router`, `sse`, `static_assets`, `task_executor`, and `types`.

## Completion Audit

- G1: Tests are moved out of `server/mod.rs` mechanically
  - Source: User requested first implementation step after recon.
  - Acceptance: Inline test block is replaced by `#[cfg(test)] mod tests;`; test code lives in `crates/oxide-agent-transport-web/src/server/tests.rs`; no behavior or test assertions are intentionally changed.
  - Evidence required: diff review plus `cargo test -p oxide-agent-transport-web --no-default-features server::tests`.
  - Status: verified
  - Evidence collected: `crates/oxide-agent-transport-web/src/server/mod.rs` now ends with `#[cfg(test)] mod tests;` at lines 2295-2296; moved test code into `crates/oxide-agent-transport-web/src/server/tests.rs`. `cargo test -p oxide-agent-transport-web --no-default-features server::tests` passed with 33 passed / 0 failed. `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite server::tests` passed with 38 passed / 0 failed.

- G2: Router/server shell is isolated without public API breakage
  - Source: Recon plan checkpoint 3.
  - Acceptance: `build_router`, `serve`, CORS, and security-header middleware live in a router/server-shell slice while `oxide_agent_transport_web::build_router` and `oxide_agent_transport_web::serve` remain valid.
  - Evidence required: diff review, `cargo check -p oxide-agent-transport-web --no-default-features`, `cargo test -p oxide-agent-transport-web --no-default-features --test e2e`.
  - Status: verified
  - Evidence collected: `crates/oxide-agent-transport-web/src/server/router.rs` now owns `build_router`, `serve`, CORS, and security headers; `crates/oxide-agent-transport-web/src/server/mod.rs` re-exports `pub use router::{build_router, serve};`. Default/profile-lite `cargo check` passed; default e2e passed with 6 passed / 24 ignored.

- G3: Simple route slices are extracted safely
  - Source: Recon plan checkpoint 4.
  - Acceptance: auth, settings, model-route, and agent-profile handlers/helpers are moved into focused modules with stable route behavior and direct tests still compiling.
  - Evidence required: focused diff review and server tests for default/profile-lite configurations.
  - Status: verified
  - Evidence collected: Added `auth_routes.rs`, `settings_routes.rs`, `model_routes.rs`, and `agent_profiles.rs`; `server/mod.rs` keeps `pub(crate)` re-exports for router/tests and is down to 1343 lines. Default/profile-lite server tests passed.

- G4: Complex session/task slices are extracted after simpler slices
  - Source: Recon plan checkpoint 5.
  - Acceptance: session and task handlers move into focused modules without task lifecycle, persistence, sandbox, attachment, SSE, or runtime-session behavior changes.
  - Evidence required: server tests, profile-lite server tests, e2e tests, and route review.
  - Status: verified
  - Evidence collected: Added `crates/oxide-agent-transport-web/src/server/session_routes.rs` for session CRUD/uploads and sandbox/session lifecycle helpers; added `crates/oxide-agent-transport-web/src/server/task_routes.rs` for task CRUD/progress/events/files/version/resume/cancel and runtime-task helpers; `server/mod.rs` is down to 112 lines and preserves route/test re-exports. Default/profile-lite checks, default/profile-lite server tests, e2e, clippy, and `git diff --check` passed.

- G5: Confirmed duplicated logic is deduplicated only after mechanical slices pass
  - Source: Recon duplication findings.
  - Acceptance: `api_login` uses the shared auth-session response path; task launch persistence/spawn boilerplate and session-save/status patterns are reduced with small local helpers; no premature generic framework is introduced.
  - Evidence required: focused diff review and the same server/e2e validation used for affected routes.
  - Status: verified
  - Evidence collected: `crates/oxide-agent-transport-web/src/server/auth_routes.rs` now uses shared rate-limit result handling and shared auth-session cookie/CSRF response construction for register/bootstrap/login. `crates/oxide-agent-transport-web/src/server/task_routes.rs` now uses local helpers for running task record construction, session save/status updates, cancelled session status, and persisted runtime task spawning. Default/profile-lite checks, default/profile-lite server tests, e2e, clippy, and `git diff --check` passed.

- Q1: Public API compatibility is preserved
  - Source: Blast-radius analysis.
  - Acceptance: Existing consumers of `serve`, `build_router`, `AppState`, and `build_r2_backed_app_state` still compile.
  - Evidence required: `cargo check -p oxide-agent-transport-web --no-default-features` and `cargo check -p oxide-agent-transport-web --no-default-features --features profile-lite`.
  - Status: verified
  - Evidence collected: Checkpoints 2-5 preserved crate-root `build_router`/`serve` exports and passed default/profile-lite `cargo check`.

- Q2: Route, auth, CSRF, SSE, and task semantics remain unchanged during mechanical extraction
  - Source: User guardrails and recon blast-radius.
  - Acceptance: No route path edits, no DTO edits, no auth/cookie/CSRF behavior edits, no SSE stream/replay edits, and no task lifecycle behavior edits are made in mechanical checkpoints.
  - Evidence required: diff review plus server/e2e tests.
  - Status: verified
  - Evidence collected: Checkpoints 1-4 did not edit route paths, DTOs, auth/cookie/CSRF behavior, SSE behavior, or task lifecycle code; checkpoint 5 only consolidated equivalent helper paths. Default e2e passed after checkpoint 5.

- N1: No over-engineering or new dependencies
  - Source: `AGENTS.md` implementation bias.
  - Must preserve: No new crates, services, storage backends, queues, middleware frameworks, or broad abstractions are introduced for this refactor.
  - Evidence required: `Cargo.toml` diff review and implementation diff review.
  - Status: verified
  - Evidence collected: No `Cargo.toml` changes and no new dependencies/services were introduced in checkpoints 1-5.

## Implementation Plan

1. Move inline server tests into `server/tests.rs`
   - Audit IDs: G1, Q1, Q2, N1.
   - Expected changes: create `crates/oxide-agent-transport-web/src/server/tests.rs`; replace inline block in `server/mod.rs` with `#[cfg(test)] mod tests;`.
   - Validation: `cargo test -p oxide-agent-transport-web --no-default-features server::tests`.
   - Exit condition: tests compile/pass and diff is mechanical.

2. Extract router/server shell
   - Audit IDs: G2, Q1, Q2, N1.
   - Expected changes: move `build_router`, `serve`, `web_cors_layer`, `add_security_headers` to a focused router module; re-export public functions from `server/mod.rs`.
   - Validation: default/profile-lite cargo checks and e2e test target.
   - Exit condition: crate-root public calls still compile.

3. Extract simple route slices
   - Audit IDs: G3, Q1, Q2, N1.
   - Expected changes: move auth, settings, model-route, and agent-profile handlers/helpers into focused modules; preserve direct test access through imports or `pub(crate)` re-exports.
   - Validation: default/profile-lite server tests.
   - Exit condition: direct handler tests still compile and pass.

4. Extract complex session/task route slices
   - Audit IDs: G4, Q1, Q2, N1.
   - Expected changes: move session CRUD, uploads, task CRUD, progress/events/files, versioning, resume/cancel, and related helpers into focused modules.
   - Validation: default/profile-lite server tests and e2e test target.
   - Exit condition: route behavior and task lifecycle tests remain green.

5. Deduplicate confirmed repeated logic
   - Audit IDs: G5, Q1, Q2, N1.
   - Expected changes: small local helpers only where duplication was confirmed: auth success response/rate-limit path, task spawn persistence suffix, session save/status update, optional task-record constructor.
   - Validation: affected server tests and e2e tests.
   - Exit condition: duplicated logic is reduced without changing contracts.

## Validation Contract

- Static checks:
  - `cargo check -p oxide-agent-transport-web --no-default-features`
  - `cargo check -p oxide-agent-transport-web --no-default-features --features profile-lite`
- Tests:
  - `cargo test -p oxide-agent-transport-web --no-default-features server::tests`
  - `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite server::tests`
  - `cargo test -p oxide-agent-transport-web --no-default-features --test e2e`
- Done when: every Completion Audit item is verified, and all non-goals remain preserved by diff review.

## Decisions

- 2026-06-04: Use `docs/goals/2026-06-04-web-server-slice-refactor.md` because the repo already stores durable goal docs under `docs/goals/`.
- 2026-06-04: First implementation step is a mechanical test move to reduce `server/mod.rs` by the largest low-risk block before production handler slicing.
- 2026-06-04: Deduplication waits until mechanical slices pass, to avoid mixing behavior refactors with file moves.
- 2026-06-04: Checkpoint 5 used only local helpers in existing route modules; no new shared framework was introduced for two or three call sites.

## Progress Log

- 2026-06-04 00:00: Goal document created from recon.
  - Changed: Added this goal contract and checkpoint plan.
  - Evidence: Existing docs convention found under `docs/goals/`.
  - Commands: none.
  - Audit IDs updated: none.
  - Next: Move inline `server::tests` into `crates/oxide-agent-transport-web/src/server/tests.rs`.

- 2026-06-04 19:57 +0300: Checkpoint 1 completed.
  - Changed: Replaced the inline test module in `crates/oxide-agent-transport-web/src/server/mod.rs` with `#[cfg(test)] mod tests;`; added `crates/oxide-agent-transport-web/src/server/tests.rs` with the moved tests; ran `cargo fmt -p oxide-agent-transport-web`.
  - Evidence: `server/mod.rs` line count is now 2296; `server/tests.rs` contains the moved 3123-line test module body.
  - Commands: `cargo test -p oxide-agent-transport-web --no-default-features server::tests` (33 passed); `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite server::tests` (38 passed); `cargo clippy -p oxide-agent-transport-web --no-default-features --tests` (passed); `git diff --check` (no output).
  - Audit IDs updated: G1 verified; Q2 and N1 have checkpoint evidence.
  - Next: Extract router/server shell into a focused module while preserving `build_router`/`serve` exports.

- 2026-06-04 20:04 +0300: Checkpoint 2 completed.
  - Changed: Added `crates/oxide-agent-transport-web/src/server/router.rs`; moved `build_router`, `serve`, CORS, and security-header middleware; re-exported `build_router`/`serve` from `server/mod.rs`.
  - Commands: `cargo check -p oxide-agent-transport-web --no-default-features`; `cargo check -p oxide-agent-transport-web --no-default-features --features profile-lite`; `cargo test -p oxide-agent-transport-web --no-default-features server::tests` (33 passed); `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite server::tests` (38 passed); `cargo test -p oxide-agent-transport-web --no-default-features --test e2e` (6 passed / 24 ignored); `cargo clippy -p oxide-agent-transport-web --no-default-features --tests`; `git diff --check`.
  - Audit IDs updated: G2 verified; Q1, Q2, N1 have checkpoint evidence.
  - Next: Extract simple route slices: auth, settings, model routes, and agent profiles.

- 2026-06-04 20:21 +0300: Checkpoint 3 completed.
  - Changed: Extracted auth, settings, model-route, and agent-profile routes/helpers into focused modules; kept `pub(crate)` re-exports for router/tests; removed stale auth/model/profile imports from `mod.rs`.
  - Commands: `cargo check -p oxide-agent-transport-web --no-default-features`; `cargo check -p oxide-agent-transport-web --no-default-features --features profile-lite`; `cargo test -p oxide-agent-transport-web --no-default-features server::tests` (33 passed); `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite server::tests` (38 passed); `cargo test -p oxide-agent-transport-web --no-default-features --test e2e` (6 passed / 24 ignored); `cargo clippy -p oxide-agent-transport-web --no-default-features --tests`; `git diff --check`.
  - Audit IDs updated: G3 verified; Q1, Q2, N1 have checkpoint evidence.
  - Next: Extract complex session/task route slices.

- 2026-06-04 20:34 +0300: Checkpoint 4 completed.
  - Changed: Extracted session CRUD/uploads into `crates/oxide-agent-transport-web/src/server/session_routes.rs`; extracted task CRUD/progress/events/files/version/resume/cancel and task/runtime helpers into `crates/oxide-agent-transport-web/src/server/task_routes.rs`; reduced `server/mod.rs` to 112 lines with stable public and test-facing re-exports.
  - Commands: `cargo fmt -p oxide-agent-transport-web`; `cargo check -p oxide-agent-transport-web --no-default-features`; `cargo check -p oxide-agent-transport-web --no-default-features --features profile-lite`; `cargo test -p oxide-agent-transport-web --no-default-features server::tests` (33 passed); `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite server::tests` (38 passed); `cargo test -p oxide-agent-transport-web --no-default-features --test e2e` (6 passed / 24 ignored); `cargo clippy -p oxide-agent-transport-web --no-default-features --tests`; `git diff --check`.
  - Audit IDs updated: G4 verified; Q1, Q2, N1 have checkpoint evidence.
  - Next: Deduplicate confirmed repeated logic in the now-sliced modules.

- 2026-06-04 20:53 +0300: Checkpoint 5 completed.
  - Changed: Deduplicated auth success/rate-limit response handling in `auth_routes.rs`; added small local task helpers in `task_routes.rs` for running task records, session save/status updates, cancellation status persistence, and persisted runtime task spawning.
  - Commands: `cargo fmt -p oxide-agent-transport-web`; `cargo check -p oxide-agent-transport-web --no-default-features`; `cargo check -p oxide-agent-transport-web --no-default-features --features profile-lite`; `cargo test -p oxide-agent-transport-web --no-default-features server::tests` (33 passed); `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite server::tests` (38 passed); `cargo test -p oxide-agent-transport-web --no-default-features --test e2e` (6 passed / 24 ignored); `cargo clippy -p oxide-agent-transport-web --no-default-features --tests`; `git diff --check`.
  - Audit IDs updated: G5, Q1, Q2, and N1 verified.
  - Next: No planned implementation checkpoint remains; keep only follow-up review/optional cleanup if requested.

## Risks and Blockers

- Direct test imports from `super::{...}` may break when handlers move.
  - Impact: Compile failures during later route-slice checkpoints.
  - Evidence: Inline tests import handlers/helpers directly from `server/mod.rs`.
  - Mitigation: Keep moves mechanical and add `pub(crate)` re-exports or update imports per slice.
  - Audit IDs affected: G3, G4.

- Public re-exports can break external consumers if moved carelessly.
  - Impact: Binary, dev example, and e2e helpers fail to compile.
  - Evidence: `lib.rs` re-exports server module symbols.
  - Mitigation: Preserve `pub use types::*` and public re-export paths for `build_router`/`serve`.
  - Audit IDs affected: G2, Q1.

## Final Verification

- Completion Audit result: G1-G5, Q1-Q2, and N1 are verified.
- Commands run: `cargo fmt -p oxide-agent-transport-web`; `cargo check -p oxide-agent-transport-web --no-default-features`; `cargo check -p oxide-agent-transport-web --no-default-features --features profile-lite`; `cargo test -p oxide-agent-transport-web --no-default-features server::tests`; `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite server::tests`; `cargo test -p oxide-agent-transport-web --no-default-features --test e2e`; `cargo clippy -p oxide-agent-transport-web --no-default-features --tests`; `git diff --check`.
- Artifacts inspected: `crates/oxide-agent-transport-web/src/server/mod.rs`, focused server route modules under `crates/oxide-agent-transport-web/src/server/`, crate-root public exports, and this goal document.
- Remaining gaps: none for the stated goal.
- User-accepted exceptions: none.
- Final status: complete.
