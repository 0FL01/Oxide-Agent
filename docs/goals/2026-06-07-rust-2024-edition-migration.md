# Goal: Rust 2024 Edition Migration + Workspace Package Centralization

Date started: 2026-06-07
Status: active
Codex goal: `/goal Implement docs/goals/2026-06-07-rust-2024-edition-migration.md until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update this document after each meaningful verification, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.`
Source spec: User request to migrate all workspace crates to Rust 2024 edition and centralize duplicated package metadata in `[workspace.package]`.
Goal doc owner: Codex
Last updated: 2026-06-07 19:52

## Objective

Migrate all 8 workspace crates from `edition = "2021"` to `edition = "2024"`, centralize shared package metadata (`version`, `edition`, `license`, `authors`) in `[workspace.package]` at workspace root, resolve any edition-related incompatibilities found during build verification, and ensure the full workspace compiles and passes clippy across all validated profiles.

Done when every required Completion Audit item is verified by its listed evidence, all workspace profiles pass `cargo check` and `cargo clippy`, and the Rust 2024 formatting conventions are applied.

## Scope

In scope:
- `Cargo.toml` (workspace root) — add `[workspace.package]` with shared metadata fields.
- All 8 crate `Cargo.toml` files — replace duplicated `version`, `edition`, `license`, `authors` with `*.workspace = true` inheritance.
- Any Rust source files (`*.rs`) that fail to compile under Rust 2024 edition rules.
- `cargo fmt` pass to apply Rust 2024 formatting conventions.
- Goal document and commit history.

Out of scope:
- Dependency version bumps beyond what is strictly required for Rust 2024 compatibility.
- New features, tools, providers, transports, or architectural changes.
- Changing lints (`[lints.rust]`, `[lints.clippy]`) — these remain per-crate.
- Changing `[dependencies]`, `[dev-dependencies]`, `[features]`, or `[package.metadata]`.
- Introducing new crates, services, queues, caches, or abstraction layers.
- Changes to CI/CD pipelines or Docker configurations.

## Missing Inputs

- None. All inputs are available in the repository.

## Repository Context

- Workspace root: `Cargo.toml` with 8 member crates.
- All crates currently use `edition = "2021"`, `version = "0.1.0"`, `license = "MIT"`, `authors = ["@0FL01"]`.
- rustc 1.94.0 installed — Rust 2024 edition was stabilized in 1.85.0, fully compatible.
- No `rust-toolchain.toml` or `rust-version` (MSRV) is defined.
- No `.rustfmt.toml` or `rustfmt.toml` exists — Rust 2024 formatting will use tool defaults.
- Validation profiles used in this repo:
  - Full: `cargo check --workspace --no-default-features --features profile-full`
  - Embedded: `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
  - Web: `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`
  - Bwrap: `cargo check --workspace --no-default-features --features profile-host-bwrap`
  - Clippy: `cargo clippy --workspace --no-default-features --features <profile>`

## RECON Summary

### Rust 2024 Breaking Changes — Codebase Audit

| Concern | Finding | Risk |
|---|---|---|
| `unsafe_op_in_unsafe_fn` deny-by-default | 0 `unsafe fn` in codebase | None |
| `missing_fragment_specifier` hard error | 4 `macro_rules!` macros, all use proper fragment specifiers | None |
| `gen` reserved keyword | No identifier conflicts (`JsCast`, `strip_gen` only) | None |
| `extern` blocks require `unsafe` | 0 `extern` blocks | None |
| `mut ref` / `mut ref mut` removed | No usage found | None |
| `impl Trait` lifetime capture rules | No `impl Trait` in return position with lifetime capture edge cases found | Low |
| `!` never type inference changes | Boolean `!` only, no never-type usage | None |

### Duplicated Package Metadata

All 8 crates have identical values for 4 fields:
```
version = "0.1.0"
edition = "2021"
license = "MIT"
authors = ["@0FL01"]
```

## Completion Audit

- G1: `[workspace.package]` exists in workspace root with all 4 shared fields
  - Source: Centralization requirement.
  - Acceptance: `Cargo.toml` (root) contains `[workspace.package]` with `version`, `edition = "2024"`, `license`, `authors`.
  - Evidence required: file diff showing added `[workspace.package]` block.
  - Status: pending
  - Evidence collected:

- G2: All 8 crate `[package]` sections inherit from workspace
  - Source: Centralization requirement.
  - Acceptance: Each crate's `Cargo.toml` uses `version.workspace = true`, `edition.workspace = true`, `license.workspace = true`, `authors.workspace = true` instead of hardcoded values.
  - Evidence required: file diffs for all 8 crate Cargo.toml files showing workspace inheritance.
  - Status: pending
  - Evidence collected:

- G3: Workspace compiles under Rust 2024 edition
  - Source: Edition migration requirement.
  - Acceptance: `cargo check --workspace --no-default-features --features profile-full` succeeds with zero errors.
  - Evidence required: command output showing successful compilation.
  - Status: pending
  - Evidence collected:

- G4: Clippy passes under Rust 2024 edition
  - Source: Repository quality gate.
  - Acceptance: `cargo clippy --workspace --no-default-features --features profile-full` succeeds with zero errors (deny-level lints pass).
  - Evidence required: command output showing no errors.
  - Status: pending
  - Evidence collected:

- G5: Rust 2024 formatting applied
  - Source: Edition migration best practice.
  - Acceptance: `cargo fmt --all` produces clean output; no diff after formatting.
  - Evidence required: command output and `git diff --name-only` showing format-only changes if any.
  - Status: pending
  - Evidence collected:

- Q1: Edition-related incompatibilities are resolved or documented
  - Source: Possible Rust 2024 breaking changes may surface during compilation.
  - Acceptance: All compilation errors related to edition change are fixed. If any breaking change requires non-trivial refactoring, it is documented in Risks and Blockers with a decision from the user.
  - Evidence required: compilation success after fixes, or explicit blocker documentation.
  - Status: pending
  - Evidence collected:

- Q2: All validated profiles compile
  - Source: Repository supports multiple profiles; migration must not break any.
  - Acceptance: `cargo check` passes for at least `profile-full`, `profile-embedded-opencode-local`, `profile-web-embedded-opencode-local`, and `profile-host-bwrap`.
  - Evidence required: command output for each profile.
  - Status: pending
  - Evidence collected:

- N1: No dependency changes beyond edition compatibility
  - Source: Scope constraint.
  - Must preserve: all current dependency versions, features, and optional flags unchanged unless a dependency explicitly breaks under Rust 2024.
  - Evidence required: `git diff --name-only` and dependency diff review.
  - Status: pending
  - Evidence collected:

- N2: Lints remain per-crate
  - Source: Scope constraint — `[lints.rust]` and `[lints.clippy]` are not centralized.
  - Must preserve: each crate retains its own lint configuration.
  - Evidence required: `git diff` review of lint sections.
  - Status: pending
  - Evidence collected:

- N3: No new files or architectural changes
  - Source: Scope constraint.
  - Must preserve: workspace structure, crate boundaries, module layout.
  - Evidence required: `git diff --name-only` review.
  - Status: pending
  - Evidence collected:

## Implementation Plan

1. Add `[workspace.package]` to workspace root
   - Audit IDs: G1, N1, N3.
   - Expected changes: `Cargo.toml` (root) — add `[workspace.package]` block with `version = "0.1.0"`, `edition = "2024"`, `license = "MIT"`, `authors = ["@0FL01"]`.
   - Validation: `git diff Cargo.toml` review.
   - Exit condition: root Cargo.toml contains the new `[workspace.package]` section.

2. Update all 8 crate Cargo.toml to use workspace inheritance
   - Audit IDs: G2, N1, N2, N3.
   - Expected changes: In each of the 8 crate `Cargo.toml` files, replace `version = "0.1.0"`, `edition = "2021"`, `license = "MIT"`, `authors = ["@0FL01"]` with `version.workspace = true`, `edition.workspace = true`, `license.workspace = true`, `authors.workspace = true`.
   - Validation: `git diff --name-only` review; all 8 files changed.
   - Exit condition: no crate has hardcoded `version`, `edition`, `license`, or `authors`.

3. Full workspace compilation check — resolve incompatibilities
   - Audit IDs: G3, Q1, Q2.
   - Expected changes: fix any Rust 2024 edition compilation errors in `.rs` files if found.
   - Validation: `cargo check --workspace --no-default-features --features profile-full`.
   - Exit condition: zero compilation errors.
   - Incompatibility handling: if `cargo check` fails, diagnose the specific Rust 2024 breaking change, apply minimal fix, re-check. If fix requires scope expansion, document in Risks and Blockers and seek user decision.

4. Profile validation
   - Audit IDs: Q2, Q1.
   - Expected changes: no new changes expected; verify profiles compile cleanly.
   - Validation: `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`; `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`; `cargo check --workspace --no-default-features --features profile-host-bwrap`.
   - Exit condition: all profiles pass.

5. Clippy check
   - Audit IDs: G4.
   - Expected changes: fix any new clippy warnings introduced by Rust 2024 edition.
   - Validation: `cargo clippy --workspace --no-default-features --features profile-full`.
   - Exit condition: zero deny-level clippy warnings.

6. Formatting pass
   - Audit IDs: G5.
   - Expected changes: `cargo fmt --all` may produce formatting diffs under Rust 2024 rules.
   - Validation: `cargo fmt --all -- --check` succeeds; `git diff --name-only` review.
   - Exit condition: code is formatted to Rust 2024 conventions.

7. Final audit and commit
   - Audit IDs: all.
   - Expected changes: update this goal doc with evidence; final commit with all changes.
   - Validation: `git status --short` clean; all audit items verified.
   - Exit condition: Completion Audit fully verified.

## Validation Contract

- Static checks:
  - `git diff --check`
  - `git diff --name-only` — only Cargo.toml and *.rs files expected
- Rust checks:
  - `cargo fmt --all -- --check`
  - `cargo check --workspace --no-default-features --features profile-full`
  - `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
  - `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`
  - `cargo check --workspace --no-default-features --features profile-host-bwrap`
  - `cargo clippy --workspace --no-default-features --features profile-full`
- Artifact verification:
  - Workspace root `Cargo.toml` has `[workspace.package]` block
  - All 8 crate `Cargo.toml` use `*.workspace = true`
  - `git log --oneline -1` shows the migration commit
- Done when: all non-dropped Completion Audit items are verified with current evidence.

## Decisions

- 2026-06-07: Use `edition = "2024"` in `[workspace.package]` so all crates inherit the new edition from a single source of truth.
- 2026-06-07: Centralize exactly 4 fields (`version`, `edition`, `license`, `authors`) that are identical across all 8 crates. Lints, dependencies, features, and metadata remain per-crate.
- 2026-06-07: Run `cargo fmt` after edition change because Rust 2024 has new formatting conventions that may produce diffs.
- 2026-06-07: Validate multiple profiles (full, embedded, web, bwrap) because the workspace uses feature-gated compilation and not all code compiles under every profile.
- 2026-06-07: Treat any Rust 2024 incompatibility as a potential scope expansion — fix if minimal, document and seek user decision if non-trivial.

## Progress Log

- 2026-06-07 19:52: Goal document created after RECON.
  - Changed: Created this goal contract with RECON summary, completion audit, and implementation plan.
  - Evidence: RECON completed. All 8 crates inventory confirmed. Rust 2024 breaking changes audited — no high-risk items found. rustc 1.94.0 available. Duplicated metadata confirmed across all crates.
  - Commands: `rustc --version`; grep/rg scans for `unsafe fn`, `macro_rules!`, `gen`, `extern`, `mut ref`; file reads of all 8 crate Cargo.toml files.
  - Audit IDs updated: G1-G5 pending, Q1-Q2 pending, N1-N3 pending.
  - Next: Checkpoint 1 — add `[workspace.package]` to root Cargo.toml.

## Risks and Blockers

- Rust 2024 `impl Trait` capture rule change may affect type inference in edge cases.
  - Impact: possible new compilation errors in code using `impl Trait` return types with lifetime parameters.
  - Evidence: RECON found no obvious `impl Trait` in return position patterns that would break, but edge cases may exist in transitive dependencies or complex generic code.
  - Mitigation or requested decision: if compilation fails, diagnose the specific lifetime capture issue and apply minimal fix; if fix is non-trivial, document and seek user decision.
  - Audit IDs affected: Q1, G3.

- `unsafe_op_in_unsafe_fn` is deny-by-default in Rust 2024.
  - Impact: any `unsafe fn` body without an explicit `unsafe` block would fail.
  - Evidence: RECON found 0 `unsafe fn` in the codebase — this is a non-risk.
  - Mitigation or requested decision: none needed.
  - Audit IDs affected: none.

- Third-party dependencies may not support Rust 2024 edition.
  - Impact: dependencies using `edition = "2021"` should still compile, but edge cases with edition-specific behavior in proc macros or build scripts are possible.
  - Evidence: RECON did not identify specific problematic dependencies.
  - Mitigation or requested decision: if a dependency breaks, check for updated version; if no fix exists, document and seek user decision on pinning or workaround.
  - Audit IDs affected: Q1, G3.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
