# Goal: Web First Message Write-Behind Latency

Date started: 2026-06-07
Status: active
Codex goal: `/goal Implement docs/goals/2026-06-07-web-first-message-write-behind.md until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update this document after each meaningful verification, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.`
Source spec: User request after web transport RECON to reduce first-message latency; container crash data loss is acceptable, low latency is the priority, and DB can catch up asynchronously.
Goal doc owner: Codex
Last updated: 2026-06-07 14:12

## Objective

Reduce web transport first-message latency by removing sequential remote-Postgres round trips from the task creation hot path and moving low-priority persistence to local RAM plus background write-behind where safe for personal-use constraints.

Done when the first-message path starts the agent with measured low latency, the selected write-behind behavior is explicitly documented, validation commands pass, and every required Completion Audit item is verified by current evidence.

## Scope

In scope:
- `crates/oxide-agent-transport-web/src/server/task_routes.rs` task creation flow and first-message spawn ordering.
- `crates/oxide-agent-transport-web/src/server/task_executor.rs` task execution boundary logging and any enqueue metadata needed for latency evidence.
- `crates/oxide-agent-transport-web/src/session.rs` runtime-session materialization and session-local cache integration.
- `crates/oxide-agent-transport-web/src/persistence/` SQLx web store hot writes, optional write-behind wrapper/queue, and local cache state.
- `crates/oxide-agent-transport-web/src/server/types.rs` only for AppState cache/queue fields.
- Focused tests, tracing logs, this goal document, and clippy config needed for current repository gates.

Out of scope:
- Telegram transport behavior.
- Core LLM/provider/tool semantics except already-added runtime-entry observability.
- New external services, databases, brokers, distributed queues, HA, or sharding.
- Direct Google Gemini provider work.
- Strong crash-durability guarantees for write-behind task/session state; user explicitly accepts possible data loss on container failure before flush.

## Missing Inputs

- Target latency is not yet given as a hard SLO.
  - Impact: success must initially use measured relative improvement instead of a fixed contract.
  - Low-risk assumption or fallback: target agent spawn under 100 ms on warm cache, under 250 ms when only session load misses cache.
  - User/external action needed: provide a stricter target only if these bounds are insufficient.

## Repository Context

- Web backend route slices live under `crates/oxide-agent-transport-web/src/server/`.
- Web SQLx persistence is in `crates/oxide-agent-transport-web/src/persistence/sqlx.rs`.
- Moka is already available in `oxide-agent-transport-web`; no new dependency is required for local in-process caches.
- Current `docker-compose.web.yml` logs `oxide_agent_core=info` and `oxide_agent_transport_web=info`, so new `INFO` latency targets are visible in `docker logs oxide_agent_web -f`.
- Validation convention for this work: `cargo fmt`, `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`, and `cargo clippy --workspace --no-default-features --features profile-web-embedded-opencode-local`.

## Completion Audit

- G1: First-message latency observability is present
  - Source: User asked to add INFO logs visible through `docker-compose.web.yml` and use them to find the bottleneck.
  - Acceptance: Logs show phase-level timing from HTTP task creation through task executor and core runner entry, plus SQLx operation timings for auth/session/task hot calls.
  - Evidence required: code diff, successful cargo checks, and user runtime log showing per-phase/per-SQL latency.
  - Status: verified
  - Evidence collected: Added `oxide_agent_transport_web::web_latency` logs in task creation, task executor, session materialization, auth helpers, and SQLx store; added `oxide_agent_core::agent_latency` logs for prepare/runner/LLM start. User runtime log on 2026-06-07 showed `create_task total=1482ms`, `task_exists=187ms`, `save_task path=515ms`, `save_session update=375ms`, and executor/runtime waits at `0ms`. Commands passed: `cargo fmt`; `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`; `cargo clippy --workspace --no-default-features --features profile-web-embedded-opencode-local`.

- G2: Hot-path redundant DB writes are removed
  - Source: User accepted plan item 1 and 2: remove redundant `ensure_user_row` and no-op `save_task_progress.delete_empty`.
  - Acceptance: `save_task`/`save_session` no longer pay `ensure_user_row` on the authenticated hot path, and initial task creation with no progress does not issue a `DELETE FROM web_task_progress` round trip.
  - Evidence required: code diff, SQLx latency logs before/after, cargo check/clippy, and a measured create-task latency reduction.
  - Status: verified
  - Evidence collected: Checkpoint 2 implementation removes `ensure_user_row` from SQLx `save_task` and `save_session`, while keeping auth/user creation writes synchronous. Initial running task records with no progress now skip the no-op `web_task_progress` delete; non-initial no-progress records still retain the old delete behavior for explicit progress clearing. Commands passed: `cargo fmt`; `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`; `cargo clippy --workspace --no-default-features --features profile-web-embedded-opencode-local`. User runtime log after rebuild showed `create_task total=771ms`, `task_saved=219ms`, `session_task_update_saved=127ms`, no hot `ensure_user_row`, and no initial `save_task_progress.delete_empty`.

- G3: Agent spawn is moved before non-critical session update persistence
  - Source: User accepted lower latency over strict immediate DB durability.
  - Acceptance: The task executor is spawned after the minimum required in-memory/task state is available; `save_session_task_update` or equivalent durable session update happens in background without blocking agent runtime entry.
  - Evidence required: code diff, runtime logs showing `core_executor_call_started` before background `save_session`, cargo check/clippy, and measured spawn latency.
  - Status: verified
  - Evidence collected: Checkpoint 3 implementation spawns the registered task immediately after `save_task`, then enqueues `save_session_task_update` in a background task with `session_task_update_background_*` latency logs. `reject_active_task` checks in-memory runtime tasks first to cover the short window before durable `active_task_id` persistence. User runtime log after rebuild showed `task_saved elapsed=587ms`, `task_spawned elapsed=588ms`, `core_executor_call_started` before `session_task_update_background_saved elapsed=718ms`, and `create_task total=588ms`.

- G4: Moka-backed write-front cache and background DB flush exist for selected task/session writes
  - Source: User asked whether Moka can save results in RAM and asynchronously send to the DB later; user explicitly accepts crash loss.
  - Acceptance: The selected task/session records are inserted/updated in a bounded in-process cache first, queued for background Postgres flush, and readable by the web transport before flush completes. Flush errors are logged and retried or left visible in a simple pending state without blocking the agent hot path.
  - Evidence required: implementation diff, cache/queue latency logs, flush success/failure logs, focused tests or route/store tests, and runtime measurement.
  - Status: in_progress
  - Evidence collected: Checkpoint 4 implementation adds Moka write-front caching for initial no-progress `save_task` records in SQLx web persistence, with background Postgres insert retries and cache reads for `load_task`, `load_task_event_state`, and session-level `task_exists` after cache warm-up. Runtime measurement after rebuild is still required before marking G4 verified.

- G5: Before/after latency is documented with current logs
  - Source: User asks “что по цифрам будет?” and expects measured improvement.
  - Acceptance: This document records baseline and post-change timings for first-message create/spawn, including breakdown by phase.
  - Evidence required: summarized runtime logs without secrets, command outputs, and a before/after table.
  - Status: in_progress
  - Evidence collected: Baseline, checkpoint 2, and checkpoint 3 runtime numbers are recorded below. Checkpoint 4 runtime numbers are pending rebuild/run.

- Q1: Prefer simple local changes
  - Source: Repository over-engineering guardrails and target personal use up to 2-3 users / 5 RPS.
  - Acceptance: No external queue/cache/service is introduced; solution uses existing Moka and tokio primitives only.
  - Evidence required: dependency diff review and implementation review.
  - Status: in_progress
  - Evidence collected: Checkpoint 4 uses existing `moka` plus `tokio::spawn` in the SQLx web store; no new dependency, service, queue, or storage backend was added.

- Q2: Write-behind durability tradeoff is explicit
  - Source: User said container crash data loss is acceptable and DB can catch up later.
  - Acceptance: Code and docs do not pretend write-behind is crash-durable; user-visible behavior remains coherent for the running process, and logs make pending/background persistence observable.
  - Evidence required: implementation notes, runtime logs, and final documentation in this goal.
  - Status: in_progress
  - Evidence collected: Checkpoint 3 makes session active-task persistence asynchronous after task spawn; this can lose the durable session marker if the container crashes before background save completes. In-process coherence is preserved by checking runtime running tasks before relying on the persisted session marker. Checkpoint 4 extends the accepted tradeoff to initial task records: the task is available from in-process Moka before the background DB insert completes, but a container crash before flush may lose that durable task row.

- V1: Web profile validates
  - Source: Repository validation convention.
  - Acceptance: `cargo fmt`, `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`, and `cargo clippy --workspace --no-default-features --features profile-web-embedded-opencode-local` succeed after each checkpoint.
  - Evidence required: command output summary in Progress Log.
  - Status: verified
  - Evidence collected: Commands passed on 2026-06-07 after observability and clippy config updates. Re-ran after checkpoint 2 SQLx hot-path changes and checkpoint 3 spawn-order changes: `cargo fmt`; `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`; `cargo clippy --workspace --no-default-features --features profile-web-embedded-opencode-local`. Checkpoint 4 passed the same command suite plus `git diff --check`.

- N1: Do not change unrelated transports or provider behavior
  - Source: User scoped the work to web transport focus.
  - Must preserve: Telegram transport behavior, provider behavior, sandbox backends, manager control plane, wiki memory semantics, and direct Gemini absence.
  - Evidence required: `git diff --name-only` review before each commit.
  - Status: in_progress
  - Evidence collected: Checkpoint 2 code change is limited to `crates/oxide-agent-transport-web/src/persistence/sqlx.rs`. Checkpoint 3 code change is limited to `crates/oxide-agent-transport-web/src/server/task_routes.rs` and `crates/oxide-agent-transport-web/src/session.rs`. Checkpoint 4 code change is limited to `crates/oxide-agent-transport-web/src/persistence/sqlx.rs`; docs update is limited to this goal document. `git diff --name-only` reviewed before commit.

## Baseline Numbers

Runtime log sample from 2026-06-07 first task creation with remote Postgres-like latency:

| Segment | Baseline |
|---|---:|
| `create_task` total until response | 1482 ms |
| `session_loaded` | 184 ms |
| `task_exists` | 187-188 ms |
| `task_saved` route phase | 515 ms |
| `save_task.ensure_user_row` | 122 ms |
| `save_task` SQL | 193 ms |
| `save_task_progress.delete_empty` | 199 ms |
| `session_task_update_saved` route phase | 375 ms |
| `save_session.ensure_user_row` | 186 ms |
| `save_session` SQL | 189 ms |
| executor queue/lock/runtime registry wait | 0 ms |
| core `Starting agent task` after core call boundary | ~140 ms |
| core `prepare_execution` | ~632 ms |

Checkpoint 2 runtime sample after removing redundant hot DB calls:

| Segment | After checkpoint 2 | Delta from baseline |
|---|---:|---:|
| `create_task` total until response | 771 ms | -711 ms |
| `session_loaded` | 213 ms | +29 ms |
| `task_exists` | 210 ms | +22 ms |
| `task_saved` route phase | 219 ms | -296 ms |
| `session_task_update_saved` route phase | 127 ms | -248 ms |
| `task_spawned` | 0 ms | -217 ms |
| hot `ensure_user_row` before `save_task`/`save_session` | absent | removed |
| initial `save_task_progress.delete_empty` | absent | removed |

Checkpoint 3 runtime sample after spawning before background session update:

| Segment | After checkpoint 3 | Delta from checkpoint 2 | Delta from baseline |
|---|---:|---:|---:|
| `create_task` total until response | 588 ms | -183 ms | -894 ms |
| `session_loaded` | 198 ms | -15 ms | +14 ms |
| `task_exists` | 183 ms | -27 ms | -4 ms |
| `task_saved` route phase | 205 ms | -14 ms | -310 ms |
| `session_task_update_saved` blocking phase | absent | removed | removed |
| `session_task_update_background_saved` | 718 ms total / 130 ms phase | moved behind response | moved behind response |
| `core_executor_call_started` | ~588 ms | ~-183 ms | ~-677 ms |
| core `Starting agent task` | ~701 ms | ~-192 ms | ~-704 ms |
| first LLM call start | ~1413 ms | ~-204 ms | ~-635 ms |

Expected by checkpoint, assuming the observed 120-200 ms DB round trip remains stable:

| Stage | Expected agent-spawn latency |
|---|---:|
| Baseline | ~1480 ms |
| Remove redundant hot DB calls | ~950-1050 ms |
| Spawn before background session update | ~550-650 ms |
| Moka write-front warm path | ~10-80 ms |
| Moka write-front with session cache miss | ~150-250 ms |

## Implementation Plan

1. Observability checkpoint
   - Audit IDs: G1, V1.
   - Expected changes: INFO latency logs across task creation, runtime session creation, task executor, core runner entry, auth, and SQLx store operations.
   - Validation: `cargo fmt`; web profile `cargo check`; web profile `cargo clippy`; runtime log review.
   - Exit condition: exact pre-runtime bottleneck is visible in `docker logs oxide_agent_web -f`.

2. Remove redundant hot DB calls
   - Audit IDs: G2, G5, V1, Q1, N1.
   - Expected changes: skip `ensure_user_row` for authenticated hot `save_task`/`save_session` paths or introduce explicit hot-path variants; skip empty progress deletion for new tasks with no progress.
   - Validation: cargo checks, clippy, runtime log showing removed SQL operations and lower `task_saved`/`session_task_update_saved` timings.
   - Exit condition: first-message path no longer logs `ensure_user_row` before hot `save_task`/`save_session`, and no initial `save_task_progress.delete_empty` appears for new tasks.

3. Reorder spawn before non-critical session persistence
   - Audit IDs: G3, G5, V1, Q2, N1.
   - Expected changes: spawn task executor as soon as task is registered and minimal task state is available; move session active-task persistence to background with logging.
   - Validation: runtime logs prove agent starts before background session save completes; cargo checks and clippy pass.
   - Exit condition: agent spawn latency is dominated only by required pre-spawn state, not by durable session update.

4. Add Moka write-front cache and background flush
   - Audit IDs: G4, G5, Q1, Q2, V1, N1.
   - Expected changes: add bounded task/session caches and a simple tokio background flusher for selected writes; coalesce obvious duplicate writes by key if simple; expose pending/flush logs under `oxide_agent_transport_web::web_latency`.
   - Validation: tests or focused route/store checks for cache-read-before-flush; runtime logs showing cache enqueue under a few ms and later DB flush; cargo checks and clippy pass.
   - Exit condition: warm first-message path starts agent without blocking on Postgres task/session writes.

5. Final measurement and audit
   - Audit IDs: G5, Q1, Q2, V1, N1.
   - Expected changes: update this doc with before/after logs, final decisions, and remaining tradeoffs.
   - Validation: final command suite and `git diff --name-only` scope review.
   - Exit condition: Completion Audit is verified or remaining gaps are explicitly blocked/dropped by user.

## Validation Contract

- Static checks:
  - `git diff --check`
  - `git diff --name-only`
- Rust checks:
  - `cargo fmt`
  - `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`
  - `cargo clippy --workspace --no-default-features --features profile-web-embedded-opencode-local`
- Runtime/manual verification:
  - Rebuild and run `docker-compose.web.yml`.
  - Send a first message in a new web session.
  - Capture `oxide_agent_transport_web::web_latency` and `oxide_agent_core::agent_latency` logs from `docker logs oxide_agent_web -f`.
  - Record `create_task`, `core_executor_call_started`, `Starting agent task`, and SQLx operation timings.
- Done when: all non-dropped Completion Audit items are verified with current evidence.

## Decisions

- 2026-06-07: Create a separate goal doc instead of reopening the completed broader web performance goal because this objective is narrower: first-message write-behind latency, not general chat/page load performance.
- 2026-06-07: Treat remote Postgres round-trip latency as the proven bottleneck; executor queue, runtime registry, and executor lock all measured at `0ms` on the sampled first-message path.
- 2026-06-07: Accept non-crash-durable write-behind for selected web task/session state because the user explicitly prioritizes latency and says container crash data loss is acceptable.
- 2026-06-07: Use existing `moka` in `oxide-agent-transport-web`; no new dependency or external cache service is justified.
- 2026-06-07: Preserve explicit progress clearing for non-initial no-progress task saves while skipping the first-message no-op progress delete. This removes the hot round trip without broadening the semantic change more than needed.
- 2026-06-07: When moving `active_task_id` persistence behind task spawn, preserve in-process busy-session behavior by checking runtime running tasks before the durable session marker. This avoids a duplicate-task race without reintroducing synchronous DB latency.
- 2026-06-07: Start write-front at the narrowest hot write: initial no-progress `save_task`. Use cache-read-before-flush for task reads, but keep auth and unrelated persistence synchronous. Initial background insert uses `ON CONFLICT DO NOTHING` so a later synchronous task update cannot be overwritten by a delayed initial flush.

## Progress Log

- 2026-06-07 10:30: Goal document created after first-message latency RECON and observability checkpoint.
  - Changed: Added this goal contract with baseline numbers, write-behind plan, validation contract, and completion audit.
  - Evidence: User runtime logs show every SQLx round trip costs roughly `120-200ms`, while task executor queue/lock/runtime wait is `0ms`. Current observability code and clippy config changes validate with `cargo fmt`, `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`, and `cargo clippy --workspace --no-default-features --features profile-web-embedded-opencode-local`.
  - Commands: `git status --short`; `git log --oneline -5`; `cargo fmt`; `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`; `cargo clippy --workspace --no-default-features --features profile-web-embedded-opencode-local`.
  - Audit IDs updated: G1 verified, V1 verified, G5 baseline recorded.
  - Next: Checkpoint 2 — remove redundant hot DB calls before introducing write-behind.

- 2026-06-07 13:29: Checkpoint 2 code path implemented.
  - Changed: Removed SQLx `ensure_user_row` calls from `save_task` and `save_session`; skipped initial no-progress task `web_task_progress` delete while retaining the delete for non-initial no-progress saves.
  - Evidence: User runtime log after container rebuild showed `create_task total=771ms`, down from `1482ms`; `task_saved=219ms`, down from `515ms`; `session_task_update_saved=127ms`, down from `375ms`; no hot `ensure_user_row` or initial `save_task_progress.delete_empty` appeared in the first-message path.
  - Commands: `cargo fmt`; `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`; `cargo clippy --workspace --no-default-features --features profile-web-embedded-opencode-local`; `git diff --check`; `git diff --name-only`.
  - Audit IDs updated: G2 verified, G5 in progress, V1 verified, N1 in progress.
  - Next: Checkpoint 3 — spawn executor before non-critical session persistence.

- 2026-06-07 13:48: Checkpoint 3 code path implemented.
  - Changed: `api_create_task` now spawns the registered task immediately after `save_task`; `save_session_task_update` runs in a background task with explicit `session_task_update_background_*` logs; `reject_active_task` checks runtime running tasks before relying on persisted `active_task_id`.
  - Evidence: User runtime log after container rebuild showed `create_task total=588ms`, down from checkpoint 2 `771ms`; `task_spawned`/`core_executor_call_started` happened at `588ms`, before `session_task_update_background_saved` at `718ms`.
  - Commands: `cargo fmt`; `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`; `cargo clippy --workspace --no-default-features --features profile-web-embedded-opencode-local`.
  - Audit IDs updated: G3 verified, Q2 in progress, V1 verified, N1 in progress.
  - Next: Checkpoint 4 — write-front initial task persistence with Moka and background DB flush.

- 2026-06-07 14:12: Checkpoint 4 initial task write-front implemented.
  - Changed: SQLx `save_task` now writes initial no-progress task records into bounded Moka caches and returns immediately; a background flush inserts the durable task row with up to three attempts. `load_task`, `load_task_event_state`, and warmed `task_exists` consult the task caches before Postgres. Non-initial task saves remain synchronous and refresh the cache.
  - Evidence: Checkpoint 3 runtime log verified `create_task total=588ms`, `core_executor_call_started` before background `save_session`, and background session save at `718ms`; G3 is verified. Checkpoint 4 implementation evidence is code-level; runtime measurement is pending after rebuild.
  - Commands: `cargo fmt`; `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`; `cargo clippy --workspace --no-default-features --features profile-web-embedded-opencode-local`; `git diff --check`.
  - Audit IDs updated: G3 verified, G4 in progress, G5 in progress, Q1 in progress, Q2 in progress, V1 verified, N1 in progress.
  - Next: Commit checkpoint 4, then rebuild/run to verify `save_task.write_front_cached` replaces blocking `save_task` on first-message path.

## Risks and Blockers

- Write-behind can lose unflushed task/session updates on container crash.
  - Impact: after restart, recent task metadata or session active-task marker may be missing/stale.
  - Evidence: User explicitly accepted this risk for low latency.
  - Mitigation or requested decision: keep the behavior local, logged, and limited to selected web task/session writes; do not apply to auth/security-critical writes.
  - Audit IDs affected: G4, Q2.

- Auth and settings writes should not be blindly write-behind.
  - Impact: stale auth/session security state could survive longer than intended.
  - Evidence: Existing auth cache uses bounded TTL and explicit invalidation; this goal targets first-message task/session persistence, not auth durability.
  - Mitigation or requested decision: keep auth writes synchronous unless separately approved.
  - Audit IDs affected: Q2, N1.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
