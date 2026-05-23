# Goal: Modular Architecture Refactor

Date started: 2026-05-23
Status: active
Codex goal: Implement the modular architecture refactor described in prd/PRD.md on branch modular-arch. Track progress in this repo-local goal document, proceed milestone by milestone, and commit after each completed major checkpoint.

## Objective

Refactor Oxide Agent into the capability-oriented modular architecture specified by `prd/PRD.md`.

Done when build profiles deterministically select compiled modules, heavy dependencies are eliminated from profiles that do not select them, runtime config can only enable compiled modules, tool/provider/storage/sandbox/transport registration flows through a single capability registry path, Docker/Compose assets match the selected profile, and the PRD acceptance criteria are validated or explicitly documented as remaining work.

## Scope

In scope:
- Add capability/module foundations, manifests, deterministic compiled module lists, and runtime validation.
- Replace broad/global provider and tool registration with feature-gated modules.
- Move LLM providers, tools, storage, sandbox, transports, MCP/media/search/browser integrations, reminders, file delivery, and manager groups toward explicit capability modules.
- Make Cargo features atomic and compose profile features from them.
- Move heavyweight dependencies behind optional features and add leakage checks.
- Make Docker, Compose, sandbox images, and optional binaries profile-aware.
- Remove legacy compatibility paths, deprecated aliases, embedding/skill runtime, duplicate registries, and old migration-only code as replacement modules land.

Out of scope:
- Preserving old config compatibility or old environment variable aliases.
- Deployment migrations for existing state.
- Enterprise-scale orchestration, sharding, HA, queueing, or extra observability.
- Adding new product features unrelated to modular architecture.
- Keeping local filesystem or SQLite as durable storage.

## Repository Context

- PRD: `prd/PRD.md`.
- Branch for this work: `modular-arch`.
- Existing workspace crates: `oxide-agent-core`, `oxide-agent-runtime`, `oxide-agent-transport-telegram`, `oxide-agent-transport-web`, `oxide-agent-telegram-bot`, `oxide-agent-sandboxd`.
- Initial feature baseline: `oxide-agent-core` had coarse features (`tavily`, `searxng`, `browser_use`, `jira`, `mattermost`) and default features enabled.
- Initial heavy dependency leakage examples: AWS SDK, RMCP, Bollard, async-openai, Gemini, ZAI, reqwest, tar/bincode/sandbox dependencies were core-level dependencies.
- Existing goal convention: repo-local files in `docs/goals/`.

## Implementation Plan

1. Phase 0: Create this goal document from `prd/PRD.md` and commit it.
2. Phase 1: Add Milestone 1 dependency/feature audit artifacts: dependency classification, atomic feature naming map, profile map, and first leakage-check script/CI-friendly commands.
3. Phase 2: Convert Cargo feature defaults toward `default = []`, introduce initial atomic/profile features, and move obvious heavy dependencies to optional features where code already has or can cheaply get cfg boundaries.
4. Phase 3: Add capability foundation types: module IDs, capability IDs, manifest structs, deterministic compiled module list scaffolding, duplicate detection, and tests.
5. Phase 4: Add config validation against compiled module manifests and CLI/test hooks to print compiled/enabled capabilities.
6. Phase 5: Start provider modularization with one narrow provider slice, then repeat provider-by-provider.
7. Phase 6: Start tool modularization with low-risk atomic tools, then split sandbox/search/MCP/media/manager tools by capability.
8. Phase 7: Modularize storage around S3/R2 durable storage and remove concrete storage construction from transport startup.
9. Phase 8: Modularize sandbox backend/tool split and gate sandbox daemon/broker/Docker dependencies.
10. Phase 9: Decouple transport startup from app composition and introduce profile-aware bootstrap.
11. Phase 10: Make Docker/Compose/sandbox images profile-aware and remove unconditional sidecars/binaries/assets.
12. Phase 11: Add final profile matrix, dependency leakage, manifest snapshot, config validation, Docker/Compose, and size-budget checks.

## Validation Contract

- Formatting: `cargo fmt --all --check`.
- Baseline workspace check during early phases: `cargo check --workspace`.
- Lint before finishing a code checkpoint: `cargo clippy --workspace --all-targets --all-features`.
- Profile checks as they become available:
  - `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
  - `cargo check --workspace --no-default-features --features profile-no-sandbox`
  - `cargo check --workspace --no-default-features --features profile-search-only`
  - `cargo check --workspace --no-default-features --features profile-lite`
  - `cargo check --workspace --no-default-features --features profile-media-enabled`
  - `cargo check --workspace --no-default-features --features profile-full`
- Dependency leakage checks as they become available:
  - embedded/profile-specific builds must not include unselected AWS SDK, RMCP, Bollard, or unrelated provider SDKs;
  - no-sandbox builds must not include Bollard or sandbox broker protocol;
  - no-MCP/search-only builds must not include RMCP.

Done when:
- PRD section 20 acceptance criteria are satisfied.
- Required profile builds are reproducible from Cargo features.
- Compiled and enabled capabilities are emitted as deterministic JSON.
- Config schema/example generation exists for compiled modules.
- Docker/Compose assets correspond to selected modules.
- CI or CI-ready scripts prove dependency absence and profile topology.
- Legacy registry/compatibility paths are deleted rather than wrapped.

## Decisions

- 2026-05-23: Treat the PRD as a multi-milestone architecture migration, not a single patch.
- 2026-05-23: Commit after every completed phase or major checkpoint.
- 2026-05-23: Use branch `modular-arch` for all work in this goal.
- 2026-05-23: Treat PRD section 22.7 as authoritative over earlier local-storage examples: S3/R2-compatible object storage is the only durable storage target; local filesystem is transient only.
- 2026-05-23: Keep implementation pragmatic for personal-scale usage; avoid extra orchestration/HA layers unless the PRD explicitly requires a modular boundary.

## Progress Log

- 2026-05-23 13:38 +03: Read `prd/PRD.md`, confirmed branch `modular-arch` with clean working tree, created active Codex goal, and started this repo-local goal document. Next: commit Phase 0, then implement Phase 1 dependency/feature audit artifacts.
- 2026-05-23 13:49 +03: Phase 1 added PRD-style atomic/profile feature names, set `oxide-agent-core` default features to empty, renamed existing feature gates from `tavily`/`searxng`/`browser_use`/`jira`/`mattermost` to canonical `tool-*` and `integration-*` names, added profile TOML stubs, dependency audit docs, a CI workflow for profile checks, and `scripts/check-cargo-tree-deny.sh`. Validation passed: `cargo check --workspace`, all six profile checks (`profile-embedded-opencode-local`, `profile-lite`, `profile-search-only`, `profile-no-sandbox`, `profile-media-enabled`, `profile-full`), `cargo fmt --all --check`, and `cargo clippy --workspace --all-targets --all-features`. Expected leakage evidence captured: `scripts/check-cargo-tree-deny.sh profile-no-sandbox`, `profile-search-only`, and `llm-opencode-go` fail because AWS SDK, Bollard, RMCP, provider SDKs, and broker deps are still unconditional. Next: Phase 2 optional dependency gates and cfg boundaries for the first heavy module slice.
- 2026-05-23 14:00 +03: Phase 2a removed the inactive legacy skills/embeddings subsystem per PRD section 22.11. Deleted `agent/skills/*`, `llm/embeddings.rs`, LLM embedding fields/methods, embedding config/env helpers, root markdown `skills/`, Docker `COPY skills/`, and stale README/env references. Prompt assembly now clears legacy loaded-skill accounting without accepting a `SkillRegistry`. Static check finds no production references to `EmbeddingProvider`, `EmbeddingTaskType`, `generate_embedding`, `probe_embedding_dimension`, `SkillRegistry`, `SkillMatcher`, `EmbeddingService`, `EMBEDDING_*`, `SKILL_*`, `SKILLS_DIR`, or `COPY skills`. Validation passed: `cargo check --workspace`, `cargo check --workspace --no-default-features --features profile-no-sandbox`, `cargo fmt --all --check`, and `cargo clippy --workspace --all-targets --all-features`. Expected leakage remains: `scripts/check-cargo-tree-deny.sh llm-opencode-go` still fails because provider SDKs, AWS SDK, Bollard, and RMCP are not optional yet. Next: gate provider SDK modules behind `llm-*` features.
- 2026-05-23 14:10 +03: Phase 2b moved provider SDK crates behind provider features: `async-openai` behind `llm-groq`/`llm-mistral`, `gemini-rust` behind `llm-gemini`, `zai-rs` behind `llm-zai`, and `claudius` behind `llm-minimax`. LLM provider modules and registration blocks are now cfg-gated by `llm-*`; `chatgpt-login` is behind `oxide-agent-telegram-bot/llm-chatgpt` and Docker full-profile build enables it explicitly. Validation passed: `cargo check --workspace`, `cargo check --workspace --no-default-features --features llm-opencode-go`, `profile-search-only`, and `profile-full`, `cargo fmt --all --check`, and `cargo clippy --workspace --all-targets --all-features`. Leakage improved: `scripts/check-cargo-tree-deny.sh llm-opencode-go` no longer reports Gemini/ZAI/async-openai/claudius, and `profile-search-only` no longer reports provider SDK leaks. Remaining expected leaks: AWS SDK, Bollard/tar, RMCP, and sandbox broker deps. Next: split storage/sandbox/MCP dependency gates.
- 2026-05-23 14:39 +03: Phase 2c moved AWS/R2 SDK crates behind `storage-s3-r2` and gated the R2 storage implementation/export behind that feature. `StorageError` no longer exposes AWS SDK types, wiki-memory's direct `R2Storage` backend is cfg-gated, and the real Telegram runtime path now requires/forwards `storage-s3-r2` through transport and binary package features while the no-storage workspace build keeps only the runtime stub/config compiled. Docker full-profile builds use `oxide-agent-telegram-bot/profile-full`. Validation passed: `cargo check --workspace`, `cargo check --workspace --no-default-features --features profile-full`, `profile-search-only`, and `profile-no-sandbox`, `cargo check -p oxide-agent-core --no-default-features --features llm-opencode-go`, `cargo check -p oxide-agent-telegram-bot --no-default-features --features profile-full`, `cargo fmt --all --check`, and `cargo clippy --workspace --all-targets --all-features`. Leakage improved: `scripts/check-cargo-tree-deny.sh llm-opencode-go` no longer reports AWS SDK crates; remaining expected leaks are `bollard`, `rmcp`, `bincode`, `serde_bytes`, and `tar` depending on profile. Next: split sandbox and RMCP dependencies.
- 2026-05-23 15:02 +03: Phase 2d moved sandbox Docker and broker dependencies behind sandbox feature gates: `bollard`, `tar`, `bytes`, `http-body-util`, `bincode`, and `serde_bytes` are no longer compiled for no-sandbox/search/opencode-only profiles. Added a no-backend `SandboxManager` stub to keep non-sandbox profiles buildable, gated `stack_logs` behind `tool-stack-logs`, and made `oxide-agent-sandboxd` require explicit `sandbox-daemon`/`profile-full` features. Docker full-profile builds now enable both `oxide-agent-telegram-bot/profile-full` and `oxide-agent-sandboxd/profile-full`. Validation passed: `cargo check --workspace`, `cargo check --workspace --no-default-features --features profile-full`, `profile-search-only`, and `profile-no-sandbox`, `cargo check -p oxide-agent-sandboxd --no-default-features --features profile-full`, `cargo fmt --all --check`, and `cargo clippy --workspace --all-targets --all-features`. Leakage improved: `scripts/check-cargo-tree-deny.sh profile-no-sandbox`, `profile-search-only`, and `llm-opencode-go` now fail only on `rmcp`. Next: split RMCP integrations.
- 2026-05-23 15:25 +03: Phase 2e moved `rmcp` behind `integration-mcp-jira`, `integration-mcp-mattermost`, and `integration-ssh-mcp`. Jira/Mattermost providers were already module-gated; SSH MCP now has a no-client stub for shared approval/preflight types so manager/topic code can compile without the upstream MCP SDK. Registry SSH tool registration is gated on `integration-ssh-mcp`, and the modular architecture CI leakage job is now enforcing rather than `continue-on-error`. Validation passed: `cargo check --workspace`, all six profile checks (`profile-embedded-opencode-local`, `profile-lite`, `profile-search-only`, `profile-no-sandbox`, `profile-media-enabled`, `profile-full`), `cargo check -p oxide-agent-core --no-default-features --features llm-opencode-go`, `cargo fmt --all --check`, and `cargo clippy --workspace --all-targets --all-features`. Leakage checks now pass for `profile-no-sandbox`, `profile-search-only`, and `llm-opencode-go`. Next: web/search/browser ownership boundaries and capability registration.
- 2026-05-23 15:42 +03: Phase 2f moved `reqwest` and `htmd` behind explicit feature owners. `reqwest` is now optional and selected only by HTTP-using LLM/tool features; `htmd` is owned by `tool-webfetch-md`. `webfetch_md`, media-file, Kokoro TTS, and Silero TTS modules/exports/registrations are cfg-gated, manager tool catalog entries now respect those tool features, and sub-agent webfetch registration no longer leaks into `llm-opencode-go`. Validation passed: `cargo check -p oxide-agent-core --no-default-features --features llm-opencode-go`, `cargo check --workspace`, all six profile checks (`profile-embedded-opencode-local`, `profile-lite`, `profile-search-only`, `profile-no-sandbox`, `profile-media-enabled`, `profile-full`), `scripts/check-cargo-tree-deny.sh llm-opencode-go`, `profile-no-sandbox`, `profile-search-only`, `profile-lite`, `profile-embedded-opencode-local`, `cargo fmt --all --check`, and `cargo clippy --workspace --all-targets --all-features`. Next: commit this checkpoint, then start capability manifest scaffolding.
- 2026-05-23 16:10 +03: Phase 3a added the first capability manifest scaffold in `oxide-agent-core::capabilities`: stable module/capability IDs, `CapabilityKind`, `CapabilityModule`, static module descriptors, deterministic `compiled_modules()`, `CompiledCapabilityManifest`, `EnabledCapabilityManifest`, and a minimal `ModuleRegistry` shell. The compiled manifest is feature-gated across current atomic features and serializes to deterministic JSON; tests cover deterministic ordering plus duplicate module/capability ID failures. Validation passed: `cargo check --workspace`, all six profile checks, `cargo check -p oxide-agent-core --no-default-features --features llm-opencode-go`, leakage checks for `llm-opencode-go`, `profile-no-sandbox`, and `profile-search-only`, `cargo test -p oxide-agent-core capabilities::manifest --all-features`, `cargo fmt --all --check`, and `cargo clippy --workspace --all-targets --all-features`. Note: a broad exploratory `cargo test -p oxide-agent-core capabilities --no-default-features --features llm-opencode-go` also matched old `llm::capabilities` tests and exposed an unrelated Nvidia capability assertion failure; the exact new manifest tests passed. Next: wire config validation/CLI output against the compiled manifest.
- 2026-05-23 16:25 +03: Phase 3b added the first CLI/debug output path for compiled capabilities: `oxide-agent-telegram-bot capabilities --compiled --json` prints the deterministic compiled manifest and exits before dotenv/logging/bot startup. Added parser tests for default bot startup, the compiled capabilities command, and partial command rejection. Validation passed: `cargo test -p oxide-agent-telegram-bot startup_command --no-default-features`, `cargo run -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features -- capabilities --compiled --json`, `cargo check --workspace`, `cargo check --workspace --no-default-features --features profile-full`, `cargo fmt --all --check`, and `cargo clippy --workspace --all-targets --all-features`. Next: add config validation against compiled module IDs.
- 2026-05-23 16:35 +03: Phase 3c added the reusable config validation primitive on `CompiledCapabilityManifest`: `contains_module_id` and `validate_configured_module_ids` reject runtime module config keys that are absent from the compiled manifest with `ManifestError::NonCompiledModuleConfig`. Added a focused test for compiled vs non-compiled module IDs. Validation passed: `cargo test -p oxide-agent-core capabilities::manifest --all-features`, `cargo check --workspace`, `cargo check --workspace --no-default-features --features profile-full`, `cargo fmt --all --check`, and `cargo clippy --workspace --all-targets --all-features`. Next: connect this primitive to parsed config and enabled-manifest selection.

## Risks and Blockers

- The PRD intentionally rejects compatibility and migrations; deletion phases must be sequenced behind replacement modules to keep the workspace buildable.
- The PRD contains early examples using durable local storage, but section 22.7 later decides against it. This goal follows the later explicit decision.
- Moving dependencies to optional features can temporarily expose many unconditional imports; use small cfg boundaries and profile checks instead of large speculative rewrites.
- Docker/Compose profile generation depends on module requirement metadata that does not exist yet; early Docker work should stay profile-specific until manifests can drive generation.

## Final Verification

- Pending.
