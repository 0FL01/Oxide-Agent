# Goal: R2 to SQLx/Postgres Durable Storage

Date started: 2026-06-05
Status: active
Codex goal: `/goal Implement docs/goals/2026-06-05-r2-to-postgres-storage.md until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update this document after each meaningful verification, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.`
Source spec: `docs/prd/PRD-r2-to-pg.md`
Goal doc owner: Codex
Last updated: 2026-06-06 09:44 +03

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
  - Low-risk assumption or fallback: Phase 2 starts with a strict configurable limit and rejects larger files until the user approves another non-R2 blob architecture.
  - User/external action needed: Approve or change the default maximum file size before Phase 7 hardening.

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
- Original PRD context: production durable storage was gated by `storage-s3-r2` and AWS SDK dependencies in production-like profiles (`docs/prd/PRD-r2-to-pg.md:45`).
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
  - Status: verified
  - Evidence collected: Phase 0 map in `## Phase 0 Deletion and SQL Entity Map`; targeted `rg` summary found 75 R2/S3/AWS reference files and classified runtime, tests, current docs, historical docs, CI/env/profile, and false-positive matches; SQL entity map covers core, web, reminders, audit, wiki, control-plane, and web UI object namespaces; SQLite search found no storage/backend/dependency work.

- G2: SQLx/Postgres foundation is added without broad business-logic porting
  - Source: `docs/prd/PRD-r2-to-pg.md:1966`
  - Requirement: Add SQLx/Postgres dependency/config, shared `PgPool`, DB health check, migration strategy, `storage/sqlx` capability/profile entries, and local Postgres CI/test strategy.
  - Acceptance: SQLx foundation builds, connects to Postgres, verifies health/migrations, appears in capability/profile outputs, and introduces no SQLite dependency.
  - Evidence required: Focused Cargo/profile diffs, migration files, capability command output, `cargo check` for affected profiles, DB health test, and CI Postgres evidence.
  - Status: verified
  - Evidence collected: Phase 1 added direct `sqlx-core` + `sqlx-postgres` optional dependencies, `storage-sqlx` feature/profile wiring, `storage/sqlx` capability/config schema, `SqlxStorageConfig`, shared `PgPool` foundation, health query, runtime migration runner, CI Postgres service/smoke step, `.env.example`/deploy/README DB notes, and `migrations/0001_storage_health.sql`. Validation: `cargo fmt --all -- --check`; `cargo check -p oxide-agent-core --no-default-features --features storage-sqlx`; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`; `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`; `cargo check --workspace --no-default-features --features profile-host-bwrap`; `cargo check --workspace --no-default-features --features profile-full`; `cargo clippy -p oxide-agent-core --no-default-features --features storage-sqlx -- -D warnings`; `cargo clippy --workspace --no-default-features --features profile-embedded-opencode-local -- -D warnings`; snapshot tests for all profile/all-features modular registry manifests; Postgres smoke `OXIDE_DATABASE_TEST_URL=postgres://oxide_agent:oxide_agent@localhost:55432/oxide_agent_test cargo test -p oxide-agent-core --no-default-features --features storage-sqlx sqlx_storage_connects_and_runs_migrations_when_test_url_is_set -- --nocapture`; SQLite guard found only the pre-existing sandbox hint `crates/oxide-agent-core/src/agent/preprocessor.rs:432`; `cargo tree -p oxide-agent-core --no-default-features --features storage-sqlx -i sqlx-sqlite` reported no matching package; `git diff --check` passed.

- G3: Web persistence uses SQLx/Postgres for durable web state
  - Source: `docs/prd/PRD-r2-to-pg.md:2014`
  - Requirement: Move web users/auth/sessions/tasks/task events/progress/task files from R2 object store to SQLx.
  - Acceptance: Production web startup uses `SqlxWebUiStore`; task events are append-only rows with unique `(user_id, session_id, task_id, seq)`; event listing uses indexed pagination; restart reconciliation uses SQL; file blobs are bounded Postgres rows or rejected by policy.
  - Evidence required: SQL migrations, `WebUiStore` SQL contract tests, web startup tests, append/list event tests, reconciliation tests, file size tests, and web restart smoke.
  - Status: verified
  - Evidence collected: Phase 2 added `migrations/0002_web_persistence.sql`, `SqlxWebUiStore`, SQLx web startup builder/selector wiring, append-only `web_task_events` rows with unique `(user_id, session_id, task_id, seq)`, indexed event/page queries, SQL unfinished-task reconciliation, separate progress snapshot rows, bounded Postgres file blob rows with configurable rejection, SQL contract tests, web startup tests, and DB-backed smoke coverage.

- G4: Core durable state uses SQLx/Postgres through `StorageProvider`
  - Source: `docs/prd/PRD-r2-to-pg.md:2065`
  - Requirement: Move user config/state, contexts, agent memory/flows, profiles, topic context, topic AGENTS.md, topic infra, topic bindings, and secrets to SQL-backed `StorageProvider` methods.
  - Acceptance: Telegram and web session manager can use SQL-backed `StorageProvider`; updating one context does not rewrite full user config; flow memory persists/reloads through SQL; control-plane CRUD uses transactions/version columns; R2 user/memory/control-plane modules are unused by production paths.
  - Evidence required: Trait-level SQL integration tests, context update tests, flow checkpoint tests, control-plane version/conflict tests, secret redaction review, and startup checks.
  - Status: verified
  - Evidence collected: Phase 3 added `migrations/0003_core_storage.sql`, implemented SQL-backed `StorageProvider` business methods in `SqlxStorage` for user config/state, per-context rows, scoped agent memory/flows, agent profiles, topic context, topic `AGENTS.md`, topic infra, topic bindings, and private secrets. SQL integration tests cover context updates without unchanged context-row version churn, scoped global/context/flow memory roundtrips, flow metadata persistence/deletion, control-plane version increments, duplicate topic prompt rejection, infra/binding enum roundtrips, and secret put/get/delete. Startup/build checks prove SQLx can be the primary core store and Telegram can compile with SQLx-only durable storage.

- G5: Reminders and audit are SQL-native
  - Source: `docs/prd/PRD-r2-to-pg.md:2113`
  - Requirement: Replace R2 reminder jobs and audit JSON-array rewrites with SQL queue rows and append-only audit rows.
  - Acceptance: Reminder claim uses transaction-safe Postgres row locks/leases; concurrent claimers cannot double-claim; audit append allocates stable per-user versions transactionally; audit pagination is indexed and stable.
  - Evidence required: Reminder CRUD/status tests, concurrent claim tests, lease expiry tests, audit append/page tests, query/index review.
  - Status: verified
  - Evidence collected: Phase 4 added `migrations/0004_reminders_audit.sql`, SQL-backed reminder job CRUD/list/due-claim/status transitions, transaction-safe lease claiming, append-only audit rows, transactional per-user audit version allocation, indexed audit pagination, and DB-backed tests for reminder status roundtrips, single-winner concurrent claims, lease expiry reclaim, and audit append/page behavior.

- G6: Wiki memory uses SQLx/Postgres rows, not object keys
  - Source: `docs/prd/PRD-r2-to-pg.md:2156`
  - Requirement: Store wiki pages and related memory text in SQL rows with deterministic scope/path metadata and content limits.
  - Acceptance: Wiki read/write/delete/list/context-delete work through SQL-backed storage; no runtime wiki path uses R2/S3 object storage or prefix delete; content limits and raw archive retention are enforced or documented.
  - Evidence required: Wiki SQL integration tests, internal API/diff review, docs update, content-size tests, and `rg` guard output for wiki runtime paths.
  - Status: verified
  - Evidence collected: Phase 5 added `migrations/0005_wiki_memory.sql`, SQL-backed `load_wiki_text`/`save_wiki_text`/`delete_wiki_text`/`delete_wiki_context` in `SqlxStorage`, deterministic logical-key parsing into typed `wiki_pages` scope/path metadata, content-byte limits for normal/raw/core/global and inbox rows, SQL context delete by derived `context_id`, and wiki docs/comment updates. DB-backed SQLx tests cover global/context/page/inbox/raw roundtrips, metadata rows, no-op same-content version behavior, changed-content version increments, page delete, context delete preserving global rows, and oversized inbox rejection.

- G7: R2/S3/AWS durable storage code, features, env vars, docs, and dependencies are removed from runtime paths
  - Source: `docs/prd/PRD-r2-to-pg.md:2201`
  - Requirement: Physically delete R2 storage modules/object-store web persistence and remove AWS SDK dependencies, `storage-s3-r2`, `storage/r2`, `OXIDE_R2_*`, and R2-specific tests/docs from current runtime/setup paths.
  - Acceptance: Production-like profiles build without AWS SDK/S3 crates; binary `required-features` no longer mention R2; runtime grep/static guard finds no R2/S3/AWS durable storage references outside explicitly historical docs.
  - Evidence required: Cargo/profile diffs, `cargo tree` output, static grep guard output, capability snapshot updates, CI/deploy/env docs diffs, and full affected-profile checks.
  - Status: verified
  - Evidence collected: Phase 6 physically removed core R2 storage modules, web object-store persistence, R2/AWS credential validation tests, `storage-s3-r2` feature wiring, `storage/r2` profile modules, AWS SDK dependencies, R2 env docs, and current runtime/setup R2 paths. Production-like profile checks passed without AWS/S3 crates, modular registry snapshots were regenerated without `storage/r2`, capability JSON outputs omitted `storage/r2|storage-s3-r2`, the targeted no-R2 static guard returned no disallowed hits, and the AWS cargo-tree deny loop found no AWS SDK/S3 packages for core, Telegram, or web production-like profiles.

- G8: Fresh local Postgres and Supabase setup is documented and wired
  - Source: `docs/prd/PRD-r2-to-pg.md:1133`, `docs/prd/PRD-r2-to-pg.md:1161`, `docs/prd/PRD-r2-to-pg.md:1845`
  - Requirement: Provide current setup docs/env/config for local PostgreSQL and Supabase Postgres fresh setup without R2.
  - Acceptance: `.env.example`, README/current docs, deploy docs, CI/deployment env, and health checks use DB vars; docs state old R2 data is intentionally ignored; no current setup path requires object-storage credentials.
  - Evidence required: Docs/env diff review, local setup smoke or documented command sequence, Supabase compatibility checklist, and CI workflow review.
  - Status: pending
  - Evidence collected: Phase 1 added initial DB vars to `.env.example`, deploy docs, README, and CI Postgres service/smoke strategy. Phase 6 updated `.env.example`, README, deploy docs, CI/deploy env, profiles, and current setup paths to use SQLx/Postgres and `OXIDE_DATABASE_URL` without `OXIDE_R2_*`, `storage/r2`, or `storage-s3-r2`; docs state old object-storage data is intentionally ignored. Full Supabase compatibility and final hardening remain pending Phase 7.

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
  - Evidence collected: Phase 0 changed only the goal document and explicitly preserves no migration/import/backfill/dual-write/R2 fallback; Phase 3 added direct SQL tables and SQL-backed core methods without importer, backfill, R2 reader, R2 object scan, or dual-write code. Phase 4 added fresh SQL reminder/audit tables and direct SQLx methods only, with no R2 reader/importer/dual-write path. Phase 5 added fresh SQL wiki rows and parses only current deterministic logical wiki keys; it does not read, import, scan, or dual-write old R2 objects. Phase 6 deleted the R2 runtime modules and web object persistence instead of adding compatibility readers, and the targeted no-R2 guard returned no disallowed runtime/setup hits.

- Q2: Keep the solution simple and storage-focused
  - Source: `AGENTS.md`, `docs/prd/PRD-r2-to-pg.md:1021`
  - Acceptance: No SQLite backend, Supabase Storage bucket, new queue/cache/service, sharding, HA, or broad framework abstraction is added.
  - Evidence required: Cargo diffs, compose/deploy diffs, dependency review, and implementation diff review.
  - Status: pending
  - Evidence collected: Phase 0 changed only the goal document; Phase 1 added only Postgres SQLx foundation dependencies/config, one shared pool, one migration stream, a CI Postgres service, and a smoke test. Phase 3 reused the same SQLx/Postgres pool and storage facade with direct queries. Phase 4 kept reminders as ordinary Postgres rows with status/lease columns and audit as append-only rows. Phase 5 stores wiki text as ordinary Postgres rows behind the existing storage facade. Phase 6 removed AWS/R2 dependencies and feature flags rather than adding another backend or service; SQLx/Postgres remains the only durable runtime storage backend.

- Q3: Data model uses typed columns for queryable fields and JSONB only where justified
  - Source: `docs/prd/PRD-r2-to-pg.md:1230`, `docs/prd/PRD-r2-to-pg.md:2544`
  - Acceptance: Identifiers, ownership/scope, status, timestamps, versions, pagination keys, and due-claim fields are typed/indexed columns; JSONB remains limited to flexible payloads/snapshots.
  - Evidence required: Migration review, query/index review, and tests proving indexed list/page/due queries.
  - Status: pending
  - Evidence collected: Phase 2 web migration uses typed identifiers, ownership, auth/login/session/task status, timestamps, version/order keys, event seq, file size/content type, and indexes for auth/session/task/event/file queries; JSONB is limited to model selections, attachment/event/progress payloads, task-file metadata, and flexible final session snapshots. Phase 3 core migration uses typed user/context/flow/profile/topic/binding/secret ownership, versions, timestamps, enum text columns, arrays, and indexes; JSONB is limited to agent memory snapshots and agent profile payloads. Phase 4 reminder/audit migration uses typed reminder status/schedule/thread kind, due/lease/run/version columns, per-user audit versions, and audit page indexes; JSONB is limited to audit payloads. Phase 5 wiki migration uses typed storage prefix, scope kind, context id, item kind, path, content byte count, retention marker, version, timestamps, and context/prefix indexes; wiki content is plain `TEXT` and uses no JSONB.

- Q4: Append-only/high-volume paths avoid object-style rewrites and hot-row churn
  - Source: `docs/prd/PRD-r2-to-pg.md:23`, `docs/prd/PRD-r2-to-pg.md:715`, `docs/prd/PRD-r2-to-pg.md:2560`
  - Acceptance: Task events and audit are append-only rows; progress uses separate coalesced/debounced latest snapshot; large event batches do not rewrite chunks or full task blobs.
  - Evidence required: Event/audit/progress tests, performance smoke, and focused diff review.
  - Status: pending
  - Evidence collected: Phase 2 task events are append-only SQL rows with duplicate seq conflict-ignore, paged by indexed `(user_id, session_id, task_id, seq)` order; latest progress is stored in separate `web_task_progress` rows instead of rewriting event chunks. Phase 4 audit events are append-only SQL rows keyed by `(user_id, version)` and paged by indexed descending version order instead of rewriting a per-user JSON array.

- Q5: SQL concurrency semantics replace ETags deliberately
  - Source: `docs/prd/PRD-r2-to-pg.md:167`, `docs/prd/PRD-r2-to-pg.md:2523`
  - Acceptance: Multi-record updates, reminders, audit version allocation, duplicate guards, and mutable records have explicit transactions, row locks, version checks, or documented last-write-wins behavior.
  - Evidence required: Concurrency tests, transaction-boundary notes, and conflict/error tests.
  - Status: pending
  - Evidence collected: Phase 3 core upserts use transactions with `SELECT ... FOR UPDATE`, transaction-scoped advisory locks for mutable record families and cross-table topic prompt duplicate guards, version columns built through existing storage builders, and SQL tests for version increments plus duplicate prompt conflicts. Phase 4 reminder mutations use row-locking updates/`SELECT ... FOR UPDATE`, explicit lease predicates, version increments, a single-winner concurrent claim test, and lease-expiry reclaim coverage; audit append locks `audit_stream_versions` with `FOR UPDATE` and allocates per-user versions transactionally.

- V1: Core Rust validation passes for affected profiles
  - Source: `AGENTS.md`
  - Acceptance: Relevant `cargo fmt`, `cargo check`, `cargo clippy`, and `cargo test` commands pass for touched crates/profiles at each checkpoint.
  - Evidence required: Command output summaries recorded in Progress Log and Final Verification.
  - Status: pending
  - Evidence collected: Phase 1 validation passed: `cargo fmt --all -- --check`; `cargo check -p oxide-agent-core --no-default-features --features storage-sqlx`; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`; `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`; `cargo check --workspace --no-default-features --features profile-host-bwrap`; `cargo check --workspace --no-default-features --features profile-full`; `cargo clippy -p oxide-agent-core --no-default-features --features storage-sqlx -- -D warnings`; `cargo clippy --workspace --no-default-features --features profile-embedded-opencode-local -- -D warnings`; modular registry snapshot tests passed for `profile-lite`, `profile-search-only`, `profile-no-sandbox`, `profile-media-enabled`, `profile-host-bwrap`, `profile-full`, `profile-embedded-opencode-local`, `profile-web-embedded-opencode-local`, and `all-features`. Phase 2 web validation passed: `cargo fmt --all -- --check`; `cargo check -p oxide-agent-transport-web --no-default-features --features storage-sqlx`; `cargo clippy -p oxide-agent-transport-web --no-default-features --features storage-sqlx -- -D warnings`; `cargo check -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`; SQLx-focused and full web library tests with `storage-sqlx`. Phase 3 core/Telegram validation passed: `cargo fmt --all -- --check`; `cargo check -p oxide-agent-core --no-default-features --features storage-sqlx`; `cargo check -p oxide-agent-transport-telegram --no-default-features --features storage-sqlx`; `cargo check -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features transport-telegram,storage-sqlx`; `cargo clippy -p oxide-agent-core --no-default-features --features storage-sqlx -- -D warnings`; `cargo clippy -p oxide-agent-transport-telegram --no-default-features --features storage-sqlx -- -D warnings`; `cargo clippy -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features transport-telegram,storage-sqlx -- -D warnings`; `cargo check -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features profile-embedded-opencode-local`; `cargo check -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`.

  - Phase 4 validation passed: `cargo fmt --all -- --check`; `cargo check -p oxide-agent-core --no-default-features --features storage-sqlx`; `cargo check -p oxide-agent-transport-telegram --no-default-features --features storage-sqlx`; `cargo check -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features transport-telegram,storage-sqlx`; `cargo clippy -p oxide-agent-core --no-default-features --features storage-sqlx -- -D warnings`; `cargo clippy -p oxide-agent-transport-telegram --no-default-features --features storage-sqlx -- -D warnings`; profile checks for embedded Telegram and web; focused SQLx DB-backed core tests with 10 passed.

  - Phase 5 validation passed: `cargo fmt --all -- --check`; `cargo check -p oxide-agent-core --no-default-features --features storage-sqlx`; `cargo check -p oxide-agent-transport-telegram --no-default-features --features storage-sqlx`; `cargo check -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features profile-embedded-opencode-local`; `cargo check -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`; `cargo clippy -p oxide-agent-core --no-default-features --features storage-sqlx -- -D warnings`; focused SQLx DB-backed core tests with 11 passed.

  - Phase 6 validation passed: `cargo fmt --all -- --check`; `cargo check -p oxide-agent-core --no-default-features --features storage-sqlx`; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`; `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`; `cargo check --workspace --no-default-features --features profile-host-bwrap`; `cargo check --workspace --no-default-features --features profile-full`; `cargo clippy --workspace --no-default-features --features profile-embedded-opencode-local -- -D warnings`; `cargo check -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features transport-telegram,storage-sqlx`; modular registry snapshot tests for all profile/all-features outputs; `cargo test -p oxide-agent-core --test tool_runtime_static_guards --no-default-features --features storage-sqlx` with 19 passed.

- V2: SQL integration and migration validation pass against clean Postgres
  - Source: `docs/prd/PRD-r2-to-pg.md:1206`, `docs/prd/PRD-r2-to-pg.md:2345`
  - Acceptance: Migrations apply to empty Postgres; SQL storage and web persistence contract tests pass without R2 env vars.
  - Evidence required: Postgres test DB command output, migration output, SQL integration test output, CI evidence.
  - Status: pending
  - Evidence collected: Phase 1 smoke passed against a temporary clean `postgres:16` container on port `55432`: `OXIDE_DATABASE_TEST_URL=postgres://oxide_agent:oxide_agent@localhost:55432/oxide_agent_test cargo test -p oxide-agent-core --no-default-features --features storage-sqlx sqlx_storage_connects_and_runs_migrations_when_test_url_is_set -- --nocapture`. The smoke creates a shared pool, runs `migrations/0001_storage_health.sql`, and executes the SQL health query. Phase 2 applied `migrations/0002_web_persistence.sql` through the SQLx-backed app-state startup smoke and passed web SQL contract tests against a clean temporary Postgres container: `cargo test -p oxide-agent-transport-web --no-default-features --features storage-sqlx sqlx_ -- --nocapture` and `cargo test -p oxide-agent-transport-web --no-default-features --features storage-sqlx --lib` with `OXIDE_DATABASE_TEST_URL` set. Phase 3 applied `migrations/0003_core_storage.sql` against a clean temporary `postgres:16` container and passed focused SQLx core contract tests: `OXIDE_DATABASE_TEST_URL=postgres://oxide_agent:oxide_agent@localhost:55432/oxide_agent_test cargo test -p oxide-agent-core --no-default-features --features storage-sqlx sqlx_ -- --nocapture`.

  - Phase 4 applied `migrations/0004_reminders_audit.sql` to a clean temporary `postgres:16` container and passed focused SQLx core contract tests: `OXIDE_DATABASE_TEST_URL=postgres://oxide_agent:oxide_agent@localhost:55432/oxide_agent_test cargo test -p oxide-agent-core --no-default-features --features storage-sqlx sqlx_ -- --nocapture` with 10 SQLx tests passing, including reminder/audit coverage.

  - Phase 5 applied `migrations/0005_wiki_memory.sql` to a clean temporary `postgres:16` container and passed focused SQLx core contract tests: `OXIDE_DATABASE_TEST_URL=postgres://oxide_agent:oxide_agent@localhost:55432/oxide_agent_test cargo test -p oxide-agent-core --no-default-features --features storage-sqlx sqlx_ -- --nocapture` with 11 SQLx tests passing, including wiki memory row coverage.

- V3: Static dependency/reference guards prove R2/AWS runtime removal
  - Source: `docs/prd/PRD-r2-to-pg.md:2232`, `docs/prd/PRD-r2-to-pg.md:2639`
  - Acceptance: `cargo tree` production profiles have no AWS SDK/S3 crates; runtime grep/static guard has no disallowed R2/S3/AWS hits; historical docs are separately classified.
  - Evidence required: `cargo tree`, `rg`, and static guard outputs.
  - Status: verified
  - Evidence collected: Phase 6 AWS cargo-tree deny loop found no `aws-sdk-s3`, `aws-config`, `aws-credential-types`, `aws-types`, `aws-runtime`, `aws-sigv4`, `aws-smithy-runtime`, or `aws-smithy-types` packages in core `profile-full`, Telegram `profile-embedded-opencode-local`, or web `profile-web-embedded-opencode-local`. Targeted no-R2 guard returned no disallowed hits for `R2Storage|R2StorageConfig|R2WebUiStore|aws_sdk_s3|aws_config|aws_credential|aws_types|storage-s3-r2|storage/r2|OXIDE_R2|OXIDE_WEB_STORE=r2|list_keys_under_prefix|delete_prefix` across crates, Cargo files, profiles, env, CI, README, deploy docs, and AGENTS.md. Capability JSON for Telegram and web had no `storage/r2|storage-s3-r2` hits, modular registry snapshots were regenerated without R2 modules/features, and broad current-doc/setup grep had only allowed non-durable false positives (`OXIDE_WEB_STORE` sqlx/postgres references and Silero `R24000`).

- V4: Runtime smoke validates durable restart behavior
  - Source: `docs/prd/PRD-r2-to-pg.md:2306`, `docs/prd/PRD-r2-to-pg.md:2390`
  - Acceptance: Local Postgres web console/auth/session/task/event restart smoke passes; Telegram startup/storage health check works; Supabase compatibility checklist is completed or explicitly deferred by user.
  - Evidence required: Smoke command/output summaries, screenshots/log snippets if relevant, and checklist results.
  - Status: pending
  - Evidence collected: Phase 2 web SQL startup smoke passed with `OXIDE_DATABASE_URL`, `OXIDE_DATABASE_MIGRATE_ON_STARTUP=true`, and `OXIDE_DATABASE_MIGRATIONS_DIR=migrations`, asserting `WebStoreKind::Sqlx`; SQL unfinished-task reconciliation test verifies queued/running web tasks become interrupted and session active task ids are cleared after startup reconciliation. Phase 3 verified SQLx-only Telegram durable startup/build wiring with `cargo check -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features transport-telegram,storage-sqlx`, and production-like Telegram/web profile checks still build with SQLx selected when configured. Full local web-console restart and Supabase checklist remain pending.

  - Phase 4 rechecked SQLx-only Telegram build/profile paths and DB-backed reminder/audit storage through Postgres. Full local restart and Supabase checklist remain pending.

- N1: Old R2 data migration remains excluded
  - Source: `docs/prd/PRD-r2-to-pg.md:29`
  - Must preserve: No migration, reader, dual-write, importer, backfill, or object-key scan story is added.
  - Evidence required: Diff review and grep guard output.
  - Status: pending
  - Evidence collected: Phase 0 map records old R2 data as intentionally out of scope and adds no importer, reader, dual-write, backfill, compatibility path, or object-key scan implementation. Phase 3 core SQLx implementation adds fresh SQL rows only and does not read, import, scan, or dual-write old R2 objects. Phase 4 reminder/audit SQLx implementation likewise adds fresh SQL rows and direct SQL mutations only, with no R2 reader/importer/dual-write path. Phase 5 wiki SQLx implementation stores fresh logical-key rows and does not scan/import/read old R2 wiki objects. Phase 6 deletes R2 runtime storage and object web persistence outright; the targeted no-R2 guard found no old object-key scan/importer/fallback path in current runtime/setup files.

- N2: SQLite remains absent
  - Source: `docs/prd/PRD-r2-to-pg.md:37`, `docs/prd/PRD-r2-to-pg.md:2314`
  - Must preserve: No SQLite dependency, feature, migration, tests, docs, or acceptance criteria.
  - Evidence required: Cargo/dependency grep and docs diff review.
  - Status: pending
  - Evidence collected: Phase 0 search `rg -n -i --hidden --glob '!target/**' --glob '!.git/**' -e 'sqlite|rusqlite|sqlx::sqlite|Sqlite' Cargo.toml crates config .github .env.example` found only `crates/oxide-agent-core/src/agent/preprocessor.rs:432`, a sandbox/database hint. Phase 1 intentionally avoided the top-level `sqlx` crate after it pulled SQLite dependencies; direct `sqlx-core` + `sqlx-postgres` left `Cargo.lock` without `sqlx-sqlite`, `libsqlite3-sys`, or `rusqlite`. Phase 2 web SQLx dependency guard again matched only the pre-existing sandbox hint, and `cargo tree -p oxide-agent-transport-web --no-default-features --features storage-sqlx -i sqlx-sqlite` reported no matching package. Phase 3 guard again matched only the pre-existing sandbox hint, and `cargo tree -p oxide-agent-core --no-default-features --features storage-sqlx -i sqlx-sqlite` reported no matching package. Phase 4 guard repeated the same result for the SQLx core package after adding reminders/audit. Phase 5 guard repeated the same result after adding wiki storage rows. Phase 6 no-SQLite guard again matched only the same pre-existing sandbox hint, and `cargo tree -p oxide-agent-core --no-default-features --features storage-sqlx -i sqlx-sqlite` reported no matching package.

- N3: R2 is not retained as a fallback or feature flag after removal
  - Source: `docs/prd/PRD-r2-to-pg.md:35`, `docs/prd/PRD-r2-to-pg.md:1183`
  - Must preserve: No R2 blob fallback, wiki fallback, memory fallback, emergency compatibility layer, or unnecessary R2 feature flags remain.
  - Evidence required: Static grep/cargo tree/docs review after Phase 6.
  - Status: verified
  - Evidence collected: Phase 5 wiki SQLx implementation adds `SqlxStorage` wiki method overrides and SQL row storage, so configured SQLx runtime no longer falls back to R2 for wiki reads/writes/deletes/context-delete. Phase 6 removed the `storage-s3-r2` Cargo feature, `storage/r2` capability/profile module, R2 storage modules, web R2 persistence, R2 credential tests, R2 env vars, and current setup docs; targeted no-R2 guard and AWS cargo-tree deny loop passed with no disallowed runtime/setup hits.

## Phase 0 Deletion and SQL Entity Map

Status: complete for implementation planning. This map is not authorization to delete R2 code before SQLx/Postgres paths have passing coverage.

### Search evidence

- Required PRD search terms verified: `R2`, `S3`, `Cloudflare`, `OXIDE_R2`, `storage-s3-r2`, `storage/r2`, `aws-sdk`, `aws_credential`, `aws_types`, `bucket`, `etag`, `list_keys_under_prefix`, `delete_prefix` (`docs/prd/PRD-r2-to-pg.md:1935`).
- Broad reference command: `rg -l --hidden --glob '!target/**' --glob '!.git/**' -e 'R2|Cloudflare|S3|AWS|storage-s3-r2|storage/r2|OXIDE_R2|aws-sdk|aws_'`.
- Broad reference result: 75 files total; `crates=51`, `current_docs=3`, `prd_docs=4`, `goal_docs=4`, `ci=1`, `profiles=8`, `root_files=4`.
- Durable seam review command: `rg -n --hidden --glob '!target/**' --glob '!.git/**' -e 'StorageProvider|WebUiStore|build_primary_storage|R2WebUiStore|InMemoryWebUiStore|durable storage|persistence' crates docs README.md .env.example .github profiles`.
- SQLite guard command: `rg -n -i --hidden --glob '!target/**' --glob '!.git/**' -e 'sqlite|rusqlite|sqlx::sqlite|Sqlite' Cargo.toml crates config .github .env.example`.
- SQLite result: only `crates/oxide-agent-core/src/agent/preprocessor.rs:432` matched; it is a sandbox hint, not a storage dependency, feature, migration, or plan.
- `config/**/*.yaml|yml` and `scripts/*.sh` had no direct R2/S3/AWS durable-storage matches in the docs/CI search.
- No durable runtime state outside the known `StorageProvider` and `WebUiStore` seams was found; local/in-memory stores remain test/dev-only or transient.

### Classification and deletion map

| Class | Covered references | Later action |
| --- | --- | --- |
| Cargo dependencies/features | AWS deps in `crates/oxide-agent-core/Cargo.toml:29`; `storage-s3-r2` in profile feature lists `crates/oxide-agent-core/Cargo.toml:80`, `crates/oxide-agent-core/Cargo.toml:121`, `crates/oxide-agent-core/Cargo.toml:146`, and atomic feature deps `crates/oxide-agent-core/Cargo.toml:239`; transport/binary feature forwarding in `crates/oxide-agent-transport-web/Cargo.toml`, `crates/oxide-agent-transport-telegram/Cargo.toml`, `crates/oxide-agent-telegram-bot/Cargo.toml`; AWS entries in `Cargo.lock`. | Phase 1 adds SQLx/Postgres deps/profile wiring; Phase 6 removes AWS crates, `storage-s3-r2`, R2 `required-features`, and regenerates `Cargo.lock`. |
| Capability/profile registry | R2 capability module `storage-s3-r2` / `storage/r2` in `crates/oxide-agent-core/src/capabilities/compiled.rs:299`; profile TOMLs enable `storage/r2`, e.g. `profiles/full.toml:20`. | Replace with `storage/sqlx`; update capability tests/snapshots and all `profiles/*.toml`. |
| Core storage runtime | R2-only factory and module in `crates/oxide-agent-core/src/storage/modules.rs:31`; AWS client/object ops in `crates/oxide-agent-core/src/storage/r2_base.rs:15`, `crates/oxide-agent-core/src/storage/r2_base.rs:89`; object key layout in `crates/oxide-agent-core/src/storage/keys.rs:16`; R2 user/memory/control-plane/reminder/provider modules. | Keep `StorageProvider`; implement SQLx backend; replace object keys, prefix scans/deletes, ETags, and health probes with typed rows, indexed queries, transactions/versions, and DB health. Delete R2 modules after SQL coverage exists. |
| Wiki memory runtime | Wiki object keys in `crates/oxide-agent-core/src/storage/keys.rs:62`; wiki context/page/inbox/raw keys in `crates/oxide-agent-core/src/storage/keys.rs:68`, `crates/oxide-agent-core/src/storage/keys.rs:80`, `crates/oxide-agent-core/src/storage/keys.rs:89`, `crates/oxide-agent-core/src/storage/keys.rs:98`; wiki docs currently describe S3/R2 at `docs/wiki-memory.md:3`. | Store wiki rows by typed scope/path metadata; replace prefix delete with SQL delete by context; update docs after runtime moves to SQL. |
| Web persistence runtime | R2 production path in `crates/oxide-agent-transport-web/src/persistence/mod.rs:3`; `WebUiStore` seam in `crates/oxide-agent-transport-web/src/persistence/store.rs:31`; web R2 key layout in `crates/oxide-agent-transport-web/src/persistence/r2.rs:779`; web startup selector in `crates/oxide-agent-transport-web/src/bin/oxide-agent-web-console.rs:292`. | Add `SqlxWebUiStore`; use append-only task-event rows; use bounded task-file rows or rejection; replace `OXIDE_WEB_STORE=r2`/`storage/r2` selection with DB-backed startup. |
| Telegram startup/runtime | R2-gated runner and R2-only failure path in `crates/oxide-agent-transport-telegram/src/runner.rs:31`; storage factory call in `crates/oxide-agent-transport-telegram/src/runner.rs:73`; R2/AWS redaction in `crates/oxide-agent-telegram-bot/src/main.rs`. | Build SQL storage via the factory, remove R2 feature gating, and use DB health/redaction for database config. |
| Tests/snapshots | R2 integration tests in `crates/oxide-agent-core/tests/r2_flow_checkpoint_integration.rs`; R2 credential validation in `crates/oxide-agent-telegram-bot/tests/integration_validation.rs`; R2 web key-layout tests in `crates/oxide-agent-transport-web/src/persistence/r2.rs`; R2 capability snapshots under `crates/oxide-agent-core/tests/snapshots/`. | Convert to SQL integration/contract tests; update capability snapshots; add static guards denying runtime AWS/R2 references after Phase 6. |
| Current env/docs/CI/deploy | `.env.example:9`, `.env.example:19`; `README.md:77`, `README.md:91`, `README.md:172`; `docs/deploy.md:18`, `docs/deploy.md:67`; CI dummy/secrets/deploy env in `.github/workflows/ci-cd.yml:18`, `.github/workflows/ci-cd.yml:103`; `AGENTS.md` current storage statements. | Replace R2 setup with local Postgres/Supabase DB variables, migration policy, pool settings, and old-R2-data-ignored note. Update `AGENTS.md` early once SQLx foundation is real. |
| Historical/allowed docs | Source PRD `docs/prd/PRD-r2-to-pg.md`; implemented PRDs under `docs/prd/implemented/`; older goal history such as `docs/goals/2026-05-27-web-console-v1.md`. | Keep as historical/spec references; final grep guard must allow these paths separately from current setup/runtime docs. |
| False positives/non-storage | `crates/oxide-agent-core/src/agent/preprocessor.rs:432` SQLite sandbox hint; UI/test text references such as “Cloudflare R2 limits”. | Do not implement around these; keep or update only if final static guard needs clearer allow-listing. |

### SQL entity map

| Old object namespace | Target SQL entity/entities | Notes |
| --- | --- | --- |
| `users/{user_id}/config.json` (`crates/oxide-agent-core/src/storage/keys.rs:16`) | `users`, `user_configs`, `user_contexts` | Split context-scoped mutable data out of full config rewrites. |
| `users/{user_id}/agent_memory.json` and `users/{user_id}/topics/{context_key}/agent_memory.json` (`crates/oxide-agent-core/src/storage/keys.rs:22`, `crates/oxide-agent-core/src/storage/keys.rs:28`) | `agent_memory_snapshots`, `context_agent_memory_snapshots` | Preserve scoped memory reload through `StorageProvider`; typed `(user_id, context_key)` ownership. |
| `users/{user_id}/topics/{context_key}/flows/{flow_id}/meta.json` and `/memory.json` (`crates/oxide-agent-core/src/storage/keys.rs:46`, `crates/oxide-agent-core/src/storage/keys.rs:52`) | `agent_flows`, `agent_flow_memory_snapshots` | Unique `(user_id, context_key, flow_id)`; flow memory can be snapshot JSONB with typed lifecycle columns. |
| `{prefix}/wiki/v1/global/*` and `{prefix}/wiki/v1/contexts/{context_id}/*` (`crates/oxide-agent-core/src/storage/keys.rs:62`, `crates/oxide-agent-core/src/storage/keys.rs:68`) | `wiki_global_files`, `wiki_context_files`, `wiki_pages`, `wiki_inbox_items`, `wiki_raw_archive` | Store derived `context_id` plus typed source scope; global ownership remains B6 until implementation confirms desired uniqueness. |
| `users/{user_id}/control_plane/agent_profiles/{agent_id}.json` (`crates/oxide-agent-core/src/storage/keys.rs:107`) | `agent_profiles` | Unique `(user_id, agent_id)`, version column for mutable updates. |
| `topic_contexts`, `topic_agents_md`, `topic_infra`, `topic_bindings` object keys (`crates/oxide-agent-core/src/storage/keys.rs:119`, `crates/oxide-agent-core/src/storage/keys.rs:125`, `crates/oxide-agent-core/src/storage/keys.rs:137`, `crates/oxide-agent-core/src/storage/keys.rs:143`) | `topic_contexts`, `topic_agents_md`, `topic_infra_configs`, `topic_bindings` | Use transactions/version checks; replace prompt guard object with transactional duplicate guard. |
| `users/{user_id}/private/secrets/{secret_ref}` (`crates/oxide-agent-core/src/storage/keys.rs:161`) | `user_secrets` | Preserve redaction; never expose secret material to prompts/logs/memory. |
| `users/{user_id}/control_plane/audit/events.json` (`crates/oxide-agent-core/src/storage/keys.rs:167`) | `audit_events` | Append-only rows; unique `(user_id, version)` allocated transactionally. |
| `users/{user_id}/control_plane/reminders/{reminder_id}.json` (`crates/oxide-agent-core/src/storage/keys.rs:149`, `crates/oxide-agent-core/src/storage/keys.rs:155`) | `reminder_jobs` | Index `(user_id, context_key, status)`, `(user_id, status, next_run_at)`; add lease/claim columns. |
| `web/auth/v1/users/{user_id}.json`, `login_index/{normalized_login}.json`, `browser_sessions/{hash}.json` (`crates/oxide-agent-transport-web/src/persistence/r2.rs:779`, `crates/oxide-agent-transport-web/src/persistence/r2.rs:783`, `crates/oxide-agent-transport-web/src/persistence/r2.rs:787`) | `web_users`, `web_auth_sessions` | Use unique index on normalized login; auth sessions by token hash. |
| `web/users/{user_id}/sessions/{session_id}.json` and `tasks/{session_id}/{task_id}.json` (`crates/oxide-agent-transport-web/src/persistence/r2.rs:791`, `crates/oxide-agent-transport-web/src/persistence/r2.rs:799`) | `web_sessions`, `web_tasks`, `web_task_progress_latest` | Typed status/timestamps; progress latest should be separate/coalesced, not task-event rewrite. |
| `web/users/{user_id}/task_events/{session_id}/{task_id}/chunk-{chunk_no}.json` (`crates/oxide-agent-transport-web/src/persistence/r2.rs:807`, `crates/oxide-agent-transport-web/src/persistence/r2.rs:823`) | `web_task_events` | Append-only rows with unique `(user_id, session_id, task_id, seq)`; no SQL chunk table needed. |
| `web/users/{user_id}/task_files/{session_id}/{task_id}/{file_id}.json/.bin` (`crates/oxide-agent-transport-web/src/persistence/r2.rs:811`, `crates/oxide-agent-transport-web/src/persistence/r2.rs:835`, `crates/oxide-agent-transport-web/src/persistence/r2.rs:843`) | `web_task_files`, `web_task_file_blobs` or `web_task_files.content BYTEA` | Enforce configurable max size before Phase 2 completion (B1). |
| R2 health/probe keys | none; DB health/migration check | Replace object write/head/list probes with SQL health query and migration status. |

## Phase 1 SQLx/Postgres Foundation Evidence

Status: complete for foundation. This phase intentionally keeps R2 as the default primary backend while SQLx/Postgres is staged in; broad business storage methods return an explicit unsupported error until entity-porting phases implement them.

### Added foundation artifacts

- Cargo/features: direct optional `sqlx-core` and `sqlx-postgres` dependencies plus `storage-sqlx` feature/profile forwarding; the top-level `sqlx` crate is intentionally not used because it introduced SQLite lockfile entries during exploration.
- Runtime foundation: `SqlxStorageConfig`, `SqlxStorage`, shared `PgPool`, `SELECT 1` health check, runtime migration runner from a configurable path, and `BuiltStorageBackend.sqlx` sidecar handle during R2/SQLx coexistence.
- Capability/profile registry: compiled module `storage/sqlx` with DB URL, pool, timeout, migration flag, and migrations-dir config properties; Phase 1 temporarily enabled `storage/sqlx` alongside `storage/r2` for coexistence before Phase 6 removed R2 profiles.
- Migration stream: `migrations/0001_storage_health.sql` creates the foundation marker table only; business tables remain for Phases 2-5.
- CI/docs/env: CI has a Postgres service and SQLx smoke step; `.env.example`, deploy docs, and README include initial DB variables and the fresh-storage/no-R2-import note.

### Validation evidence

- `cargo fmt --all -- --check`
- `cargo check -p oxide-agent-core --no-default-features --features storage-sqlx`
- `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
- `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`
- `cargo check --workspace --no-default-features --features profile-host-bwrap`
- `cargo check --workspace --no-default-features --features profile-full`
- `cargo clippy -p oxide-agent-core --no-default-features --features storage-sqlx -- -D warnings`
- `cargo clippy --workspace --no-default-features --features profile-embedded-opencode-local -- -D warnings`
- `INSTA_UPDATE=always cargo test -p oxide-agent-core --test modular_registry_snapshots ...` for `profile-lite`, `profile-search-only`, `profile-no-sandbox`, `profile-media-enabled`, `profile-host-bwrap`, `profile-full`, `profile-embedded-opencode-local`, `profile-web-embedded-opencode-local`, and `--all-features`
- `OXIDE_DATABASE_TEST_URL=postgres://oxide_agent:oxide_agent@localhost:55432/oxide_agent_test cargo test -p oxide-agent-core --no-default-features --features storage-sqlx sqlx_storage_connects_and_runs_migrations_when_test_url_is_set -- --nocapture`
- `rg -n -i --hidden --glob '!target/**' --glob '!.git/**' -e 'sqlx-sqlite|libsqlite3-sys|rusqlite|sqlx::sqlite|Sqlite|sqlite' Cargo.toml Cargo.lock crates config .github .env.example docs/deploy.md README.md migrations` matched only `crates/oxide-agent-core/src/agent/preprocessor.rs:432`.
- `cargo tree -p oxide-agent-core --no-default-features --features storage-sqlx -i sqlx-sqlite` reported no matching package.
- `rg -n --hidden --glob '!target/**' --glob '!.git/**' -e 'query!|migrate!' crates/oxide-agent-core/src/storage migrations` returned no matches.
- `git diff --check`

## Phase 2 Web SQLx Persistence Evidence

Status: complete for web durable persistence. Phase 6 later removed R2 web persistence, so production-like durable web startup now uses SQLx/Postgres and rejects unsupported web-store values.

### Added web persistence artifacts

- Migration stream: `migrations/0002_web_persistence.sql` adds typed Postgres tables and indexes for web users, login identities, auth sessions, sessions, tasks, append-only task events, latest progress snapshots, task-file metadata, and bounded `BYTEA` file blobs.
- Store implementation: `SqlxWebUiStore` implements `WebUiStore` with direct SQLx/Postgres queries, JSONB only for flexible payload/snapshot fields, duplicate event seq protection, indexed event listing, startup reconciliation, and configurable task-file byte rejection.
- Startup wiring: `build_sqlx_backed_app_state` and the web console selector use the shared `SqlxStorage` pool for both `StorageProvider` and web persistence; Phase 2 made R2 explicit-only, and Phase 6 removed that path so only `sqlx|postgres` web-store values remain supported.
- Tests: SQLx web contract tests cover users/auth sessions, sessions, tasks, append/list task events, progress snapshots, task files, oversized file rejection, unfinished-task reconciliation, missing DB config, and configured SQLx startup smoke.

### Validation evidence

- `cargo fmt --all -- --check`
- `cargo check -p oxide-agent-transport-web --no-default-features --features storage-sqlx`
- `OXIDE_DATABASE_TEST_URL=postgres://oxide_agent:oxide_agent@localhost:55432/oxide_agent_test cargo test -p oxide-agent-transport-web --no-default-features --features storage-sqlx sqlx_ -- --nocapture`
- `OXIDE_DATABASE_TEST_URL=postgres://oxide_agent:oxide_agent@localhost:55432/oxide_agent_test cargo test -p oxide-agent-transport-web --no-default-features --features storage-sqlx --lib`
- `cargo clippy -p oxide-agent-transport-web --no-default-features --features storage-sqlx -- -D warnings`
- `cargo clippy -p oxide-agent-transport-web --no-default-features -- -D warnings`
- `cargo check -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`
- `rg -n -i 'sqlx-sqlite|libsqlite3-sys|rusqlite|sqlx::sqlite|Sqlite|sqlite' Cargo.toml Cargo.lock crates config .github .env.example migrations || true` matched only `crates/oxide-agent-core/src/agent/preprocessor.rs:432`.
- `cargo tree -p oxide-agent-transport-web --no-default-features --features storage-sqlx -i sqlx-sqlite` reported no matching package.
- `git diff --check`

## Phase 3 Core SQLx StorageProvider Evidence

Status: complete for core durable storage. Configured SQLx/Postgres is the durable storage backend, and Telegram can build with SQLx-only durable storage.

### Added core storage artifacts

- Migration stream: `migrations/0003_core_storage.sql` adds typed Postgres tables and indexes for user config/state, user contexts, scoped agent memory snapshots, agent flow metadata, agent profiles, topic context, topic `AGENTS.md`, topic infra config, topic bindings, and private secrets.
- Store implementation: `SqlxStorage` now implements the core `StorageProvider` durable methods with direct SQLx/Postgres queries, scoped rows instead of whole-config rewrites, JSONB only for agent memory/profile payloads, transaction-bound upserts, row locks, advisory locks for mutable record families, and existing validation/builder semantics.
- Startup wiring: `build_primary_storage` prefers configured `storage/sqlx`; Telegram transport/binary durable-storage gates accept `storage-sqlx`, and the Telegram bot binary no longer requires `storage-s3-r2`.
- Tests: SQLx core tests cover context state updates without unchanged context-row version churn, global/context/flow memory scopes, flow metadata persistence and cleanup, control-plane profile/topic/infra/binding versions, duplicate prompt conflicts, and private secret put/get/delete.

### Validation evidence

- `cargo fmt --all -- --check`
- `cargo check -p oxide-agent-core --no-default-features --features storage-sqlx`
- `cargo check -p oxide-agent-transport-telegram --no-default-features --features storage-sqlx`
- `cargo check -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features transport-telegram,storage-sqlx`
- `OXIDE_DATABASE_TEST_URL=postgres://oxide_agent:oxide_agent@localhost:55432/oxide_agent_test cargo test -p oxide-agent-core --no-default-features --features storage-sqlx sqlx_ -- --nocapture`
- `cargo clippy -p oxide-agent-core --no-default-features --features storage-sqlx -- -D warnings`
- `cargo clippy -p oxide-agent-transport-telegram --no-default-features --features storage-sqlx -- -D warnings`
- `cargo clippy -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features transport-telegram,storage-sqlx -- -D warnings`
- `cargo check -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features profile-embedded-opencode-local`
- `cargo check -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`
- `rg -n -i 'sqlx-sqlite|libsqlite3-sys|rusqlite|sqlx::sqlite|Sqlite|sqlite' Cargo.toml Cargo.lock crates config .github .env.example migrations || true` matched only `crates/oxide-agent-core/src/agent/preprocessor.rs:432`.
- `cargo tree -p oxide-agent-core --no-default-features --features storage-sqlx -i sqlx-sqlite` reported no matching package.
- `git diff --check`

## Phase 4 Reminders and Audit SQLx Evidence

Status: complete for reminder jobs and manager audit. SQLx/Postgres implements reminder queue rows and append-only audit rows through `StorageProvider`; Phase 6 later removed the R2 reminder/audit runtime modules.

### Added reminder/audit artifacts

- Migration stream: `migrations/0004_reminders_audit.sql` adds typed Postgres tables and indexes for reminder jobs, per-user audit stream versions, and append-only audit events.
- Reminder implementation: `SqlxStorage` implements reminder create/get/list/due-claim/status/delete methods with direct SQLx/Postgres queries, status/lease predicates, version increments, `SELECT ... FOR UPDATE` mutations, and an atomic single-winner claim update.
- Audit implementation: `SqlxStorage` appends audit events by locking `audit_stream_versions`, allocating stable per-user versions transactionally, inserting one `audit_events` row, and serving recent/windowed pages by indexed version order.
- Tests: SQLx core tests cover reminder CRUD/list/due behavior, active-lease rejection, lease-expiry reclaim, concurrent single-winner claims, status transitions, deletion, per-user audit version allocation, recent audit windows, and descending cursor pages.

### Validation evidence

- `cargo fmt --all -- --check`
- `cargo check -p oxide-agent-core --no-default-features --features storage-sqlx`
- `cargo check -p oxide-agent-transport-telegram --no-default-features --features storage-sqlx`
- `cargo check -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features transport-telegram,storage-sqlx`
- `OXIDE_DATABASE_TEST_URL=postgres://oxide_agent:oxide_agent@localhost:55432/oxide_agent_test cargo test -p oxide-agent-core --no-default-features --features storage-sqlx sqlx_ -- --nocapture`
- `cargo clippy -p oxide-agent-core --no-default-features --features storage-sqlx -- -D warnings`
- `cargo clippy -p oxide-agent-transport-telegram --no-default-features --features storage-sqlx -- -D warnings`
- `cargo check -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features profile-embedded-opencode-local`
- `cargo check -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`
- `rg -n -i 'sqlx-sqlite|libsqlite3-sys|rusqlite|sqlx::sqlite|Sqlite|sqlite' Cargo.toml Cargo.lock crates config .github .env.example migrations || true` matched only `crates/oxide-agent-core/src/agent/preprocessor.rs:432`.
- `cargo tree -p oxide-agent-core --no-default-features --features storage-sqlx -i sqlx-sqlite` reported no matching package.

## Phase 5 Wiki Memory SQLx Evidence

Status: complete for wiki memory SQLx runtime. Configured SQLx/Postgres persists wiki memory through typed rows instead of object storage operations; Phase 6 later removed the R2 wiki runtime modules.

### Added wiki artifacts

- Migration stream: `migrations/0005_wiki_memory.sql` adds `wiki_pages` with typed storage prefix, scope kind, context id, item kind, path, content byte count, retention marker, version, schema version, timestamps, and context/prefix/retention indexes.
- Store implementation: `SqlxStorage` implements wiki load/save/delete/context-delete by parsing current deterministic logical wiki keys into SQL address metadata and reading/upserting/deleting `wiki_pages` rows.
- Limits/docs: SQLx wiki saves enforce 64 KiB normal/global/core/raw limits and 16 KiB inbox limits; wiki docs and runtime comments now describe storage-facade/SQLx rows instead of S3/R2 object storage.
- Tests: SQLx core tests cover global/context/page/inbox/raw wiki roundtrips, metadata row assertions, same-content no-op versions, changed-content version increments, page delete, context delete, global-row preservation, and oversized inbox rejection.

### Validation evidence

- `cargo fmt --all -- --check`
- `cargo check -p oxide-agent-core --no-default-features --features storage-sqlx`
- `cargo check -p oxide-agent-transport-telegram --no-default-features --features storage-sqlx`
- `cargo check -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features profile-embedded-opencode-local`
- `cargo check -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`
- `OXIDE_DATABASE_TEST_URL=postgres://oxide_agent:oxide_agent@localhost:55432/oxide_agent_test cargo test -p oxide-agent-core --no-default-features --features storage-sqlx sqlx_ -- --nocapture`
- `cargo clippy -p oxide-agent-core --no-default-features --features storage-sqlx -- -D warnings`
- `rg -n 'load_wiki_text|save_wiki_text|delete_wiki_text|delete_wiki_context|wiki_pages|delete_prefix|load_text\(|save_text\(' crates/oxide-agent-core/src/storage crates/oxide-agent-core/src/agent/wiki_memory docs/wiki-memory.md docs/tips/cache-hit.md migrations` shows SQLx wiki paths use `wiki_pages`; Phase 6 later removed the remaining R2 runtime modules.
- `rg -n -i 'sqlx-sqlite|libsqlite3-sys|rusqlite|sqlx::sqlite|Sqlite|sqlite' Cargo.toml Cargo.lock crates config .github .env.example migrations || true` matched only `crates/oxide-agent-core/src/agent/preprocessor.rs:432`.
- `cargo tree -p oxide-agent-core --no-default-features --features storage-sqlx -i sqlx-sqlite` reported no matching package.

## Phase 6 Physical R2 Removal Evidence

Status: complete for physical runtime removal. SQLx/Postgres is now the only durable runtime storage backend in current profiles, startup paths, CI/env setup, and current docs.

### Removed R2/AWS runtime artifacts

- Cargo/features: removed AWS SDK/S3 dependencies, the `storage-s3-r2` feature, `storage/r2` capability/profile modules, R2 feature forwarding, AWS dev-dependencies, and regenerated `Cargo.lock`.
- Runtime code: deleted core R2 storage modules, storage telemetry used only by R2, web object-store persistence, R2 credential validation tests, and the R2 flow checkpoint integration test.
- Startup/config: `build_primary_storage` now selects SQLx/Postgres as the only durable backend; Telegram durable gates require `storage-sqlx`; web startup supports only `OXIDE_WEB_STORE=sqlx|postgres`; profiles, `.env.example`, CI/deploy env, README, deploy docs, and `AGENTS.md` now describe SQLx/Postgres durable storage.
- Artifacts/tests: modular registry snapshots were regenerated without `storage/r2`/`storage-s3-r2`; static guards and capability JSON checks prove R2/AWS runtime references are absent from current runtime/setup paths.

### Validation evidence

- `cargo fmt --all -- --check`
- `cargo check -p oxide-agent-core --no-default-features --features storage-sqlx`
- `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
- `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`
- `cargo check --workspace --no-default-features --features profile-host-bwrap`
- `cargo check --workspace --no-default-features --features profile-full`
- `cargo clippy --workspace --no-default-features --features profile-embedded-opencode-local -- -D warnings`
- `cargo check -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features transport-telegram,storage-sqlx`
- `INSTA_UPDATE=always cargo test -p oxide-agent-core --test modular_registry_snapshots ...` for all profile features plus `--all-features`
- `cargo test -p oxide-agent-core --test tool_runtime_static_guards --no-default-features --features storage-sqlx`
- AWS cargo-tree deny loop for `aws-sdk-s3`, `aws-config`, `aws-credential-types`, `aws-types`, `aws-runtime`, `aws-sigv4`, `aws-smithy-runtime`, and `aws-smithy-types` across core `profile-full`, Telegram `profile-embedded-opencode-local`, and web `profile-web-embedded-opencode-local` returned no packages.
- Targeted no-R2 guard for `R2Storage|R2StorageConfig|R2WebUiStore|aws_sdk_s3|aws_config|aws_credential|aws_types|storage-s3-r2|storage/r2|OXIDE_R2|OXIDE_WEB_STORE=r2|list_keys_under_prefix|delete_prefix` returned no disallowed hits across current runtime/setup files.
- Capability JSON for Telegram and web production-like profiles had no `storage/r2|storage-s3-r2` hits.
- No-SQLite guard matched only `crates/oxide-agent-core/src/agent/preprocessor.rs:432`; `cargo tree -p oxide-agent-core --no-default-features --features storage-sqlx -i sqlx-sqlite` reported no matching package.

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
- 2026-06-05: Phase 0 classifies source PRDs, implemented PRDs, and older completed goals as historical/allowed R2 references; current setup docs, profiles, env examples, CI, Cargo features, and runtime code remain planned replacement/deletion targets.
- 2026-06-05: Phase 1 uses direct `sqlx-core` + `sqlx-postgres` dependencies instead of the top-level `sqlx` crate because the top-level crate pulled `sqlx-sqlite`/`libsqlite3-sys` into `Cargo.lock`, violating the no-SQLite constraint.
- 2026-06-05: During Phase 1 coexistence, R2 remains primary when `storage/r2` is enabled; `storage/sqlx` can build a sidecar pool when configured or become primary only when R2 is disabled. This avoids a broken SQL primary before Phases 2-5 port business methods.
- 2026-06-05: Phase 2 keeps direct `sqlx-core` + `sqlx-postgres` dependencies in the web crate instead of adding the top-level `sqlx` crate, and makes R2 web startup explicit-only while SQLx/Postgres is selected for durable web mode when configured.
- 2026-06-05: Phase 3 supersedes the Phase 1 primary-storage default: configured `storage/sqlx` is now preferred by `build_primary_storage`, while R2 remains only as a transitional fallback until reminders, audit, wiki, and physical removal phases are complete.
- 2026-06-05: Phase 4 keeps reminders as ordinary SQL rows with status/lease predicates and audit as append-only rows with per-user stream-version rows; no external queue, cache, or service is introduced.
- 2026-06-06: Phase 5 keeps the existing deterministic wiki key API at the `StorageProvider` seam but parses keys into typed SQL `wiki_pages` metadata; global wiki rows are globally shared per storage prefix under B6 until the user requests user-scoped global wiki ownership.
- 2026-06-06: Phase 6 removes the R2/AWS runtime outright; SQLx/Postgres is the only durable storage backend, and old object-storage data remains ignored rather than migrated, read, dual-written, or kept as fallback.

## Progress Log

- 2026-06-05: Goal drafted from PRD.
  - Changed: Created `docs/goals/2026-06-05-r2-to-postgres-storage.md` with objective, scope, audit ledger, phased checkpoints, validation contract, blockers, and first step.
  - Evidence: Source PRD read through end; existing `docs/goals/` convention inspected; README and CI R2 references reviewed; no existing matching goal found.
  - Commands: `git status --short --branch`; `git diff --check`; goal-doc read/diff review.
  - Audit IDs updated: none; implementation not started.
  - Next: Phase 0 — deletion map and SQL entity map.

- 2026-06-05 17:19 +03: Phase 0 deletion/entity map completed.
  - Changed: Added the Phase 0 deletion map and SQL entity map to this goal document only; no SQLx implementation, R2 deletion, migration, or SQLite work was added.
  - Evidence: Targeted R2/S3/AWS search matched 75 files and was classified by runtime, tests, current docs, historical docs, CI/env/profiles, and false positives; SQL entity mapping now covers old core/web/wiki/control-plane/reminder/audit/web object namespaces.
  - Commands: `rg -l --hidden --glob '!target/**' --glob '!.git/**' -e 'R2|Cloudflare|S3|AWS|storage-s3-r2|storage/r2|OXIDE_R2|aws-sdk|aws_'`; `rg -n -i --hidden --glob '!target/**' --glob '!.git/**' -e 'sqlite|rusqlite|sqlx::sqlite|Sqlite' Cargo.toml crates config .github .env.example`; `rg -n --hidden --glob '!target/**' --glob '!.git/**' -e 'StorageProvider|WebUiStore|build_primary_storage|R2WebUiStore|InMemoryWebUiStore|durable storage|persistence' crates docs README.md .env.example .github profiles`; `git diff --check` passed.
  - Audit IDs updated: G1 verified; Q1, Q2, N1, and N2 have Phase 0 evidence but remain pending until implementation/final static guards re-run.
  - Next: Phase 1 — SQLx/Postgres foundation.

- 2026-06-05 18:34 +03: Phase 1 SQLx/Postgres foundation completed.
  - Changed: Added SQLx/Postgres feature/dependency/profile/capability wiring, shared `PgPool` foundation, DB health check, runtime migration runner, foundation migration, CI Postgres smoke step, initial DB env/docs, and capability snapshots.
  - Evidence: Foundation builds in SQLx-only and production-like profiles, modular registry snapshots include `storage/sqlx`, clean Postgres smoke connected and ran `0001_storage_health.sql`, and static SQLite/dependency guards found no SQLite dependency.
  - Commands: `cargo fmt --all -- --check`; `cargo check -p oxide-agent-core --no-default-features --features storage-sqlx`; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`; `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`; `cargo check --workspace --no-default-features --features profile-host-bwrap`; `cargo check --workspace --no-default-features --features profile-full`; `cargo clippy -p oxide-agent-core --no-default-features --features storage-sqlx -- -D warnings`; `cargo clippy --workspace --no-default-features --features profile-embedded-opencode-local -- -D warnings`; modular registry snapshot tests for every profile/all-features; SQLx/Postgres smoke test; SQLite/dependency guard `rg`; `cargo tree -p oxide-agent-core --no-default-features --features storage-sqlx -i sqlx-sqlite`; `git diff --check`.
  - Audit IDs updated: G2 verified; G8, Q2, V1, V2, and N2 received Phase 1 evidence but remain pending for later phases/final audit.
  - Next: Phase 2 — web persistence on SQLx.

- 2026-06-05 21:11 +03: Phase 2 web SQLx persistence completed.
  - Changed: Added web persistence tables, `SqlxWebUiStore`, SQLx-backed web app-state startup, durable web store selection, bounded Postgres task-file blobs, append-only task events, SQL progress snapshots, and startup reconciliation.
  - Evidence: SQLx web contract tests passed against a clean temporary `postgres:16` database; configured app-state startup selected `WebStoreKind::Sqlx`; full web library tests passed with `storage-sqlx`; SQLite dependency guards found no SQLite package.
  - Commands: `cargo fmt --all -- --check`; `cargo check -p oxide-agent-transport-web --no-default-features --features storage-sqlx`; `OXIDE_DATABASE_TEST_URL=postgres://oxide_agent:oxide_agent@localhost:55432/oxide_agent_test cargo test -p oxide-agent-transport-web --no-default-features --features storage-sqlx sqlx_ -- --nocapture`; `OXIDE_DATABASE_TEST_URL=postgres://oxide_agent:oxide_agent@localhost:55432/oxide_agent_test cargo test -p oxide-agent-transport-web --no-default-features --features storage-sqlx --lib`; `cargo clippy -p oxide-agent-transport-web --no-default-features --features storage-sqlx -- -D warnings`; `cargo clippy -p oxide-agent-transport-web --no-default-features -- -D warnings`; `cargo check -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`; SQLite/dependency guard `rg`; `cargo tree -p oxide-agent-transport-web --no-default-features --features storage-sqlx -i sqlx-sqlite`; `git diff --check`.
  - Audit IDs updated: G3 verified; Q3, Q4, V1, V2, V4, and N2 received Phase 2 evidence but remain pending where later phases/final audit still apply.
  - Next: Phase 3 — core durable state on SQLx.

- 2026-06-05 22:16 +03: Phase 3 core SQLx `StorageProvider` completed.
  - Changed: Added core storage migration tables, SQL-backed `StorageProvider` methods for user config/state, contexts, scoped agent memory/flows, control-plane records, secrets, SQLx-preferred primary storage selection, and SQLx-compatible Telegram durable startup gates.
  - Evidence: SQLx core contract tests passed against a clean temporary `postgres:16` database; context row version checks prove `update_user_state` does not rewrite context rows; flow memory/metadata and control-plane version/duplicate-guard tests passed; Telegram SQLx-only binary check/clippy passed; SQLite guards found no SQLite package.
  - Commands: `cargo fmt --all -- --check`; `cargo check -p oxide-agent-core --no-default-features --features storage-sqlx`; `cargo check -p oxide-agent-transport-telegram --no-default-features --features storage-sqlx`; `cargo check -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features transport-telegram,storage-sqlx`; `OXIDE_DATABASE_TEST_URL=postgres://oxide_agent:oxide_agent@localhost:55432/oxide_agent_test cargo test -p oxide-agent-core --no-default-features --features storage-sqlx sqlx_ -- --nocapture`; `cargo clippy -p oxide-agent-core --no-default-features --features storage-sqlx -- -D warnings`; `cargo clippy -p oxide-agent-transport-telegram --no-default-features --features storage-sqlx -- -D warnings`; `cargo clippy -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features transport-telegram,storage-sqlx -- -D warnings`; `cargo check -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features profile-embedded-opencode-local`; `cargo check -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`; SQLite/dependency guard `rg`; `cargo tree -p oxide-agent-core --no-default-features --features storage-sqlx -i sqlx-sqlite`; `git diff --check`.
  - Audit IDs updated: G4 verified; Q1, Q2, Q3, Q5, V1, V2, V4, N1, and N2 received Phase 3 evidence but remain pending where later phases/final audit still apply.
  - Next: Phase 4 — reminders and audit on SQLx.

- 2026-06-05 22:43 +03: Phase 4 reminders and audit SQLx completed.
  - Changed: Added SQL reminder/audit migration tables, SQL-backed reminder job CRUD/list/due-claim/status/delete methods, atomic lease claiming, append-only audit event append/list/page methods, and DB-backed reminder/audit tests.
  - Evidence: SQLx core contract tests passed against a clean temporary `postgres:16` database with 10 SQLx tests; reminder tests cover status roundtrips, active-lease rejection, lease-expiry reclaim, deletion, and concurrent single-winner claims; audit tests cover per-user version allocation, recent windows, and descending cursor pages; SQLite guards found no SQLite package.
  - Commands: `cargo fmt --all -- --check`; `cargo check -p oxide-agent-core --no-default-features --features storage-sqlx`; `cargo check -p oxide-agent-transport-telegram --no-default-features --features storage-sqlx`; `cargo check -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features transport-telegram,storage-sqlx`; `OXIDE_DATABASE_TEST_URL=postgres://oxide_agent:oxide_agent@localhost:55432/oxide_agent_test cargo test -p oxide-agent-core --no-default-features --features storage-sqlx sqlx_ -- --nocapture`; `cargo clippy -p oxide-agent-core --no-default-features --features storage-sqlx -- -D warnings`; `cargo clippy -p oxide-agent-transport-telegram --no-default-features --features storage-sqlx -- -D warnings`; `cargo check -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features profile-embedded-opencode-local`; `cargo check -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`; SQLite/dependency guard `rg`; `cargo tree -p oxide-agent-core --no-default-features --features storage-sqlx -i sqlx-sqlite`.
  - Audit IDs updated: G5 verified; Q1, Q2, Q3, Q4, Q5, V1, V2, V4, N1, and N2 received Phase 4 evidence but remain pending where later phases/final audit still apply.
  - Next: Phase 5 — wiki memory on SQLx.

- 2026-06-06 08:49 +03: Phase 5 wiki memory SQLx completed.
  - Changed: Added SQL wiki memory migration rows, SQL-backed wiki load/save/delete/context-delete methods, logical-key-to-row metadata parsing, content-size enforcement, DB-backed wiki tests, and wiki documentation/comment updates.
  - Evidence: SQLx core contract tests passed against a clean temporary `postgres:16` database with 11 SQLx tests; wiki test covers global/context/page/inbox/raw roundtrips, typed metadata, version behavior, page/context delete, global preservation, and oversized inbox rejection; wiki runtime grep shows configured SQLx paths use `wiki_pages`; SQLite guards found no SQLite package.
  - Commands: `cargo fmt --all -- --check`; `cargo check -p oxide-agent-core --no-default-features --features storage-sqlx`; `cargo check -p oxide-agent-transport-telegram --no-default-features --features storage-sqlx`; `cargo check -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features profile-embedded-opencode-local`; `cargo check -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`; `OXIDE_DATABASE_TEST_URL=postgres://oxide_agent:oxide_agent@localhost:55432/oxide_agent_test cargo test -p oxide-agent-core --no-default-features --features storage-sqlx sqlx_ -- --nocapture`; `cargo clippy -p oxide-agent-core --no-default-features --features storage-sqlx -- -D warnings`; wiki runtime grep; SQLite/dependency guard `rg`; `cargo tree -p oxide-agent-core --no-default-features --features storage-sqlx -i sqlx-sqlite`.
  - Audit IDs updated: G6 verified; Q1, Q2, Q3, N1, N2, N3, V1, and V2 received Phase 5 evidence but remain pending where physical R2 removal/final audit still apply.
  - Next: Phase 6 — physical R2/S3/AWS runtime removal.

- 2026-06-06 09:44 +03: Phase 6 physical R2 removal completed.
  - Changed: Deleted R2/AWS storage modules, web object-store persistence, R2 credential/flow tests, `storage-s3-r2`/`storage/r2` feature and capability wiring, AWS SDK dependencies, R2 env setup, and current docs/profile/CI R2 paths; regenerated capability snapshots and `Cargo.lock`.
  - Evidence: Production-like profile checks build without AWS/S3 crates; cargo-tree deny loop found no AWS SDK/S3 packages; targeted no-R2 guard returned no disallowed runtime/setup hits; capability JSON and snapshots omit `storage/r2|storage-s3-r2`; no-SQLite guard still reports no SQLite package.
  - Commands: `cargo fmt --all -- --check`; `cargo check -p oxide-agent-core --no-default-features --features storage-sqlx`; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`; `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`; `cargo check --workspace --no-default-features --features profile-host-bwrap`; `cargo check --workspace --no-default-features --features profile-full`; `cargo clippy --workspace --no-default-features --features profile-embedded-opencode-local -- -D warnings`; `cargo check -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features transport-telegram,storage-sqlx`; modular registry snapshot tests for all profiles/all-features; `cargo test -p oxide-agent-core --test tool_runtime_static_guards --no-default-features --features storage-sqlx`; AWS cargo-tree deny loop; targeted no-R2 guard; capability JSON no-R2 checks; no-SQLite guard.
  - Audit IDs updated: G7, V3, and N3 verified; G8, Q1, Q2, N1, N2, and V1 received Phase 6 evidence but remain pending where Phase 7/final audit still applies.
  - Next: Phase 7 — hardening and final verification.

## Risks and Blockers

- Postgres-only task file blobs can grow WAL/backups quickly.
  - Impact: Large uploads/artifacts can make Supabase/local backups expensive or unstable.
  - Evidence: PRD calls out current 200 MB web upload default as risky for Postgres-only storage.
  - Mitigation or requested decision: Phase 2 enforces a strict configurable max size for web task files; ask user to approve or change the default before final hardening.
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
