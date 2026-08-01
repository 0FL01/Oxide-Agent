# Goal: Restore Web task versioning after backend restart

Status: complete
Source: User-approved audited restart/versioning fix plan, 2026-08-01
Last updated: 2026-08-01

## Objective

After startup reconciliation interrupts a durable Web task, editing and rerunning that latest task creates a new version without a stale-status 409; task state has one durable owner after the initial write-front flush, then the change is committed, pushed, built, and redeployed.

## Execution Directive

Complete the frozen Required Outcomes using the listed Change Envelope and Primary Evidence. Work on the smallest unresolved outcome. Do not add requirements from reviews, tests, tools, speculative risks, or optional source text. Finish when every required outcome is resolved and affected constraints remain satisfied.

## Frozen Contract

### Required Outcomes

- R1: Durable Web task state has one owner after initial write-front persistence.
  - Source: User-approved audited plan and request to remove obstructive duplicate state.
  - Acceptance: Pending initial tasks remain immediately readable, but after the main `web_tasks` row is durable, `load_task`, `load_task_event_state`, and task lists observe PostgreSQL state rather than a long-lived full-record cache.
  - Primary evidence: Real-PostgreSQL two-store reconciliation regression covering task and event-state reads.
  - Status: verified
  - Evidence: The real-PostgreSQL two-store regression failed on baseline with `list_tasks=Interrupted` but `load_task=Running`, then passed after full records were restricted to pending initial writes and cleared at durability handoff; task and event-state reads now both return `Interrupted`.
- R2: The latest interrupted task can be edited and rerun after backend startup reconciliation.
  - Source: User-reported `Only terminal tasks can be versioned. (409)` and approved route plan.
  - Acceptance: The version endpoint checks parent existence, terminal status, and latest status from one durable task snapshot; latest `Interrupted` succeeds while active, non-latest, and missing-task responses retain their contracts.
  - Primary evidence: Focused Web route tests plus deployed restart/edit/rerun smoke.
  - Status: verified
  - Evidence: The focused route test versions latest `Interrupted`, preserves active/non-latest 409 responses and missing-task 404, and the complete 169-test Web library suite plus 7 runnable E2E tests pass.
- R3: The completed fix is committed, pushed, built, and redeployed.
  - Source: Explicit user instruction.
  - Acceptance: One coherent commit is synchronized to `origin/main`; the Web release image builds; `oxide_web` is recreated and healthy with clean startup logs.
  - Primary evidence: Git synchronization, Docker Compose build/rollout, Web health, container state, and startup logs.
  - Status: verified
  - Evidence: Implementation commit `2093ca88` is on `origin/main`; Web image `sha256:edec48ce163842e06372e7bab8420ba3cbf731fd921b0d944783ccae5823aa93` built successfully and runs in `oxide_agent_web`; health is OK and startup logs are clean.

### Constraints

- C1: Preserve the accepted nonblocking initial task write-front and its duplicate-flush coalescing.
- C2: No schema, migration, config, dependency, public API, service, queue, cache, fallback, or compatibility path is added.
- C3: Preserve task ownership/resource hiding, lineage, latest-task ordering, and existing error codes.
- C4: Prefer deletion of durable cache ownership and redundant reads; do not create a new store abstraction.

### Non-goals

- Event/file foreign-key race redesign.
- Atomic task/session reconciliation redesign.
- Concurrent version-request serialization or multi-instance execution coordination.
- UI changes, all-status permutation coverage, or unrelated cache cleanup.

## Change Envelope

- Target: SQLx Web task write-front handoff, durable task reads, startup reconciliation invalidation, and task-version validation.
- Expected paths, symbols, and direct consumers: `crates/oxide-agent-transport-web/src/persistence/sqlx.rs`; `crates/oxide-agent-transport-web/src/server/task_routes.rs`; directly affected tests in the same crate; this goal document.
- Allowed artifacts: Existing Rust source/tests, this goal document, one implementation commit, and current root Docker Compose deployment.
- Forbidden artifacts: Schema/migration/config changes, new dependencies or runtime mechanisms, UI changes, compatibility branches, and unrelated refactors.
- User or harness budget: KISS/YAGNI/Pareto; production code should remove duplicate ownership and remain LoC-neutral or deletion-dominant.

## Current Checkpoint

- Closes: None; the goal is complete.
- Smallest next action: Stop.
- Expected evidence: R1-R3 are verified below.
- Stop or replan if: Not applicable.

## Current State

- Resolved: R1-R3.
- Last relevant evidence: Commit `2093ca88` is pushed; its release image runs in `oxide_agent_web`, health is OK, and startup logs are clean.
- Blocker: None.
- Next: Stop.

## Material Decisions

- 2026-08-01: Keep the initial nonblocking write-front and separate in-flight marker; PostgreSQL becomes canonical immediately after the main task-row write succeeds.
- 2026-08-01: Use the existing task list snapshot in the version endpoint instead of adding a store API.
- 2026-08-01: Exclude adjacent persistence and concurrency defects that do not cause the reported 409.

## Checkpoint History

- 2026-08-01: Contract frozen from the user-approved audited plan; implementation not started.
- 2026-08-01: R1 regression established against temporary PostgreSQL: the desired two-store coherence assertion fails on baseline because `load_task` returns stale `Running`; next checkpoint is the pending-only cache handoff.
- 2026-08-01: R1 verified: durable cache fills were removed, pending initial records clear after main-row persistence, and the two-store reconciliation test now observes `Interrupted` through all task reads. Next: R2 route ownership.
- 2026-08-01: R2 local behavior verified: version validation now uses one task-list snapshot; focused tests cover interrupted success plus active, non-latest, and missing-task responses. Next: affected and mandatory gates.
- 2026-08-01: R1-R2 closure validation passed: eight real-PostgreSQL SQLx tests, the complete Web package suite, workspace check, all-feature Clippy, fmt, and diff checks are green. Next: R3 delivery.
- 2026-08-01: R3 verified: implementation commit `2093ca88` was built, pushed, and deployed; the running container uses the built image, Web health is OK, and startup logs are clean. Goal complete.

## Completion

- Resolved outcomes: R1-R3 verified.
- Commands and artifacts: Baseline-red then green real-PostgreSQL reconciliation regression; eight SQLx tests; 169 Web library tests and 7 runnable E2E tests; workspace profile-full check; all-target/all-feature Clippy; fmt and diff checks; Compose release build/rollout; Git, image, container, health, and startup-log checks.
- Constraint and diff-scope check: No schema, migration, config, dependency, API, service, queue, cache, compatibility path, or unrelated flow changed. Initial write-front/coalescing remains, durable task cache ownership and redundant version read are gone, and production Rust decreased by 17 net LOC.
- Final status: Complete on the current host.
