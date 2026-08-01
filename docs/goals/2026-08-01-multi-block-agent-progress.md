# Goal: Multi-block agent progress in Web and Telegram

Status: complete
Source: User-approved audited Web and Telegram progress plan, 2026-08-01
Last updated: 2026-08-01

## Objective

While an agent execution is running, show a bounded chronology of distinct real progress statuses in Web and Telegram; when execution waits or terminates, replace the complete transient chronology with only the authoritative outcome, then commit, push, build, and redeploy both transports.

## Execution Directive

Complete the frozen Required Outcomes using the listed Change Envelope and Primary Evidence. Work on the smallest unresolved outcome. Do not add requirements from reviews, tests, tools, speculative risks, or optional source text. Finish when every required outcome is resolved and affected constraints remain satisfied.

## Frozen Contract

### Required Outcomes

- R1: Web renders multiple task- and run-scoped progress cards from real Activity events.
  - Source: User clarification and approved audited plan.
  - Acceptance: During `Queued` or `Running`, distinct qualifying Activity events render as separate chronological cards, adjacent duplicates collapse, and at most the newest eight remain; task, version, and resumed execution histories do not mix.
  - Primary evidence: Focused native Web projection tests plus wasm build.
  - Status: verified
  - Evidence: Web now projects exact-task/current-run Activity events by sequence into adjacent-deduplicated keyed cards capped at eight; 44 native tests, wasm check, and release Trunk build pass.
- R2: Web authoritative outcomes replace the complete transient progress stack.
  - Source: User clarification and approved audited plan.
  - Acceptance: Waiting, completed, failed, cancelled, and interrupted tasks render only their prompt/final/error outcome; stale or prior-generation streams and progress cannot resurrect running cards.
  - Primary evidence: Web state/stream tests, profile-full resume integration, wasm check, and release build.
  - Status: verified
  - Evidence: Exact terminal status match remains the sole output owner; run floors, cleared resume progress, and generation-owned streams prevent prior-run resurrection. Native tests, wasm check, release Trunk build, and the profile-full resume integration pass.
- R3: Telegram shows multiple bounded progress sections in one edited anchor.
  - Source: User clarification and approved audited plan.
  - Acceptance: Real emitted progress states append consecutive-distinct compact sections to one message ID; non-adjacent repeats remain, at most eight sections are retained, and the complete rendered anchor remains within the 4000-character safety budget.
  - Primary evidence: Telegram progress history/render tests and package tests.
  - Status: verified
  - Evidence: The progress target owns a run-local consecutive-distinct section deque, caps at eight, evicts complete oldest sections to stay within 4000 HTML characters, and records successful text only after the edit; full profile Telegram tests pass.
- R4: Real Telegram progress changes are delivered promptly without synthetic heartbeat output.
  - Source: User's approximately-every-few-seconds example and approved audited correction.
  - Acceptance: The first changed state is emitted immediately, a throttled dirty state gets one trailing flush within the existing throttle window, and idle time emits nothing.
  - Primary evidence: Focused `oxide-agent-runtime` progress-loop tests.
  - Status: verified
  - Evidence: Runtime now emits the first real state immediately, schedules one trailing dirty flush through the existing throttle, and emits nothing while clean; all 8 focused progress-loop tests pass.
- R5: Telegram authoritative outcomes retain the deployed edit-first replacement contract.
  - Source: Approved audited plan and existing commit `26688f52` behavior.
  - Acceptance: Short outcomes replace the anchor, long finals replace it with chunk one and send only final overflow in the same thread, waiting/error/manual outcomes replace it, silent no-change removes it, and loop UI remains its special anchor owner.
  - Primary evidence: Full profile Telegram tests and focused terminal/chunk tests.
  - Status: verified
  - Evidence: Terminal snapshots are excluded from history and the existing outcome-owned edit-first task runner/messaging path is unchanged; 140 library tests, 12 XML/progress integrations, remaining integrations, and doctests pass under `profile-full`.
- R6: The completed change is committed, pushed, built, and redeployed.
  - Source: Explicit user instruction.
  - Acceptance: One coherent implementation commit is synchronized to `origin/main`; release images build; `oxide_agent` and `oxide_web` run successfully on the current host with Web health and transport startup logs clean.
  - Primary evidence: Git synchronization, Docker Compose build/rollout, Web health, service status, and startup logs.
  - Status: verified
  - Evidence: Implementation commit `19dd2101` is on `origin/main`; Compose built release images `oxide-agent@sha256:4403207b74c...` and `oxide-agent-web@sha256:f575adc58102...`, recreated both transports, Web health returns `{"status":"ok"}`, and startup logs show Telegram long polling plus Web workers/listener running without errors.

### Constraints

- C1: Status blocks come only from real Activity/progress changes; no periodic fake heartbeat or repeated “still thinking” entries.
- C2: Web terminal ownership remains `TaskStatus`; Telegram terminal ownership remains `AgentExecutionOutcome`.
- C3: Telegram live history uses one message ID; no live overflow messages are created.
- C4: No new dependency, service, queue, cache, config key, public API, schema, migration, or persisted transient-history format.
- C5: Preserve task-scoped Activity, stale-terminal rejection, resume event sequencing, final file artifacts, Telegram thread identity, and progress-disabled reminder behavior.
- C6: Keep full Activity uncapped; only the inline/live projection is bounded.

### Non-goals

- Exact wall-clock updates while no real status changes.
- Exact reconstruction of prior transient coalescing after browser reload.
- Shared Web/Telegram presentation model.
- Multiple Telegram progress message IDs or atomic long-final delivery.
- Life Mode progress redesign, auto-scroll infrastructure, process-restart Telegram anchor recovery, or unrelated cleanup.

## Change Envelope

- Target: Runtime progress emission cadence; Web Activity-to-card projection, resumed-run stream ownership, and terminal switching; Telegram run-local section accumulation/rendering; directly affected tests and this goal document.
- Expected paths, symbols, and direct consumers: `crates/oxide-agent-runtime/src/agent/runtime/progress.rs`; `crates/oxide-agent-web-ui/src/{sse.rs,tasks/state.rs,tasks/task_card.rs,tasks/streaming.rs,tasks/workspace.rs,styles/04-chat.css}`; `crates/oxide-agent-transport-web/src/server/{task_routes.rs,tests.rs}`; `crates/oxide-agent-transport-telegram/src/bot/{progress_render.rs,agent_transport.rs}`; `crates/oxide-agent-transport-telegram/tests/agent_xml_leak_prevention.rs`.
- Allowed artifacts: Existing Rust source/tests, existing Web CSS, this goal document, one implementation commit, and current Docker Compose deployment.
- Forbidden artifacts: New dependencies, contracts/endpoints, schema/migrations, persistent progress history, timers that emit without state changes, compatibility/fallback paths, live Telegram continuation messages, or unrelated refactors.
- User or harness budget: KISS/YAGNI/Pareto; use existing Activity and progress data, keep at most eight live statuses, and stop when R1-R6 are verified.

## Current Checkpoint

- Closes: None; the goal is complete.
- Smallest next action: Stop.
- Expected evidence: R1-R6 are verified below.
- Stop or replan if: Not applicable.

## Current State

- Resolved: R1-R6.
- Last relevant evidence: Commit `19dd2101` is pushed; release images built and both transport containers run; Web health is OK and startup logs are clean.
- Blocker: None.
- Next: Stop.

## Material Decisions

- 2026-08-01: Web uses separate visual cards; Telegram uses multiple sections inside one edited anchor so terminal replacement remains deterministic.
- 2026-08-01: Web chronology is projected from persisted Activity events ordered by `seq`; `ProgressSnapshot` does not own historical card identity.
- 2026-08-01: The inline/live window is capped at eight, while full Activity remains uncapped.
- 2026-08-01: Cadence is state-change-driven with an immediate first update and trailing flush, not a synthetic heartbeat.

## Checkpoint History

- 2026-08-01: R1-R6 frozen from the user-approved audited plan; implementation not started.
- 2026-08-01: R4 verified with 8 focused runtime tests; next checkpoint is R1-R2 Web progress and resume isolation.
- 2026-08-01: R1 verified and R2 implementation validated locally; deployed terminal smoke remains. Next checkpoint is R3/R5 Telegram history.
- 2026-08-01: R3 and R5 verified by the complete `profile-full` Telegram package suite; next checkpoint is final diff and affected gates.
- 2026-08-01: R2 and the final constraint review verified; full workspace Clippy passes, with known wasm-only `voice.rs` warnings outside this diff recorded separately. Next checkpoint is R6 delivery.
- 2026-08-01: R6 verified; implementation committed/pushed, release images built, both transports redeployed, Web health OK, and startup logs clean. Goal complete.

## Completion

- Resolved outcomes: R1-R6.
- Commands and artifacts: Focused runtime tests; 44 Web UI tests; wasm check; release Trunk build; profile-full Web resume integration; full profile Telegram package tests; profile-full bot check; full workspace Clippy; fmt/diff checks; Compose release build and rollout; Web health, container, image, and startup-log checks.
- Constraint and diff-scope check: No dependency, API, config, schema, migration, persisted transient history, fake heartbeat, live Telegram overflow, shared presentation model, or unrelated runtime path was added. Full Activity and existing terminal/file/thread/reminder owners remain.
- Final status: complete
