# Goal: R2 to SQLx/Postgres Durable Storage

Date started: 2026-06-05
Status: active
Codex goal: `/goal Implement docs/goals/2026-06-05-r2-to-postgres-storage.md until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update this document after each meaningful verification, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.`
Source spec: `docs/prd/PRD-r2-to-pg.md`
Goal doc owner: Codex
Last updated: 2026-06-05

## Objective

Move Oxide Agent durable runtime storage from Cloudflare R2/S3 object storage to a single SQLx/Postgres storage layer that works for local PostgreSQL and Supabase Postgres fresh setups, without reading, importing, dual-writing, or preserving old R2 data.

Done when every required Completion Audit item is verified by its listed evidence, production-like Cargo profiles no longer include R2/AWS durable storage dependencies, current setup docs no longer require R2 credentials, and the app has SQL-backed durable storage for core, web, reminders, audit, wiki memory, tests, CI, and deployment paths.

## Scope

In scope:
- Core storage facade and backend modules under `crates/oxide-agent-core/src/storage/`.
- Capability manifests, profile feature composition, Cargo dependencies, and `Cargo.lock`.
- Web persistence under `crates/oxide-agent-transport-web/src/persistence/` and web startup/store selection.
- Telegram and web durable startup paths that call `storage::build_primary_storage` or replacement builders.
- Top-level SQL migrations and SQL integration test infrastructure.
- Durable user/context state, agent memory/flows, control-plane records, secrets, reminders, audit events, wiki memory, web users/auth/sessions/tasks/events/files.
- CI, `.env.example`, deploy docs, README/current docs, and static guards needed to prove R2 is gone from runtime durable paths.
- Goal progress, checkpoint evidence, and decisions recorded in this document.

Out of scope:
- Migrating, importing, scanning, reading, or backfilling old R2 objects.
- Dual-write or temporary compatibility readers between R2 and SQL.
- R2 blob fallback, Supabase Storage buckets, SQLite, Redis, queues, sharding, HA, or extra storage services.
- Deleting R2 code before SQL-backed acceptance tests cover the corresponding runtime paths, unless the user explicitly requests a big-bang rewrite.
- Large product behavior changes unrelated to durable storage migration.

## Missing Inputs

- B1: Maximum task file/blob size for Postgres-only storage.
  - Impact: Current web upload defaults are risky for `bytea` storage and Supabase WAL/backups.
  - Low-risk assumption or fallback: Start with a strict configurable limit and reject larger files until the user approves another non-R2 blob architecture.
  - User/external action needed: Approve a maximum file size before Phase 2 task-file persistence is finalized.

- B2: Retention defaults for task events, task files, wiki raw archives, old auth sessions, and optionally audit.
  - Impact: Infinite retention can grow Postgres/WAL; aggressive cleanup can break replay expectations.
  - Low-risk assumption or fallback: Preserve final task/session state, add `retention_expires_at` fields, and keep cleanup bounded but disabled or conservative until policy is approved.
  - User/external action needed: Decide retention policy before Phase 7 completion.

- B3: Migration execution policy.
  - Impact: Local dev can benefit from startup migrations, but production/Supabase should prefer deploy-step migrations.
  - Low-risk assumption or fallback: Add config with production default `OXIDE_DATABASE_MIGRATE_ON_STARTUP=false`; local docs may opt in explicitly.
  - User/external action needed: Confirm whether local startup should run migrations automatically.

- B4: Supabase production connection endpoint and pool limits.
  - Impact: Wrong pool defaults can exhaust Supabase connection limits.
  - Low-risk assumption or fallback: Use one shared pool per process and conservative defaults.
  - User/external action needed: Verify the intended Supabase connection URL/pooler mode for the target deployment during Phase 1/7.

- B5: Conflict semantics for mutable records.
  - Impact: Old R2 ETag conflicts must become explicit SQL version checks, row locks, or last-write-wins behavior.
  - Low-risk assumption or fallback: Use version columns and transaction tests where user-visible conflicts matter; keep existing logical behavior where tests define it.
  - User/external action needed: Decide any underspecified user-visible conflict behavior found during Phase 3/4.

- B6: Global wiki page ownership.
  - Impact: SQL schema needs deterministic uniqueness for global pages.
  - Low-risk assumption or fallback: Treat global wiki pages as globally shared unless implementation evidence requires user scoping.
  - User/external action needed: Confirm ownership if Phase 5 finds collisions or product ambiguity.

## Repository Context

- The PRD maps the existing R2 surface across Cargo features, capability registry, core storage, web persistence, Telegram startup, tests, docs, deploy, and CI (`docs/prd/PRD-r2-to-pg.md:41`).
- Current production durable storage is gated by `storage-s3-r2` and AWS SDK dependencies in production-like profiles (`docs/prd/PRD-r2-to-pg.md:45`).
- `StorageProvider` and `WebUiStore` are the main seams to preserve while changing implementations (`docs/prd/PRD-r2-to-pg.md:108`, `docs/prd/PRD-r2-to-pg.md:410`).
- Web task events currently use chunked JSON object rewrites; the target invariant is append-only SQL rows (`docs/prd/PRD-r2-to-pg.md:410`, `docs/prd/PRD-r2-to-pg.md:715`).
- Reminders currently list/filter R2 objects and mutate by ETag; the target invariant is SQL due claiming with row locks/leases (`docs/prd/PRD-r2-to-pg.md:351`, `docs/prd/PRD-r2-to-pg.md:748`).
- Current CI injects dummy/real R2 env vars and validates R2 credentials; target CI needs Postgres service/migrations and no R2 secrets (`docs/prd/PRD-r2-to-pg.md:554`, `.github/workflows/ci-cd.yml:13`, `.github/workflows/ci-cd.yml:73`).
- Current docs and README still describe R2-backed context/wiki storage (`README.md:22`, `README.md:77`, `docs/prd/PRD-r2-to-pg.md:554`).

## Completion Audit

- G1: Phase 0 deletion and entity map is complete
  - Source: `docs/prd/PRD-r2-to-pg.md:1918`
  - Requirement: Verify all R2/S3/AWS references and classify them as runtime, tests, current docs, historical docs, or false positives; map old object namespaces to SQL entities.
  - Acceptance: Deletion map covers core, web, Telegram, docs, tests, CI, profiles, Cargo files, env vars, and all runtime R2/S3/AWS references have a planned removal/replacement.
  - Evidence required: Targeted `rg` output summary, deletion map artifact or updated goal section, SQL entity mapping, and diff review showing no SQLite work was added.
  - Status: pending
  - Evidence collected:

- G2: SQLx/Postgres foundation is added without broad business-logic porting
  - Source: `docs/prd/PRD-r2-to-pg.md:1966`
  - Requirement: Add SQLx/Postgres dependency/config, shared `PgPool`, DB health check, migration strategy, `storage/sqlx` capability/profile entries, and local Postgres CI/test strategy.
  - Acceptance: SQLx foundation builds, connects to Postgres, verifies health/migrations, appears in capability/profile outputs, and introduces no SQLite dependency.
  - Evidence required: Focused Cargo/profile diffs, migration files, capability command output, `cargo check` for affected profiles, DB health test, and CI Postgres evidence.
  - Status: pending
  - Evidence collected:

- G3: Web persistence uses SQLx/Postgres for durable web state
  - Source: `docs/prd/PRD-r2-to-pg.md:2014`
  - Requirement: Move web users/auth/sessions/tasks/task events/progress/task files from R2 object store to SQLx.
  - Acceptance: Production web startup uses `SqlxWebUiStore`; task events are append-only rows with unique `(user_id, session_id, task_id, seq)`; event listing uses indexed pagination; restart reconciliation uses SQL; file blobs are bounded Postgres rows or rejected by policy.
  - Evidence required: SQL migrations, `WebUiStore` SQL contract tests, web startup tests, append/list event tests, reconciliation tests, file size tests, and web restart smoke.
  - Status: pending
  - Evidence collected:

- G4: Core durable state uses SQLx/Postgres through `StorageProvider`
  - Source: `docs/prd/PRD-r2-to-pg.md:2065`
  - Requirement: Move user config/state, contexts, agent memory/flows, profiles, topic context, topic AGENTS.md, topic infra, topic bindings, and secrets to SQL-backed `StorageProvider` methods.
  - Acceptance: Telegram and web session manager can use SQL-backed `StorageProvider`; updating one context does not rewrite full user config; flow memory persists/reloads through SQL; control-plane CRUD uses transactions/version columns; R2 user/memory/control-plane modules are unused by production paths.
  - Evidence required: Trait-level SQL integration tests, context update tests, flow checkpoint tests, control-plane version/conflict tests, secret redaction review, and startup checks.
  - Status: pending
  - Evidence collected:

- G5: Reminders and audit are SQL-native
  - Source: `docs/prd/PRD-r2-to-pg.md:2113`
  - Requirement: Replace R2 reminder jobs and audit JSON-array rewrites with SQL queue rows and append-only audit rows.
  - Acceptance: Reminder claim uses transaction-safe Postgres row locks/leases; concurrent claimers cannot double-claim; audit append allocates stable per-user versions transactionally; audit pagination is indexed and stable.
  - Evidence required: Reminder CRUD/status tests, concurrent claim tests, lease expiry tests, audit append/page tests, query/index review.
  - Status: pending
  - Evidence collected:

- G6: Wiki memory uses SQLx/Postgres rows, not object keys
  - Source: `docs/prd/PRD-r2-to-pg.md:2156`
  - Requirement: Store wiki pages and related memory text in SQL rows with deterministic scope/path metadata and content limits.
  - Acceptance: Wiki read/write/delete/list/context-delete work through SQL-backed storage; no runtime wiki path uses R2/S3 object storage or prefix delete; content limits and raw archive retention are enforced or documented.
  - Evidence required: Wiki SQL integration tests, internal API/diff review, docs update, content-size tests, and `rg` guard output for wiki runtime paths.
  - Status: pending
  - Evidence collected:

- G7: R2/S3/AWS durable storage code, features, env vars, docs, and dependencies are removed from runtime paths
  - Source: `docs/prd/PRD-r2-to-pg.md:2201`
  - Requirement: Physically delete R2 storage modules/object-store web persistence and remove AWS SDK dependencies, `storage-s3-r2`, `storage/r2`, `OXIDE_R2_*`, and R2-specific tests/docs from current runtime/setup paths.
  - Acceptance: Production-like profiles build without AWS SDK/S3 crates; binary `required-features` no longer mention R2; runtime grep/static guard finds no R2/S3/AWS durable storage references outside explicitly historical docs.
  - Evidence required: Cargo/profile diffs, `cargo tree` output, static grep guard output, capability snapshot updates, CI/deploy/env docs diffs, and full affected-profile checks.
  - Status: pending
  - Evidence collected:

- G8: Fresh local Postgres and Supabase setup is documented and wired
  - Source: `docs/prd/PRD-r2-to-pg.md:1133`, `docs/prd/PRD-r2-to-pg.md:1161`, `docs/prd/PRD-r2-to-pg.md:1845`
  - Requirement: Provide current setup docs/env/config for local PostgreSQL and Supabase Postgres fresh setup without R2.
  - Acceptance: `.env.example`, README/current docs, deploy docs, CI/deployment env, and health checks use DB vars; docs state old R2 data is intentionally ignored; no current setup path requires object-storage credentials.
  - Evidence required: Docs/env diff review, local setup smoke or documented command sequence, Supabase compatibility checklist, and CI workflow review.
  - Status: pending
  - Evidence collected:

- G9: SQL backend is hardened for production-like use
  - Source: `docs/prd/PRD-r2-to-pg.md:2250`
  - Requirement: Add indexes, retention/cleanup policies, pool tuning, failure-mode tests, performance smoke tests, and Supabase compatibility notes.
  - Acceptance: Large task event append/page does not scan O(n) or rewrite large blobs; cleanup jobs are bounded/idempotent; pool defaults are conservative; DB unavailable/migration/transaction/duplicate/claim failure modes produce actionable errors.
  - Evidence required: Query/index review, cleanup tests, performance smoke summaries, failure-mode tests, pool config docs, and Supabase checklist.
  - Status: pending
  - Evidence collected:

- Q1: Fresh setup only; no old R2 data migration or compatibility path
  - Source: `docs/prd/PRD-r2-to-pg.md:29`
  - Acceptance: No importer, backfill, R2 reader, R2 fallback, dual-write, or old object scan tooling is implemented.
  - Evidence required: Diff review, static grep guard output, and docs stating old R2 data is ignored.
  - Status: pending
  - Evidence collected:

- Q2: Keep the solution simple and storage-focused
  - Source: `AGENTS.md`, `docs/prd/PRD-r2-to-pg.md:1021`
  - Acceptance: No SQLite backend, Supabase Storage bucket, new queue/cache/service, sharding, HA, or broad framework abstraction is added.
  - Evidence required: Cargo diffs, compose/deploy diffs, dependency review, and implementation diff review.
  - Status: pending
  - Evidence collected:

- Q3: Data model uses typed columns for queryable fields and JSONB only where justified
  - Source: `docs/prd/PRD-r2-to-pg.md:1230`, `docs/prd/PRD-r2-to-pg.md:2544`
  - Acceptance: Identifiers, ownership/scope, status, timestamps, versions, pagination keys, and due-claim fields are typed/indexed columns; JSONB remains limited to flexible payloads/snapshots.
  - Evidence required: Migration review, query/index review, and tests proving indexed list/page/due queries.
  - Status: pending
  - Evidence collected:

- Q4: Append-only/high-volume paths avoid object-style rewrites and hot-row churn
  - Source: `docs/prd/PRD-r2-to-pg.md:23`, `docs/prd/PRD-r2-to-pg.md:715`, `docs/prd/PRD-r2-to-pg.md:2560`
  - Acceptance: Task events and audit are append-only rows; progress uses separate coalesced/debounced latest snapshot; large event batches do not rewrite chunks or full task blobs.
  - Evidence required: Event/audit/progress tests, performance smoke, and focused diff review.
  - Status: pending
  - Evidence collected:

- Q5: SQL concurrency semantics replace ETags deliberately
  - Source: `docs/prd/PRD-r2-to-pg.md:167`, `docs/prd/PRD-r2-to-pg.md:2523`
  - Acceptance: Multi-record updates, reminders, audit version allocation, duplicate guards, and mutable records have explicit transactions, row locks, version checks, or documented last-write-wins behavior.
  - Evidence required: Concurrency tests, transaction-boundary notes, and conflict/error tests.
  - Status: pending
  - Evidence collected:

- V1: Core Rust validation passes for affected profiles
  - Source: `AGENTS.md`
  - Acceptance: Relevant `cargo fmt`, `cargo check`, `cargo clippy`, and `cargo test` commands pass for touched crates/profiles at each checkpoint.
  - Evidence required: Command output summaries recorded in Progress Log and Final Verification.
  - Status: pending
  - Evidence collected:

- V2: SQL integration and migration validation pass against clean Postgres
  - Source: `docs/prd/PRD-r2-to-pg.md:1206`, `docs/prd/PRD-r2-to-pg.md:2345`
  - Acceptance: Migrations apply to empty Postgres; SQL storage and web persistence contract tests pass without R2 env vars.
  - Evidence required: Postgres test DB command output, migration output, SQL integration test output, CI evidence.
  - Status: pending
  - Evidence collected:

- V3: Static dependency/reference guards prove R2/AWS runtime removal
  - Source: `docs/prd/PRD-r2-to-pg.md:2232`, `docs/prd/PRD-r2-to-pg.md:2639`
  - Acceptance: `cargo tree` production profiles have no AWS SDK/S3 crates; runtime grep/static guard has no disallowed R2/S3/AWS hits; historical docs are separately classified.
  - Evidence required: `cargo tree`, `rg`, and static guard outputs.
  - Status: pending
  - Evidence collected:

- V4: Runtime smoke validates durable restart behavior
  - Source: `docs/prd/PRD-r2-to-pg.md:2306`, `docs/prd/PRD-r2-to-pg.md:2390`
  - Acceptance: Local Postgres web console/auth/session/task/event restart smoke passes; Telegram startup/storage health check works; Supabase compatibility checklist is completed or explicitly deferred by user.
  - Evidence required: Smoke command/output summaries, screenshots/log snippets if relevant, and checklist results.
  - Status: pending
  - Evidence collected:

- N1: Old R2 data migration remains excluded
  - Source: `docs/prd/PRD-r2-to-pg.md:29`
  - Must preserve: No migration, reader, dual-write, importer, backfill, or object-key scan story is added.
  - Evidence required: Diff review and grep guard output.
  - Status: pending
  - Evidence collected:

- N2: SQLite remains absent
  - Source: `docs/prd/PRD-r2-to-pg.md:37`, `docs/prd/PRD-r2-to-pg.md:2314`
  - Must preserve: No SQLite dependency, feature, migration, tests, docs, or acceptance criteria.
  - Evidence required: Cargo/dependency grep and docs diff review.
  - Status: pending
  - Evidence collected:

- N3: R2 is not retained as a fallback or feature flag after removal
  - Source: `docs/prd/PRD-r2-to-pg.md:35`, `docs/prd/PRD-r2-to-pg.md:1183`
  - Must preserve: No R2 blob fallback, wiki fallback, memory fallback, emergency compatibility layer, or unnecessary R2 feature flags remain.
  - Evidence required: Static grep/cargo tree/docs review after Phase 6.
  - Status: pending
  - Evidence collected:

## Implementation Plan

1. Phase 0 — deletion map and SQL entity map
   - Audit IDs: G1, Q1, Q2, N1, N2
   - Expected changes: Re-run targeted searches; classify all R2/S3/AWS references; produce final deletion list and SQL entity mapping; update this goal/PRD only with evidence-backed findings.
   - Validation: Targeted `rg` searches from the PRD, `git diff --check`, and focused review that no implementation or SQLite work was added.
   - Exit condition: Deletion map is complete enough for implementation checkpoints and committed separately.

2. Phase 1 — SQLx/Postgres foundation
   - Audit IDs: G2, G8, Q2, Q3, V1, V2
   - Expected changes: Add SQLx Postgres dependency/config, shared pool builder/handle, storage capability/profile entries, migration stream, DB health check, local Postgres/CI strategy, and initial docs/env updates.
   - Validation: `cargo fmt`; focused `cargo check` for affected crates/profiles; capability JSON commands for affected profiles; migration/health test against clean Postgres; `git diff --check`.
   - Exit condition: SQLx foundation exists and is verified, but broad storage business logic is still not ported.

3. Phase 2 — web persistence on SQLx
   - Audit IDs: G3, Q3, Q4, V1, V2, V4
   - Expected changes: Add web auth/session/task/event/progress/file migrations and `SqlxWebUiStore`; wire production web startup to SQLx durable store; keep in-memory only for explicit dev/test use.
   - Validation: Web persistence SQL contract tests; web startup tests; append-only event pagination tests; restart reconciliation tests; focused web check/clippy/test commands; local Postgres web restart smoke.
   - Exit condition: Web durable state works through SQLx and R2 web persistence is out of production paths.

4. Phase 3 — core durable state on SQLx
   - Audit IDs: G4, Q3, Q5, V1, V2, V4
   - Expected changes: Implement SQL-backed `StorageProvider` for user config/state, contexts, memory/flows, profiles, topic records, infra/bindings, and secrets.
   - Validation: Trait-level SQL integration tests; flow checkpoint tests; context update tests; control-plane CRUD/version tests; Telegram/web startup storage checks.
   - Exit condition: Core durable state for Telegram and web session manager no longer depends on R2 production paths.

5. Phase 4 — reminders and audit on SQLx
   - Audit IDs: G5, Q4, Q5, V1, V2
   - Expected changes: Implement reminders table/status transitions/due claiming/leases and append-only audit version allocation/pages.
   - Validation: Reminder CRUD/status tests; concurrent due-claim tests; lease expiry tests; audit append/page/concurrency tests; query/index review.
   - Exit condition: Reminder scheduler and manager audit work without R2 semantics.

6. Phase 5 — wiki memory on SQLx
   - Audit IDs: G6, Q3, N3, V1, V2
   - Expected changes: Implement SQL wiki page storage, typed wiki address mapping, context delete, content limits/retention, and update wiki docs/comments.
   - Validation: Wiki SQL integration tests; content-size tests; runtime grep for object-key/prefix usage; docs diff review.
   - Exit condition: Wiki memory runtime uses SQL rows and no object storage path.

7. Phase 6 — physical R2 removal
   - Audit IDs: G7, G8, Q1, N1, N2, N3, V1, V3
   - Expected changes: Delete R2 modules/object-store web persistence, remove AWS SDK dependencies and R2 features/env vars/docs/current setup paths, update snapshots/static guards, and regenerate `Cargo.lock`.
   - Validation: Affected-profile `cargo check`/`cargo clippy`; `cargo tree` deny review; static grep guard; capability snapshot tests; CI/deploy/env/docs review.
   - Exit condition: Runtime durable architecture has no R2/S3/AWS storage dependency or setup requirement.

8. Phase 7 — hardening and final verification
   - Audit IDs: G9, Q3, Q4, Q5, V1, V2, V3, V4
   - Expected changes: Add retention/cleanup, indexes, failure-mode tests, performance smokes, pool tuning, Supabase compatibility checklist, and final docs.
   - Validation: Full validation contract, large task event smoke, cleanup/failure-mode tests, local Postgres restart smoke, Supabase checklist, final audit.
   - Exit condition: Every Completion Audit item is verified or explicitly dropped by user, and the goal is marked complete.

## Validation Contract

- Static checks: `cargo fmt`; `cargo fmt --check`; focused `cargo check`/`cargo clippy` for touched crates; profile checks from `AGENTS.md` for `profile-embedded-opencode-local`, `profile-web-embedded-opencode-local`, `profile-full`, and any profile touched by feature changes; `git diff --check`.
- Tests: `cargo test --workspace` when feasible; focused core storage SQL tests; web persistence contract tests; reminder/audit/wiki SQL integration tests; snapshot/static guard tests after capability changes.
- SQL/runtime verification: Apply migrations to a clean local Postgres database; run SQL health/migration tests; run local Postgres web restart smoke; verify Telegram durable startup health path; complete Supabase compatibility checklist without requiring a real Supabase project in CI.
- Artifact verification: Review migrations, Cargo feature/dependency diffs, `Cargo.lock`, capability outputs/snapshots, CI workflow, `.env.example`, deploy docs, README/current docs, and static grep/cargo-tree outputs.
- Done when: All Completion Audit items are `verified`, all blockers are resolved or explicitly dropped by user, and Final Verification is filled with current evidence.

## Decisions

- 2026-06-05: Created a dedicated goal document because no existing active R2-to-Postgres goal file was found under `docs/goals/`.
- 2026-06-05: The long-running goal targets the future full migration described by the PRD phases, while this commit only creates the goal contract and does not start implementation.
- 2026-06-05: First implementation checkpoint is Phase 0 deletion/entity mapping, not SQLx coding or R2 deletion, because the PRD explicitly warns not to delete R2 before SQL paths have passing coverage.
- 2026-06-05: Keep one top-level migration stream by default because core and web tables share the same database and may need foreign keys/order guarantees.
- 2026-06-05: Preserve `StorageProvider` and `WebUiStore` as seams unless implementation evidence shows a simpler local change is required.

## Progress Log

- 2026-06-05: Goal drafted from PRD.
  - Changed: Created `docs/goals/2026-06-05-r2-to-postgres-storage.md` with objective, scope, audit ledger, phased checkpoints, validation contract, blockers, and first step.
  - Evidence: Source PRD read through end; existing `docs/goals/` convention inspected; README and CI R2 references reviewed; no existing matching goal found.
  - Commands: `git status --short --branch`; `git diff --check`; goal-doc read/diff review.
  - Audit IDs updated: none; implementation not started.
  - Next: Phase 0 — deletion map and SQL entity map.

## Risks and Blockers

- Postgres-only task file blobs can grow WAL/backups quickly.
  - Impact: Large uploads/artifacts can make Supabase/local backups expensive or unstable.
  - Evidence: PRD calls out current 200 MB web upload default as risky for Postgres-only storage.
  - Mitigation or requested decision: Enforce strict configurable max size and retention; ask user to approve the limit before Phase 2 completion.
  - Audit IDs affected: G3, G9, B1

- Retention policy is unresolved for high-growth rows.
  - Impact: Task events, task files, wiki raw archives, old sessions, and possibly audit can grow unbounded.
  - Evidence: PRD lists retention open questions for task events and wiki/task artifacts.
  - Mitigation or requested decision: Add retention fields and bounded cleanup; obtain policy before final hardening.
  - Audit IDs affected: G3, G6, G9, B2

- Supabase pool/endpoint behavior can differ from local Postgres.
  - Impact: Production can hit connection limits even when CI passes.
  - Evidence: PRD notes Supabase connection limits and recommends conservative pools.
  - Mitigation or requested decision: Use one shared pool, conservative defaults, and a Supabase smoke checklist.
  - Audit IDs affected: G2, G8, G9, B4

- Historical docs can cause noisy final R2 grep results.
  - Impact: A naive grep may fail despite runtime R2 removal.
  - Evidence: Existing implemented goals and PRDs mention R2 historically.
  - Mitigation or requested decision: Phase 0 and Phase 6 must define allowed historical paths and separate runtime/current-doc grep guards.
  - Audit IDs affected: G1, G7, V3

- SQL conflict semantics are not fully specified for every old ETag path.
  - Impact: Mutable records could silently change behavior if translated naively.
  - Evidence: PRD calls out ETag-to-SQL transaction/version translation as a risk.
  - Mitigation or requested decision: Write transaction/concurrency tests per operation and ask for user decision only where behavior remains underspecified.
  - Audit IDs affected: G4, G5, Q5, B5

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
