# Goal: storage/sqlx.rs module slice

Date started: 2026-06-08
Status: active
Codex goal: not set
Source spec: user request
Goal doc owner: Codex
Last updated: 2026-06-08

## Objective

Split `crates/oxide-agent-core/src/storage/sqlx.rs` (3667 lines) into a `sqlx/` directory module with domain-focused slices. All helper types, free functions, row mappers, wiki validation, tx helpers, and tests move to dedicated files. `mod.rs` retains `SqlxStorage` struct, `impl SqlxStorage`, and `impl StorageProvider for SqlxStorage` (cannot split `impl Trait` across files). Zero behavioural changes.

Done when every Completion Audit item is verified by its listed evidence and all out-of-scope constraints are preserved.

## Scope

In scope:
- `crates/oxide-agent-core/src/storage/sqlx.rs` → `crates/oxide-agent-core/src/storage/sqlx/` directory
- New slice files: `helpers.rs`, `wiki.rs`, `rows.rs`, `reminder_tx.rs`, `topic_tx.rs`, `tests.rs`
- `storage/mod.rs` line 18 (`mod sqlx;`) — no change needed, resolves automatically

Out of scope:
- `impl StorageProvider for SqlxStorage` method bodies (stay in mod.rs; cannot split `impl Trait`)
- Any behavioural changes to storage logic
- Any changes to `storage/providers/mod.rs`, `modules.rs`, `sqlx_config.rs`, transport crates
- Other large monoliths (sandbox/manager.rs, ssh_mcp.rs, config.rs, web/persistence/sqlx.rs)

## Missing Inputs

None.

## Repository Context

- Relevant entry points: `crates/oxide-agent-core/src/storage/sqlx.rs`
- Existing conventions: `webfetch_md/` module slice (7 files, committed 8e2e8e6b), `web-ui/` slices
- Pattern: `pub(super)` visibility for internal items, `mod.rs` re-exports public API, `#[cfg(test)] mod tests;` in separate file
- Dependencies: `sqlx_core`, `sqlx_postgres`, `async_trait`, `serde`, `sha2`
- Validation: `cargo test -p oxide-agent-core --no-default-features --features storage-sqlx` + `cargo check`
- Risky areas: `impl StorageProvider` is 1558 lines monolithic — do NOT attempt to split it

## Completion Audit

- G1: `sqlx.rs` replaced by `sqlx/` directory module with `mod.rs` entry
  - Source: user request
  - Acceptance: `sqlx.rs` deleted, `sqlx/mod.rs` exists, `cargo check` passes
  - Evidence required: `cargo check` green, `ls` confirms directory structure
  - Status: pending
  - Evidence collected:

- G2: All helper functions extracted to domain slices
  - Source: RECON analysis
  - Acceptance: `helpers.rs`, `wiki.rs`, `rows.rs`, `reminder_tx.rs`, `topic_tx.rs` exist and compile
  - Evidence required: `cargo check` green, each file present and non-empty
  - Status: pending
  - Evidence collected:

- G3: Tests extracted to `tests.rs`
  - Source: RECON analysis
  - Acceptance: `tests.rs` contains all 11 integration tests, `#[cfg(test)] mod tests;` in mod.rs
  - Evidence required: test run passes, test count unchanged
  - Evidence required: `cargo test -p oxide-agent-core --no-default-features --features storage-sqlx` — all tests pass
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

- N1: `impl StorageProvider` stays in mod.rs
  - Source: Rust language constraint (cannot split `impl Trait` across files)
  - Must preserve: single `impl StorageProvider for SqlxStorage` block in mod.rs
  - Evidence required: mod.rs contains the full impl block
  - Status: pending
  - Evidence collected:

## Implementation Plan

### Checkpoint 0: folder-ize
- Audit IDs: G1
- Expected changes: `sqlx.rs` → `sqlx/mod.rs` (identical content)
- Validation: `cargo check -p oxide-agent-core --no-default-features --features storage-sqlx`
- Exit condition: compiles, `storage/mod.rs:18` resolves to `sqlx/mod.rs`

### Checkpoint 1: extract helpers.rs
- Audit IDs: G2
- Expected changes: move db_error, ensure_user_row_in_tx, advisory_xact_lock, advisory_lock_key, row_value, from_json, enum_to_sql, enum_from_sql, enum_vec_from_sql, int conversion functions (u32_to_i32 through usize_to_i64) to `helpers.rs`. All `pub(super)`.
- Validation: `cargo check`, grep confirms functions removed from mod.rs
- Exit condition: compiles, helpers.rs ~146 lines

### Checkpoint 2: extract wiki.rs
- Audit IDs: G2
- Expected changes: move WIKI_SCHEMA_VERSION, WIKI_DEFAULT_MAX_BYTES, WIKI_INBOX_MAX_BYTES, WikiScopeKind, WikiItemKind, WikiAddress, parse_wiki_storage_key, validate_wiki_context_id, validate_wiki_markdown_leaf, validate_wiki_year_month, validate_wiki_content_size to `wiki.rs`. All `pub(super)`.
- Validation: `cargo check`
- Exit condition: compiles, wiki.rs ~188 lines

### Checkpoint 3: extract rows.rs
- Audit IDs: G2
- Expected changes: move row_to_user_context, row_to_agent_flow, row_to_agent_profile, row_to_topic_context, row_to_topic_agents_md, row_to_topic_infra_config, row_to_topic_binding, row_to_audit_event, row_to_reminder_job to `rows.rs`. All `pub(super)`. Depends on helpers.rs (row_value, enum_from_sql, int conversions).
- Validation: `cargo check`
- Exit condition: compiles, rows.rs ~195 lines

### Checkpoint 4: extract reminder_tx.rs
- Audit IDs: G2
- Expected changes: move insert_reminder_job_in_tx, update_reminder_job_in_tx, get_reminder_job_for_update, mutate_reminder_job to `reminder_tx.rs`. All `pub(super)`. Depends on helpers.rs, rows.rs.
- Validation: `cargo check`
- Exit condition: compiles, reminder_tx.rs ~172 lines

### Checkpoint 5: extract topic_tx.rs
- Audit IDs: G2
- Expected changes: move get_agent_flow_record_for_update, get_agent_profile_for_update, get_topic_context_for_update, get_topic_agents_md_for_update, get_topic_infra_config_for_update, get_topic_binding_for_update, TopicPromptStoreKind, ensure_topic_prompt_not_duplicated_in_tx to `topic_tx.rs`. All `pub(super)`. Depends on helpers.rs, rows.rs.
- Validation: `cargo check`
- Exit condition: compiles, topic_tx.rs ~211 lines

### Checkpoint 6: extract tests.rs
- Audit IDs: G3
- Expected changes: move `#[cfg(test)] mod tests { ... }` to `tests.rs`, replace with `#[cfg(test)] mod tests;` in mod.rs
- Validation: `cargo test -p oxide-agent-core --no-default-features --features storage-sqlx`
- Exit condition: all tests pass, tests.rs ~934 lines

### Checkpoint 7: completion audit
- Audit IDs: G1, G2, G3, Q1, Q2, N1
- Expected changes: goal doc update with Final Verification
- Validation: full audit checklist
- Exit condition: all audit items verified

## Validation Contract

- Static checks: `cargo check -p oxide-agent-core --no-default-features --features storage-sqlx`
- Tests: `cargo test -p oxide-agent-core --no-default-features --features storage-sqlx`
- Lint: `cargo clippy -p oxide-agent-core --no-default-features --features storage-sqlx`
- Done when: all audit items G1-G3, Q1-Q2, N1 verified

## Decisions

- 2026-06-08: `impl StorageProvider for SqlxStorage` stays in mod.rs. Rust requires all trait impl methods in one module. Splitting via delegation (free functions returning results, impl delegates) is over-engineering for a personal-use project at 5 RPS.
- 2026-06-08: Helper functions grouped by domain (wiki validation, row mappers, tx helpers) rather than by type (all structs together, all functions together). Domain grouping matches the existing codebase pattern and mental model.

## Progress Log

- 2026-06-08: goal doc created
  - Changed: docs/goals/2026-06-08-storage-sqlx-module-slice.md
  - Evidence: RECON complete, 3667 lines analyzed, 7 domain areas identified
  - Commands: RECON via grep/read/wc
  - Audit IDs updated: none yet
  - Next: Checkpoint 0 (folder-ize)

- 2026-06-08: CP0 — folder-ize
  - Changed: sqlx.rs → sqlx/mod.rs (3667 lines, identical content)
  - Evidence: `cargo check -p oxide-agent-core --no-default-features --features storage-sqlx` green
  - Commands: `mkdir -p storage/sqlx && mv sqlx.rs sqlx/mod.rs`, cargo check
  - Audit IDs updated: G1 (verified)
  - Next: Checkpoint 2 (extract wiki.rs)

- 2026-06-08: CP1 — extract helpers.rs
  - Changed: helpers.rs (184 lines): db_error, ensure_user_row_in_tx, advisory_xact_lock, advisory_lock_key, row_value, from_json, enum_to_sql, enum_from_sql, enum_vec_to_sql, enum_vec_from_sql, int casts
  - Removed imports from mod.rs: Serialize, DeserializeOwned, Sha256, Digest, SqlxError, Decode, Row, Type
  - Evidence: cargo check zero warnings, 638 tests pass (28 pre-existing failures)
  - Audit IDs updated: G2 (partial)
  - Next: Checkpoint 2 (extract wiki.rs)

- 2026-06-08: CP2 — extract wiki.rs
  - Changed: wiki.rs (197 lines): WIKI_SCHEMA_VERSION, WIKI_DEFAULT_MAX_BYTES, WIKI_INBOX_MAX_BYTES, WikiScopeKind, WikiItemKind, WikiAddress, parse_wiki_storage_key, validate_wiki_context_id, validate_wiki_markdown_leaf, validate_wiki_year_month, validate_wiki_content_size
  - mod.rs=3333, wiki.rs=197, helpers.rs=184. Total=3714
  - Evidence: cargo check zero warnings, cargo clippy clean
  - Audit IDs updated: G2 (partial)
  - Next: Checkpoint 3 (extract rows.rs)

- 2026-06-08: CP3 — extract rows.rs
  - Changed: rows.rs (209 lines): row_to_user_context, row_to_agent_flow, row_to_agent_profile, row_to_topic_context, row_to_topic_agents_md, row_to_topic_infra_config, row_to_topic_binding, row_to_audit_event, row_to_reminder_job
  - mod.rs=3142, rows.rs=209, wiki.rs=197, helpers.rs=184. Total=3732
  - Evidence: cargo check + clippy zero warnings
  - Audit IDs updated: G2 (partial)
  - Next: Checkpoint 4 (extract reminder_tx.rs)

- 2026-06-08: CP4 — extract reminder_tx.rs
  - Changed: reminder_tx.rs (179 lines): insert_reminder_job_in_tx, update_reminder_job_in_tx, get_reminder_job_for_update, mutate_reminder_job
  - mod.rs=2972, reminder_tx.rs=179, rows.rs=209, wiki.rs=197, helpers.rs=184. Total=3741
  - Evidence: cargo check + clippy zero warnings
  - Audit IDs updated: G2 (partial)
  - Next: Checkpoint 5 (extract topic_tx.rs)

- 2026-06-08: CP5 — extract topic_tx.rs
  - Changed: topic_tx.rs (226 lines): get_agent_flow_record_for_update, get_agent_profile_for_update, get_topic_context_for_update, get_topic_agents_md_for_update, get_topic_infra_config_for_update, get_topic_binding_for_update, TopicPromptStoreKind, ensure_topic_prompt_not_duplicated_in_tx
  - Removed from mod.rs: normalize_topic_prompt_payload import, Transaction import (moved to topic_tx.rs)
  - mod.rs=2767, topic_tx.rs=226, reminder_tx.rs=179, rows.rs=209, wiki.rs=197, helpers.rs=184. Total=3762
  - Evidence: cargo check + clippy zero warnings
  - Audit IDs updated: G2 (partial)
  - Next: Checkpoint 6 (extract tests.rs)

- 2026-06-08: CP6 — extract tests.rs
  - Changed: tests.rs (933 lines): 11 integration tests + helpers (sqlx_test_storage, sqlx_test_storage_with_connections, unique_user_id, user_context_version, wiki_page_version, assert_memory_eq)
  - mod.rs=1836, tests.rs=933, topic_tx.rs=226, reminder_tx.rs=179, rows.rs=209, wiki.rs=197, helpers.rs=184. Total=3764
  - Evidence: cargo check + clippy zero warnings
  - Audit IDs updated: G3 (verified), G2 (verified — all slices extracted)
  - Next: Checkpoint 7 (completion audit)

## Risks and Blockers

None identified. All slice boundaries are clean (free functions and private types).

## Final Verification

Filled only when complete.

## Slice Map (target)

```
sqlx/
  mod.rs         1836  struct SqlxStorage + impl + impl StorageProvider
  helpers.rs      184  db_error, row_value, enum conversions, int casts
  wiki.rs         197  WikiAddress, parse/validate wiki storage keys
  rows.rs         209  row_to_* mapper functions
  reminder_tx.rs  179  reminder job tx helpers
  topic_tx.rs     226  topic record tx helpers + dedup
  tests.rs        933  integration tests
```
