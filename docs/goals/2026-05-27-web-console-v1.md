# Goal: Web Console V1

Date started: 2026-05-27
Status: active
Codex goal: Study `docs/prd/PRD_web.md`, create repo-local goal documentation from it, and iteratively implement the web PRD in Oxide-Agent with validation checkpoints.
Source spec: `docs/prd/PRD_web.md`
Goal doc owner: Codex
Last updated: 2026-05-28 09:59 +03

## Objective

Implement the PRD-defined Rust-only web console for Oxide Agent V1: authenticated browser access, user-isolated sessions and tasks, durable R2-backed web state, live task progress/events with replayable SSE, persisted final answers, safe Markdown rendering, and a simple Rust/WASM frontend served by the Rust backend.

Done when every required Completion Audit item is verified by its listed evidence and all non-goals remain preserved.

## Scope

In scope:
- Evolve `crates/oxide-agent-transport-web` from test-only HTTP transport into the production browser API backend while preserving core/runtime boundaries.
- Add shared Rust API contracts in `crates/oxide-agent-web-contracts`.
- Add Rust-only frontend in `crates/oxide-agent-web-ui` using Leptos CSR and Trunk.
- Add web auth, browser sessions, CSRF, user registration/bootstrap/change-password, and strict user isolation.
- Add durable web persistence for users, auth sessions, web sessions, task records, progress snapshots, final answers, and chunked task events.
- Add `/api/v1/...` browser API, replayable SSE, task resume/cancel/edit flows, and static asset serving.
- Update tests, docs, config examples, and validation commands for the new web console.

Out of scope:
- Replacing `oxide-agent-core`, `oxide-agent-runtime`, or `SessionRegistry`.
- Creating a new web transport from scratch while `oxide-agent-transport-web` can be evolved.
- TypeScript/React/Vue/Svelte/Solid/Next/Nuxt/Vite+TS application code.
- OAuth/SAML/SSO, org/workspace multitenancy, advanced admin UI, complex role matrix, workflow builder, or polished mobile app.
- Approve/reject browser UI. V1 runs web tasks in YOLO/full-permission mode and maps unexpected approval waits to failed tasks.
- SQL databases, migrations, Redis/memcached, distributed locks, cross-instance broadcast, sharding, or HA.

## Repository Context

- PRD: `docs/prd/PRD_web.md`.
- Existing backend base: `crates/oxide-agent-transport-web/src/server.rs`, `session.rs`, `web_transport.rs`, `in_memory_storage.rs`.
- Runtime execution: `crates/oxide-agent-core/src/agent/executor.rs`, `agent/executor/execution.rs`, `crates/oxide-agent-runtime/src/session_registry.rs`.
- Storage base: `crates/oxide-agent-core/src/storage/`, especially `R2Storage`, `build_primary_storage`, JSON/list helpers, and context-scoped memory APIs.
- Useful transport reference: `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/session.rs` and task runner behavior for durable memory/outcome handling.
- Test base: `crates/oxide-agent-transport-web/tests/e2e/*`.
- Repo constraints: Rust 1.94, empty default features, no direct Gemini provider, R2 is production durable storage, personal-scale single instance up to 5 RPS.
- Current gaps confirmed from PRD and code: web endpoints are unversioned/test-oriented, `CreateSessionBody` accepts browser `user_id`, task/session/event/progress state is mostly in memory, final answers are discarded, `/progress` is stale until collection ends, events lack payload/sequence, SSE has a 60-second cutoff and no replay, and multiple active tasks per session are allowed.

## Completion Audit

- G1: Rust-only web console architecture
  - Source: PRD sections 2, 3, 6, 7.
  - Requirement: Build a usable web console with Rust backend and Rust/WASM frontend, reusing `oxide-agent-transport-web`.
  - Acceptance: Workspace contains Rust contracts/backend/frontend crates; no TypeScript SPA stack is introduced; core/runtime do not depend on transport/frontend crates.
  - Evidence required: `Cargo.toml` workspace membership, crate manifests, `rg --files -g '*.ts' -g '*.tsx' -g '*.js' -g '*.jsx'` review, `cargo check` for affected crates.
  - Status: in_progress
  - Evidence collected: `crates/oxide-agent-web-contracts` added as a Rust workspace crate. Added `crates/oxide-agent-web-ui` as a Rust workspace crate with Leptos CSR, Trunk entrypoint/config, Rust API client modules, auth/session/task UI modules, and no handwritten TypeScript/JS files. `cargo check -p oxide-agent-web-ui` and `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown` pass.

- G2: Shared browser API contracts
  - Source: PRD sections 6.6, 7.2, 14, 19 milestone 0.
  - Requirement: Add `oxide-agent-web-contracts` as the single Rust source of truth for auth/session/task/event DTOs, status enums, and error envelope.
  - Acceptance: Backend imports browser-facing DTOs from the contracts crate; DTO serialization tests cover key JSON shapes.
  - Evidence required: crate files, backend dependency, focused contract tests.
  - Status: in_progress
  - Evidence collected: Added `crates/oxide-agent-web-contracts` with auth/config/error/session/task/event DTOs and serialization tests for error envelope, task status, and persisted task events. Backend imports `PublicConfigResponse`.

- G3: Versioned production API namespace
  - Source: PRD sections 8.1, 14.1, 20.
  - Requirement: Browser API lives under `/api/v1/...`; old unversioned e2e endpoints and tests are removed after migration.
  - Acceptance: Frontend and e2e tests use only `/api/v1`; `build_router` no longer exposes legacy `/sessions`, `/debug/event_logs`, or unversioned task paths.
  - Evidence required: route review, e2e tests, `rg -n '"/sessions|/debug/event_logs' crates/oxide-agent-transport-web`.
  - Status: in_progress
  - Evidence collected: Added initial `GET /api/v1/public-config` route backed by shared `PublicConfigResponse`; `bootstrap_required` now reads `WebUiStore::users_count()` plus bootstrap-token config. Auth/session/task/event/progress/SSE browser handlers now live under `/api/v1`. Removed the legacy unversioned route registrations and handler bodies for `/sessions`, `/sessions/...`, and `/debug/event_logs`; focused router test proves `/api/v1/public-config` works and the old paths return 404. `rg -n 'route\("/sessions|"/sessions|/debug/event_logs' crates/oxide-agent-transport-web/src` returns no matches after this slice. Socket e2e helpers now create test auth sessions through `WebUiStore`, send cookie/CSRF headers, and call authenticated `/api/v1` session/task/events/progress/SSE endpoints. Full `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite,socket_e2e --test e2e -- --nocapture` passed with 22 passed / 0 failed / 6 ignored after compaction regression migration; feature-specific delegation and compression socket checks also pass with explicit `delegation_e2e` / `compression_e2e` gates. Frontend API usage remains pending, so this item is not yet verified.

- G4: Auth, registration, bootstrap, and password management
  - Source: PRD sections 9, 14.2-14.8, 16.
  - Requirement: Implement login/register/bootstrap/logout/me/change-password with Argon2id password hashes, opaque server-side cookies, disabled-user handling, generic invalid credentials, first-admin bootstrap, and session rotation.
  - Acceptance: No plaintext password storage; registration obeys config; bootstrap requires token when registration is disabled and no users exist; change-password revokes other sessions.
  - Evidence required: auth unit/integration tests, cookie flag tests, config tests, code review for password/token logging.
  - Status: in_progress
  - Evidence collected: Added `auth` module with ASCII login normalization, password length bounds, Argon2id password hashing/verification, random positive 63-bit user ID allocation, first-user admin registration, disabled-registration rejection, and bootstrap-token enforcement. Added `/api/v1/auth/register`, `/api/v1/auth/bootstrap`, `/api/v1/auth/login`, `/api/v1/me`, `/api/v1/auth/logout`, and `/api/v1/auth/change-password` handlers backed by `WebUiStore` and shared contracts. Login creates opaque browser auth tokens, stores only SHA-256 token hashes, returns CSRF token, and sets HttpOnly SameSite=Lax cookies with Secure enabled in production or via `OXIDE_WEB_COOKIE_SECURE`. Logout revokes the auth session and clears the cookie. Change-password verifies the current password, writes a new Argon2id hash, and revokes other sessions for the user. Rate limiting and full route-wide CSRF enforcement remain pending.

- G5: CSRF and user isolation
  - Source: PRD sections 8.2, 8.18, 9.9, 15.1, 16.2, 17.5.
  - Requirement: All mutating browser endpoints require CSRF; current user comes from server-side session; ownership is checked for every session/task/event/SSE endpoint; foreign resources return 404.
  - Acceptance: Browser bodies never supply trusted `user_id`; user A cannot list/read/cancel/stream user B resources.
  - Evidence required: integration tests for CSRF and cross-user access; route handler review.
  - Status: in_progress
  - Evidence collected: Browser session identity now comes from opaque cookie lookup through `current_user_for_token`; logout/change-password and `/api/v1/sessions` mutating routes check `X-CSRF-Token` against the server-side auth session. Added authenticated `/api/v1/sessions` create/list/get/rename/delete handlers that load records by current `user_id`; foreign session IDs return 404. Added task create/list/detail/events/stream/edit/resume/cancel ownership checks and CSRF coverage for mutating task APIs, with focused tests for foreign task/event/SSE hiding.

- G6: Durable web persistence
  - Source: PRD sections 3, 5.7, 13.
  - Requirement: Add `WebUiStore` with in-memory test implementation and R2-backed implementation for dev/prod users/auth sessions/web sessions/tasks/progress/final answers/event chunks.
  - Acceptance: Persisted JSON docs include `schema_version`; production-like startup fails fast without durable storage; in-memory is explicit dev/test only.
  - Evidence required: store unit tests, R2 key review, startup config tests, persistence integration tests.
  - Status: in_progress
  - Evidence collected: Added `persistence` module with `WebUiStore` trait, local persisted auth/user records, in-memory test store, and store tests covering user/auth session round-trip, user-scoped sessions/tasks/events, event replay pagination, and delete cleanup. Added feature-gated `R2WebUiStore` built on `oxide-agent-core::storage::R2Storage` plus public R2 prefix helpers. The R2-backed implementation uses PRD key prefixes for users/login index/browser sessions/tasks and stores task events as schema-versioned chunk documents. `AppState` now owns a `WebUiStore`, tracks the store kind, exposes a feature-gated `build_r2_backed_app_state` startup builder that constructs one R2 backend for both runtime storage and web UI storage, and `serve` validates/reconciles the store before binding. In-memory startup now fails when `RUN_MODE=prod|production`, `OXIDE_WEB_ENABLED=true`, or `OXIDE_WEB_REQUIRE_DURABLE_STORAGE=true` unless `OXIDE_WEB_ALLOW_IN_MEMORY_STORE=true` explicitly opts into dev/test mode. Full binary/config integration and real R2 smoke remain pending.

- G7: Web session memory scope
  - Source: PRD sections 8.3, 13.7.
  - Requirement: Each web session gets isolated `context_key = web-session-{session_id}` and `agent_flow_id = main`; runtime memory loads/flushes through durable storage.
  - Acceptance: Multiple web sessions for one user do not share agent memory; task terminal/paused states flush memory checkpoint.
  - Evidence required: focused session tests and code review.
  - Status: in_progress
  - Evidence collected: Added `WebSessionManager::create_session_with_id` so browser API can create runtime sessions using the persisted session id. `/api/v1/sessions` create now sets `context_key = web-session-{session_id}` and `agent_flow_id = main`; focused server test verifies persisted record scope fields. Added `WebSessionManager::new_with_storage` and switched the per-session memory checkpoint to use the manager's configured `StorageProvider`, so the R2-backed app-state builder wires runtime memory, wiki memory, topic AGENTS.md, reminder context, and web UI records to the same R2 provider. A full R2 restart smoke test remains pending.

- G8: Session APIs and history
  - Source: PRD sections 8.2, 11.6-11.7, 12.2, 14.9-14.13, 20.
  - Requirement: Implement create/list/get/delete/rename sessions, auto-title from first prompt preview until manual rename, and task-history listing.
  - Acceptance: Session list is current-user-only, sorted by `updated_at desc`, and survives refresh/restart.
  - Evidence required: integration tests and persistence restart-style test.
  - Status: in_progress
  - Evidence collected: Added authenticated `/api/v1/sessions` list/create/get/delete and `PATCH /api/v1/sessions/{session_id}` rename. Session records are stored in `WebUiStore`, listed per current user, and foreign access returns 404 in focused tests. Added `/api/v1/sessions/{session_id}/tasks` task-history listing and auto-title from the first task prompt preview until manual rename. Restart persistence through R2 remains pending.

- G9: Task lifecycle policy
  - Source: PRD sections 8.4-8.6, 8.10, 11.8, 14.14-14.17, 20.
  - Requirement: Support queued/running/waiting_for_user_input/completed/failed/cancelled/interrupted; enforce one active task per session; implement cancel, resume, and edit-last-input APIs.
  - Acceptance: New task during running returns `409 session_busy`; waiting task returns `409 task_waiting_for_user_input`; edit is allowed only for the last terminal task.
  - Evidence required: backend integration/e2e tests and status transition unit tests.
  - Status: in_progress
  - Evidence collected: Added authenticated `/api/v1/sessions/{session_id}/tasks` create/list and `/api/v1/sessions/{session_id}/tasks/{task_id}` detail handlers backed by `WebUiStore`. New task creation validates input, restores missing runtime sessions from persisted web session metadata, persists `running` records, updates session active-task fields, and enforces one active task per session. Added `PATCH /api/v1/sessions/{session_id}/tasks/{task_id}/input`, `POST /api/v1/sessions/{session_id}/tasks/{task_id}/resume`, and `POST /api/v1/sessions/{session_id}/tasks/{task_id}/cancel`. Focused tests cover completed task persistence, `409 session_busy`, `409 task_waiting_for_user_input` with resume `task_id` detail, edit only for latest terminal task, `409 task_active`, foreign task 404, idempotent cancel, and cancelled session/task persistence. Socket e2e `session_tests::e2e_web_followup_while_running_returns_session_busy` now uses the authenticated `/api/v1` helper and verifies the PRD `409 session_busy` behavior. Full `profile-lite,socket_e2e` revalidation passed after the compaction regression migration. Broader cancelled-vs-completed race coverage and interrupted HTTP flows remain pending.

- G10: Correct `AgentExecutionOutcome` mapping
  - Source: PRD sections 5.3.4, 8.7-8.9, 8.15, 18.
  - Requirement: Persist `Completed(String)` final answer, expose `WaitingForUserInput` as distinct paused state, resume the same task ID, and map unexpected `WaitingForApproval` to failed with YOLO diagnostic.
  - Acceptance: Completed tasks cannot be stored without final response; final response is available after refresh/restart.
  - Evidence required: focused outcome mapping tests and HTTP task detail tests.
  - Status: in_progress
  - Evidence collected: Shared task execution now matches `AgentExecutionOutcome`: `Completed(String)` persists `final_response_markdown`, clears the session active task, and updates the session preview/status; `WaitingForUserInput` persists distinct paused metadata and keeps the task active; unexpected `WaitingForApproval` is mapped to failed with the PRD YOLO diagnostic. Focused `profile-lite` HTTP tests verify completed task final answer is returned by task detail after persistence and `POST /resume` continues the same task id from `waiting_for_user_input` to `completed`. Event stream continuation/backfill remains pending with G12/G13.

- G11: Live progress persistence
  - Source: PRD sections 5.3.7, 8.11, 11.9.
  - Requirement: Update and persist progress snapshots while tasks run, including iteration, thought, todos, token snapshot, errors, compaction/retry/failover statuses.
  - Acceptance: `/api/v1` task detail/progress reflects live state before task completion and after refresh.
  - Evidence required: e2e test that observes mid-run progress and persisted final progress.
  - Status: in_progress
  - Evidence collected: Event collector now persists the final `ProgressSnapshot` into `WebTaskRecord.last_progress`; focused task test verifies completed task detail has persisted progress and non-zero `last_event_seq`. `ProgressSnapshot` and `SerializableProgress` now include todos, LLM retry, provider failover, token snapshot, compaction/history-repair/error/thought fields. `collect_events` now emits live `ProgressState` snapshots during event collection, `spawn_event_collector` wires those snapshots into a live progress persister, and `WebTaskRecord.last_progress` is updated while the task is still running. Added authenticated `GET /api/v1/sessions/{session_id}/tasks/{task_id}/progress` backed by durable task records, plus focused tests for live snapshot fan-out, running-task progress persistence, auth-scoped persisted progress reads, and progress snapshot serialization. Full browser/e2e observation of mid-run progress remains pending.

- G12: Rich event model and retention
  - Source: PRD sections 5.3.6, 8.12, 8.16, 13.5, 13.9.
  - Requirement: Persist sequence-numbered browser events with kind, timestamp, summary, safe payload, redaction/truncation flags, and chunked retention.
  - Acceptance: Events are not just names; large/sensitive payloads are previewed/truncated/redacted; raw file bytes are not written into regular event logs.
  - Evidence required: event mapping/truncation tests, chunk store tests, API response tests.
  - Status: in_progress
  - Evidence collected: Added browser event mapping from `AgentEvent` to `PersistedTaskEvent` with `seq`, `kind`, timestamp, summary, serializable payload, and truncation/redaction flags. Tool inputs/results are previewed and truncated; file events store metadata/byte length and mark payload redacted instead of writing raw bytes. Event collection now streams persisted events through a live persister into `WebUiStore` and updates `WebTaskRecord.last_event_seq`. Added authenticated `GET /api/v1/sessions/{session_id}/tasks/{task_id}/events?after_seq=&limit=` backed by `WebUiStore::list_task_events`. R2-backed storage now writes task events under `users/{user_id}/web/v1/task_events/{session_id}/{task_id}/chunk-{chunk_no}.json` as schema-versioned chunk documents and replay flattens chunks by sequence. Focused tests cover payload preview/truncation, REST replay pagination, foreign-event 404, SSE replay after `after_seq`, R2 key layout, chunked event replay pagination, and session delete cleanup for chunked events. Broader event redaction coverage and size-budget retention policy remain pending.

- G13: Replayable SSE
  - Source: PRD sections 5.3.8, 7.7-7.8, 8.13-8.14, 14.18-14.19, 15.2.
  - Requirement: Implement task-specific SSE with no fixed 60-second cutoff, keepalive, `after_seq` and `Last-Event-ID` replay/backfill, snapshot/progress/status events, and reconnect correctness.
  - Acceptance: Page refresh or dropped SSE does not lose task visibility; duplicate events are avoided.
  - Evidence required: SSE reconnect/backfill e2e tests.
  - Status: in_progress
  - Evidence collected: Added authenticated `/api/v1/sessions/{session_id}/tasks/{task_id}/stream` backed by persisted task events and ownership checks. The stream emits an initial `snapshot`, replayed `task_event` records with SSE `id = seq`, `progress` when the persisted progress snapshot changes, `task_status` on status changes/terminal state, and `keepalive` while the task is still active. Replay starts from `after_seq` or `Last-Event-ID`; focused tests cover query-vs-header sequence selection, terminal replay after `after_seq`, duplicate avoidance for skipped seq 1, and foreign-user 404. The old in-memory `/sessions/.../stream` route and handler have been removed from `build_router`. Socket e2e `sse_tests::e2e_sse_stream` now connects to the authenticated `/api/v1` SSE endpoint and passed with `profile-lite,socket_e2e`; live running-task reconnect e2e and frontend SSE client behavior remain pending.

- G14: Startup reconciliation
  - Source: PRD sections 8.4, 13.8, 15.2.
  - Requirement: On backend startup, persisted queued/running tasks from a prior process become `interrupted` and sessions clear stale active task IDs without losing history.
  - Acceptance: Simulated restart test marks stale tasks interrupted and preserves final/event data for terminal tasks.
  - Evidence required: store reconciliation unit/integration test.
  - Status: in_progress
  - Evidence collected: `InMemoryWebUiStore::mark_unfinished_tasks_interrupted` marks queued/running tasks interrupted, preserves completed tasks, and clears matching session `active_task_id`; covered by focused persistence test. The R2/object-store implementation performs the same reconciliation by scanning web task record keys under `users/`, updating queued/running tasks to `interrupted`, and clearing matching session `active_task_id`; covered by focused object-store persistence test. `AppState::reconcile_unfinished_tasks_on_startup` is now called from `serve`, and the R2-backed app-state builder also validates/reconciles before returning. Real process-restart smoke remains pending.

- G15: Rust frontend shell and auth pages
  - Source: PRD sections 7, 9, 11, 19 milestones 4-5.
  - Requirement: Add Leptos CSR frontend with login/register/bootstrap, protected app shell, sidebar, settings/change-password page, routing, loading/error states, and narrow viewport behavior.
  - Acceptance: Authenticated user can navigate sessions/tasks and logout/change password from UI; frontend code is Rust.
  - Evidence required: frontend build, component/state tests where practical, manual QA checklist.
  - Status: in_progress
  - Evidence collected: Added `crates/oxide-agent-web-ui` with Leptos CSR app entrypoint, route parsing for `/login`, `/register`, `/bootstrap`, `/app`, `/app/session/:session_id`, `/settings`, cookie/CSRF-aware Rust API client over `/api/v1`, login/register/bootstrap forms, settings/change-password/logout page, topbar, session sidebar, responsive workspace layout, loading/empty/error states, and narrow viewport CSS. `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown` and `cargo clippy -p oxide-agent-web-ui --target wasm32-unknown-unknown` pass. Browser manual QA and Trunk build remain pending.

- G16: Task console UI
  - Source: PRD sections 11.8-11.13, 12.3, 20.
  - Requirement: UI shows transcript from persisted task records, live events/progress panel, send/cancel/resume/edit flows, final answer, and distinct task states.
  - Acceptance: Running composer is blocked for new top-level task; paused task resumes same task; terminal task allows new task and last-input edit.
  - Evidence required: frontend/runtime QA and backend integration tests.
  - Status: in_progress
  - Evidence collected: Added first task console UI slice in `crates/oxide-agent-web-ui/src/tasks.rs`: persisted task history list, raw Markdown textarea composer, send/resume decision based on `waiting_for_user_input`, stop/cancel action for active task, final answer/error/pending-user-input display, status badges, and an events side panel that can refresh persisted task events. User input and final answers now render through the shared `MarkdownContent` boundary. Live SSE client, progress auto-refresh, edit-last-input UI, and browser QA remain pending.

- G17: Safe Markdown rendering
  - Source: PRD sections 10, 11.10, 12.4, 16.6, 17.6.
  - Requirement: Provide one `MarkdownContent` boundary using Rust markdown parsing and HTML sanitization, safe links/images, code block copy buttons, long-line handling, and invalid/streaming fallback.
  - Acceptance: Unsafe HTML/attrs/protocols are removed; external images become links or are blocked; code blocks are readable/copyable.
  - Evidence required: sanitization tests and manual rendering checks for headings/lists/code/tables/task lists/links.
  - Status: in_progress
  - Evidence collected: Added `crates/oxide-agent-web-ui/src/markdown.rs` with one `MarkdownContent` component and pure `render_markdown`/`sanitize_html` functions using `comrak` with no default features plus `ammonia`. The sanitizer strips script content/event attributes, removes unsafe link protocols, removes image tags, and keeps tables/code blocks. `cargo test -p oxide-agent-web-ui markdown` passes with 4 tests. Task transcript user/final-answer content now uses `MarkdownContent`; copy buttons, full task-list rendering, same-origin image policy, and manual rendering QA remain pending.

- G18: Static assets, CORS, and security headers
  - Source: PRD sections 7.11, 8.19-8.20, 16.8, 18.
  - Requirement: Prod backend serves built frontend assets same-origin, rejects missing prod assets, avoids permissive CORS in prod, and sets sane security headers compatible with WASM.
  - Acceptance: Dev flow supports backend + Trunk proxy; prod flow serves index/assets/browser routes from Rust backend.
  - Evidence required: route/static serving tests, config tests, manual prod smoke.
  - Status: in_progress
  - Evidence collected: Added static asset configuration to `AppState` through `OXIDE_WEB_STATIC_DIR` and `OXIDE_WEB_REQUIRE_STATIC_ASSETS`, with production-mode startup validation requiring a frontend `index.html`. Backend fallback now serves browser routes from the configured frontend dist directory, serves static files with MIME types including `application/wasm`, leaves missing `/api/*` paths as 404, and adds security headers (`X-Content-Type-Options`, `Referrer-Policy`, `X-Frame-Options`, CSP) through middleware. Production CORS now uses `CorsLayer::new()` while non-prod dev mode uses explicit `Any` origins/methods/headers for Trunk/backend development. Focused tests cover missing-index fail-fast, SPA fallback, WASM content type, security headers, and API 404 behavior; `rg -n 'CorsLayer::permissive|/debug/event_logs|route\("/sessions' crates/oxide-agent-transport-web/src` returns no matches. Real `trunk build` output and production manual smoke remain pending.

- Q1: Keep implementation simple for personal scale
  - Source: `AGENTS.md` scale principles and PRD section 20 single-instance decision.
  - Acceptance: No Redis, DB migrations, distributed locks, queues, sharding, or heavy observability added.
  - Evidence required: dependency/config review.
  - Status: pending
  - Evidence collected:

- Q2: Preserve architecture invariants
  - Source: `AGENTS.md` architectural invariants.
  - Acceptance: `oxide-agent-core` and `oxide-agent-runtime` remain transport/frontend-independent; teloxide stays outside core/runtime/web UI.
  - Evidence required: `cargo tree`/manifest review and compile checks.
  - Status: pending
  - Evidence collected:

- Q3: Security baseline
  - Source: PRD sections 15-16.
  - Acceptance: No secrets/tokens/passwords in logs/docs; cookies/CSRF/security headers/rate limits implemented; markdown and event payloads are treated as untrusted.
  - Evidence required: focused security tests and review.
  - Status: in_progress
  - Evidence collected: Password hashes now use Argon2id self-describing strings for register/bootstrap/change-password paths; plaintext password storage is not used in `WebUserRecord`. Browser auth sessions store SHA-256 hashes of opaque tokens instead of raw cookie tokens; cookies are HttpOnly, SameSite=Lax, Path=/, Max-Age bounded, and Secure in production/configured secure mode. Change-password revokes other sessions after password update. Broader route-wide CSRF/rate-limit/security-header/markdown/event security remains pending.

- Q4: Durable JSON compatibility
  - Source: PRD sections 13, 15.3.
  - Acceptance: Each persisted document has `schema_version`; corrupt/unknown records fail safely instead of panicking.
  - Evidence required: serialization tests and corrupt-record tests.
  - Status: pending
  - Evidence collected:

- Q5: Basic UX quality
  - Source: PRD sections 7.10, 11, 17.9.
  - Acceptance: UI has explicit loading/empty/error/session-busy/SSE-reconnect states and remains usable on narrow viewport.
  - Evidence required: manual QA checklist and screenshots if Playwright/browser tooling is used.
  - Status: in_progress
  - Evidence collected: First frontend slice includes explicit auth/session/task loading, empty, and error states plus responsive CSS for narrow viewports. Session-busy and SSE reconnect UI states remain pending with the SSE client work; no browser screenshot/manual QA evidence has been collected yet.

- V1: Formatting and lint validation
  - Source: `AGENTS.md` development practice.
  - Requirement: Run `cargo fmt` and relevant `cargo clippy` before claiming completion.
  - Evidence required: command outputs.
  - Status: in_progress
  - Evidence collected: `cargo fmt`, `cargo fmt --check`, `cargo clippy -p oxide-agent-web-contracts -p oxide-agent-transport-web --no-default-features`, `cargo clippy -p oxide-agent-transport-web --no-default-features --features profile-lite`, `cargo clippy -p oxide-agent-web-ui`, and `cargo clippy -p oxide-agent-web-ui --target wasm32-unknown-unknown` passed after contracts, `/api/v1/public-config`, persistence interface, registration/bootstrap, browser-session auth, authenticated sessions, task create/list/detail/edit/resume/cancel APIs, persisted event replay API, live event persistence, `/api/v1` SSE, R2-backed web store, startup guard, R2 app-state builder, durable runtime storage wiring, live progress persistence, legacy route removal, authenticated socket e2e helper migration, socket compaction regression migration, the first Leptos frontend shell slice, Markdown boundary, and backend static/security serving. Broader workspace clippy remains pending for later checkpoints.

- V2: Backend compile/test validation
  - Source: `AGENTS.md`, PRD section 17.
  - Requirement: Run focused and broad backend checks for web contracts/transport/core affected features.
  - Evidence required: `cargo check -p oxide-agent-web-contracts`, `cargo check -p oxide-agent-transport-web --no-default-features`, focused web tests, and broader checks as implementation scope grows.
  - Status: in_progress
  - Evidence collected: `cargo check -p oxide-agent-web-contracts`, `cargo test -p oxide-agent-web-contracts`, `cargo check -p oxide-agent-transport-web --no-default-features`, `cargo check -p oxide-agent-transport-web --no-default-features --features profile-lite`, `cargo test -p oxide-agent-transport-web --no-default-features web_transport`, `cargo test -p oxide-agent-transport-web --no-default-features server::tests`, `cargo test -p oxide-agent-transport-web --no-default-features session::tests`, `cargo test -p oxide-agent-transport-web --no-default-features persistence`, `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite persistence`, `cargo test -p oxide-agent-transport-web --no-default-features auth`, `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite server::tests`, `cargo test -p oxide-agent-transport-web --no-default-features --test e2e`, `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite,socket_e2e --test e2e -- --nocapture`, `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite,socket_e2e,delegation_e2e --test e2e delegation_tests::e2e_spawned_sub_agent_does_not_block_task_completion -- --nocapture`, `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite,socket_e2e,compression_e2e --test e2e compaction_regression_tests::e2e_compress_tool_triggers_manual_compaction -- --nocapture`, `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite,socket_e2e,compression_e2e --test e2e compaction_regression_tests::e2e_compress_preserves_tool_heavy_batch_continuation -- --nocapture`, focused static serving tests, and post-static-serving `server::tests` runs under no-default and `profile-lite` passed after the startup guard/R2 app-state builder, live progress persistence, legacy route removal, authenticated socket helper migration, supported route alignment, structured scripted-response slices, compaction socket assertion migration, and backend static/security serving. The task execution outcome/resume tests are `profile-lite` gated because no-default builds intentionally have no compiled provider capability module for the mocked task runner route.

- V3: Frontend build/test validation
  - Source: PRD sections 7, 17, 19.
  - Requirement: Build Rust/WASM frontend and run available component/contract/markdown tests.
  - Evidence required: Trunk/frontend build command and test outputs.
  - Status: in_progress
  - Evidence collected: `cargo check -p oxide-agent-web-ui`, `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`, `cargo clippy -p oxide-agent-web-ui`, `cargo clippy -p oxide-agent-web-ui --target wasm32-unknown-unknown`, and `cargo test -p oxide-agent-web-ui markdown` pass for the first Leptos CSR shell and Markdown boundary. `command -v trunk` found no installed Trunk binary in the current environment, so an actual `trunk build` is still pending.

- V4: Runtime/manual QA validation
  - Source: PRD sections 17.9, 18.
  - Requirement: Complete the manual QA checklist covering auth, sessions, tasks, refresh/reconnect, cancel/resume, isolation, markdown, and narrow layout.
  - Evidence required: checklist entries with commands/URLs/results.
  - Status: pending
  - Evidence collected:

- N1: No TypeScript SPA stack
  - Source: PRD sections 3-4, 6.5.
  - Must preserve: No React/Vue/Svelte/Solid/Next/Nuxt/Vite+TS application stack.
  - Evidence required: file/dependency review.
  - Status: in_progress
  - Evidence collected: Current frontend slice added only Rust crate/code plus `index.html`, `Trunk.toml`, and CSS. `rg --files -g '*.ts' -g '*.tsx' -g '*.js' -g '*.jsx'` returns no files after adding `oxide-agent-web-ui`; no TypeScript SPA stack or package manifest was introduced.

- N2: No approve/reject UI
  - Source: PRD sections 4, 8.9, 20.
  - Must preserve: Web V1 uses YOLO/full-permission mode; no browser approval workflow is added.
  - Evidence required: code/API/UI review.
  - Status: in_progress
  - Evidence collected: No browser approval routes or UI were added. Backend maps unexpected `AgentExecutionOutcome::WaitingForApproval` to a failed task with the documented YOLO diagnostic.

- N3: No SQL or migration framework
  - Source: PRD sections 4, 13.
  - Must preserve: Durable state is versioned JSON over R2/storage APIs.
  - Evidence required: dependency/config review.
  - Status: pending
  - Evidence collected:

- N4: No new transport replacement
  - Source: PRD sections 3, 6, 8.1.
  - Must preserve: Existing `oxide-agent-transport-web` is evolved instead of replaced by a new transport.
  - Evidence required: manifest/module review.
  - Status: in_progress
  - Evidence collected: Existing `oxide-agent-transport-web` was extended with `/api/v1/public-config`; no replacement transport was introduced.

## Implementation Plan

1. Goal and Milestone 0 contracts
   - Audit IDs: G1, G2, G3, Q1, Q2, N1, N4, V2.
   - Expected changes: create this goal doc, add `oxide-agent-web-contracts`, define DTO/status/error models, add serialization tests, and start `/api/v1` router plumbing without broad behavior changes.
   - Validation: `cargo fmt --check`, `cargo check -p oxide-agent-web-contracts`, `cargo test -p oxide-agent-web-contracts`, `cargo check -p oxide-agent-transport-web --no-default-features`.
   - Exit condition: shared contracts compile and initial `/api/v1/public-config` or equivalent plumbing is backed by contracts.

2. Durable web persistence
   - Audit IDs: G6, G7, G8, G9, G11, G12, G14, Q4, N3.
   - Expected changes: `persistence` module, in-memory test store, R2 store, records/key layout, chunked events, startup reconciliation, production in-memory guard.
   - Validation: store unit tests, reconciliation tests, `cargo test -p oxide-agent-transport-web --no-default-features persistence`.
   - Exit condition: web sessions/tasks/events/final answers can round-trip through durable store interfaces.

3. Auth foundation
   - Audit IDs: G4, G5, Q3.
   - Expected changes: auth module, password hashing, login normalization, browser session cookies, CSRF, registration/bootstrap/change-password, auth middleware.
   - Validation: auth unit/integration tests, cookie/CSRF/rate-limit tests.
   - Exit condition: `/api/v1/auth/*`, `/api/v1/me`, and protected routes enforce user identity.

4. Task/session API hardening
   - Audit IDs: G3, G5, G7-G14.
   - Expected changes: migrate sessions/tasks endpoints to `/api/v1`, implement task detail/list/final response/resume/edit/cancel, outcome mapping, live progress persistence, event replay, remove legacy endpoints/tests.
   - Validation: web e2e tests for lifecycle, isolation, final answer persistence, waiting user input, session busy, cancel, SSE replay.
   - Exit condition: backend browser API is production-shaped and old unversioned routes are gone.

5. Rust frontend shell and auth UI
   - Audit IDs: G1, G15, G18, Q5, N1.
   - Expected changes: Leptos CSR crate, Trunk config, API client, auth pages/guards, app shell/sidebar/settings/change-password, dev proxy docs.
   - Validation: frontend build/tests where practical, manual auth/session QA.
   - Exit condition: browser user can authenticate and navigate session shell.

6. Task console and Markdown
   - Audit IDs: G16, G17, Q3, Q5.
   - Expected changes: transcript, composer, cancel/resume/edit UI, events/progress panel, SSE client reconnect/backfill, sanitized Markdown component and code copy buttons.
   - Validation: markdown sanitization tests, frontend build, manual task/refresh/reconnect/cancel/resume QA.
   - Exit condition: usable browser console displays live work and final answers safely.

7. Packaging and final audit
   - Audit IDs: all remaining G/Q/V/N items.
   - Expected changes: static serving, prod asset checks, CORS/security headers, README/config/docs, full validation sweep.
   - Validation: `cargo fmt`, relevant `cargo clippy`, cargo checks/tests, manual QA checklist.
   - Exit condition: Completion Audit is fully verified or remaining exceptions are explicitly user-accepted.

## Validation Contract

- Formatting: `cargo fmt` and `cargo fmt --check`.
- Contracts: `cargo check -p oxide-agent-web-contracts` and `cargo test -p oxide-agent-web-contracts`.
- Backend focused: `cargo check -p oxide-agent-transport-web --no-default-features` and focused `cargo test -p oxide-agent-transport-web --no-default-features ...` commands for touched modules.
- Backend profile regression as scope grows: `cargo check --workspace --no-default-features --features profile-lite` and `cargo clippy --workspace --no-default-features --features profile-lite`.
- Frontend: Trunk/Leptos build command once `oxide-agent-web-ui` exists, plus markdown/component tests where practical.
- Grep guards:
  - `rg --files -g '*.ts' -g '*.tsx' -g '*.js' -g '*.jsx'` should show no handwritten app stack outside generated build artifacts or explicitly documented tooling exceptions.
  - `rg -n 'user_id' crates/oxide-agent-transport-web/src` must show no browser-trusted `user_id` request fields after auth migration.
  - `rg -n 'CorsLayer::permissive|/debug/event_logs|route\\(\"/sessions' crates/oxide-agent-transport-web/src` must show no production route/CORS violations after API migration.
- Done when: every Completion Audit item is `verified` with current evidence.

## Decisions

- 2026-05-27: Use `docs/goals/2026-05-27-web-console-v1.md` following the existing `docs/goals/<date>-<slug>.md` convention.
- 2026-05-27: Start implementation with Milestone 0 contracts and additive `/api/v1` plumbing. This keeps the first slice small and gives backend/frontend one Rust DTO source before auth, persistence, and UI work.
- 2026-05-27: Keep legacy unversioned endpoints temporarily during Milestone 0 only so existing tests continue to validate current behavior while `/api/v1` is introduced. The goal still requires deleting those routes and migrating tests in the task/session API checkpoint.
- 2026-05-28: Remove legacy unversioned route registrations before frontend work. Socket e2e helpers may still contain old URL strings until their migration, but production `build_router` now exposes only `/api/v1` browser routes plus `/health`.
- 2026-05-27: Gate task-execution HTTP tests behind `profile-lite` while the no-default crate has no compiled LLM provider modules. The test still uses `ScriptedLlmProvider`; `profile-lite` only supplies route capabilities for the agent runner.
- 2026-05-28: Socket e2e helpers should create real test auth sessions directly through the test `WebUiStore`, then exercise the browser API through cookie and CSRF headers. This preserves end-to-end HTTP coverage for browser routes without reintroducing trusted `user_id` request bodies.
- 2026-05-28: Keep delegation socket e2e behind explicit `delegation_e2e` because `profile-lite` intentionally excludes `tool-delegation`; running the delegation check requires `--features profile-lite,socket_e2e,delegation_e2e`.
- 2026-05-28: Keep compression socket e2e behind explicit `compression_e2e` because `profile-lite` intentionally excludes `tool-compression`; running compression checks requires `--features profile-lite,socket_e2e,compression_e2e`.
- 2026-05-28: Normalize seeded compaction socket history for strict OpenCode Go tool-history rules, and assert durable/event/progress invariants instead of assuming old inline tool-result messages always survive hot-memory compaction.
- 2026-05-28: Start frontend as a separate `oxide-agent-web-ui` Leptos CSR crate with Trunk metadata. Keep it browser/API-client based and dependency-free from core/runtime/transport crates except for shared `oxide-agent-web-contracts`.
- 2026-05-28: For the first frontend checkpoint, validate with `cargo check`/`cargo clippy` on both host and `wasm32-unknown-unknown`. A real `trunk build` remains pending because `trunk` is not installed in the current environment.
- 2026-05-28: Use `comrak` with default features disabled plus `ammonia` for the frontend Markdown security boundary. This keeps Markdown rendering in Rust while avoiding comrak's heavier CLI/syntax-highlighting dependency surface in the frontend crate.
- 2026-05-28: Serve frontend assets from `OXIDE_WEB_STATIC_DIR`, with `crates/oxide-agent-web-ui/dist` auto-detected when present. Production and `OXIDE_WEB_REQUIRE_STATIC_ASSETS=true` require `index.html`; dev/test can run API-only when no dist directory exists.

## Progress Log

- 2026-05-27 21:49 +03: Read `docs/prd/PRD_web.md`, repo `AGENTS.md`, README overview, existing web transport code/tests, storage references, and existing goal docs. Created active Codex goal and this repo-local goal contract. Next checkpoint: implement `oxide-agent-web-contracts` and initial `/api/v1` backend plumbing.
- 2026-05-27 21:55 +03: Completed the first Milestone 0 slice. Added `oxide-agent-web-contracts` to the workspace with shared Rust DTOs for auth, public config, API errors, sessions, tasks, progress snapshots, and persisted task events. Wired `oxide-agent-transport-web` to the contracts crate and added `GET /api/v1/public-config` using `PublicConfigResponse` plus env parsing tests for `OXIDE_WEB_REGISTRATION_ENABLED`. Verified `cargo fmt`, `cargo fmt --check`, `cargo check -p oxide-agent-web-contracts`, `cargo test -p oxide-agent-web-contracts`, `cargo check -p oxide-agent-transport-web --no-default-features`, and `cargo test -p oxide-agent-transport-web --no-default-features server::tests`. Next checkpoint: continue Milestone 0 by adding initial typed `/api/v1` response/error plumbing around session/task routes or start the `WebUiStore` persistence interface, depending on the smallest compile-safe slice.
- 2026-05-27 21:59 +03: Started the durable persistence checkpoint with a compile-safe interface slice. Added `crates/oxide-agent-transport-web/src/persistence/` with `WebUiStore`, persisted auth/user records, and `InMemoryWebUiStore` for tests/dev-only use. Store tests cover user/auth session round-trip, login index, user-scoped sessions/tasks/events, event replay pagination, session delete cleanup, and startup reconciliation from queued/running to interrupted. Verified `cargo fmt`, `cargo fmt --check`, `cargo test -p oxide-agent-transport-web --no-default-features persistence`, `cargo check -p oxide-agent-transport-web --no-default-features`, and `cargo test -p oxide-agent-web-contracts`. Next checkpoint: add R2-backed store skeleton/key layout or wire the in-memory store into `/api/v1` session APIs behind auth scaffolding.
- 2026-05-27 22:00 +03: Wired `WebUiStore` into `AppState` with `new_with_web_store` for tests/future production configuration and default in-memory store for current test setup. `GET /api/v1/public-config` now computes `bootstrap_required` from registration config, configured bootstrap token, and store `users_count()`. Verified `cargo fmt`, `cargo fmt --check`, `cargo test -p oxide-agent-transport-web --no-default-features server::tests`, and `cargo check -p oxide-agent-transport-web --no-default-features`. Next checkpoint: implement auth utilities/store-backed bootstrap/register endpoints or R2 key layout.
- 2026-05-27 22:02 +03: Ran focused lint gate for touched crates: `cargo clippy -p oxide-agent-web-contracts -p oxide-agent-transport-web --no-default-features` passed. Next checkpoint remains auth utilities/store-backed bootstrap/register endpoints or R2 key layout.
- 2026-05-27 22:07 +03: Implemented the first auth foundation slice. Added Argon2id dependency, `crates/oxide-agent-transport-web/src/auth.rs`, login/password validation, Argon2id hash/verify helpers, random positive web user ID allocation, register/bootstrap user creation backed by `WebUiStore`, and `/api/v1/auth/register` plus `/api/v1/auth/bootstrap` handlers returning shared `AuthUserResponse`/`ErrorEnvelope` shapes. Verified `cargo fmt --check`, `cargo test -p oxide-agent-transport-web --no-default-features auth`, `cargo test -p oxide-agent-transport-web --no-default-features server::tests`, `cargo check -p oxide-agent-transport-web --no-default-features`, and `cargo clippy -p oxide-agent-web-contracts -p oxide-agent-transport-web --no-default-features`. Next checkpoint: implement opaque browser auth sessions, login/logout/me, cookie flags, and CSRF middleware.
- 2026-05-27 22:13 +03: Implemented opaque browser auth sessions. Added direct `sha2`/`base64` dependencies aligned with existing workspace versions, session token generation, SHA-256 token hashing, server-side `WebAuthSessionRecord` creation, current-user lookup, logout revocation, HttpOnly/SameSite cookie construction with production/configurable Secure flag, `X-CSRF-Token` validation for logout, and `/api/v1/auth/login`, `/api/v1/me`, `/api/v1/auth/logout` handlers. Verified `cargo fmt --check`, `cargo test -p oxide-agent-transport-web --no-default-features auth`, `cargo test -p oxide-agent-transport-web --no-default-features server::tests`, `cargo check -p oxide-agent-transport-web --no-default-features`, and `cargo clippy -p oxide-agent-web-contracts -p oxide-agent-transport-web --no-default-features`. Next checkpoint: change-password endpoint and broader authenticated `/api/v1/sessions` ownership path, or R2-backed `WebUiStore` key layout.
- 2026-05-27 22:16 +03: Implemented `POST /api/v1/auth/change-password`. Added `WebUiStore::revoke_auth_sessions_for_user_except`, in-memory implementation, auth helper that verifies current password and CSRF token, writes a new Argon2id hash, and revokes other browser sessions while keeping the current session. Verified `cargo fmt --check`, `cargo test -p oxide-agent-transport-web --no-default-features auth`, `cargo test -p oxide-agent-transport-web --no-default-features persistence`, `cargo test -p oxide-agent-transport-web --no-default-features server::tests`, `cargo check -p oxide-agent-transport-web --no-default-features`, and `cargo clippy -p oxide-agent-web-contracts -p oxide-agent-transport-web --no-default-features`. Next checkpoint: authenticated `/api/v1/sessions` ownership path or R2-backed `WebUiStore` key layout.
- 2026-05-27 22:21 +03: Implemented authenticated `/api/v1/sessions` ownership path. Added `WebSessionManager::create_session_with_id`, current-user and CSRF helpers in the web server, list/create/get/rename/delete handlers backed by `WebUiStore`, session title validation, and focused server test proving user-scoped listing, foreign-session 404, missing-CSRF rejection, and `context_key = web-session-{session_id}` / `agent_flow_id = main`. Verified `cargo fmt --check`, `cargo test -p oxide-agent-transport-web --no-default-features server::tests`, `cargo test -p oxide-agent-transport-web --no-default-features auth`, `cargo test -p oxide-agent-transport-web --no-default-features persistence`, `cargo check -p oxide-agent-transport-web --no-default-features`, and `cargo clippy -p oxide-agent-web-contracts -p oxide-agent-transport-web --no-default-features`. Next checkpoint: task API hardening or R2-backed `WebUiStore` key layout.
- 2026-05-27 22:39 +03: Implemented the first task API hardening slice. Added authenticated `/api/v1/sessions/{session_id}/tasks` list/create and `/api/v1/sessions/{session_id}/tasks/{task_id}` detail routes, task input validation, prompt-preview auto-title, one-active-task enforcement, runtime-session restoration from persisted session metadata, shared legacy/API task spawning, persisted final answer/progress updates, `WaitingForUserInput` paused-state persistence, and `WaitingForApproval` to YOLO failed mapping. Verified `cargo fmt`, `cargo fmt --check`, `cargo test -p oxide-agent-web-contracts`, `cargo test -p oxide-agent-transport-web --no-default-features server::tests`, `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite server::tests::api_tasks_are_auth_scoped_and_persist_final_response`, `cargo test -p oxide-agent-transport-web --no-default-features auth`, `cargo test -p oxide-agent-transport-web --no-default-features persistence`, `cargo check -p oxide-agent-transport-web --no-default-features`, `cargo clippy -p oxide-agent-web-contracts -p oxide-agent-transport-web --no-default-features`, and `cargo clippy -p oxide-agent-transport-web --no-default-features --features profile-lite`. Next checkpoint: implement cancel/resume/edit-last-input APIs or R2-backed `WebUiStore` key layout.
- 2026-05-27 22:54 +03: Completed the next task lifecycle API slice. Added `WebSessionManager::register_existing_task` for same-task resume, introduced shared execute-vs-resume task run requests, and added authenticated `/api/v1` edit-input, resume, and cancel handlers. Edit is limited to the latest terminal task and sets `input_edited_at`; resume requires `waiting_for_user_input` and continues the same task id through `AgentExecutor::resume_after_user_input`; cancel persists `cancelled`, clears the session active task, and is idempotent for already-cancelled tasks. `task_waiting_for_user_input` conflicts now include `details.task_id` for resume. Verified `cargo fmt`, `cargo fmt --check`, `cargo test -p oxide-agent-web-contracts`, `cargo test -p oxide-agent-transport-web --no-default-features server::tests`, `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite server::tests`, `cargo test -p oxide-agent-transport-web --no-default-features auth`, `cargo test -p oxide-agent-transport-web --no-default-features persistence`, `cargo check -p oxide-agent-transport-web --no-default-features`, `cargo clippy -p oxide-agent-web-contracts -p oxide-agent-transport-web --no-default-features`, and `cargo clippy -p oxide-agent-transport-web --no-default-features --features profile-lite`. Next checkpoint: event persistence/replayable SSE or R2-backed `WebUiStore` key layout.
- 2026-05-27 23:06 +03: Implemented the persisted browser event replay slice. Added `BrowserEventScope` and persisted event construction in `web_transport`, mapping `AgentEvent` into browser-facing `PersistedTaskEvent` records with sequence numbers, stable kind, timestamp, summary, payload previews, truncation flags, and file-content redaction metadata. Event collection now appends events through `WebUiStore` and updates task `last_event_seq`. Added authenticated `GET /api/v1/sessions/{session_id}/tasks/{task_id}/events?after_seq=&limit=` with ownership checks and bounded limits. Verified `cargo fmt`, `cargo fmt --check`, `cargo test -p oxide-agent-web-contracts`, `cargo test -p oxide-agent-transport-web --no-default-features web_transport`, `cargo test -p oxide-agent-transport-web --no-default-features server::tests`, `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite server::tests`, `cargo test -p oxide-agent-transport-web --no-default-features auth`, `cargo test -p oxide-agent-transport-web --no-default-features persistence`, `cargo check -p oxide-agent-transport-web --no-default-features`, `cargo clippy -p oxide-agent-web-contracts -p oxide-agent-transport-web --no-default-features`, and `cargo clippy -p oxide-agent-transport-web --no-default-features --features profile-lite`. Next checkpoint: replace legacy SSE with `/api/v1` replayable SSE or implement R2-backed `WebUiStore`.
- 2026-05-27 23:19 +03: Implemented the `/api/v1` replayable SSE slice and live event persistence. `collect_events` can now fan out each `PersistedTaskEvent` to a live persister, so task events are appended to `WebUiStore` during execution instead of only after collector completion. Added authenticated `GET /api/v1/sessions/{session_id}/tasks/{task_id}/stream` with initial snapshot, persisted event replay using SSE `id = seq`, `after_seq`/`Last-Event-ID` resume, status/progress/keepalive events, and ownership checks. Verified `cargo fmt`, `cargo fmt --check`, `cargo check -p oxide-agent-web-contracts`, `cargo test -p oxide-agent-web-contracts`, `cargo test -p oxide-agent-transport-web --no-default-features web_transport`, `cargo test -p oxide-agent-transport-web --no-default-features server::tests`, `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite server::tests`, `cargo test -p oxide-agent-transport-web --no-default-features auth`, `cargo test -p oxide-agent-transport-web --no-default-features persistence`, `cargo check -p oxide-agent-transport-web --no-default-features`, `cargo clippy -p oxide-agent-web-contracts -p oxide-agent-transport-web --no-default-features`, and `cargo clippy -p oxide-agent-transport-web --no-default-features --features profile-lite`. Next checkpoint: R2-backed `WebUiStore` key layout or live progress mid-run persistence/frontend client.
- 2026-05-27 23:29 +03: Implemented the first R2-backed `WebUiStore` slice. Added public prefix list/delete helpers to `R2Storage`, `storage-s3-r2` feature plumbing for `oxide-agent-transport-web`, `R2WebUiStore` over the existing core R2 storage, PRD key layout helpers, schema-versioned `WebTaskEventChunkRecord`, chunked append/replay/delete behavior for task events, and R2/object-store startup reconciliation logic. Hermetic object-store tests verify key layout, users/auth sessions, duplicate login conflicts, auth-session revocation, session/task round-trip, chunked event replay pagination, session delete cleanup for task/event objects, and queued/running task interruption across users. Verified `cargo fmt`, `cargo fmt --check`, `cargo test -p oxide-agent-web-contracts`, `cargo test -p oxide-agent-transport-web --no-default-features web_transport`, `cargo test -p oxide-agent-transport-web --no-default-features server::tests`, `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite server::tests`, `cargo test -p oxide-agent-transport-web --no-default-features auth`, `cargo test -p oxide-agent-transport-web --no-default-features persistence`, `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite persistence`, `cargo check -p oxide-agent-transport-web --no-default-features`, `cargo check -p oxide-agent-transport-web --no-default-features --features profile-lite`, `cargo clippy -p oxide-agent-web-contracts -p oxide-agent-transport-web --no-default-features`, `cargo clippy -p oxide-agent-transport-web --no-default-features --features profile-lite`, and `git diff --check`. Next checkpoint: wire production startup/config to choose `R2WebUiStore` and fail fast outside explicit dev/test in-memory mode, or implement live mid-run progress persistence.
- 2026-05-27 23:37 +03: Wired durable web startup selection and guardrails. Added `WebSessionManager::new_with_storage` and changed web session memory checkpoint persistence to use the configured manager storage provider. Added `WebStoreKind`, `WebStartupError`, `AppState::new_with_r2_web_store`, `build_r2_backed_app_state`, startup store validation, startup unfinished-task reconciliation from `serve`, and env guard behavior: in-memory store is rejected when `RUN_MODE=prod|production`, `OXIDE_WEB_ENABLED=true`, or `OXIDE_WEB_REQUIRE_DURABLE_STORAGE=true`, unless `OXIDE_WEB_ALLOW_IN_MEMORY_STORE=true` is explicitly set for dev/test. Focused tests cover in-memory startup rejection/allowance and R2 app-state builder failure on missing R2 config. Verified `cargo fmt`, `cargo fmt --check`, `cargo test -p oxide-agent-web-contracts`, `cargo test -p oxide-agent-transport-web --no-default-features web_transport`, `cargo test -p oxide-agent-transport-web --no-default-features server::tests`, `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite server::tests`, `cargo test -p oxide-agent-transport-web --no-default-features session::tests`, `cargo test -p oxide-agent-transport-web --no-default-features auth`, `cargo test -p oxide-agent-transport-web --no-default-features persistence`, `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite persistence`, `cargo check -p oxide-agent-transport-web --no-default-features`, `cargo check -p oxide-agent-transport-web --no-default-features --features profile-lite`, `cargo clippy -p oxide-agent-web-contracts -p oxide-agent-transport-web --no-default-features`, `cargo clippy -p oxide-agent-transport-web --no-default-features --features profile-lite`, and `git diff --check`. Next checkpoint: live mid-run progress persistence or removal/migration of legacy unversioned endpoints.
- 2026-05-27 23:52 +03: Implemented live mid-run progress persistence. `collect_events` now fans out live `ProgressState` snapshots, `spawn_event_collector` starts a live progress persister for durable web tasks, and `ProgressSnapshot` now carries todos plus retry/provider-failover fields alongside existing thought/token/compaction/error fields. Added authenticated durable `GET /api/v1/sessions/{session_id}/tasks/{task_id}/progress`. Focused tests cover contract serialization, live collector progress fan-out, running-task progress persistence into `WebTaskRecord.last_progress`, and auth-scoped persisted progress reads. Verified `cargo fmt`, `cargo fmt --check`, `cargo test -p oxide-agent-web-contracts`, `cargo test -p oxide-agent-transport-web --no-default-features web_transport`, `cargo test -p oxide-agent-transport-web --no-default-features server::tests`, `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite server::tests`, `cargo test -p oxide-agent-transport-web --no-default-features persistence`, `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite persistence`, `cargo check -p oxide-agent-transport-web --no-default-features`, `cargo check -p oxide-agent-transport-web --no-default-features --features profile-lite`, `cargo clippy -p oxide-agent-web-contracts -p oxide-agent-transport-web --no-default-features`, `cargo clippy -p oxide-agent-transport-web --no-default-features --features profile-lite`, and `git diff --check`. Next checkpoint: remove/migrate legacy unversioned endpoints and e2e tests, or begin Rust frontend shell once backend API surface is sufficiently stable.
- 2026-05-28 00:03 +03: Removed legacy unversioned web routes from the production router. Deleted the old `/sessions`, `/sessions/...`, and `/debug/event_logs` handler bodies and updated crate/server docs away from the E2E-only route list. Added a focused router test proving `/api/v1/public-config` remains available while old paths return 404, and verified `rg -n 'route\("/sessions|"/sessions|/debug/event_logs' crates/oxide-agent-transport-web/src` returns no matches. Verified `cargo fmt`, `cargo fmt --check`, `cargo test -p oxide-agent-web-contracts`, `cargo test -p oxide-agent-transport-web --no-default-features server::tests`, `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite server::tests`, `cargo test -p oxide-agent-transport-web --no-default-features --test e2e`, `cargo check -p oxide-agent-transport-web --no-default-features`, `cargo check -p oxide-agent-transport-web --no-default-features --features profile-lite`, `cargo clippy -p oxide-agent-web-contracts -p oxide-agent-transport-web --no-default-features`, `cargo clippy -p oxide-agent-transport-web --no-default-features --features profile-lite`, and `git diff --check`. Next checkpoint: migrate socket e2e helpers/tests to authenticated `/api/v1` calls or start the Rust frontend shell.
- 2026-05-28 09:02 +03: Migrated the socket e2e helper path to authenticated browser API calls. Test server setup now registers the `AppState` for helpers, seeds/logs in test users through `WebUiStore`, stores per-session auth material, sends cookie/CSRF headers, and uses `/api/v1` for session creation, task creation, progress, event replay, deletion, and SSE streaming. Helper compatibility layers map persisted browser events/progress/timeline data back to the existing assertions while tests are incrementally migrated. Also updated socket test setup to register the scripted provider under the compiled `opencode_go` route for `profile-lite,socket_e2e`. Verified `cargo fmt`, `cargo fmt --check`, `cargo test -p oxide-agent-web-contracts`, `cargo test -p oxide-agent-transport-web --no-default-features --test e2e`, `cargo test -p oxide-agent-transport-web --no-default-features server::tests`, `cargo check -p oxide-agent-transport-web --no-default-features`, `cargo check -p oxide-agent-transport-web --no-default-features --features profile-lite`, `cargo clippy -p oxide-agent-web-contracts -p oxide-agent-transport-web --no-default-features`, `cargo clippy -p oxide-agent-transport-web --no-default-features --features profile-lite`, `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite,socket_e2e --test e2e sse_tests::e2e_sse_stream -- --nocapture`, `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite,socket_e2e --test e2e session_tests::e2e_web_followup_while_running_returns_session_busy -- --nocapture`, and `git diff --check`. Next checkpoint: finish semantic migration/revalidation of the remaining socket e2e cases, then start the Rust frontend shell.
- 2026-05-28 09:14 +03: Stabilized more socket e2e after the `/api/v1` migration. Aligned custom socket test providers and compaction setup with the compiled `opencode-go/deepseek-v4-flash` route required by typed tool runtime, converted scripted final answers to structured-output JSON where that route is active, made reminder assertions use the authenticated web user id and `web-session-{session_id}` context, and added explicit `delegation_e2e` feature gating for sub-agent tests. Verified `cargo test -p oxide-agent-transport-web --no-default-features --test e2e`, focused socket reminder/runtime-context/session-busy/SSE checks, focused delegation with `profile-lite,socket_e2e,delegation_e2e`, `cargo check -p oxide-agent-transport-web --no-default-features`, `cargo check -p oxide-agent-transport-web --no-default-features --features profile-lite`, `cargo fmt --check`, `cargo clippy -p oxide-agent-web-contracts -p oxide-agent-transport-web --no-default-features`, `cargo clippy -p oxide-agent-transport-web --no-default-features --features profile-lite`, and `git diff --check`. Full `profile-lite,socket_e2e` now reaches 13 passed / 11 compaction assertion failures / 4 ignored; next checkpoint is migrating the remaining compaction regression assertions to the new authenticated web-session scope and current compaction event/progress model.
- 2026-05-28 09:30 +03: Completed the socket e2e compaction migration. Compaction seeded histories now include assistant tool-call messages for strict OpenCode Go tool-history validity, route setup uses the compiled `opencode-go/deepseek-v4-flash` profile route, compression/delegation scenarios are behind explicit feature gates, and assertions now allow old tool-result messages to be compacted away while still checking event/progress/hot-memory marker invariants. Verified `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite,socket_e2e --test e2e -- --nocapture` with 22 passed / 0 failed / 6 ignored, `cargo test -p oxide-agent-transport-web --no-default-features --test e2e` with 6 passed / 0 failed / 22 ignored, focused delegation with `profile-lite,socket_e2e,delegation_e2e`, focused compression checks with `profile-lite,socket_e2e,compression_e2e`, `cargo test -p oxide-agent-web-contracts`, `cargo check -p oxide-agent-transport-web --no-default-features`, `cargo check -p oxide-agent-transport-web --no-default-features --features profile-lite`, `cargo fmt --check`, both relevant clippy commands, and `git diff --check`. Next checkpoint: start the Rust frontend shell and auth UI.
- 2026-05-28 09:45 +03: Committed the completed backend/e2e checkpoint as `fa0f8732 feat(web): harden browser task backend`, then started Milestone 4 frontend implementation. Added `crates/oxide-agent-web-ui` to the workspace with Leptos CSR dependencies, Trunk metadata, `index.html`, responsive CSS, route parsing, cookie/CSRF-aware Rust API client over `/api/v1`, auth/register/bootstrap/settings pages, session sidebar, and first task console/events panel. Verified `cargo check -p oxide-agent-web-ui`, `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`, `cargo clippy -p oxide-agent-web-ui`, `cargo clippy -p oxide-agent-web-ui --target wasm32-unknown-unknown`, `cargo fmt --check`, and the no-TypeScript grep guard. `trunk build` remains pending because `command -v trunk` found no installed binary. Next checkpoint: add frontend SSE/progress behavior, safe Markdown rendering, and backend static asset serving.
- 2026-05-28 09:50 +03: Added the first safe Markdown rendering slice. `MarkdownContent` is now the only raw-HTML insertion boundary in the frontend, backed by `comrak` Markdown parsing and `ammonia` sanitization; task user input and final answers render through it. Tests cover script/event-attribute removal, unsafe protocol removal, table/code preservation, and image stripping. Verified `cargo test -p oxide-agent-web-ui markdown`, `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`, `cargo clippy -p oxide-agent-web-ui`, `cargo clippy -p oxide-agent-web-ui --target wasm32-unknown-unknown`, `cargo fmt --check`, `cargo tree -p oxide-agent-web-ui -e features | rg "comrak|syntect|onig|clap|ammonia"`, and `git diff --check`. Next checkpoint: frontend SSE/progress reconnect behavior or backend static asset serving.
- 2026-05-28 09:59 +03: Implemented backend static asset serving and security headers. `AppState` now carries `WebAssetsConfig` from `OXIDE_WEB_STATIC_DIR` / default `crates/oxide-agent-web-ui/dist`; production/static-required startup fails without `index.html`. Router fallback serves SPA browser routes, static files, WASM MIME type, cache headers, and keeps missing `/api/*` as 404. Added security headers middleware and replaced production CORS behavior with non-permissive `CorsLayer::new()`, keeping explicit `Any` CORS only for non-prod dev. Verified focused static tests, `cargo test -p oxide-agent-transport-web --no-default-features server::tests`, `cargo test -p oxide-agent-transport-web --no-default-features --features profile-lite server::tests`, `cargo check -p oxide-agent-transport-web --no-default-features`, both relevant clippy commands, `cargo fmt --check`, `rg -n 'CorsLayer::permissive|/debug/event_logs|route\("/sessions' crates/oxide-agent-transport-web/src`, and `git diff --check`. Next checkpoint: frontend SSE/progress reconnect behavior and Trunk build evidence.

## Risks and Blockers

- This is a broad PRD spanning backend, storage, auth, frontend, and security. Mitigation: implement checkpoint by checkpoint and update this doc only with evidence-backed progress.
- Frontend dependency downloads require network and registry writes outside the sandbox. Mitigation: Leptos dependencies were added/fetched with explicit escalation; future frontend crates should still be added with `cargo add` and validated on `wasm32-unknown-unknown`.
- `trunk` is not installed in the current environment, so the Trunk build evidence is still missing. Mitigation: keep `cargo check`/wasm clippy green now and run or install Trunk during the packaging/static-serving checkpoint.
- R2-backed integration tests may need fakes or careful abstraction to stay hermetic. Mitigation: define `WebUiStore` and keep R2 tests focused on key/serialization behavior unless real credentials are explicitly configured.
- Backend/socket coverage is now green for the current checkpoint, including the full `profile-lite,socket_e2e` suite and explicit delegation/compression feature checks. The largest remaining PRD gaps are frontend SSE/progress behavior, Trunk build evidence, complete Markdown UX details, and manual QA.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
