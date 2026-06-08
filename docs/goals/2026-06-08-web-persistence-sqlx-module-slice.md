# Goal: web/persistence/sqlx.rs module slice

Date started: 2026-06-08
Status: active
Codex goal: not set
Source spec: user request
Goal doc owner: Codex
Last updated: 2026-06-08

## Objective

Split `crates/oxide-agent-transport-web/src/persistence/sqlx.rs` (2978 lines) into a `sqlx/` directory module with domain-focused slices. Extract cache infrastructure, free helpers (row mappers, SQL builders, serde converters, error mappers), and tests into dedicated files. `mod.rs` retains `SqlxWebUiStore` struct, private `impl` helpers, and `impl WebUiStore for SqlxWebUiStore` (cannot split `impl Trait` across files). Zero behavioural changes.

Done when every Completion Audit item is verified by its listed evidence and all out-of-scope constraints are preserved.

## Scope

In scope:
- `crates/oxide-agent-transport-web/src/persistence/sqlx.rs` → `persistence/sqlx/` directory
- New slice files: `cache.rs`, `helpers.rs`, `tests.rs`
- `persistence/mod.rs` line 14-15 (`pub use sqlx::SqlxWebUiStore;`) — no change needed, resolves automatically

Out of scope:
- `impl WebUiStore for SqlxWebUiStore` method bodies (stay in mod.rs; cannot split `impl Trait`)
- Any behavioural changes to persistence logic
- Any changes to `persistence/store.rs`, `persistence/in_memory.rs`, server crates
- Other large monoliths (sandbox/manager.rs, ssh_mcp.rs, config.rs, storage/sqlx/mod.rs)

## Missing Inputs

None.

## Repository Context

- Relevant entry points: `crates/oxide-agent-transport-web/src/persistence/sqlx.rs`
- Existing conventions: `storage/sqlx/` slice (7 files, committed 03cae248), `webfetch_md/` slice (7 files, committed 8e2e8e6b)
- Pattern: `pub(super)` visibility for internal items, `mod.rs` re-exports public API, `#[cfg(test)] mod tests;` in separate file
- Dependencies: `sqlx_core`, `sqlx_postgres`, `async_trait`, `serde`, `moka`, `chrono`, `oxide_agent_core::storage::SqlxStorage`, `oxide_agent_web_contracts`
- Validation: `cargo check -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`
- Tests require `OXIDE_DATABASE_TEST_URL` env var (skip if unset)
- Risky areas: `impl WebUiStore` is 973 lines monolithic — do NOT attempt to split it

## Completion Audit

- G1: `sqlx.rs` replaced by `sqlx/` directory module with `mod.rs` entry
  - Source: user request
  - Acceptance: `sqlx.rs` deleted, `sqlx/mod.rs` exists, `cargo check` passes
  - Evidence required: `cargo check` green, `ls` confirms directory structure
  - Status: pending
  - Evidence collected:

- G2: Cache infrastructure extracted to `cache.rs`
  - Source: RECON analysis (lines 35-222)
  - Acceptance: `cache.rs` exists with cache keys, logging helpers, predicates, cache constructors
  - Evidence required: `cargo check` green, `cache.rs` ~188 lines
  - Status: pending
  - Evidence collected:

- G3: Free helpers extracted to `helpers.rs`
  - Source: RECON analysis (lines 1820-2195)
  - Acceptance: `helpers.rs` exists with SQL builders, 12 row mappers, serde/enum converters, int casts, error mappers, env config
  - Evidence required: `cargo check` green, `helpers.rs` ~376 lines
  - Status: pending
  - Evidence collected:

- G4: Tests extracted to `tests.rs`
  - Source: RECON analysis (lines 2197-2978)
  - Acceptance: `tests.rs` contains all 7 integration tests + 8 helpers, `#[cfg(test)] mod tests;` in mod.rs
  - Evidence required: test run passes, test count unchanged
  - Status: pending
  - Evidence collected:

- Q1: Zero behavioural changes
  - Source: project conventions
  - Acceptance: No SQL changes, no logic changes, only visibility/move refactor
  - Evidence required: `git diff --stat` shows only moves + visibility + import adjustments
  - Status: pending
  - Evidence collected:

- Q2: Zero new dependencies or abstractions
  - Source: AGENTS.md
  - Acceptance: No new crates, no new traits, no new macros, no delegation patterns
  - Evidence required: `cargo check` without new deps
  - Status: pending
  - Evidence collected:

- N1: `impl WebUiStore` stays in mod.rs
  - Source: Rust language constraint (cannot split `impl Trait` across files)
  - Must preserve: single `impl WebUiStore for SqlxWebUiStore` block in mod.rs
  - Evidence required: mod.rs contains the full impl block
  - Status: pending
  - Evidence collected:

## Implementation Plan

### Checkpoint 0: folder-ize
- Audit IDs: G1
- Expected changes: `sqlx.rs` → `sqlx/mod.rs` (identical content)
- Validation: `cargo check -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`
- Exit condition: compiles, `persistence/mod.rs:14-15` resolves to `sqlx/mod.rs`

### Checkpoint 1: extract cache.rs
- Audit IDs: G2
- Expected changes: move cache key structs (`TaskCacheKey`, `TaskSessionCacheKey`, `SessionCacheKey`), logging helpers (`log_store_query`, `log_task_write_front`, `log_task_cache`, `log_session_cache`), predicates (`is_initial_task_without_progress`, `session_record_task_existence`), cache constructors (`task_write_front_cache`, `task_session_write_front_cache`, `session_write_front_cache`, `initial_task_flush_cache`) to `cache.rs`. All `pub(super)`.
- Validation: `cargo check`
- Exit condition: compiles, cache.rs ~188 lines

### Checkpoint 2: extract helpers.rs
- Audit IDs: G3
- Expected changes: move SQL builders (`task_select_sql`, `task_list_select_sql`), row mappers (`row_to_user`, `row_to_login_index`, `row_to_auth_session`, `row_to_session`, `row_to_session_summary`, `row_to_session_context_keys`, `row_to_task`, `row_to_task_event_state`, `row_to_event`, `row_to_task_file`), utility functions (`row_value`, `json_value`, `optional_json`, `from_json`, `optional_from_json`, `enum_to_sql`, `optional_enum_to_sql`, `enum_from_sql`, `optional_enum_from_sql`, int casts, `db_error`, `login_conflict_error`, `json_error`, `max_task_file_bytes_from_env`) to `helpers.rs`. All `pub(super)`.
- Validation: `cargo check`
- Exit condition: compiles, helpers.rs ~376 lines

### Checkpoint 3: extract tests.rs
- Audit IDs: G4
- Expected changes: move `#[cfg(test)] mod tests { ... }` to `tests.rs`, replace with `#[cfg(test)] mod tests;` in mod.rs
- Validation: `cargo check`
- Exit condition: compiles, tests.rs ~782 lines

### Checkpoint 4: completion audit
- Audit IDs: G1, G2, G3, G4, Q1, Q2, N1
- Expected changes: goal doc update with Final Verification
- Validation: full audit checklist
- Exit condition: all audit items verified

## Validation Contract

- Static checks: `cargo check -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`
- Lint: `cargo clippy -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local -- -D warnings`
- Done when: all audit items G1-G4, Q1-Q2, N1 verified

## Decisions

- 2026-06-08: `impl WebUiStore for SqlxWebUiStore` stays in mod.rs. Rust requires all trait impl methods in one module. Splitting via delegation is over-engineering.
- 2026-06-08: Cache infrastructure grouped as one slice (keys + logging + predicates + constructors) rather than separate key/logging/cache files — all three are tightly coupled and total only ~188 lines.

## Progress Log

- 2026-06-08: goal doc created
  - Changed: docs/goals/2026-06-08-web-persistence-sqlx-module-slice.md
  - Evidence: RECON complete, 2978 lines analyzed, 9 domain areas identified
  - Commands: RECON via explore agent
  - Audit IDs updated: none yet
  - Next: Checkpoint 0 (folder-ize)

## Risks and Blockers

None identified. All slice boundaries are clean (free functions and private types).

## Final Verification

Filled only when complete.

## Slice Map (target)

```
sqlx/
  mod.rs      ~2100  struct SqlxWebUiStore + private impl + impl WebUiStore + private helper
  cache.rs    ~188   cache keys, logging, predicates, cache constructors
  helpers.rs  ~376   SQL builders, row mappers, serde/enum converters, int casts, errors
  tests.rs    ~782   7 integration tests + 8 helpers
```
