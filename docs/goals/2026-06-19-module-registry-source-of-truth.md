# Goal: Module registry as source of truth

Date started: 2026-06-19
Status: active
Codex goal: see `/goal` objective below
Source spec: user-approved RECON and decisions from 2026-06-19
Goal doc owner: Codex
Last updated: 2026-06-19 13:00

## Objective

Replace the manually synchronized Cargo feature/profile, runtime capability manifest, profile TOML, provider wiring, and feature-gated test contract with a single declarative module registry that is verified by generated artifacts and matrix gates.

Done when every Completion Audit item is verified by its listed evidence, each checkpoint has its own blast-radius review, and every completed checkpoint is committed before the next one starts.

## Codex `/goal` objective

```text
/goal Implement docs/goals/2026-06-19-module-registry-source-of-truth.md until every Completion Audit item is verified by its required evidence, preserving listed constraints and non-goals. Work checkpoint by checkpoint, update the doc after each meaningful verification, commit each completed checkpoint, and stop only on verified completion or a blocker with exact evidence and smallest external action needed.
```

## Scope

In scope:
- Declarative module registry, expected path: `crates/oxide-agent-core/module_registry.toml` unless implementation discovers a better repo-local placement.
- Generator/check command, expected shape: `xtask module-registry check` plus generation mode.
- Generated or checked feature/profile surfaces for `Cargo.toml` feature blocks, `profiles/*.toml`, `compiled.rs`, and test cfg aliases.
- Raw feature-gated tests that encode module/capability availability.
- Shared runtime shape in `ToolModuleContext`, `ToolModuleContextParts`, and `LlmClient` where feature-gated fields make public/runtime shape profile-dependent.
- Validation docs and local profile matrix commands.

Out of scope:
- Adding product modules, providers, transports, LLM behavior, browser behavior, or user-facing capabilities except to preserve current behavior.
- Replacing Cargo as the build system.
- Runtime plugin loading or dynamic linking.
- New storage backend, service, queue, cache, or broad observability layer.
- Changing user-visible profile names unless required to eliminate a verified contradiction.
- Silently dropping a currently compiled module from a profile without explicit audit evidence and decision log update.

## Missing Inputs

(none now — user approved the proposed decisions. If implementation reveals a Cargo limitation or dependency cycle, record it as a blocker with the exact command/output.)

## Repository Context

- Current source split verified by RECON:
  - Cargo profile features: `crates/oxide-agent-core/Cargo.toml:72`, `:112`, `:134`, `:158`.
  - Transport/binary forwarding: `crates/oxide-agent-telegram-bot/Cargo.toml:34`, `crates/oxide-agent-transport-telegram/Cargo.toml:32`, `crates/oxide-agent-transport-web/Cargo.toml:46`.
  - Capability manifest: `crates/oxide-agent-core/src/capabilities/compiled.rs:8`, `:24`, `:255`, `:301`, `:327`, `:385`, `:559`.
  - Runtime enabled manifest: `crates/oxide-agent-core/src/config.rs:224`, `:233`.
  - Shared shape drift: `crates/oxide-agent-core/src/agent/tool_runtime/modules.rs:216`, `crates/oxide-agent-core/src/llm/client.rs:14`.
  - Raw feature-gated tests: `crates/oxide-agent-core/src/agent/executor/tests/basics.rs:193`, `:208`; `crates/oxide-agent-core/tests/sub_agent_delegation.rs:213`; `crates/oxide-agent-core/src/capabilities/manifest.rs:965`, `:1020`.
- Real drift already verified:
  - Cargo compiles Browser Live in full/web profiles: `crates/oxide-agent-core/Cargo.toml:92`, `:152`.
  - Profile TOMLs omit runtime enablement and keep stale deferred comment: `profiles/full.toml:31`, `:32`; `profiles/web-embedded-opencode-local.toml:17`, `:26`.
  - Snapshot reflects Cargo world: `crates/oxide-agent-core/tests/snapshots/modular_registry_snapshots__modular_registry_snapshot@profile-full.snap:658`, `:661`.
- Existing conventions:
  - Default Cargo features intentionally empty: `AGENTS.md:55`.
  - Profile-specific tests require scoped commands: `AGENTS.md:144`.
  - Style expectations: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings` in `AGENTS.md:137`.
- No `.github/workflows/*` files were found in this checkout; local validation commands are authoritative unless CI is added later.

## Contract Analysis

Current transmitting side: Cargo feature/profile selections.

Current receiving sides:
- Rust module declarations.
- Tool/provider registration and optional dependencies.
- Runtime profile TOMLs.
- Capability manifest and enabled manifest.
- Tests and snapshots.
- Deployment/build documentation.

Current unreliable requirements:
- Every receiver must manually know stable Cargo feature strings.
- Every receiver must manually map feature strings to module IDs and capability IDs.
- Tests must encode dependency requirements such as sandbox backend combinations.
- Profile TOMLs must manually mirror Cargo profile membership.
- Shared runtime structs must compile to different shapes under different profiles.

Corrected contract:
- Developers declare module facts once in the registry: module id, kind, Cargo feature, dependency features, provided capabilities, required capabilities, profile membership, and test requirements.
- Cargo/Rust/profile/test surfaces consume generated artifacts or generated cfg aliases.
- Runtime callers pass intent/config; receiving registries resolve availability from the generated manifest.
- Public/shared runtime structs keep stable shape across profiles; feature-gated code is confined to provider adapters and generated registration boundaries.

## Completion Audit

- G1: Declarative module registry exists and covers all current modules/profiles
  - Source: user-approved decision `module_registry.toml` in repo and RECON source split.
  - Acceptance: registry describes every module currently emitted by `compiled_capability_manifest()`, every atomic Cargo capability feature, all supported profiles, module kind, provided capabilities, required capabilities, and generated feature dependencies.
  - Evidence required: generator/check report showing zero missing/extra modules/features/profiles; diff or snapshot proving registry coverage of current compiled manifest for all supported profiles.
  - Status: in_progress
  - Evidence collected: CP1 added `crates/oxide-agent-core/module_registry.toml` with 40 module records. `cargo run -p xtask -- module-registry check` passed and reported `40 modules`, `45 Cargo features`, and `40 compiled declarations`. CP4 enhanced check to also verify `provides` (ordered capability list) and `requires` presence for all 40 modules — all match.

- G2: Cargo feature/profile surfaces are generated or checked from the registry
  - Source: user-approved decision checked-in generated files plus check gate.
  - Acceptance: core profile feature lists and transport/binary forwarding cannot drift from registry without `xtask module-registry check` failing. Cargo defaults remain empty.
  - Evidence required: clean `xtask module-registry check`; intentional mismatch test or unit/snapshot equivalent; inspected generated sections in relevant Cargo.toml files.
  - Status: verified
  - Evidence collected: CP2 added `generate` subcommand and marked `# BEGIN/END OXIDE-REGISTRY: profiles` section in core Cargo.toml. `cargo run -p xtask -- module-registry generate` regenerates the 4 profile feature lists from registry module order. `check` verifies the marked section is not stale and verifies forwarding crates (transport-telegram, transport-web, telegram-bot) have correct profile features forwarding to `oxide-agent-core/profile-X`. Registry drift `tool/brave-search`/`tool/crw` in `embedded-opencode-local` fixed (registry now matches Cargo, not runtime TOML — runtime TOML fix is CP3). All 5 profile `cargo check` commands pass. `default = []` preserved at line 67.

- G3: Runtime profile TOMLs are generated or checked from the same registry
  - Source: verified Browser Live drift and user-approved policy.
  - Acceptance: `profiles/*.toml` module membership matches registry; Browser Live is explicitly included and enabled in `full` and `web-embedded-opencode-local`; stale deferred comments are removed.
  - Evidence required: `xtask module-registry check`; `git grep 'Browser Live Agent profile wiring lands with CP-7' profiles/` returns nothing; profile files contain `tool/browser-live` where registry says enabled.
  - Status: verified
  - Evidence collected: CP3 `generate` now generates all 4 `profiles/*.toml` files from registry; `check` does exact content comparison (no warnings, no errors); `git grep 'Browser Live Agent profile wiring lands with CP-7' profiles/` returns nothing (exit 1); `tool/browser-live` present in `profiles/full.toml:22` and `profiles/web-embedded-opencode-local.toml:11`; `tool/brave-search` and `tool/crw` removed from `profiles/embedded-opencode-local.toml` (matching Cargo profile). RECON confirmed no Rust runtime code or scripts read these files — they are reference-only.

- G4: `compiled.rs` module declarations are generated or checked from the registry
  - Source: RECON `compiled.rs` feature-gated macros duplicate Cargo/profile knowledge.
  - Acceptance: module id, kind, cargo feature, provides, requires, and config schema references in compiled manifest are derived from registry or compared against registry by a failing check.
  - Evidence required: clean `xtask module-registry check`; focused tests for `compiled_capability_manifest()`; snapshot update showing no unintended module loss.
  - Status: verified
  - Evidence collected: CP4 enhanced `xtask module-registry check` to parse and compare `provides` (ordered capability ID list) and `requires` presence (macro variant `push_module_with_requires!` vs registry `requires` field) for all 40 modules. Config properties remain in Rust (builder expressions with env/defaults — not expressible in TOML); config schema drift is caught by existing snapshot tests and `openrouter_module_declares_provider_config_schema`. `cargo test -p oxide-agent-core --no-default-features --features profile-full --test modular_registry_snapshots` passes (snapshot unchanged). `cargo test -p oxide-agent-core --no-default-features --features profile-full -- capabilities` passes (34 tests). Intentional mismatch test: adding `tool/extra-cap` to registry provides for `tool/todos` → check fails with `provides mismatch for module 'tool/todos': registry=["tool/todos", "tool/extra-cap"] compiled=["tool/todos"]`. Removing `requires` from registry for `tool/sandbox-fileops` → check fails with `requires mismatch for module 'tool/sandbox-fileops': registry_requires=false compiled_uses_push_module_with_requires=true`.

- G5: Test gating uses module/capability requirements instead of raw feature knowledge where practical
  - Source: user-approved cfg alias plan.
  - Acceptance: tests that assert module/capability behavior use generated cfg aliases such as `oxide_module` or `oxide_capability`; compound sandbox/backend requirements are derived from registry data or registry-owned helpers instead of manually repeated feature combinations.
  - Evidence required: `git grep '#\[cfg(feature =' crates/oxide-agent-core/src/agent/executor crates/oxide-agent-core/tests` reviewed with remaining occurrences justified; tests pass under relevant profile matrix.
  - Status: verified
  - Evidence collected: CP5 added `crates/oxide-agent-core/build.rs` that reads `module_registry.toml` and emits `oxide_module_<id>` cfg aliases (with `cargo:rustc-check-cfg` declarations) for each enabled module. Verified `CARGO_FEATURE_*` env vars include transitively-enabled features (profile-full → tool-todos → `CARGO_FEATURE_TOOL_TODOS=1`). Migrated ~149 `#[cfg(feature = "...")]` to `#[cfg(oxide_module_...)]` across 15 test files: `executor/tests/{mod,basics,resume,registry}.rs`, `tests/{sub_agent_delegation,json_decode_error,anthropic_e2e,mistral_e2e,hermetic_agent,rate_limit}.rs`, `capabilities/{compiled,manifest}.rs` test modules, `llm/{client,capabilities}.rs` test modules, `llm/providers/modules.rs` test module. `git grep 'feature = "' crates/oxide-agent-core/src/agent/executor/tests/` returns 0 results. `git grep 'feature = "' crates/oxide-agent-core/tests/` returns only profile gates in `modular_registry_snapshots.rs` (justified: profile features are composite Cargo features, not module features). All remaining `feature = "..."` in `llm/` is implementation code (lines before test module boundary), justified per goal decision "raw Cargo features remain acceptable only for implementation/dependency gating" (CP6/CP7 will address shared struct shape). `cargo test -p oxide-agent-core --no-default-features --features profile-full` passes: 1328 lib + 35 integration tests, 0 failed. All 5 profile `cargo check` commands pass. `cargo clippy --workspace --all-targets --features profile-full -- -D warnings` passes.

- G6: Shared runtime structs no longer expose profile-dependent public shape
  - Source: RECON on `ToolModuleContext`, `ToolModuleContextParts`, and `LlmClient`.
  - Acceptance: shared structs keep stable fields across supported profiles; feature-gated provider-specific heavy code is inside adapters/registrations; no call site must construct a different shape for different profiles.
  - Evidence required: `git grep '#\[cfg(feature ='` around `ToolModuleContext`, `ToolModuleContextParts`, and `pub struct LlmClient` shows no feature-gated fields; all call sites compile in matrix.
  - Status: pending
  - Evidence collected:

- G7: Registry matrix gate covers supported profiles
  - Source: user-approved matrix gate.
  - Acceptance: validation includes default/no-default, embedded, web-embedded, search-only, full, and scoped web tests where workspace-wide tests are not valid for a transport-specific profile.
  - Evidence required: command outputs for the Validation Contract matrix, or documented pre-existing/environment failures proven by rollback or import-scope evidence.
  - Status: pending
  - Evidence collected:

- Q1: No new service/storage/cache/queue or unjustified dependency layer
  - Source: AGENTS.md scale/implementation bias.
  - Acceptance: generator/check is repo-local tooling; no runtime service or external dependency is introduced. Any new Rust crate inside workspace is justified as `xtask` tooling only.
  - Evidence required: `git diff -- Cargo.toml crates/*/Cargo.toml` review; dependency additions listed with reason.
  - Status: in_progress
  - Evidence collected: CP1 added only a workspace `xtask` crate with no dependencies; `Cargo.lock` gained only the local `xtask` package entry.

- Q2: Each checkpoint includes blast-radius review before code and evidence after code
  - Source: user request and П0.6.
  - Acceptance: every checkpoint log names touched symbols/files, consumers, regression hypotheses, validation, failures, and classification.
  - Evidence required: Progress Log entries for CP0..final; `git grep`/diff/status evidence before each checkpoint commit.
  - Status: in_progress
  - Evidence collected: CP1 blast radius reviewed before implementation: root workspace members, root `Cargo.toml`, new `xtask`, `Cargo.lock`, core registry path, runtime profiles, and `compiled.rs` parser surface. CP2 blast radius reviewed for Cargo.toml profile section and forwarding crates. CP3 blast radius reviewed for profile TOML files (RECON confirmed no runtime code reads them). CP4 blast radius reviewed: only `xtask/src/main.rs` changed (80 insertions, 33 deletions); no `compiled.rs`, `module_registry.toml`, Cargo.toml, or Rust source changes; no snapshot changes expected (compiled.rs unchanged) — confirmed by snapshot test passing. CP5 blast radius reviewed: new `build.rs` affects all compilations of `oxide-agent-core` (unit tests, integration tests, examples, main crate); cfg aliases are per-package (not visible to dependents like `transport-web`); 15 test files modified with mechanical `feature = "X"` → `oxide_module_Y` replacements; implementation code untouched (verified by grep — all remaining `feature = "..."` in `llm/` is before test module boundary); profile gates in `modular_registry_snapshots.rs` intentionally not migrated (profile features are composite, not module features); `transport-web` tests use their own crate features (not affected by core's build.rs).

- Q3: Generated artifacts are checked in and drift-proofed
  - Source: user-approved decision.
  - Acceptance: ordinary `cargo check` works from a fresh checkout without first running a generator; check command fails if generated surfaces are stale.
  - Evidence required: clean checkout-equivalent `cargo check` command; `xtask module-registry check` output; changed generated files committed.
  - Status: in_progress
  - Evidence collected: CP1 adds a checked-in registry and check gate; CP2 adds generated profile section in core Cargo.toml with `generate`/`check` drift gate; CP3 adds generated `profiles/*.toml` with exact content check; `cargo check --workspace --no-default-features` passes without running generate first (all generated sections are checked in); CP4 enhances check to verify `compiled.rs` `provides` and `requires` presence against registry — no generation needed (check-only approach, config properties remain in Rust).

- N1: Cargo remains the build system with empty default features
  - Source: AGENTS.md and approved plan.
  - Must preserve: `default = []`; profile feature names stay available.
  - Evidence required: `grep -n 'default = \[\]' crates/oxide-agent-core/Cargo.toml`; profile feature names still compile.
  - Status: verified
  - Evidence collected: CP2 verified `default = []` at `crates/oxide-agent-core/Cargo.toml:67`; all 5 profile `cargo check` commands pass (`no-default`, `profile-embedded-opencode-local`, `profile-search-only`, `profile-full`, `profile-web-embedded-opencode-local`).

- N2: Existing runtime behavior is preserved except explicit Browser Live profile drift correction
  - Source: scope boundary and RECON.
  - Must preserve: no module is removed from compiled profiles unless documented; Browser Live full/web runtime enablement is explicit.
  - Evidence required: before/after compiled manifest snapshots; profile TOML review.
  - Status: verified
  - Evidence collected: CP3 generated profile TOMLs match Cargo compiled profiles exactly — `tool/browser-live` now explicitly enabled in `full` and `web-embedded-opencode-local` TOMLs (was compiled but not listed); `tool/brave-search` and `tool/crw` removed from `embedded-opencode-local` TOML (were listed but not compiled); all 5 `cargo check` profile commands pass (CP2 evidence); no Rust runtime code reads profile TOMLs (RECON verified by grep).

- N3: No direct Gemini provider or transport/core dependency inversion is introduced
  - Source: AGENTS.md architecture invariants.
  - Must preserve: Gemini remains OpenRouter-routed; core/runtime do not depend on transport crates.
  - Evidence required: `cargo tree -p oxide-agent-core` or import grep review if dependencies change; code review of generated/tooling imports.
  - Status: pending
  - Evidence collected:

## Implementation Plan

### CP0 — Registry design verification and exact current-state inventory

- Audit IDs: G1, G2, G3, G4, Q2, N1, N2.
- Expected changes: update this doc only with verified inventory and the exact generator format before implementation; no code behavior change.
- Blast radius to review before code: all `Cargo.toml` feature blocks, `profiles/*.toml`, `compiled.rs`, snapshots, tests gated on features.
- Regression hypotheses: inventory misses a module with multiple module IDs per feature; opencode-go/opencode-zen and webfetch-md/web-crawler one-feature-many-modules mapping gets modeled incorrectly; Browser Live policy accidentally changes compiled membership.
- Validation: targeted reads/greps; `cargo metadata --no-deps --format-version 1`; current compiled manifest/profile command if available.
- Exit condition: registry schema is fully specified in the doc with every exception named.
- Commit: `docs(goal): define module registry inventory`.

### CP1 — Add registry and `xtask module-registry check` in check-only mode

- Audit IDs: G1, G2, G3, G4, Q1, Q2, Q3.
- Expected changes: add registry file; add minimal workspace `xtask` if absent; parser/checker compares registry to existing Cargo/profile/compiled facts without rewriting them yet.
- Blast radius to review before code: workspace members, root `Cargo.toml`, new tooling crate, `Cargo.lock`, local developer commands.
- Regression hypotheses: new workspace member changes default workspace commands; new deps slow or break minimal profile; parser accepts duplicate module IDs silently; check command only validates happy path.
- Validation: `cargo run -p xtask -- module-registry check`; focused unit tests for duplicate IDs/missing features if xtask has tests; `cargo check --workspace --no-default-features`.
- Exit condition: check command passes on current state or reports only the Browser Live drift as an explicitly classified known mismatch.
- Commit: `feat(registry): add module registry check`.

### CP2 — Make Cargo feature/profile blocks registry-owned

- Audit IDs: G2, Q2, Q3, N1.
- Expected changes: generated marked sections or generator-backed check for core profile feature lists and transport/binary forwarding.
- Blast radius to review before code: every profile feature in core, telegram bot, telegram transport, web transport; binary `required-features`; default feature behavior.
- Regression hypotheses: forwarding omits storage feature in transports; `chatgpt-login` loses `llm-chatgpt`; web e2e feature flags collide with generated profile flags; profile names change.
- Validation: `cargo run -p xtask -- module-registry check`; `cargo check --workspace --no-default-features`; profile check commands for embedded/search/full.
- Exit condition: registry drift in Cargo surfaces is impossible without check failure.
- Commit: `feat(registry): generate cargo profile surfaces`.

### CP3 — Make `profiles/*.toml` registry-owned and fix Browser Live drift

- Audit IDs: G3, N2, Q2, Q3.
- Expected changes: generated/checked profile TOMLs; Browser Live included in `full` and `web-embedded-opencode-local`; stale comments removed.
- Blast radius to review before code: runtime module enablement, config examples, snapshot expectations, web/telegram profile behavior.
- Regression hypotheses: enabling Browser Live at runtime requires config not present; generated profile omits opencode-zen/web-crawler secondary modules; operational profiles diverge from compiled profile names.
- Validation: `cargo run -p xtask -- module-registry check`; `git grep 'Browser Live Agent profile wiring lands with CP-7' profiles/`; capability enabled-manifest tests/snapshots.
- Exit condition: Cargo compiled profile and runtime profile TOML membership match the registry.
- Commit: `fix(registry): align runtime profiles with registry`.

### CP4 — Make capability manifest declarations registry-owned

- Audit IDs: G4, G1, N2, Q2.
- Expected changes: generate `compiled.rs` declarations or make existing declarations checked by registry-owned tests; preserve config schema constants and requirement semantics.
- Blast radius to review before code: `capabilities` module API, snapshots, config schema output commands, module kind/provides/requires ordering.
- Regression hypotheses: deterministic ordering changes snapshots; config properties disconnect from modules; feature-gated constants cause dead code warnings; one feature with two modules modeled incorrectly.
- Validation: `cargo test -p oxide-agent-core --no-default-features --features profile-full capabilities`; snapshot tests for modular registry; `cargo run ... capabilities --compiled --json` for representative profiles if binaries compile.
- Exit condition: compiled manifest cannot drift from registry without failing check/test.
- Commit: `feat(registry): drive capability manifest from registry`.

### CP5 — Add generated module/capability cfg aliases and migrate tests

- Audit IDs: G5, G7, Q2.
- Expected changes: build script or generated cfg surface emits domain aliases; tests use module/capability requirements rather than raw Cargo feature names where they validate module behavior.
- Blast radius to review before code: `build.rs`, test compilation under no-default and profiles, Rust `unexpected_cfgs` lint if present, all raw feature-gated tests.
- Regression hypotheses: cfg aliases are not visible to integration tests; aliases hide code from typecheck; remaining raw feature cfg is still appropriate for dependency-gated implementation code but not tests.
- Validation: grep review of remaining test cfg; profile matrix tests; `cargo check --workspace --no-default-features`.
- Exit condition: test requirement contract is expressed in module/capability terms or each raw feature exception is documented.
- Commit: `test(registry): gate tests by module capabilities`.

### CP6 — Stabilize shared runtime context shape

- Audit IDs: G6, Q2, N2, N3.
- Expected changes: replace feature-gated fields in `ToolModuleContext`/`ToolModuleContextParts` with a stable service/context registry or always-compiled lightweight context slots; keep heavy provider adapters feature-gated.
- Blast radius to review before code: all construction sites for `ToolModuleContextParts`, all module accessors, provider modules, executor setup, transports that pass optional contexts.
- Regression hypotheses: optional context becomes unavailable at runtime; erased registry loses type safety; always-compiled context accidentally pulls optional heavy deps; sub-agent restrictions change.
- Validation: exhaustive `git grep ToolModuleContextParts`; `git grep '#\[cfg(feature =' crates/oxide-agent-core/src/agent/tool_runtime/modules.rs`; full and slim profile checks/tests.
- Exit condition: shared context struct shape no longer changes by feature; all module access remains explicit and typed enough for current usage.
- Commit: `refactor(runtime): stabilize tool module context shape`.

### CP7 — Stabilize `LlmClient` shape and provider discovery ownership

- Audit IDs: G6, Q2, N2.
- Expected changes: move provider-specific catalog storage/discovery out of `LlmClient` fields into provider/module layer or stable erased registry; remove compound cfg field from shared client shape.
- Blast radius to review before code: model discovery calls, OpenCode Go/OpenCode Zen/OpenAI-base route behavior, `LlmClient::new`, provider module registration, config tests.
- Regression hypotheses: discovered model list loses OpenCode Zen; OpenAI-base catalog sharing breaks; route aliases change; media model selection changes.
- Validation: grep for feature-gated fields in `llm/client.rs`; LLM provider module tests; profile matrix check.
- Exit condition: `pub struct LlmClient` has stable shape across supported profiles.
- Commit: `refactor(llm): move discovery state behind providers`.

### CP8 — Full matrix validation and documentation refresh

- Audit IDs: G7, Q1, Q2, Q3, N1, N2, N3.
- Expected changes: update AGENTS/README/docs only if commands or registry workflow changed; fill Completion Audit evidence.
- Blast radius to review before code: developer workflow docs, profile command docs, generated artifacts, entire workspace test surface.
- Regression hypotheses: docs promise commands that do not pass; workspace-wide test command invalid for transport-specific profile; failures misclassified without proof.
- Validation: full Validation Contract below.
- Exit condition: every audit item verified or blocked with exact evidence and smallest external action; final commit made.
- Commit: `docs(registry): record module registry validation`.

## Validation Contract

Static and registry gates:
- `cargo run -p xtask -- module-registry check`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`

Profile check matrix:
- `cargo check --workspace --no-default-features`
- `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
- `cargo check --workspace --no-default-features --features profile-search-only`
- `cargo check --workspace --no-default-features --features profile-full`
- `cargo check -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`

Test matrix:
- `cargo test --workspace --no-default-features --features profile-full`
- `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`
- Additional focused tests named by the checkpoint when a narrower gate gives better failure locality.

Artifact verification:
- `git grep 'Browser Live Agent profile wiring lands with CP-7' profiles/` returns nothing.
- `git grep '#\[cfg(feature =' crates/oxide-agent-core/src/agent/executor crates/oxide-agent-core/tests` reviewed and remaining occurrences documented.
- `git grep '#\[cfg(feature =' crates/oxide-agent-core/src/llm/client.rs crates/oxide-agent-core/src/agent/tool_runtime/modules.rs` reviewed for shared-shape fields.
- `git status --short` clean after each checkpoint commit.

Done when all Completion Audit items are `verified`, generated artifacts are checked in, each checkpoint has a commit, and any command failure is classified with proof rather than assumption.

## Decisions

- 2026-06-19: Source of truth is a pre-Cargo declarative registry file, not Rust `compiled_capability_manifest()`, because Cargo features/dependencies must exist before Rust compilation.
- 2026-06-19: Generated outputs are checked in, with a hard `xtask module-registry check` drift gate.
- 2026-06-19: Browser Live is treated as really included in `full` and `web-embedded-opencode-local`; runtime profiles must be aligned to Cargo instead of silently compiling disabled code.
- 2026-06-19: Use one `profiles` membership list for compiled-and-enabled modules unless a future checkpoint proves a real need for separate `compiled_profiles` and `enabled_profiles`.
- 2026-06-19: Generate/check only marked Cargo feature/profile sections, not whole Cargo manifests.
- 2026-06-19: Test requirements should be domain-level module/capability cfg aliases; raw Cargo features remain acceptable only for implementation/dependency gating and documented test exceptions.
- 2026-06-19: Stabilize shared runtime shapes using a `ModuleServiceRegistry`-style design or always-compiled lightweight contexts, whichever preserves explicitness with smaller blast radius after code inspection.
- 2026-06-19: Move provider-specific `LlmClient` discovery state into provider/module ownership or a stable erased registry; do not keep feature-gated public/shared fields.

## Progress Log

- 2026-06-19 09:55: CP0 goal contract created
  - Changed: created this goal document from verified RECON and user-approved decisions.
  - Evidence: existing goal convention inspected at `docs/goals/2026-06-18-browser-screenshots-postgres.md`; no `.github/workflows/*` found; current worktree was clean before doc creation.
  - Commands: `git status --short`; targeted reads of `README.md`, root `Cargo.toml`, existing goal doc, and relevant RECON files from prior verification; `cargo fmt --all -- --check` passed after doc creation.
  - Audit IDs updated: Q2 in progress only; implementation audit items remain pending.
  - Next: CP1 design/inventory and check-only registry tooling after reviewer approval of this goal doc.

- 2026-06-19 10:47: CP1 registry and check-only gate implemented
  - Changed: added `crates/oxide-agent-core/module_registry.toml`, new no-dependency workspace crate `xtask`, and root workspace membership for `xtask`.
  - Blast radius reviewed: root workspace membership and `Cargo.lock` local package entry; no runtime crate depends on `xtask`; parser/checker reads `crates/oxide-agent-core/Cargo.toml`, `profiles/*.toml`, and `crates/oxide-agent-core/src/capabilities/compiled.rs` without rewriting them.
  - Regression hypotheses checked: duplicate registry module IDs fail; missing Cargo features fail; extra/missing compiled declarations fail; runtime profile drift fails except explicitly classified current Browser Live mismatch.
  - Evidence: `cargo run -p xtask -- module-registry check` passed with warnings only for missing `tool/browser-live` in `profiles/full.toml` and `profiles/web-embedded-opencode-local.toml`; check reported `40 modules`, `45 Cargo features`, `40 compiled declarations`.
  - Commands: `cargo run -p xtask -- module-registry check`; `cargo fmt --all -- --check`; `cargo check --workspace --no-default-features`.
  - Audit IDs updated: G1, G2, G3, G4, Q1, Q2, Q3 moved to `in_progress` with CP1 evidence.
  - Next: review diff and commit CP1, then CP2 makes Cargo profile/forwarding surfaces registry-owned.

- 2026-06-19 11:30: CP2 Cargo profile/forwarding surfaces registry-owned
  - Changed: `module_registry.toml` (removed `embedded-opencode-local` from `tool/brave-search` and `tool/crw` — Cargo does not compile them for that profile); `crates/oxide-agent-core/Cargo.toml` (added `# BEGIN/END OXIDE-REGISTRY: profiles` markers, regenerated 4 profile feature lists from registry via `generate`); `xtask/src/main.rs` (added `generate` subcommand, `compute_profile_compositions`, `render_profile_section`, `check_core_profile_section`, `check_forwarding`, `check_profile_coverage`, `parse_cargo_features_with_deps`, `brackets_balanced`, known drift for brave-search/crw embedded extras).
  - Blast radius reviewed: core `Cargo.toml` profile section (feature sets unchanged, only order changed to match registry module order — Cargo treats arrays as sets); forwarding crates (transport-telegram, transport-web, telegram-bot — verified all have correct profiles and core forwarding); `xtask` is dev tooling only, no runtime dependency; `module_registry.toml` only consumed by xtask.
  - Regression hypotheses checked: (1) feature reordering changes Cargo behavior — NO, arrays are sets; (2) missing features in generated profiles — verified same sets; (3) forwarding check false positives — verified all 3 crates pass; (4) parser edge cases — tested on real Cargo.toml; (5) brave-search/crw embedded drift classification — real stale entry in runtime TOML, to be fixed in CP3.
  - Evidence: `cargo run -p xtask -- module-registry check` passed (40 modules, 45 features, 40 declarations, 4 known runtime-profile warnings); `cargo fmt --all -- --check` passed; `cargo clippy -p xtask -- -D warnings` passed; `cargo check --workspace --no-default-features` passed; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` passed; `cargo check --workspace --no-default-features --features profile-search-only` passed; `cargo check --workspace --no-default-features --features profile-full` passed; `cargo check -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local` passed; `grep -n 'default = \[\]' crates/oxide-agent-core/Cargo.toml` confirmed at line 67.
  - Commands: all of the above.
  - Audit IDs updated: G2 verified, N1 verified, Q3 in_progress (CP2 evidence added), Q2 in_progress (CP2 blast radius + regression hunt recorded).
  - Next: CP3 — make `profiles/*.toml` registry-owned and fix Browser Live + brave-search/crw runtime drift.

- 2026-06-19 11:50: CP3 profile TOMLs registry-owned and Browser Live drift fixed
  - Changed: `profiles/full.toml`, `profiles/web-embedded-opencode-local.toml`, `profiles/embedded-opencode-local.toml`, `profiles/search-only.toml` (all regenerated from registry via `generate`); `xtask/src/main.rs` (added `render_profile_toml`, `generate_profile_tomls`, `check_profile_tomls` with exact content comparison; removed `check_profiles`, `parse_profile_modules`, `is_known_runtime_profile_drift`, and `warnings` vector — no known drifts remain).
  - Blast radius reviewed: RECON confirmed no Rust runtime code or scripts read `profiles/*.toml` — they are reference-only files; only xtask reads them; generated format matches previous format (alphabetical module order, same header fields).
  - Regression hypotheses checked: (1) adding `tool/browser-live` to runtime TOMLs — no runtime effect, TOMLs are reference-only; (2) removing `tool/brave-search`/`tool/crw` from embedded TOML — correct, Cargo doesn't compile them for embedded; (3) generated format mismatch — only xtask reads them, self-consistent; (4) stale comment removal — correct, comment referenced CP-7 which no longer exists.
  - Evidence: `cargo run -p xtask -- module-registry check` passed with zero warnings and zero errors; `git grep 'Browser Live Agent profile wiring lands with CP-7' profiles/` returned nothing (exit 1); `tool/browser-live` present in `profiles/full.toml:22` and `profiles/web-embedded-opencode-local.toml:11`; `cargo fmt --all -- --check` passed; `cargo clippy -p xtask -- -D warnings` passed; `cargo check --workspace --no-default-features` passed.
  - Commands: all of the above.
  - Audit IDs updated: G3 verified, N2 verified, Q3 in_progress (CP3 evidence added).
  - Next: CP4 — make capability manifest declarations registry-owned.

- 2026-06-19 12:15: CP4 capability manifest declarations registry-owned (check-only)
  - Changed: `xtask/src/main.rs` only (80 insertions, 33 deletions): added `provides: Vec<String>` and `requires: Vec<String>` to `RegistryModule`; added `CompiledModule` struct with `key`, `provides`, `has_requires`; enhanced `parse_registry` with multi-line array support and `provides`/`requires` field parsing; rewrote `parse_compiled_modules` to return `Vec<CompiledModule>` with provides extraction (strings[2..]) and macro variant detection (`_with_requires` suffix); rewrote `check_compiled_modules` to compare provides (ordered Vec) and requires presence (bool) in addition to existing key bidirectionality.
  - Blast radius reviewed: only `xtask/src/main.rs` changed — no `compiled.rs`, `module_registry.toml`, Cargo.toml, or Rust source changes; xtask is dev tooling with no runtime dependency; no snapshot changes expected (compiled.rs unchanged) — confirmed by snapshot test passing.
  - Regression hypotheses checked: (1) provides list order mismatch between registry and compiled.rs — verified all 40 modules match exactly via Python pre-check; (2) multi-line arrays in registry TOML — added `brackets_balanced` join logic in `parse_registry`; (3) `has_requires` detection picking up macro definitions — cursor starts at `fn push_transport_and_storage_modules`, skipping macro defs; (4) duplicate module keys — `check_compiled_modules` builds `BTreeMap` by key, last-wins; (5) config property drift not caught by xtask — caught by existing snapshot tests and `openrouter_module_declares_provider_config_schema` test (34 capabilities tests pass).
  - Evidence: `cargo run -p xtask -- module-registry check` passed (40 modules, 45 features, 40 declarations, zero warnings, zero errors); `cargo clippy -p xtask -- -D warnings` passed; `cargo fmt --all -- --check` passed; `cargo check --workspace --no-default-features` passed; `cargo test -p oxide-agent-core --no-default-features --features profile-full --test modular_registry_snapshots` passed (snapshot unchanged); `cargo test -p oxide-agent-core --no-default-features --features profile-full -- capabilities` passed (34 tests); intentional mismatch test confirmed provides drift detection; intentional mismatch test confirmed requires presence drift detection.
  - Commands: all of the above.
  - Audit IDs updated: G4 verified, G1 in_progress (CP4 provides/requires evidence added), Q2 in_progress (CP4 blast radius recorded), Q3 in_progress (CP4 check-only evidence added).
  - Next: CP5 — add generated module/capability cfg aliases and migrate tests.

- 2026-06-19 13:00: CP5 generated cfg aliases and test migration
  - Changed: added `crates/oxide-agent-core/build.rs` (reads `module_registry.toml`, emits `oxide_module_<id>` cfg aliases with `cargo:rustc-check-cfg` declarations); migrated ~149 `#[cfg(feature = "...")]` to `#[cfg(oxide_module_...)]` across 15 test files: `executor/tests/{mod,basics,resume,registry}.rs`, `tests/{sub_agent_delegation,json_decode_error,anthropic_e2e,mistral_e2e,hermetic_agent,rate_limit}.rs`, `capabilities/{compiled,manifest}.rs` test modules, `llm/{client,capabilities}.rs` test modules, `llm/providers/modules.rs` test module (including dead-code suppression `cfg_attr`).
  - Blast radius reviewed: new `build.rs` runs on every compilation of `oxide-agent-core` (all targets: lib, tests, examples); cfg aliases are per-package (`cargo:rustc-cfg` visible to unit tests, integration tests, examples, and benchmarks within the package, NOT to dependents); 15 test files modified with mechanical replacements only (no logic changes); implementation code in `llm/` untouched (verified: all remaining `feature = "..."` is before test module `#[cfg(test)]` boundary); profile gates in `modular_registry_snapshots.rs` intentionally kept (profile features are composite Cargo features, not module features); `transport-web` tests use own crate features (unaffected).
  - Regression hypotheses checked: (1) cfg aliases not emitted for transitively-enabled features — verified `CARGO_FEATURE_TOOL_TODOS=1` is set when `profile-full` is enabled (Cargo resolves features before build.rs); (2) `unexpected_cfgs` lint fires for custom cfgs — prevented by `cargo:rustc-check-cfg` declarations for all 40 module cfgs; (3) tests silently disappear if cfg alias not emitted — verified all 1328 lib + 35 integration tests pass with `profile-full`; (4) implementation code accidentally migrated — verified by grep, all remaining `feature = "..."` in `llm/` is before `#[cfg(test)]` boundary; (5) `not(feature = "X")` patterns break — verified `not(oxide_module_X)` works correctly (checked no-default-features compilation); (6) compound `all(feature = "X", feature = "Y")` patterns — verified `all(oxide_module_X, oxide_module_Y)` works (sandbox tests pass); (7) cross-crate cfg visibility — `transport-web` tests not affected (they use their own features, confirmed by `cargo check -p oxide-agent-transport-web`).
  - Evidence: `cargo run -p xtask -- module-registry check` passed (40 modules, 45 features, 40 declarations); `cargo fmt --all -- --check` passed; `cargo clippy --workspace --all-targets --features profile-full -- -D warnings` passed; `cargo check --workspace --no-default-features` passed; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` passed; `cargo check --workspace --no-default-features --features profile-search-only` passed; `cargo check -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local` passed; `cargo test -p oxide-agent-core --no-default-features --features profile-full` passed (1328 lib + 35 integration, 0 failed); `git grep 'feature = "' crates/oxide-agent-core/src/agent/executor/tests/` returns 0 results; `git grep 'feature = "' crates/oxide-agent-core/tests/` returns only profile gates in `modular_registry_snapshots.rs`.
  - Commands: all of the above.
  - Audit IDs updated: G5 verified, Q2 in_progress (CP5 blast radius recorded), G7 in_progress (profile matrix validated for CP5, full matrix in CP8).
  - Next: CP6 — stabilize shared runtime context shape (`ToolModuleContext`/`ToolModuleContextParts`).

## Risks and Blockers

- Risk: Cargo cannot consume generated feature lists at build-script time.
  - Impact: registry must generate checked-in Cargo sections rather than runtime-generated feature config.
  - Evidence: Cargo feature resolution precedes crate build scripts by design; no current blocker.
  - Mitigation: checked-in generated files plus `xtask module-registry check`.
  - Audit IDs affected: G2, Q3.

- Risk: One Cargo feature maps to multiple modules (`llm-opencode-go` => opencode-go/opencode-zen, `tool-webfetch-md` => webfetch-md/web-crawler).
  - Impact: naive one-feature-one-module generator would lose modules or corrupt profiles.
  - Evidence: RECON found `compiled.rs` emits secondary modules under one feature.
  - Mitigation: registry schema must allow multiple module records sharing one Cargo feature.
  - Audit IDs affected: G1, G3, G4.

- Risk: Stable context registry could become an over-generic service locator.
  - Impact: violates local understandability and hides missing contexts until runtime.
  - Evidence: not yet implemented.
  - Mitigation: prefer typed lightweight context slots if they avoid optional heavy deps; otherwise restrict erased registry to module-owned service keys with tests.
  - Audit IDs affected: G6, Q1.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
