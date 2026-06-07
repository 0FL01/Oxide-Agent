# Goal: Fix Rust 2024 edition test compilation — unsafe env + API drift

Date started: 2026-06-07
Status: active
Codex goal: `/goal Implement docs/goals/2026-06-07-rust-2024-test-unsafe-env.md until every Completion Audit item is verified by its listed evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update this document after each meaningful verification, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.`
Source spec: User request after `cargo test` failed across all profiles due to Rust 2024 edition breaking changes in test code.
Goal doc owner: Codex
Last updated: 2026-06-07

## Objective

Fix all test compilation errors introduced by the Rust 2024 edition migration so that `cargo test --workspace` compiles and passes across all profiles. The previous edition migration goal (2026-06-07-rust-2024-edition-migration.md) validated `cargo check` and `cargo clippy` only — `cargo test` was out of scope and is now broken.

Done when every required Completion Audit item is verified by its listed evidence, all profiles compile test binaries, and tests pass.

## Scope

In scope:
- `crates/oxide-agent-core/src/testing.rs` — add `test_set_env` / `test_remove_env` helpers.
- All test code using `std::env::set_var` / `std::env::remove_var` (9 files, 249 call sites total).
- Test code with missing struct fields or wrong argument counts after API changes (4 files, 18 call sites).
- `crates/oxide-agent-transport-telegram/src/bot/progress_render.rs` test — missing `AgentEventSource` import.
- `crates/oxide-agent-transport-web/examples/web_console_dev.rs` — 1 env call.

Out of scope:
- Production (non-test) code — no changes.
- New crates, dependencies, abstractions, or architectural changes.
- Changing any test logic or assertions — only fixing compilation.
- Changing `[lints]`, `[dependencies]`, or `[features]`.
- Non-Rust files (YAML, TOML, Docker, CSS, etc.).

## Missing Inputs

- None. All errors identified via RECON.

## Repository Context

- Rust 2024 edition makes `std::env::set_var` and `std::env::remove_var` unsafe.
- All 249 call sites are in `#[cfg(test)]` modules or test files — safe to wrap.
- `crates/oxide-agent-core/src/testing.rs` is a public module with existing test helpers (`mock_llm_simple`, `mock_storage_noop`). All transport crates depend on core and can import from it.
- Validation profiles: `profile-full`, `profile-lite`, `profile-web-embedded-opencode-local`, `profile-host-bwrap`, `profile-embedded-opencode-local`, `profile-no-sandbox`, `profile-media-enabled`, `profile-search-only`.

## Completion Audit

- G1: `test_set_env` and `test_remove_env` exist in `testing.rs`
  - Source: RECON — need centralized unsafe wrappers.
  - Acceptance: `crates/oxide-agent-core/src/testing.rs` exports `pub fn test_set_env(key: &str, value: &str)` and `pub fn test_remove_env(key: &str)` with `#[track_caller]`, wrapping `unsafe { std::env::set_var/remove_var }`.
  - Evidence required: file read showing both functions with correct signatures and `#[track_caller]`.
  - Status: verified
  - Evidence collected: Both functions added with `#[track_caller]`, generic `impl AsRef<OsStr>` signatures to accept `String`, `&str`, and `OsString`. `cargo check -p oxide-agent-core --features profile-full` passes.

- G2: All `std::env::set_var` / `std::env::remove_var` calls in test code replaced with helpers
  - Source: RECON — 249 call sites across 10 files.
  - Acceptance: `rg 'std::env::(set_var|remove_var)' crates/ --glob '*.rs'` returns zero hits in test code. Only the `testing.rs` helpers themselves contain the raw `unsafe` calls.
  - Evidence required: `rg` command returning zero results outside `testing.rs`.
  - Status: in_progress
  - Evidence collected: Batch A complete (6 core files, 304 calls replaced). Batch B complete (4 core files, 4 calls; transport-web 23 calls + local wrappers; web_console_dev 1 inline unsafe). Core crate env helpers in `testing.rs` (cfg-test). Transport-web has local wrappers (forbid unsafe_code in lib prevents cross-crate sharing).

- G3: Missing `reasoning_effort` field in `ChatWithToolsRequest` fixed
  - Source: RECON — 16 call sites in 3 integration test files.
  - Acceptance: All `ChatWithToolsRequest { ... }` literals include `reasoning_effort: None`.
  - Evidence required: `cargo test --workspace --no-default-features --features profile-full` compiles without `E0063`.
  - Status: pending
  - Evidence collected:

- G4: Wrong argument count in `client.rs` unit test fixed
  - Source: RECON — 1 call site missing `reasoning_effort` arg.
  - Acceptance: `chat_with_tools_single_attempt_for_model_info` call passes 8 args including `None` for `reasoning_effort`.
  - Evidence required: file read of the fixed call.
  - Status: pending
  - Evidence collected:

- G5: Missing `AgentEventSource` import in `progress_render.rs` test fixed
  - Source: RECON — `profile-lite` fails with `E0433`.
  - Acceptance: `use oxide_agent_core::agent::progress::{..., AgentEventSource}` in test module.
  - Evidence required: file read of the import.
  - Status: pending
  - Evidence collected:

- G6: Missing `before_seq` field in `TaskEventsQuery` test literal fixed
  - Source: RECON — `profile-lite` fails with `E0063` in `tests.rs:2831`.
  - Acceptance: `TaskEventsQuery { after_seq: Some(0), before_seq: None, limit: Some(200) }`.
  - Evidence required: file read of the fixed literal.
  - Status: pending
  - Evidence collected:

- Q1: No production code changed
  - Source: Constraint — only test code may be modified.
  - Acceptance: All changed files are either `testing.rs` (test helper), `#[cfg(test)]` blocks, or test files.
  - Evidence required: `git diff --name-only` review.
  - Status: pending
  - Evidence collected:

- Q2: All profiles compile test binaries
  - Source: Validation gate.
  - Acceptance: `cargo test --workspace --no-default-features --features <profile>` compiles for `profile-full`, `profile-lite`, `profile-web-embedded-opencode-local`, `profile-host-bwrap`, `profile-embedded-opencode-local`.
  - Evidence required: command output for each profile.
  - Status: pending
  - Evidence collected:

- Q3: Clippy still clean
  - Source: Regression gate — changes must not introduce clippy warnings.
  - Acceptance: `cargo clippy --workspace --no-default-features --features profile-full` produces zero warnings.
  - Evidence required: command output.
  - Status: pending
  - Evidence collected:

- N1: No new dependencies
  - Source: Constraint.
  - Must preserve: all Cargo.toml files unchanged.
  - Evidence required: `git diff -- '*.toml'` shows no changes.
  - Status: pending
  - Evidence collected:

- N2: No changes to test logic or assertions
  - Source: Constraint — only mechanical compilation fixes.
  - Must preserve: all test function bodies unchanged except for import additions, field additions, arg additions, and env call replacements.
  - Evidence required: diff review.
  - Status: pending
  - Evidence collected:

## Implementation Plan

### Checkpoint 1: Add helpers to `testing.rs`

- Audit IDs: G1
- Add `test_set_env` and `test_remove_env` to `crates/oxide-agent-core/src/testing.rs`.
- Both functions: `#[track_caller]`, `pub`, wrapping `unsafe { std::env::set_var/remove_var }`.
- Validation: `cargo check --workspace --no-default-features --features profile-full`.
- Exit condition: helpers exist and compile.

### Checkpoint 2: Batch A — `oxide-agent-core` test files (6 files, ~214 calls)

- Audit IDs: G2
- Files:
  - `crates/oxide-agent-core/src/config.rs` (~92 calls) — uses `env::set_var` via `use std::env;`
  - `crates/oxide-agent-core/src/agent/executor/tests/registry.rs` (~91 calls)
  - `crates/oxide-agent-core/src/sandbox/bwrap/tests.rs` (~110 calls)
  - `crates/oxide-agent-core/src/sandbox/manager.rs` (6 calls)
  - `crates/oxide-agent-core/src/storage/sqlx_config.rs` (2 calls)
  - `crates/oxide-agent-core/src/llm/providers/modules.rs` (13 calls)
- For each file: add `use oxide_agent_core::testing::{test_set_env, test_remove_env};` to the test module, replace all `std::env::set_var(...)` with `test_set_env(...)` and all `std::env::remove_var(...)` with `test_remove_env(...)`.
- Special case `config.rs`: test modules use `use std::env;` then call `env::set_var` — replace with `test_set_env` / `test_remove_env` and add import.
- Special case `bwrap/tests.rs`: has a helper closure that calls `set_var`/`remove_var` via iteration — adapt closure to use helpers.
- Validation: `cargo test -p oxide-agent-core --no-default-features --features profile-full -- --list` compiles.
- Exit condition: `rg 'std::env::(set_var|remove_var)' crates/oxide-agent-core/` returns only `testing.rs`.

### Checkpoint 3: Batch B — transport and remaining files (3 files, ~25 calls)

- Audit IDs: G2
- Files:
  - `crates/oxide-agent-transport-web/src/server/tests.rs` (~23 calls)
  - `crates/oxide-agent-core/src/agent/providers/file_delivery.rs` (2 calls)
  - `crates/oxide-agent-core/src/agent/providers/tts/client.rs` (1 call)
  - `crates/oxide-agent-core/src/agent/providers/silero_tts/client.rs` (1 call)
  - `crates/oxide-agent-transport-web/examples/web_console_dev.rs` (1 call) — if env call is in non-test code, wrap in `unsafe {}` instead since examples may not import from testing module.
- For each file: same pattern as Checkpoint 2.
- Validation: `cargo test -p oxide-agent-transport-web --no-default-features --features profile-full -- --list` compiles.
- Exit condition: `rg 'std::env::(set_var|remove_var)' crates/ --glob '*.rs'` returns only `testing.rs` and `web_console_dev.rs` (if example).

### Checkpoint 4: Fix API drift — `reasoning_effort`, `before_seq`, `AgentEventSource`

- Audit IDs: G3, G4, G5, G6
- Files:
  - `crates/oxide-agent-core/tests/llm_provider_check.rs` — add `reasoning_effort: None`
  - `crates/oxide-agent-core/tests/mistral_e2e.rs` — add `reasoning_effort: None` (7 sites)
  - `crates/oxide-agent-core/tests/minimax_e2e.rs` — add `reasoning_effort: None` (7 sites)
  - `crates/oxide-agent-core/src/llm/client.rs` — add `None` as 8th arg (1 site)
  - `crates/oxide-agent-transport-telegram/src/bot/progress_render.rs` — add `AgentEventSource` to import (1 site)
  - `crates/oxide-agent-transport-web/src/server/tests.rs` — add `before_seq: None` (1 site)
- Validation: `cargo test --workspace --no-default-features --features profile-full` compiles.
- Exit condition: zero E0063, E0061, E0433 errors.

### Checkpoint 5: Multi-profile validation

- Audit IDs: Q2, Q3
- Run `cargo test --workspace --no-default-features --features <profile>` for:
  - `profile-full`
  - `profile-lite`
  - `profile-web-embedded-opencode-local`
  - `profile-host-bwrap`
  - `profile-embedded-opencode-local`
- Run `cargo clippy --workspace --no-default-features --features profile-full`.
- Exit condition: all profiles compile test binaries; clippy clean.

### Checkpoint 6: Final audit

- Audit IDs: all
- Review all diffs, verify constraints (N1, N2, Q1), update goal document.
- Exit condition: every audit item verified with current evidence.

## Validation Contract

- Static checks:
  - `rg 'std::env::(set_var|remove_var)' crates/ --glob '*.rs'` — only `testing.rs` (and `web_console_dev.rs` if example)
  - `git diff -- '*.toml'` — no changes
  - `git diff --name-only` — only test/helper files
- Rust checks:
  - `cargo test --workspace --no-default-features --features profile-full` compiles
  - `cargo test --workspace --no-default-features --features profile-lite` compiles
  - `cargo test --workspace --no-default-features --features profile-web-embedded-opencode-local` compiles
  - `cargo test --workspace --no-default-features --features profile-host-bwrap` compiles
  - `cargo test --workspace --no-default-features --features profile-embedded-opencode-local` compiles
  - `cargo clippy --workspace --no-default-features --features profile-full` — zero warnings
- Done when: all non-dropped Completion Audit items verified with current evidence.

## Decisions

- 2026-06-07: Use `test_set_env`/`test_remove_env` helpers in `testing.rs` instead of inline `unsafe {}` blocks. Rationale: 249 call sites, single unsafe audit point, `#[track_caller]` preserves panic location, project already has `testing.rs` as established pattern.
- 2026-06-07: No new dependencies — helpers use only `std::env`.
- 2026-06-07: Each batch is committed separately for clear bisect and review.
- 2026-06-07: No regex/sed bulk replacements — all edits are manual via Read + Edit tool.
- 2026-06-07: Transport-web crate cannot import from `oxide-agent-core::testing` (cfg-test gated) and core has `forbid(unsafe_code)` in lib. Transport-web gets local wrapper functions in its test file instead of cross-crate dependency.
- 2026-06-07: `web_console_dev.rs` example uses inline `unsafe {}` block — not test code, no access to testing module.

## Progress Log

- 2026-06-07: Goal document created after RECON.
  - Evidence: RECON identified 249 unsafe env calls in 10 files, 16 missing `reasoning_effort` fields in 3 files, 1 wrong arg count, 1 missing import, 1 missing struct field.
  - Audit IDs updated: all pending.
  - Next: Checkpoint 1 — add helpers to `testing.rs`.

- 2026-06-07: Checkpoint 1 — `test_set_env` / `test_remove_env` added to `testing.rs`.
  - Changed: `crates/oxide-agent-core/src/testing.rs` — added two `#[track_caller]` pub functions wrapping `unsafe { std::env::set_var/remove_var }`.
  - Evidence: `cargo check -p oxide-agent-core --features profile-full` passes.
  - Audit IDs updated: G1 → verified.
  - Next: Checkpoint 2 — Batch A (core crate test files, ~214 calls).

- 2026-06-07: Checkpoint 2 — Batch A complete (6 core crate test files, 304 calls replaced).
  - Changed:
    - `config.rs`: 92 replacements (env::set_var → test_set_env, env::remove_var → test_remove_env), added `use crate::testing`, removed redundant inner `use std::env`.
    - `registry.rs`: 91 replacements (std::env::set_var → test_set_env, std::env::remove_var → test_remove_env), added import.
    - `bwrap/tests.rs`: 110 replacements, added import.
    - `manager.rs`: 6 replacements, added import.
    - `sqlx_config.rs`: 2 replacements, added import.
    - `modules.rs`: 13 replacements, added import.
  - Refined `test_set_env`/`test_remove_env` signatures to `impl AsRef<OsStr>` to handle `String`, `&str`, and `OsString` call sites.
  - Evidence: `rg` returns zero unsafe env calls in all 6 files. Core compilation passes (remaining errors are Batch B + API drift from Checkpoints 3-4).
  - Audit IDs updated: G2 → in_progress (Batch A done, Batch B pending).
  - Next: Checkpoint 3 — Batch B (transport + remaining files, ~25 calls).

- 2026-06-07: Checkpoint 3 — Batch B complete.
  - Changed:
    - `file_delivery.rs`: 2 replacements, added import.
    - `tts/client.rs`: 1 replacement, added import.
    - `silero_tts/client.rs`: 1 replacement, added import.
    - `transport-web/tests.rs`: 23 replacements, added local wrapper functions (cross-crate import blocked by cfg-test + forbid unsafe).
    - `web_console_dev.rs`: 1 inline `unsafe {}` wrap in example function.
  - Evidence: `rg` returns zero direct `set_var`/`remove_var` call sites outside wrappers. `cargo test -p oxide-agent-core` and `cargo test -p oxide-agent-transport-web` compile (remaining errors are API drift from Checkpoint 4).
  - Audit IDs updated: G2 → in_progress (all env calls replaced; wrappers in 2 locations: core testing.rs + transport-web tests.rs).
  - Next: Checkpoint 4 — API drift (`reasoning_effort`, `before_seq`, `AgentEventSource`).

## Risks and Blockers

- `web_console_dev.rs` example file may not have access to `testing` module (examples vs tests). If import fails, use inline `unsafe {}` block instead.
  - Impact: 1 call site.
  - Evidence: to be verified at Checkpoint 3.
  - Mitigation: fallback to inline `unsafe {}`.
  - Audit IDs affected: G2.

- Some test files may use `env::set_var` via a local `use std::env;` import that also pulls in `env::var()` and other safe calls. The `use std::env;` import must be preserved or trimmed carefully.
  - Impact: `config.rs` test modules use `use std::env;` for both `var()` and `set_var()`.
  - Evidence: to be verified per-file at batch time.
  - Mitigation: keep `use std::env;` for `var()` calls, replace only `set_var`/`remove_var` with helpers.
  - Audit IDs affected: G2.
