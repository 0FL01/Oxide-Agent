# Modular Architecture Final Completion Audit

Date: 2026-05-25
Source PRD: `prd/PRD.md`
Primary acceptance audit: `docs/modular-architecture/final-acceptance-audit.md`
Goal: `docs/goals/2026-05-23-modular-architecture-refactor.md`
Status: verified.

## Audit Scope

This audit covers the full PRD, not only section 20 acceptance criteria. Requirements are classified as:

- **Implemented**: current files and commands directly prove the requirement.
- **Superseded by PRD decision**: an earlier example or recommendation is overridden by a later explicit PRD decision.
- **Recommendation / non-binding shape**: the PRD presents a suggested shape where the hard requirement is the architectural invariant, not exact paths.
- **Non-goal / deletion policy**: the PRD explicitly says not to preserve old behavior.

## Section-Level Completion Matrix

### 1-3: Summary, Goals, Non-Goals

Status: implemented.

- Deterministic profiles: Cargo profile features exist in `crates/oxide-agent-core/Cargo.toml`; `profiles/*.toml` mirrors compiled module IDs; `scripts/check-compiled-capabilities.sh` rejects drift.
- Minimal embedded and custom builds: profile checks run with `--no-default-features`; atomic module features are available for custom composition.
- Single source of truth: compiled capability modules and runtime tool modules are the registration source; static guards reject legacy registries/wrappers.
- Compile-time dependency elimination: optional heavy dependencies are owned by module features; `scripts/check-cargo-tree-deny.sh` proves absence for minimal/provider/no-sandbox slices.
- Runtime config only for compiled modules: generated schemas use `additionalProperties=false`, and enabled capability validation rejects unknown module IDs.
- Non-goals preserved: compatibility aliases, migrations, deprecated wrappers, and local durable storage were deleted or explicitly rejected by guards/docs.

### 4: Current Architecture RECON

Status: addressed by replacement/deletion work.

- The legacy registry, legacy `ToolProvider` trait, global provider registration, concrete transport R2 startup, hardcoded sandbox assumptions, unconditional sidecars, and compatibility/migration startup paths described in RECON are now covered by deletion/static guards.
- Former direct Google Gemini SDK/provider usage is absent; Gemini-family model IDs remain valid only through OpenRouter routes.
- Embeddings/skills described in RECON are removed and guarded.

### 5-8: Target Architecture, Capability Model, Cargo Features, Unified Registry

Status: implemented with an acceptable in-repo layout.

- Capability IDs, module IDs, manifests, requirements, config properties, duplicate checks, and deterministic compiled/enabled manifests live under `crates/oxide-agent-core/src/capabilities/`.
- Profile and atomic feature names follow the PRD naming scheme; `default = []`.
- Workspace binaries expose deterministic `capabilities` / config schema output before runtime startup.
- Exact separate `oxide-agent-modules` crate from PRD section 23 was a recommended shape, not a hard requirement. The hard requirement is that shared core must not import heavy modules unconditionally; dependency deny checks and cfg-gated modules prove that boundary.

### 9: Tool Architecture

Status: implemented.

- Tool registration uses `agent::tool_runtime` typed executors and `ToolModule` registration.
- Legacy registry/wrapper symbols are statically rejected.
- Required tool decomposition is represented by module IDs and feature gates for todos, compression, delegation, agents.md, reminder, wiki memory, webfetch, Tavily, SearXNG, Browser Use, sandbox fileops/exec/recreate, file delivery, media audio/image/video, yt-dlp, TTS, stack logs, MCP integrations, SSH, and manager control-plane.
- Tool availability is validated through compiled manifests and registry snapshots for all PRD profiles.

### 10: LLM Provider Architecture

Status: implemented.

- Provider modules are feature-gated and expose module-owned config properties.
- Global provider config fields were removed from runtime settings and guarded.
- Provider route validation rejects disabled/removed providers.
- Direct Google Gemini provider aliases and SDK remain absent; OpenRouter owns Gemini-family model IDs.
- Embeddings are removed and guarded.

### 11: Storage Architecture

Status: implemented with PRD section 22.7 as the authoritative durable-storage decision.

- `storage-s3-r2` / `storage/r2` is the only production durable storage module and capability.
- Transient local filesystem workspace is not registered as a durable storage backend.
- R2 construction is behind storage module factories; Telegram startup is guarded against concrete `R2Storage`.
- Wiki memory and tools use storage facades/interfaces rather than concrete transport-owned storage.

### 12: Sandbox Architecture

Status: implemented.

- Docker direct, sandboxd client, and sandbox daemon capabilities are separate.
- Sandbox command execution, file operations, lifecycle/recreate, and diagnostics are split.
- `oxide-agent-sandboxd` is feature-gated.
- Sandbox image variants are explicit (`minimal`, `exec`, `media`, `dev`) and the legacy fat sandbox image path is guarded against returning.

### 13: Transport Architecture

Status: implemented for the current supported transports.

- Core/runtime do not depend on transport crates; transport crates depend on core/runtime.
- Telegram/Web transports use module/factory boundaries for storage and runtime wiring.
- Telegram Agent Mode legacy unscoped session fallback and flow memory migration paths are deleted and guarded.
- DM-only context fallback allowed by repository invariants remains outside the PRD's old-architecture migration target.

### 14: Config Architecture

Status: implemented.

- Module config is keyed by stable module IDs under `modules`.
- Provider-specific config is module-owned and exposed through generated schema metadata.
- Removed env aliases and config compatibility fields are statically guarded.
- Config schema/example generation exists for compiled modules and is checked for every PRD profile.

### 15-16: Docker and Compose Architecture

Status: implemented.

- `docker/Dockerfile.app` is profile-aware via build args and selected binaries.
- Embedded and full images are built/inspected by size budget checks.
- MCP binaries are only copied when selected.
- Compose topology is validated against profile modules for embedded/search/media/dev/full/root-full.
- Browser Use is intentionally dormant/disabled unless selected, consistent with repository instructions.

### 17: Deletion Plan

Status: implemented and guarded.

- Deleted or guarded surfaces include legacy registry paths, legacy provider dispatch, global provider registration/config, direct R2 transport startup, hardcoded sandbox startup assumptions, unconditional sidecars/binaries/assets, compatibility aliases, migration cleanup paths, fallback response shape labels where they were old-architecture compatibility, and transport composition responsibilities.
- Domain-valid protocol compatibility terms remain only where they describe external API wire formats, provider schema quirks, or user-facing media fallback behavior rather than old architecture preservation.

### 18-19: Implementation Plan and Testing Strategy

Status: implemented.

- All implementation milestones are represented in the goal log.
- Build, dependency leakage, registry snapshot, config validation, tool availability, Docker, Compose, and size budget checks exist as scripts and CI jobs.
- The final PRD 20.x/25 acceptance evidence is recorded in `docs/modular-architecture/final-acceptance-audit.md`.

### 20: Acceptance Criteria

Status: implemented.

Evidence: `docs/modular-architecture/final-acceptance-audit.md`.

### 21: Risks and Tradeoffs

Status: addressed.

- Breaking compatibility is accepted and enforced by deletion guards.
- Feature explosion is contained through stable naming and profile features.
- Config complexity is mitigated by generated schema and example output.
- Optional dependency mistakes are covered by dependency deny checks.
- Docker/profile drift is covered by profile TOML, Compose checks, Dockerfile checks, image content checks, and size budgets.

### 22: Open Questions

Status: resolved.

- Profiles use both Cargo features and `profiles/*.toml`.
- Compose files are profile-specific and guarded, not generated yet; generation is not required for acceptance.
- Embedded default has no sandbox command execution.
- Media, MCP, sandbox, provider, and manager capabilities are split by module/feature boundary.
- Local durable storage and SQLite are excluded; S3/R2 is authoritative.
- Old persisted tool names/state are not migrated.
- Skills/embeddings are removed and guarded.
- Stack logs are a selected diagnostics tool module.
- `chatgpt-login` remains a feature-gated utility binary.

### 23: Proposed Final Repository Shape

Status: recommendation satisfied by architectural invariants rather than exact path layout.

- The repository keeps modules inside existing crates with cfg-gated module boundaries instead of adding a separate `oxide-agent-modules` crate.
- This is acceptable because the PRD marks the listed layout as recommended and explicitly allows alternative layouts if dependency isolation is preserved.
- Hard requirement satisfied: shared core does not import heavy modules unconditionally; dependency leakage checks and feature gates prove selected-module boundaries.

### 24: Example Profiles

Status: implemented.

- `full`, `embedded-opencode-local`, `search-only`, `no-sandbox`, `media-enabled`, and `provider-specific-opencode-go` expectations are covered by profile features, profile TOML defaults, compiled capability checks, registry snapshots, dependency deny checks, Compose checks, and size/image checks.
- Provider-specific OpenCode Go dependency isolation is checked by `scripts/check-cargo-tree-deny.sh llm-opencode-go`.

### 25: Required Output Format

Status: implemented.

Evidence: `docs/modular-architecture/final-acceptance-audit.md`.

## Final Verification Commands

These commands passed on 2026-05-25 before marking the goal complete:

```bash
scripts/check-runtime-env-surface.sh
scripts/check-binary-feature-gates.sh
cargo test -p oxide-agent-core --test tool_runtime_static_guards --all-features
cargo test -p oxide-agent-core config --all-features
cargo test -p oxide-agent-core route_provider_validation_rejects_removed_direct_gemini_provider --all-features
cargo test -p oxide-agent-core removed_direct_gemini_provider_aliases_are_absent --all-features
for feature in profile-embedded-opencode-local profile-lite profile-search-only profile-no-sandbox profile-media-enabled profile-full; do cargo check --workspace --no-default-features --features "$feature"; done
for feature in profile-embedded-opencode-local profile-lite profile-no-sandbox profile-search-only profile-media-enabled llm-opencode-go; do scripts/check-cargo-tree-deny.sh "$feature"; done
for profile in embedded-opencode-local lite search-only no-sandbox media-enabled full; do scripts/check-compiled-capabilities.sh "$profile"; done
for profile in embedded-opencode-local lite search-only no-sandbox media-enabled full; do scripts/check-registry-snapshots.sh "$profile"; done
for profile in embedded-opencode-local search media dev full root-full; do scripts/check-compose-profile.sh "$profile"; done
scripts/check-sandbox-image-variants.sh
for profile in embedded-opencode-local full; do for mode in binary metrics image; do scripts/check-profile-size-budget.sh "$profile" "$mode"; done; done
cargo fmt --all --check
cargo check --workspace
cargo clippy --workspace --all-targets --all-features
git diff --check
```

## Completion Decision

All final verification commands passed, and no known PRD requirement remains unimplemented or unverified.
