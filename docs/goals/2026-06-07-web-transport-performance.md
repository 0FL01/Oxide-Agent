# Goal: Web Transport Performance

Date started: 2026-06-07
Status: active
Codex goal: `/goal Implement docs/goals/2026-06-07-web-transport-performance.md until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update this document after each meaningful verification, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.`
Source spec: User request to focus web transport, run RECON, and plan how to accelerate frontend chat/page loading, including aggressive options.
Goal doc owner: Codex
Last updated: 2026-06-07 00:30

## Objective

Make web console chat/page loading measurably faster by removing request waterfalls, reducing overfetch, avoiding eager full-history event loads, and replacing DB-polling SSE with lower-latency live delivery where justified.

Done when the selected checkpoints have current before/after evidence, every required Completion Audit item is verified, and web transport/frontend behavior remains compatible with existing session/task/SSE contracts or explicitly migrated contracts.

## Scope

In scope:
- `crates/oxide-agent-web-ui/src/` frontend loading flow, session sidebar, task workspace, SSE client, event/activity rendering, and markdown rendering.
- `crates/oxide-agent-transport-web/src/server/` session/task/SSE routes and route-local state needed for performance.
- `crates/oxide-agent-transport-web/src/persistence/` SQLx query shape and event persistence batching.
- `crates/oxide-agent-web-contracts/src/` only for explicit lightweight DTOs, pagination, or cursor contract changes.
- Focused tests, benchmarks, tracing/logging, and this goal document.

Out of scope:
- Telegram transport, core/runtime LLM execution semantics, provider integrations, sandbox backends, manager control plane, and wiki memory behavior.
- New databases, queues, sharding, HA, external caches, or heavy observability platforms.
- Direct Google Gemini provider work.
- Visual redesign unless required to expose pagination/load-more states.

## Missing Inputs

- Real production-size baseline data is not currently captured.
  - Impact: percentage improvements must start as estimates until measured with representative sessions/events.
  - Low-risk assumption or fallback: create local seeded scenarios for 20 sessions, 100+ sessions, 20 tasks, 1000+ events, and one active SSE task.
  - User/external action needed: provide real browser/network traces only if local scenarios do not reproduce perceived slowness.

## Repository Context

- Web backend route slices live under `crates/oxide-agent-transport-web/src/server/`; task/session APIs are in `task_routes.rs` and `session_routes.rs`, SSE is in `sse.rs`.
- Web SQLx persistence is in `crates/oxide-agent-transport-web/src/persistence/sqlx.rs`.
- Web contracts are shared through `crates/oxide-agent-web-contracts/src/tasks.rs` and session/config modules.
- Frontend route loading and task workspace logic live in `crates/oxide-agent-web-ui/src/tasks/workspace.rs`.
- Frontend SSE client is `crates/oxide-agent-web-ui/src/sse.rs`; activity rendering is `crates/oxide-agent-web-ui/src/tasks/activity.rs`; session sidebar is `crates/oxide-agent-web-ui/src/sessions.rs`; markdown rendering is `crates/oxide-agent-web-ui/src/markdown.rs`.
- Validation should prefer focused web checks before broader workspace checks: `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`, `cargo check -p oxide-agent-transport-web`, `cargo check -p oxide-agent-web-contracts`, and web E2E tests where behavior changes touch user flows.

## Completion Audit

- G1: Baseline and measurement harness exists
  - Source: User asked for expected acceleration percentages; RECON showed multiple bottlenecks that need measured before/after evidence.
  - Acceptance: There is a reproducible way to capture request waterfall, transferred bytes, key endpoint latency, event count, and SSE DB/query cadence for representative scenarios.
  - Evidence required: documented commands or scripts, captured baseline table, and at least one run covering ordinary session load plus long-event session load.
  - Status: in_progress
  - Evidence collected: Added debug-level `oxide_agent_transport_web::web_perf` backend measurements for response latency, list sizes, task-event page sizes, and SSE DB polling cadence; added `Server-Timing: app;dur=...` response header for browser waterfall captures. Baseline scenario runs still pending.

- G2: Initial page/session load waterfall is removed
  - Source: RECON found sequential requests in `crates/oxide-agent-web-ui/src/tasks/workspace.rs:400` and settings/profile sequence at `crates/oxide-agent-web-ui/src/tasks/workspace.rs:135`.
  - Acceptance: independent requests for settings, profiles, session detail, and task list run concurrently where possible; redundant `get_task` after `list_tasks` is removed or justified by a contract change.
  - Evidence required: code diff, browser/network waterfall before/after, and `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`.
  - Status: pending
  - Evidence collected:

- G3: List endpoints avoid large overfetch
  - Source: RECON found full task markdown in `TaskSummary` at `crates/oxide-agent-web-contracts/src/tasks.rs:120`, full task list loading at `crates/oxide-agent-transport-web/src/persistence/sqlx.rs:714`, and full session columns at `crates/oxide-agent-transport-web/src/persistence/sqlx.rs:561`.
  - Acceptance: task/session list endpoints return lightweight fields needed for list rendering, support bounded pagination or explicit limits, and keep full markdown/details on detail/events paths.
  - Evidence required: contract/server/frontend diff, payload-size before/after table, SQL query review, and focused cargo checks for web UI, web transport, and web contracts.
  - Status: pending
  - Evidence collected:

- G4: Task events are loaded incrementally instead of eager full-history load
  - Source: RECON found `load_all_task_events` at `crates/oxide-agent-web-ui/src/tasks/workspace.rs:35` and merge/sort cost at `crates/oxide-agent-web-ui/src/tasks/workspace.rs:60`.
  - Acceptance: opening a session loads only the latest bounded event window required for immediate UI; older events are loaded by cursor on demand; event dedup/merge avoids per-event full sort.
  - Evidence required: long-event session before/after load time, transferred bytes, frontend scripting time, and cargo/web E2E validation for activity/SSE behavior.
  - Status: pending
  - Evidence collected:

- G5: Live SSE delivery does not poll Postgres every second for active streams
  - Source: RECON found SSE loop DB replay/reload/sleep in `crates/oxide-agent-transport-web/src/server/sse.rs:91`, `sse_replay_batch` at `crates/oxide-agent-transport-web/src/server/sse.rs:161`, and task reload at `crates/oxide-agent-transport-web/src/server/sse.rs:192`.
  - Acceptance: initial connect can replay from durable DB, but live events/progress/status are delivered through an in-process channel/event bus or equivalent low-latency mechanism without periodic DB polling for every connected client.
  - Evidence required: implementation diff, SSE latency before/after, SQL query cadence before/after, reconnect/replay validation, and web transport tests.
  - Status: pending
  - Evidence collected:

- G6: Frontend render hot paths are bounded
  - Source: RECON found per-event sort in `crates/oxide-agent-web-ui/src/sse.rs:611`, activity full-vector filter at `crates/oxide-agent-web-ui/src/tasks/activity.rs:123`, sidebar full-list clone at `crates/oxide-agent-web-ui/src/sessions.rs:44`, task grouping clones at `crates/oxide-agent-web-ui/src/tasks/versions.rs:10`, and markdown render cost at `crates/oxide-agent-web-ui/src/markdown.rs:11`.
  - Acceptance: event append, activity filtering, sidebar filtering, task grouping, and markdown rendering avoid unnecessary full-list/full-markdown recomputation on common reactive ticks.
  - Evidence required: code diff, frontend profiler or timing evidence on large scenarios, and wasm cargo check.
  - Status: pending
  - Evidence collected:

- Q1: Keep architecture simple and local
  - Source: Repository guardrail against over-engineering and target load up to 5 RPS.
  - Acceptance: no new services, queues, databases, caches, frameworks, or broad abstraction layers unless a checkpoint proves a simpler local change cannot meet the target.
  - Evidence required: dependency diff review and architecture decision notes.
  - Status: in_progress
  - Evidence collected: Checkpoint 1 uses existing `tracing`, existing router middleware, and standard HTTP `Server-Timing`; no new services, crates, queues, databases, or external observability.

- Q2: Preserve web behavior and compatibility during migrations
  - Source: Existing web console contracts and user-facing chat/task flows must continue working.
  - Acceptance: session creation, task creation, attachment upload, task detail, activity drawer, SSE reconnect/replay, waiting-for-user-input, and terminal task summary refresh continue working.
  - Evidence required: focused tests/E2E/manual validation for changed flows.
  - Status: pending
  - Evidence collected:

- V1: Web frontend compiles
  - Source: Leptos CSR crate validation convention.
  - Acceptance: `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown` succeeds after frontend changes.
  - Evidence required: command output summary in Progress Log.
  - Status: pending
  - Evidence collected:

- V2: Web backend/contracts compile
  - Source: Web transport and contracts route changes require Rust validation.
  - Acceptance: `cargo check -p oxide-agent-transport-web` and `cargo check -p oxide-agent-web-contracts` succeed after backend/contract changes.
  - Evidence required: command output summary in Progress Log.
  - Status: pending
  - Evidence collected:

- N1: No unrelated transport/runtime changes
  - Source: Scope boundary from user request to focus web transport.
  - Must preserve: Telegram transport, core/runtime/provider behavior, sandbox backends, manager control plane, wiki memory, and direct Gemini absence.
  - Evidence required: `git diff --name-only` and final diff audit.
  - Status: pending
  - Evidence collected:

## Implementation Plan

1. Baseline measurement checkpoint
   - Audit IDs: G1, Q1.
   - Expected changes: add the smallest reusable measurement path: documented browser DevTools steps, optional local seed/test fixture, and/or lightweight tracing logs for web endpoint latency and payload bytes.
   - Validation: run baseline scenarios and record results in this document.
   - Exit condition: before numbers exist for ordinary session load and long-event session load.

2. Low-risk frontend waterfall and event-merge checkpoint
   - Audit IDs: G2, G6, V1, Q2, N1.
   - Expected changes: parallelize independent requests, remove redundant latest-task `get_task` if current contracts allow, cache settings/profiles briefly, and replace per-event sort with batched/monotonic merge.
   - Validation: wasm cargo check, browser waterfall before/after, active task SSE smoke.
   - Exit condition: ordinary session opening is measurably faster without API contract changes.

3. Lightweight list payload checkpoint
   - Audit IDs: G3, V1, V2, Q1, Q2, N1.
   - Expected changes: introduce list-specific task/session DTOs or query flags, narrow SQL selects, add explicit `limit` defaults, and update frontend consumers.
   - Validation: payload-size before/after table, focused cargo checks, route tests if existing coverage supports them.
   - Exit condition: list payloads are bounded and detail markdown remains available on detail paths.

4. Lazy event history checkpoint
   - Audit IDs: G4, G6, V1, V2, Q2, N1.
   - Expected changes: load latest bounded events on session entry, add older-event cursor path if needed, backfill on activity drawer demand, and index/dedup events by `(task_id, seq)`.
   - Validation: long-event session load before/after, activity drawer load-more smoke, SSE reconnect replay smoke.
   - Exit condition: long chats open without eager full-history download.

5. True live SSE checkpoint
   - Audit IDs: G5, V2, Q1, Q2, N1.
   - Expected changes: add in-process task event/progress/status broadcast, publish from task execution/persistence path, keep DB replay for reconnects, and stop periodic live DB polling.
   - Validation: SSE latency and SQL cadence before/after, reconnect replay test, terminal/waiting-for-input behavior check.
   - Exit condition: active streams deliver events promptly with near-zero steady-state DB polling per client.

6. Frontend render nuclear checkpoint
   - Audit IDs: G6, V1, Q1, Q2, N1.
   - Expected changes: memoize markdown rendering, memo/index activity event filtering, debounce or optimize sidebar search, and optionally virtualize long lists if measured DOM cost remains high.
   - Validation: frontend profiler/timing evidence on large sessions and wasm cargo check.
   - Exit condition: reactive hot paths are bounded enough for representative large sessions.

## Validation Contract

- Static checks:
  - `git diff --check`
  - `git diff --name-only`
- Frontend checks:
  - `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`
  - `env -u NO_COLOR trunk build --release` from `crates/oxide-agent-web-ui/` when asset/build behavior changes.
- Backend/contracts checks:
  - `cargo check -p oxide-agent-transport-web`
  - `cargo check -p oxide-agent-web-contracts`
- Tests:
  - `cargo test -p oxide-agent-transport-web` for server/route behavior when backend changes are covered.
  - `cargo test -p oxide-agent-web-contracts` when contract serialization changes.
  - `cargo test -p oxide-agent-transport-web --test e2e` or the focused available web E2E command when chat flow behavior changes.
- Runtime/manual verification:
  - Browser Network waterfall for `/app/session/:id`.
  - Transferred bytes for `/api/v1/sessions`, `/api/v1/sessions/:id/tasks`, task events, and SSE stream.
  - Frontend profiler scripting/rendering time for long-event activity view.
  - SSE latency from generated event to browser receipt and SQL query cadence per active stream.
- Done when: every Completion Audit item is verified with current evidence or explicitly dropped by user.

## Baseline Measurement Procedure

Backend measurement is debug-only and local to the web transport. Start the web console with:

```bash
RUST_LOG=oxide_agent_transport_web::web_perf=debug,tower_http=warn cargo run -p oxide-agent-web-console --no-default-features --features profile-web-embedded-opencode-local
```

Capture these two scenarios with browser DevTools Network open, cache disabled, and log preserved:

1. Ordinary session load: open `/app/session/:session_id` for a recent session with a normal task count.
2. Long-event session load: open `/app/session/:session_id` for a task with hundreds/thousands of persisted events, then open the activity drawer.

Record for each scenario:

| Scenario | Endpoint or stream | Requests | `Server-Timing app` ms | Transferred bytes | Event/list count | SSE DB queries/sec | Notes |
|---|---:|---:|---:|---:|---:|---:|---|
| Ordinary session load | `/api/v1/sessions` | pending | pending | pending | pending | n/a | pending baseline run |
| Ordinary session load | `/api/v1/sessions/:id/tasks` | pending | pending | pending | pending | n/a | pending baseline run |
| Ordinary session load | latest task events | pending | pending | pending | pending | n/a | pending baseline run |
| Long-event session load | latest task events | pending | pending | pending | pending | n/a | pending baseline run |
| Active task stream | `/stream` | pending | pending | pending | pending | pending | pending baseline run |

Use backend logs with `target=oxide_agent_transport_web::web_perf` to fill list/event counts and SSE DB query cadence. Use browser Network to fill request waterfall and transferred bytes; `Server-Timing` is visible per response where the browser exposes timing details.

## Decisions

- 2026-06-07: Store this as `docs/goals/2026-06-07-web-transport-performance.md` because the repo already uses `docs/goals/` for durable goal contracts.
- 2026-06-07: Start with measurement and low-risk waterfall/payload fixes before the more invasive SSE event bus. RECON indicates these provide large wins with less architectural risk.
- 2026-06-07: Treat true push SSE as the main aggressive backend option, but only after baseline and simpler list/event-load fixes establish remaining need and expected payoff.
- 2026-06-07: Do not add external queues/caches/services for the target scale; prefer local in-process channels and bounded API payloads.
- 2026-06-07: Implement Checkpoint 1 as debug tracing plus standard `Server-Timing` instead of adding a metrics stack or synthetic benchmark crate; this keeps measurement reusable without new dependencies.

## Progress Log

- 2026-06-07 00:00: Goal document created from web transport performance RECON.
  - Changed: Added this goal contract with bottleneck-derived Completion Audit, checkpoint plan, validation contract, and first-step guidance.
  - Evidence: RECON identified sequential workspace load at `crates/oxide-agent-web-ui/src/tasks/workspace.rs:400`, full task list payload at `crates/oxide-agent-web-contracts/src/tasks.rs:120`, eager events load at `crates/oxide-agent-web-ui/src/tasks/workspace.rs:35`, polling SSE backend at `crates/oxide-agent-transport-web/src/server/sse.rs:91`, per-event sort at `crates/oxide-agent-web-ui/src/sse.rs:611`, and activity full-vector filtering at `crates/oxide-agent-web-ui/src/tasks/activity.rs:123`.
  - Commands: `git status --short`; `git log --oneline -5`; `git diff -- AGENTS.md`; docs convention reviewed under `docs/goals/`.
  - Audit IDs updated: none; this is the planning checkpoint.
  - Next: Checkpoint 1 — baseline measurement for ordinary and long-event session loads.

- 2026-06-07 00:30: Checkpoint 1 measurement harness started.
  - Changed: Added debug web performance logs for HTTP responses, session/task list sizes, task event page sizes, and SSE DB query cadence; added `Server-Timing` response header; documented the baseline capture procedure and table.
  - Evidence: Code paths touched are limited to `crates/oxide-agent-transport-web/src/server/router.rs`, `session_routes.rs`, `task_routes.rs`, and `sse.rs`; no new dependencies or services added.
  - Commands: `cargo fmt`; `cargo check -p oxide-agent-transport-web`; `git diff --check`.
  - Audit IDs updated: G1 in progress, Q1 in progress.
  - Next: Run focused backend validation, commit harness, then capture ordinary/long-event baseline numbers before optimization checkpoint 2.

## Risks and Blockers

- Missing baseline could make percentage claims misleading.
  - Impact: optimizations may target the wrong bottleneck or overstate gains.
  - Evidence: current RECON has code-level bottlenecks but no measured browser/backend timing table yet.
  - Mitigation or requested decision: start with Checkpoint 1 and only commit implementation checkpoints with before/after evidence.
  - Audit IDs affected: G1-G6.

- Contract changes for lightweight task summaries can ripple through frontend and tests.
  - Impact: changing `TaskSummary` may break existing consumers if done too broadly.
  - Evidence: current `TaskSummary` and `TaskDetail` overlap heavily in `crates/oxide-agent-web-contracts/src/tasks.rs:120` and `crates/oxide-agent-web-contracts/src/tasks.rs:145`.
  - Mitigation or requested decision: prefer an additive list DTO or explicit summary mode before removing fields from existing detail contracts.
  - Audit IDs affected: G3, Q2, V1, V2.

- True push SSE needs careful reconnect semantics.
  - Impact: losing DB replay/reconnect behavior would regress active task UX.
  - Evidence: current SSE combines replay, progress, status, keepalive, and terminal close handling in `crates/oxide-agent-transport-web/src/server/sse.rs:91`.
  - Mitigation or requested decision: keep DB replay on connect and only replace steady-state live polling after tests/smoke checks cover reconnect and terminal states.
  - Audit IDs affected: G5, Q2, V2.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
