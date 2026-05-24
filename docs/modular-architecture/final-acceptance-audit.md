# Modular Architecture Final Acceptance Audit

Date: 2026-05-24
Source PRD: `prd/PRD.md`
Goal: `docs/goals/2026-05-23-modular-architecture-refactor.md`
Status: evidence collected for PRD 20.x/25; goal remains active until final verification is completed and recorded.

## Fresh Evidence Run

The following checks were run from `modular-arch` on 2026-05-24 after checkpoint `d4a30192`:

- `scripts/check-cargo-tree-deny.sh` for `profile-embedded-opencode-local`, `profile-lite`, `profile-no-sandbox`, `profile-search-only`, `profile-media-enabled`, and `llm-opencode-go`.
- `scripts/check-compiled-capabilities.sh` for `embedded-opencode-local`, `lite`, `search-only`, `no-sandbox`, `media-enabled`, and `full`.
- `scripts/check-registry-snapshots.sh` for `embedded-opencode-local`, `lite`, `search-only`, `no-sandbox`, `media-enabled`, and `full`.
- `cargo check --workspace --no-default-features --features <profile>` for all six PRD profiles.
- `scripts/check-compose-profile.sh` for `embedded-opencode-local`, `search`, `media`, `dev`, `full`, and `root-full`.
- `scripts/check-sandbox-image-variants.sh`.
- `scripts/check-profile-size-budget.sh` for `embedded-opencode-local` and `full` in `binary`, `metrics`, and `image` modes.
- `scripts/check-binary-feature-gates.sh`.
- `scripts/check-runtime-env-surface.sh`.
- `cargo fmt --all --check`.
- `cargo check --workspace`.
- `cargo clippy --workspace --all-targets --all-features`.
- `git diff --check`.

All commands above passed in the current worktree.

## PRD 20 Acceptance Matrix

### 20.1 Architecture

Status: covered by static guards, registry snapshots, and capability manifests.

- Single module registration path: `crates/oxide-agent-core/tests/tool_runtime_static_guards.rs` guards deleted duplicate provider/registry construction surfaces; `scripts/check-registry-snapshots.sh` validates all profile registries through the compiled module path.
- Single typed tool registry and no legacy tool registry/wrapper: static guards reject `agent/registry.rs`, `build_tool_registry`, `legacy_provider`, `FilteredToolProvider`, legacy bridge modules, and compatibility wrapper labels.
- New tool/provider module contract: Cargo feature profiles and `scripts/check-compiled-capabilities.sh` enforce compiled module IDs, cargo feature metadata, config schemas, examples, and profile default drift against `profiles/*.toml`.
- Global provider match chains removed and aliases module-owned: provider snapshot tests assert registered aliases are owned by enabled provider modules and keep removed direct Gemini aliases absent.
- Runtime config cannot enable absent compile-time modules: `scripts/check-compiled-capabilities.sh` writes an invalid config with `tool/not-compiled` and asserts startup validation rejects it for every profile.

### 20.2 Minimal Build

Status: covered by profile build, dependency deny, capability manifest, and size/image checks.

- Required command passes: `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`.
- Heavy dependency absence: `scripts/check-cargo-tree-deny.sh profile-embedded-opencode-local` rejects Docker/Bollard, MCP/RMCP, direct provider SDKs, Tavily, broker protocol, and other non-embedded dependencies.
- Capability absence: `scripts/check-compiled-capabilities.sh embedded-opencode-local` proves the profile has exactly the embedded module set and excludes browser-use, searxng, MCP, sandbox, media, yt-dlp, TTS, and direct Gemini provider IDs.
- Docker/image budget: `scripts/check-profile-size-budget.sh embedded-opencode-local image` builds the image and checks selected binary content plus compressed/uncompressed budgets.

### 20.3 Providers

Status: covered by feature-gated provider modules, route validation tests, env-surface guards, and registry snapshots.

- Only selected providers compile: profile checks and `scripts/check-cargo-tree-deny.sh llm-opencode-go` prove unrelated provider SDKs are absent from the OpenCode-only slice.
- Only selected aliases are present: `scripts/check-registry-snapshots.sh` snapshots provider IDs and aliases per profile.
- Provider-specific config is module-owned: `scripts/check-compiled-capabilities.sh` validates provider module config schemas and env metadata from compiled manifests.
- Disabled/removed providers fail validation: core config tests reject removed direct Gemini provider names; enabled capability validation rejects non-compiled module IDs.

### 20.4 Tools

Status: covered by registry snapshots, compiled capability manifests, and static guards.

- Selected tool presence/absence: `scripts/check-compiled-capabilities.sh` and `scripts/check-registry-snapshots.sh` validate module and tool lists for all six profiles.
- `execute_command` absent from embedded/lite defaults: embedded/lite capability manifests forbid `tool/sandbox-*` prefixes.
- Sandbox fileops/exec split: capability manifests expose `tool/sandbox-fileops`, `tool/sandbox-exec`, and sandbox backend capabilities separately only in profiles that select them.
- Search/browser split, independent MCP integrations, independent media modules: profile manifests validate search-only, media-enabled, no-sandbox, and full module composition.

### 20.5 Storage

Status: covered by storage decision docs, feature-gated R2 code, dependency deny checks, and startup topology checks.

- `storage-s3-r2` is the only production durable storage capability: `profiles/*.toml` and capability manifests use `storage/r2`; `docs/wiki-memory.md` documents S3/R2-backed wiki memory as the durable memory source.
- AWS SDK only when `storage-s3-r2` selected: dependency deny checks for provider-only and non-storage slices reject AWS SDK crates.
- No concrete R2 construction in transport startup: storage facade and module wiring keep R2 construction behind the storage module; static guards and dependency audit track this boundary.
- Tools consume storage traits/interfaces: provider modules use the storage facade rather than transport-owned concrete backend construction.

### 20.6 Sandbox

Status: covered by profile builds, dependency deny checks, binary gates, sandbox image checks, and Compose checks.

- No-sandbox profile compiles without Bollard/broker protocol: `cargo check --workspace --no-default-features --features profile-no-sandbox` and `scripts/check-cargo-tree-deny.sh profile-no-sandbox` pass.
- `oxide-agent-sandboxd` is feature-gated: `scripts/check-binary-feature-gates.sh` requires `sandbox-daemon` for the sandbox daemon binary and now fails on unexpected untracked binaries.
- Docker socket and sandboxd services are selected only by profile topology: `scripts/check-compose-profile.sh` validates service and volume topology against `profiles/*.toml`.
- Minimal sandbox image excludes heavy packages: `scripts/check-sandbox-image-variants.sh` checks explicit minimal/exec/media/dev sandbox Dockerfiles and rejects legacy fat image references.
- Sandbox exec and file operations are separate modules: capability manifests and snapshots validate separate module IDs.

### 20.7 Docker and Compose

Status: covered by Compose topology checks, Dockerfile profile checks in CI, and image size/content checks.

- Embedded/full app image composition: `scripts/check-profile-size-budget.sh <profile> image` builds images and validates selected binaries.
- Compose selected services and volumes: `scripts/check-compose-profile.sh` validates embedded, search, media, dev, full, and root-full topology against profile module declarations.
- MCP binaries absent unless enabled: Docker build args and image content checks validate embedded has only `oxide-agent-telegram-bot`, while full includes selected MCP binaries and `oxide-agent-sandboxd`.
- Browser-use/searxng/sandboxd service selection: Compose checks validate absent/present service requirements per profile.

### 20.8 CI

Status: covered by `.github/workflows/modular-architecture.yml` plus local execution of the same scripts.

- Profile builds: `profile-checks` matrix runs all six profile `cargo check` commands.
- Dependency absence: `dependency-leakage-check` runs `scripts/check-cargo-tree-deny.sh` for required minimal/provider slices.
- Registry snapshots: `registry-snapshot-checks` runs `scripts/check-registry-snapshots.sh` for all profiles.
- Config validation: `compiled-capability-manifest-checks` runs config schema/example/enabled-manifest validation; `static-guard-tests` runs core config tests.
- Docker/Compose: `dockerfile-profile-checks`, `docker-compose-profile-checks`, and `sandbox-image-variant-checks` validate Dockerfiles, Compose topology, and sandbox variants.
- Size budgets: `profile-size-budget-checks` runs binary, metrics, and image budget modes.

## PRD 25 Output Guarantees

Status: covered by CLI manifest output, config schema/example output, Docker/Compose checks, and static guards.

- Reproducible profile builds from Cargo features: six profile `cargo check` commands pass without default features.
- Deterministic compiled capabilities: `scripts/check-compiled-capabilities.sh` validates sorted module/capability IDs for every profile.
- Deterministic enabled capabilities for config: the same script disables `transport/telegram` via config and validates enabled output removes it.
- Config schema for compiled modules: the same script validates module schemas, metadata, env ownership, and `additionalProperties=false`.
- Docker/Compose assets correspond to selected modules: Compose and size/image checks validate profile-to-Docker/Compose alignment.
- CI proves minimal dependency absence: modular architecture workflow contains dedicated dependency leakage jobs.
- No legacy registry or compatibility path remains: static guards reject deleted registry, wrapper, alias, migration, direct Gemini, embedding/skills, and stale env/config surfaces; runtime env surface guard covers deployment/docs/code env names.

## Residual Verification Notes

- This audit records evidence for PRD 20.x/25 only. It does not by itself mark the long-running goal complete.
- Before completing the goal, update the goal document's `Final Verification` section with the final command set and ensure the worktree is clean.
- Any future binary target, module ID, provider alias, runtime env name, or Compose service added after this audit must be represented in the corresponding guard script or snapshot before completion is claimed.
