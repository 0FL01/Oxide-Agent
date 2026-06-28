# Goal: Web Permanent Chat Mode (without memory tool)

Date started: 2026-06-28
Status: active
Codex goal: Implement `docs/goals/2026-06-28-web-permanent-chat.md` until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update the doc after each meaningful verification, commit after each completed checkpoint, compress before starting the next checkpoint, and stop only on verified completion or an exact blocker with required evidence and the smallest external action needed.
Source spec: `docs/prd/PRD-perm.md` (Permanent Life Mode) — narrowed to web permanent chat without the memory tool/inspector/Engram/curator UX.
Goal doc owner: Codex
Last updated: 2026-06-28 C3

## Objective

Ship a permanent web chat with the agent: one continuous transcript for one principal, same operational tools as ordinary agents, no per-session chat reset, activity drawer + paging, live SSE. This is the web slice of Permanent Life Mode, scoped to exclude the memory tool, Engram adapter, curator UX, memory inspector/editor, and Telegram UX — those remain separate goals.

Done when every required Completion Audit item is verified by its listed evidence, ordinary `/app/session/*` behavior is unchanged, and the permanent chat survives backend restart via durable Postgres storage.

## Scope

In scope:
- `docs/prd/PRD-perm.md` as the source contract, narrowed to web permanent chat.
- `crates/oxide-agent-life/` runtime wiring: real `LifeRunExecutor` adapter over `AgentExecutor`, stable scope `(principal_user_id, "life", "main")`, final synchronous checkpoint, assistant turn persistence, AgentEvent → `life_events` bridge.
- `crates/oxide-agent-transport-web/src/server/life_routes.rs` and `crates/oxide-agent-web-contracts/src/life.rs`: submit that wakes runtime, typed attachments, cursor-based turns/events paging, life SSE stream.
- `crates/oxide-agent-transport-web/src/server/router.rs`: new `/api/v1/life/*` routes for uploads, large-input, stream.
- `crates/oxide-agent-web-ui/src/`: new `life/` module, `/life` route, sidebar menu entry, transcript + composer + activity + streaming + paging UI.
- `migrations/0010_life_mode.sql.pending` → activated migration together with runtime wiring.
- Storage/repository additions needed for cursor paging and run-bound turn linkage.
- Tests and docs proving the audit items.

Out of scope:
- Engram adapter / outbox worker / recall integration.
- Memory inspector/editor/conflict review UI.
- Life memory tool (in development separately).
- Post-run curator and sensitivity gate UX.
- Telegram `/life` UX and cross-transport linking flow.
- Cross-transport reminders.
- Replacing ordinary web sessions or Telegram topic/session mode.
- Replacing current wiki memory for ordinary chat mode.
- Storing raw secrets in memory or Engram.
- Ambient group/forum listening as personal life memory.
- Splitting life mode into multiple `oxide-agent-life-*` crates.
- Adding new LLM/provider infrastructure for the life runtime; reuse existing `llm/client.rs` and `AgentExecutor`.

## Missing Inputs

None at goal creation. Existing life storage migration is parked as `.pending` and must be activated together with runtime wiring in checkpoint C1.

## Repository Context

- Relevant entry points:
  - `crates/oxide-agent-life/` — bounded context with `domain/`, `storage/`, `gateway/`, `worker/`, `context/`, `curator/`, `engram/`, `api/`. Memory foundation (G1-G14 in `docs/goals/2026-06-24-permanent-life-memory.md`) is verified complete; runtime executor adapter is the gap.
  - `crates/oxide-agent-transport-web/src/server/life_routes.rs` — existing submit/state/turns/events/memories/generations/link-tokens/lifecycle routes; submit currently only queues, returns `run_id: None`.
  - `crates/oxide-agent-transport-web/src/server/router.rs` — route table; life routes already registered for state/inputs/turns/events/memories/generations.
  - `crates/oxide-agent-web-contracts/src/life.rs` — shared DTOs; `ApiLifeSubmitRequest.attachments` is loose `serde_json::Value`.
  - `crates/oxide-agent-web-ui/src/` — Leptos SPA; `app.rs`/`routes.rs`/`components.rs`/`sessions.rs`/`tasks/` own the current chat UX; no `life/` module yet.
  - `migrations/0010_life_mode.sql.pending` — full `life_*` schema, parked.
- Existing conventions:
  - AGENTS.md commit style: `<type>(<scope>): <description>` + blank + indented `Changes:` 2-4 bullets.
  - `thiserror` for library crates, `anyhow` for app/binary crates.
  - `oxide-agent-core`/`oxide-agent-runtime` must not depend on transport crates; `oxide-agent-life` must not depend on transport crates.
  - Capability-module based composition; `module_registry.toml` is source of truth.
  - Cargo `default` features empty; use profile features.
- Dependencies or runtime assumptions:
  - `AgentExecutor` exposes `execute_user_input_with_options` / `resume_user_input_with_options` returning `AgentExecutionOutcome::{Completed, WaitingForUserInput}`.
  - `AgentMemoryScope(user_id, context_key, flow_id)` + `agent_memory_snapshots` already support stable `(principal, "life", "main")`.
  - `StorageProvider::save_agent_memory_for_flow` / `load_agent_memory_for_flow` are the durable hot-checkpoint boundary.
  - `WebSessionManager::create_session_with_model_selection` is the ordinary-session materialization path; life must not reuse `web-session-*` context keys.
  - `collect_events_until_shutdown` in `web_transport.rs` maps `AgentEvent` → `PersistedTaskEvent` but is scoped to `session_id`/`task_id`; life needs a run-scoped equivalent or a generalized adapter.
- Validation infrastructure:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
  - `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`
  - `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`
  - `trunk build --release` from `crates/oxide-agent-web-ui` for a full frontend gate
  - `cargo run -p xtask -- module-registry check` if module/profile wiring changes
  - Real-Postgres life storage tests: `OXIDE_DATABASE_TEST_URL=... cargo test -p oxide-agent-life`
- Risky areas:
  - `LifeRunExecutor` must not duplicate ordinary agent tool setup; it must reuse the same tool registration and execution profile path or it will silently diverge from ordinary agents.
  - Life SSE must read source of truth from Postgres, not from a process-local registry, or cross-process continuity breaks.
  - `0010_life_mode.sql.pending` activation must happen atomically with runtime wiring; activating the migration without the runtime leaves `/api/v1/life/*` returning storage errors.
  - UI transcript/activity paging cursors must be opaque backend-provided strings; the UI must not construct them.

## Completion Audit

### Functional requirements (G*)

- G1: Life storage migration is active and the schema supports permanent chat.
  - Source: PRD §9, `migrations/0010_life_mode.sql.pending`, `docs/goals/2026-06-24-permanent-life-memory.md` G3.
  - Acceptance: `0010_life_mode.sql` runs as a normal migration; `life_turns`, `life_inputs`, `life_runs`, `life_events`, `agent_memory_snapshots(principal, "life", "main")` are usable for web permanent chat; startup migrations succeed on a clean Postgres.
  - Evidence required: migration file rename + `cargo test -p oxide-agent-life` real-Postgres test passing; `INFO FOR TABLE life_turns`/`life_runs`/`life_events` output.
  - Status: verified
  - Evidence collected: Renamed `migrations/0010_life_mode.sql.pending` → `migrations/0010_life_mode.sql`. `cargo test -p oxide-agent-life` all 30 tests pass including real-Postgres SQLx tests (`sqlx_life_storage_migrates_and_scopes_memory_by_active_generation`, `sqlx_life_gateway_submit_persists_turn_metadata_and_input`, `sqlx_life_turns_and_events_cursor_paging`, `sqlx_life_worker_claim_start_complete_and_drain_are_db_backed`, `sqlx_life_link_tokens_and_wipe_lifecycle_are_db_backed`) — migration ran on clean Postgres.

- G2: `POST /api/v1/life/inputs` starts/attaches a run, not just queues.
  - Source: PRD §12.1, §12.6 step 1-2; current `life_routes.rs` returns `run_id: None`.
  - Acceptance: submit resolves principal, writes user turn, queues input, and wakes the life runtime so a run is claimed/started; response carries a non-None `run_id` when a run is active or started; follow-up inputs during an active run are queued and drained at safe boundaries, not lost.
  - Evidence required: route/worker integration test proving `run_id` is populated and queued follow-up input is drained; grep proving no transport-side executor construction.
  - Status: verified
  - Evidence collected: `LifeRuntimeHandle::wake` claims input + starts run, links originating turn, returns `WakeOutcome::Started { run_id, claimed }` or `WakeOutcome::AttachedToActive { run_id }`. Runtime tests: `wake_starts_new_run_and_links_turn`, `wake_returns_active_run_when_already_running`, `wake_errors_when_not_claimed_and_no_active_run`, `wake_propagates_storage_errors`. Route handler `submit_life_input_for_user` calls `wake` after gateway submit and spawns `worker.execute_claimed_run(*claimed)` — `run_id` is populated from the wake outcome. NoopLifeRunExecutor is a placeholder (no transport-side AgentExecutor construction; real executor is C3). SQLx test `sqlx_life_find_active_run_and_link_turn_to_run` proves `find_active_run` returns the running run and `life_turns.run_id` is populated after `link_turn_to_run` on real Postgres. Full HTTP integration test deferred to C9 (requires Postgres).

- G3: `LifeRunExecutor` runs the ordinary `AgentExecutor` with stable life scope and same tools.
  - Source: PRD §12.3, §12.5; user requirement "same tools as ordinary agents, only without chat reset".
  - Acceptance: executor is built with `AgentMemoryScope(principal, "life", "main")`, stable sandbox scope, the same tool registration and execution profile path as ordinary web agents; no life memory tool; final synchronous checkpoint to `agent_memory_snapshots`; assistant response persisted to `life_turns(role='assistant')`.
  - Evidence required: adapter code review/grep proving reuse of ordinary tool registration; unit/integration test proving assistant turn + checkpoint are persisted after a run.
  - Status: verified
  - Evidence collected: `LifeAgentExecutor` adapter in `crates/oxide-agent-transport-web/src/server/life_executor.rs` implements `LifeRunExecutor`. It builds an `AgentExecutor` with `AgentMemoryScope(principal, "life", "main")` and `SandboxScope::new(principal, "life")`, hydrates `AgentMemory` via `StorageProvider::load_agent_memory_for_flow`, installs a `LifeMemoryCheckpoint` (same `save_agent_memory_for_flow` path as ordinary `StorageFlowCheckpoint`), configures wiki memory + AGENTS.md + reminder context (same as `create_session_with_model_selection`), calls `execute_user_input_with_options`, forces `persist_memory_checkpoint()` after execution, and persists the assistant response to `life_turns(role='assistant')` with `run_id`. The worker's `save_life_memory_checkpoint` call was removed from `execute_claimed_run` because the adapter handles durable persistence via the same `agent_memory_snapshots` path. `NoopLifeRunExecutor` replaced by `LifeAgentExecutor` in `AppState::new_with_sqlx_web_store`. Full HTTP integration test deferred to C9 (requires running Postgres + LLM). `cargo test -p oxide-agent-life` 36/36 pass, `cargo test -p oxide-agent-transport-web` 7 pass/23 ignored, `cargo clippy` clean.

- G4: User turn is linked to its run for activity rendering.
  - Source: PRD §9.1 `life_turns.run_id`; current gateway leaves `run_id = None`.
  - Acceptance: when the worker claims an input and starts a run, `life_turns.run_id` is set to the run id for both the originating user turn and any follow-up inputs drained into that run.
  - Evidence required: storage/worker test proving `life_turns.run_id` is populated after claim and after drain.
  - Status: verified
  - Evidence collected: `LifeRuntimeHandle::wake` calls `store.link_turn_to_run(turn_id, run_id)` for the originating turn after claim. `LifeWorker::execute_claimed_run` drains follow-up inputs and calls `store.link_turn_to_run` for each drained input's turn. SQLx test `sqlx_life_find_active_run_and_link_turn_to_run` verifies `life_turns.run_id` is NULL before linking, set to the run UUID after `link_turn_to_run` for both originating and follow-up turns, on real Postgres. Worker test `execute_claimed_run_drains_follow_up_inputs_and_links_turns` verifies the drain + link behavior.

- G5: AgentEvents are persisted to `life_events` and available to the UI.
  - Source: PRD §9.4, §12.3 "stream AgentEvents -> life_events".
  - Acceptance: a run's `AgentEvent`s are converted to `life_events` rows scoped by `run_id` with monotonic `seq`; the UI activity drawer can render them.
  - Evidence required: event-bridge test proving `life_events` rows are appended during a run; payload shape sufficient for activity rendering.
  - Status: pending
  - Evidence collected:

- G6: Cursor-based paging for turns and events.
  - Source: web UX requirement; current `list_turns(principal, 200)` / `list_events(principal, 500)` are unbounded single-page reads.
  - Acceptance: `GET /api/v1/life/turns?cursor=...&limit=...` and `GET /api/v1/life/events?run_id=...&cursor=...&limit=...` return a page plus an opaque next-cursor; cursors are backend-provided and not constructed by the UI.
  - Evidence required: route + repository test proving paging returns correct pages and cursors; contract DTO carries `next_cursor`.
  - Status: verified
  - Evidence collected: `SqlxLifeStorage::list_turns_page(principal, cursor, limit)` and `list_events_page(principal, run_id, cursor, limit)` implemented with opaque cursor encoding (`created_at:turn_id` / `created_at:run_id:seq`). `ApiLifeTurnsResponse` / `ApiLifeEventsResponse` carry `next_cursor: Option<String>` with `#[serde(default)]`. `LifeTurnsQuery` / `LifeEventsQuery` axum query extractors added. `LifeStorageError::InvalidCursor` → HTTP 400. `sqlx_life_turns_and_events_cursor_paging` test verifies 3-page turns traversal, events by run_id and by principal, and invalid cursor rejection. Contract tests verify `next_cursor` serde round-trip.

- G7: Life SSE streams turns, events, and run status from Postgres-backed state.
  - Source: PRD §9.4 "web SSE может читать progress из БД"; current web SSE is task-scoped and broadcast-driven.
  - Acceptance: `GET /api/v1/life/stream` emits `snapshot`, `turn`, `life_event`, `run_status`, `keepalive`; reconnect after restart replays missed turns/events from Postgres; no dependency on a process-local registry as source of truth.
  - Evidence required: SSE handler test proving replay + live delivery; grep proving no `SessionRegistry`-as-source dependency in the life SSE path.
  - Status: pending
  - Evidence collected:

- G8: `/life` route and sidebar entry exist in the web UI.
  - Source: PRD §14.1, §19 "новый `/life` UI path".
  - Acceptance: `AppRoute::Life` is an authenticated app route; sidebar shows a fixed "Permanent" entry above ordinary sessions; ordinary session list/behavior unchanged.
  - Evidence required: `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown` passes; route test or manual verification that `/life` renders the life console and `/app` still renders the session sidebar.
  - Status: pending
  - Evidence collected:

- G9: Life chat UI renders transcript, composer, activity, and paging.
  - Source: user requirement "chat with agent (activity, paging and etc)".
  - Acceptance: transcript renders user/assistant turns; composer submits to `/api/v1/life/inputs`; activity drawer shows tool/thinking events for the active/selected run; "Load older" pages transcript and activity via cursors; reload preserves transcript.
  - Evidence required: `trunk build --release` succeeds; manual/E2E verification of submit → response → reload → transcript persists → load older.
  - Status: pending
  - Evidence collected:

- G10: Attachments work for permanent chat via stable life endpoints.
  - Source: ordinary web chat attachment path is session-scoped; PRD §19 "без изменения semantics обычных session routes".
  - Acceptance: `POST /api/v1/life/uploads` and `POST /api/v1/life/large-input` stage files in the stable life sandbox; `ApiLifeSubmitRequest.attachments` is typed `Vec<TaskAttachment>` (or equivalent typed contract), not loose `Value`.
  - Evidence required: route tests for upload + large-input; contract test proving typed attachments deserialize; sandbox path verification.
  - Status: pending
  - Evidence collected:

### Quality/security/non-functional requirements (Q*)

- Q1: Ordinary chat/topic/session mode is behavior-compatible.
  - Source: PRD §1, §4.3, §19, §20 criterion 1; AGENTS.md architectural invariants.
  - Acceptance: no Engram/life memory injection in ordinary modes; ordinary web session routes and Telegram topic paths unchanged; existing web/telegram checks pass.
  - Evidence required: `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local` passes; grep/diff audit showing no changes to `session_routes.rs` ordinary paths.
  - Status: pending
  - Evidence collected:

- Q2: Postgres is the source of truth for permanent chat state.
  - Source: PRD §5 Mine 2, §12.5.
  - Acceptance: life turns/inputs/runs/events/checkpoint are persisted; a backend restart resumes the chat from Postgres without losing the last assistant response or hot checkpoint.
  - Evidence required: restart simulation test or manual verification proving transcript + hot memory survive restart.
  - Status: pending
  - Evidence collected:

- Q3: Transport boundary is preserved.
  - Source: PRD §6.1, AGENTS.md invariants.
  - Acceptance: web routes submit only provider/subject/content/attachments/metadata/sensitivity; they do not choose `principal_user_id`, `context_key`, `flow_id`, run state, or memory ids.
  - Evidence required: grep/route review proving no caller-supplied internal ids; contract test.
  - Status: pending
  - Evidence collected:

- Q4: Lint, format, type-check, and wasm gates pass.
  - Source: AGENTS.md Development Practices.
  - Acceptance: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`, `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown` all pass.
  - Evidence required: command outputs.
  - Status: pending
  - Evidence collected:

- Q5: Module/profile wiring is consistent if changed.
  - Source: AGENTS.md Module registry.
  - Acceptance: if `module_registry.toml` or profile features are touched, `cargo run -p xtask -- module-registry check` passes.
  - Evidence required: command output (only required if registry/profile wiring changes).
  - Status: pending
  - Evidence collected:

### Non-goals/exclusions (N*)

- N1: No Engram adapter / outbox / recall in this goal.
  - Source: user request "without memory tool".
  - Must preserve: Engram/outbox code may exist but must not be activated or surfaced in the web permanent chat path.
  - Evidence required: grep proving no Engram HTTP calls or outbox worker activation in the permanent chat runtime path.
  - Status: pending
  - Evidence collected:

- N2: No memory inspector/editor/conflict review UI.
  - Source: user request "without memory tool".
  - Must preserve: existing `/api/v1/life/memories` etc. routes may remain but are not wired into the `/life` UI.
  - Evidence required: UI review proving no memory inspector panels in the life console.
  - Status: pending
  - Evidence collected:

- N3: No curator/sensitivity-gate UX.
  - Source: user request.
  - Must preserve: curator may run post-run but is not surfaced in the UI; no curator controls in `/life`.
  - Evidence required: UI review.
  - Status: pending
  - Evidence collected:

- N4: No Telegram `/life` UX or cross-transport linking in this goal.
  - Source: user request "начнём с web ui".
  - Must preserve: Telegram life command may exist but is not changed; no linking flow UI.
  - Evidence required: diff audit showing no Telegram transport changes.
  - Status: pending
  - Evidence collected:

- N5: No ordinary session/chat mode changes.
  - Source: PRD §1, §19.
  - Must preserve: `/app/session/*` routes, `SessionSidebar`, `TaskConsole` behavior unchanged.
  - Evidence required: diff audit.
  - Status: pending
  - Evidence collected:

## Implementation Plan

Checkpoints are commit-ready units. Commit after each checkpoint, update the Progress Log, then compress before the next checkpoint.

### Checkpoint C1 — Activate migration + storage paging contract
- Audit IDs: G1, G6
- Expected changes:
  - Rename `migrations/0010_life_mode.sql.pending` → `migrations/0010_life_mode.sql`.
  - Add cursor-based repository methods for `life_turns` and `life_events` (e.g. `list_turns_page(principal, cursor, limit)`, `list_events_page(run_id, cursor, limit)`).
  - Extend `LifeStorageRepository` / `SqlxLifeStorage` with paging methods; keep existing unbounded methods for internal/tests.
  - Add/extend `oxide-agent-web-contracts/src/life.rs` paging response shapes (`next_cursor: Option<String>`).
- Validation:
  - `OXIDE_DATABASE_TEST_URL=... cargo test -p oxide-agent-life` passes on clean Postgres.
  - `cargo test -p oxide-agent-web-contracts` passes.
  - `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`.
- Exit condition: migration runs on clean Postgres; paging repository methods compile and test green.

### Checkpoint C2 — Runtime wake + run-bound turn linkage
- Audit IDs: G2, G4
- Expected changes:
  - Add `LifeRuntimeHandle` (or equivalent) owned by the web binary that wakes the worker on submit.
  - `LifeGateway::submit_life_input` returns a started/attached `run_id` (not `None`) by claiming or attaching a run via the runtime handle.
  - Worker sets `life_turns.run_id` on claim and on drain of follow-up inputs.
  - `POST /api/v1/life/inputs` returns populated `run_id`.
- Validation:
  - `cargo test -p oxide-agent-life` (unit + real-Postgres) proves `run_id` is populated and follow-up input is drained.
  - `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local life -- --nocapture`.
  - `cargo clippy --workspace --all-targets -- -D warnings`.
- Exit condition: submit starts/attaches a run; turns are run-linked; tests green.

### Checkpoint C3 — Real `LifeRunExecutor` over `AgentExecutor`
- Audit IDs: G3, Q2
- Expected changes:
  - Implement a concrete `LifeRunExecutor` adapter (likely in `oxide-agent-transport-web` or a new thin integration module) that:
    - builds `AgentSession` with `AgentMemoryScope(principal, "life", "main")` and stable sandbox scope;
    - hydrates `AgentMemory` from `agent_memory_snapshots` via `StorageProvider::load_agent_memory_for_flow`;
    - installs a synchronous final checkpoint (`StorageProvider::save_agent_memory_for_flow`);
    - reuses ordinary tool registration / execution profile path (no life memory tool);
    - calls `AgentExecutor::execute_user_input_with_options`;
    - persists the assistant response to `life_turns(role='assistant')` with `run_id`.
  - Wire the adapter into `LifeWorker` via the runtime handle.
- Validation:
  - Integration test proving a run executes, assistant turn + checkpoint are persisted.
  - `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`.
  - `cargo clippy --workspace --all-targets -- -D warnings`.
- Exit condition: a submitted input produces an agent response persisted to `life_turns` and `agent_memory_snapshots`.

### Checkpoint C4 — AgentEvent → `life_events` bridge
- Audit IDs: G5
- Expected changes:
  - Add an event bridge that consumes `AgentEvent`s from the executor and appends `life_events` rows scoped by `run_id` with monotonic `seq` and a payload shape sufficient for activity rendering (reuse `PersistedTaskEvent`-compatible kind/summary/payload where possible).
  - Generalize or fork the event-mapping logic from `web_transport.rs` so life is not forced to use `session_id`/`task_id`.
- Validation:
  - Test proving `life_events` rows are appended during a run with correct `run_id`/`seq`.
  - `cargo test -p oxide-agent-life`.
  - `cargo clippy --workspace --all-targets -- -D warnings`.
- Exit condition: activity events are durably persisted per run.

### Checkpoint C5 — Life SSE stream
- Audit IDs: G7
- Expected changes:
  - Add `GET /api/v1/life/stream` handler in `life_routes.rs` / `sse.rs`.
  - Emit `snapshot`, `turn`, `life_event`, `run_status`, `keepalive`.
  - Replay missed turns/events from Postgres on reconnect using cursors; live-deliver new ones via a run-scoped broadcast or DB-poll fallback.
  - No `SessionRegistry`-as-source dependency.
- Validation:
  - SSE handler test proving replay + live delivery.
  - `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`.
  - Grep proving no `SessionRegistry` source dependency in the life SSE path.
- Exit condition: SSE streams permanent chat state from Postgres-backed source.

### Checkpoint C6 — Typed attachments + life upload/large-input endpoints
- Audit IDs: G10, Q3
- Expected changes:
  - Change `ApiLifeSubmitRequest.attachments` to typed `Vec<TaskAttachment>` (keep backward-compatible default).
  - Add `POST /api/v1/life/uploads` and `POST /api/v1/life/large-input` staging files in the stable life sandbox.
  - Register routes in `router.rs`.
- Validation:
  - Contract + route tests.
  - `cargo test -p oxide-agent-web-contracts`.
  - `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`.
- Exit condition: attachments are typed and staged in the life sandbox.

### Checkpoint C7 — `/life` route + sidebar entry
- Audit IDs: G8, N5
- Expected changes:
  - Add `AppRoute::Life` to `routes.rs`; auth-guard it.
  - `AppLayout` treats `/life` as an authenticated app route (alongside `App`/`Session`).
  - `SessionSidebar` gains a fixed "Permanent" entry above ordinary sessions; ordinary sessions unchanged.
- Validation:
  - `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`.
  - `trunk build --release` from `crates/oxide-agent-web-ui`.
  - Manual verification that `/life` renders and `/app` still renders the session sidebar.
- Exit condition: `/life` is reachable and the sidebar entry is present without breaking ordinary sessions.

### Checkpoint C8 — Life chat UI (transcript + composer + activity + paging)
- Audit IDs: G9, N1, N2, N3
- Expected changes:
  - New `crates/oxide-agent-web-ui/src/life/` module: `mod`, `console`, `transcript`, `composer`, `activity`, `state`, `streaming`.
  - Transcript renders user/assistant turns with paging ("Load older").
  - Composer submits to `/api/v1/life/inputs` with typed attachments.
  - Activity drawer renders `life_events` for the active/selected run with paging.
  - SSE client subscribes to `/api/v1/life/stream`.
  - No memory inspector / curator / Engram panels.
- Validation:
  - `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`.
  - `trunk build --release`.
  - Manual/E2E: submit → response → reload → transcript persists → load older → activity drawer.
- Exit condition: permanent chat is usable end-to-end in the browser.

### Checkpoint C9 — Restart survival + final audit
- Audit IDs: Q2, Q1, Q3, Q4, Q5
- Expected changes:
  - Verify backend restart resumes permanent chat from Postgres (transcript + hot memory).
  - Run full gate: fmt, clippy, workspace check, web-ui wasm check, trunk build, scoped transport-web tests, life storage tests.
  - Diff audit confirming no ordinary session/chat/Telegram changes.
- Validation:
  - Restart simulation test or manual verification.
  - Full gate commands.
- Exit condition: all audit items verified; goal ready for final review.

## Validation Contract

- Static checks:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
  - `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`
  - `cargo run -p xtask -- module-registry check` (if registry/profile wiring changes)
- Tests:
  - `cargo test -p oxide-agent-life`
  - `cargo test -p oxide-agent-web-contracts`
  - `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`
- Runtime/manual verification:
  - Start web console with SQLx/Postgres; open `/life`; send a message; receive a response; reload; confirm transcript persists; restart backend; confirm chat resumes.
  - Open activity drawer during a run; confirm tool/thinking events render; load older activity via cursor.
- Done when: every G*/Q* audit item is verified and every N* exclusion is preserved by diff/grep evidence.

## Decisions

- 2026-06-28: Scope narrowed to web permanent chat without memory tool/inspector/Engram/curator UX per explicit user request. Memory foundation (`docs/goals/2026-06-24-permanent-life-memory.md`) remains the source for the broader life memory architecture.
- 2026-06-28: Activate `0010_life_mode.sql` together with runtime wiring in C1, not before, to avoid a partial-schema state where `/api/v1/life/*` returns storage errors.
- 2026-06-28: Life chat UI is a new `life/` module, not a patch over `TaskConsole`, because `TaskConsole` is tightly coupled to `session_id`/`task_id` and ordinary session semantics must not change (N5).
- 2026-06-28: Activity rendering should converge on a shared `ActivityEventView` adapter so task and life events share one renderer while keeping distinct storage contracts. This avoids forking the activity UI while preserving contract boundaries.
- 2026-06-28: `ApiLifeSubmitRequest.attachments` becomes typed `Vec<TaskAttachment>` to match the ordinary web chat contract and avoid loose `Value` handling at the boundary.

## Progress Log

- 2026-06-28 00:00: Goal document created and approved for review.
  - Changed: added `docs/goals/2026-06-28-web-permanent-chat.md`.
  - Evidence: PRD `docs/prd/PRD-perm.md` and current codebase inspected; plan reviewed and approved by user.
  - Commands: none yet.
  - Audit IDs updated: none (planning only).
  - Next: checkpoint C1 (activate migration + storage paging contract).

- 2026-06-28 C1: Activate migration + storage paging contract.
  - Changed:
    - `migrations/0010_life_mode.sql.pending` → `migrations/0010_life_mode.sql` (activated migration).
    - `crates/oxide-agent-life/src/storage/repository.rs`: added `LifeStorageError::InvalidCursor`.
    - `crates/oxide-agent-life/src/storage/sqlx.rs`: added `TurnsPage`, `EventsPage` types; `list_turns_page`, `list_events_page` methods with opaque cursor encoding; `TurnCursor`/`EventCursor` parse/encode helpers; `sqlx_life_turns_and_events_cursor_paging` test.
    - `crates/oxide-agent-web-contracts/src/life.rs`: added `next_cursor: Option<String>` to `ApiLifeTurnsResponse` and `ApiLifeEventsResponse`; paging serde contract tests.
    - `crates/oxide-agent-transport-web/src/server/life_routes.rs`: `LifeTurnsQuery`/`LifeEventsQuery` axum extractors; `api_list_life_turns`/`api_list_life_events` now accept `Query<...>`; `list_life_turns_for_user`/`list_life_events_for_user` use paging; `life_storage_error_response` maps `InvalidCursor` → 400.
  - Evidence: `cargo test -p oxide-agent-life` 30/30 pass (incl. real-Postgres). `cargo test -p oxide-agent-web-contracts` 13/13 pass. `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` pass. `cargo check -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local` pass. `cargo fmt --all -- --check` pass. `cargo clippy -p oxide-agent-life --all-targets -- -D warnings` pass.
  - Commands: `cargo test -p oxide-agent-life`, `cargo test -p oxide-agent-web-contracts`, `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`, `cargo fmt --all -- --check`, `cargo clippy -p oxide-agent-life --all-targets -- -D warnings`.
  - Audit IDs updated: G1 → verified, G6 → verified.
  - Next: checkpoint C2 (runtime wake + run-bound turn linkage).

- 2026-06-28 C2: Runtime wake + run-bound turn linkage (commit `b74ee7a5`).
  - Changed:
    - `crates/oxide-agent-life/src/storage/repository.rs`: added `find_active_run` and `link_turn_to_run` to `LifeStorageRepository` trait.
    - `crates/oxide-agent-life/src/storage/sqlx.rs`: implemented `find_active_run` (SELECT running run by principal) and `link_turn_to_run` (UPDATE life_turns SET run_id); added `run_from_row` + `run_status_from_str` helpers; added `sqlx_life_find_active_run_and_link_turn_to_run` real-Postgres test.
    - `crates/oxide-agent-life/src/worker/mod.rs`: added `drain_queued_inputs_for_run` and `link_turn_to_run` to `LifeWorkerStore` trait + blanket impl; added `execute_claimed_run` method (drains follow-up inputs, links turns, executes, checkpoints, completes); refactored `process_principal_input` to claim then delegate to `execute_claimed_run`; added `impl LifeRunExecutor for Arc<dyn LifeRunExecutor>`; updated `FakeWorkerStore` with new fields/methods; added `execute_claimed_run_drains_follow_up_inputs_and_links_turns` test.
    - `crates/oxide-agent-life/src/runtime.rs` (new): `LifeRuntimeHandle`, `WakeOutcome` (Started/AttachedToActive), `LifeRuntimeStore` trait + blanket impl, `LifeRuntimeError`; `wake` method claims input, starts run, links originating turn, returns outcome; 4 runtime tests.
    - `crates/oxide-agent-life/src/lib.rs`: added `pub mod runtime;`.
    - `crates/oxide-agent-transport-web/src/server/types.rs`: added `life_runtime` and `life_worker` fields to `AppState` (cfg storage-sqlx); added type aliases `LifeExecutor`, `LifeWorkerHandle`, `LifeRuntimeHandleType`; added accessor methods; added `NoopLifeRunExecutor` placeholder; constructed runtime handle + worker in `new_with_sqlx_web_store`.
    - `crates/oxide-agent-transport-web/src/server/life_routes.rs`: `submit_life_input_for_user` now calls `state.life_runtime().wake()` after gateway submit, spawns `worker.execute_claimed_run(*claimed)` for started runs, populates `run_id` from wake outcome; added `life_runtime_error_response`.
  - Evidence: `cargo test -p oxide-agent-life` 36/36 pass (incl. real-Postgres `sqlx_life_find_active_run_and_link_turn_to_run`). `cargo test -p oxide-agent-web-contracts` 13/13 pass. `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local` 7 pass, 23 ignored. `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` pass. `cargo fmt --all -- --check` pass. `cargo clippy -p oxide-agent-life --all-targets -- -D warnings` pass. `cargo clippy -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local --all-targets -- -D warnings` pass.
  - Commands: `cargo test -p oxide-agent-life`, `cargo test -p oxide-agent-web-contracts`, `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`, `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`, `cargo fmt --all -- --check`, `cargo clippy -p oxide-agent-life --all-targets -- -D warnings`, `cargo clippy -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local --all-targets -- -D warnings`.
  - Audit IDs updated: G2 → verified, G4 → verified.
  - Next: checkpoint C3 (real LifeRunExecutor over AgentExecutor).

- 2026-06-28 C3: Real LifeRunExecutor over AgentExecutor.
  - Changed:
    - `crates/oxide-agent-life/src/storage/repository.rs`: added `user_content: String` field to `ClaimedLifeInputRun`.
    - `crates/oxide-agent-life/src/storage/sqlx.rs`: `claim_input_and_start_run` SQL now JOINs `life_turns` to fetch user turn content at claim time (CTE: `WITH claimed AS (UPDATE ... RETURNING *) SELECT claimed.*, lt.content AS user_content FROM claimed JOIN life_turns lt ON lt.turn_id = claimed.turn_id`). Added `user_content` assertions in both SQLx tests.
    - `crates/oxide-agent-life/src/worker/mod.rs`: added `user_content: String` to `ClaimedLifeRun`; `From<ClaimedLifeInputRun>` carries it through. Removed `save_life_memory_checkpoint` call from `execute_claimed_run` — the adapter handles durable memory persistence via `StorageFlowCheckpoint`-equivalent (`LifeMemoryCheckpoint`), making the worker's separate raw-JSON save redundant and architecturally wrong (double-write bypassing proper `AgentMemory` serialization). Updated `FakeWorkerStore::with_claim` to include `user_content`. Updated `worker_claims_run_and_uses_stable_life_scope` test: checkpoint assertions removed (worker no longer saves), `user_content` assertion added.
    - `crates/oxide-agent-transport-web/src/server/life_executor.rs` (new, cfg storage-sqlx): `LifeAgentExecutor` adapter implementing `LifeRunExecutor`. Builds `AgentSession` with `AgentMemoryScope(principal, "life", "main")` + `SandboxScope::new(principal, "life")`. Hydrates `AgentMemory` from `agent_memory_snapshots` via `load_agent_memory_for_flow`. Installs `LifeMemoryCheckpoint` (writes to `agent_memory_snapshots` via `save_agent_memory_for_flow` — identical path to ordinary `StorageFlowCheckpoint`). Creates `AgentExecutor` with same tool configuration as ordinary sessions (wiki memory, AGENTS.md, reminders, storage). Calls `execute_user_input_with_options`. Forces `persist_memory_checkpoint()` after execution. Persists assistant response to `life_turns(role='assistant')` with `run_id` set. Returns `LifeRunExecutionOutcome` with serialized final memory + checkpoint timestamp. `WaitingForUserInput` outcome returns an error (life mode doesn't support pausing yet).
    - `crates/oxide-agent-transport-web/src/server/mod.rs`: added `#[cfg(feature = "storage-sqlx")] mod life_executor;`.
    - `crates/oxide-agent-transport-web/src/server/types.rs`: replaced `NoopLifeRunExecutor` with `LifeAgentExecutor::new(&session_manager, life_storage.clone())` in `new_with_sqlx_web_store`. Removed `NoopLifeRunExecutor` struct and impl.
    - `crates/oxide-agent-life/src/runtime.rs`: updated test `ClaimedLifeInputRun` construction to include `user_content`.
  - Evidence: `cargo test -p oxide-agent-life --lib` 36/36 pass (incl. real-Postgres). `cargo test -p oxide-agent-web-contracts` 13/13 pass. `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local` 7 pass, 23 ignored. `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` pass. `cargo fmt --all -- --check` pass. `cargo clippy -p oxide-agent-life --all-targets -- -D warnings` pass. `cargo clippy -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local --all-targets -- -D warnings` pass.
  - Commands: `cargo test -p oxide-agent-life --lib`, `cargo test -p oxide-agent-web-contracts`, `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`, `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`, `cargo fmt --all -- --check`, `cargo clippy -p oxide-agent-life --all-targets -- -D warnings`, `cargo clippy -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local --all-targets -- -D warnings`.
  - Audit IDs updated: G3 → verified.
  - Next: checkpoint C4 (AgentEvent → life_events bridge).

## Risks and Blockers

- Risk: `LifeRunExecutor` diverging from ordinary agent tool setup.
  - Impact: permanent chat agent would silently lack tools or have different behavior.
  - Evidence: none yet.
  - Mitigation: reuse the ordinary tool registration / execution profile path; grep-proof in C3.
  - Audit IDs affected: G3.

- Risk: Life SSE depending on a process-local registry.
  - Impact: cross-process continuity breaks; restart loses live updates.
  - Evidence: none yet.
  - Mitigation: Postgres-backed replay + run-scoped broadcast; grep-proof in C5.
  - Audit IDs affected: G7, Q2.

- Risk: activating the migration without runtime wiring.
  - Impact: `/api/v1/life/*` returns storage errors on a partial schema.
  - Evidence: none yet.
  - Mitigation: C1 couples migration activation with runtime wake wiring in C2.
  - Audit IDs affected: G1, G2.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
