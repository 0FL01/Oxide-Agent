# Modular Architecture Dependency Audit

Date: 2026-05-23
Source PRD: `prd/PRD.md`
Goal: `docs/goals/2026-05-23-modular-architecture-refactor.md`

## Phase 1 Scope

This document is the Milestone 1 dependency and feature audit baseline. It records current dependency ownership, target feature names, and known leakage that later phases must remove.

The feature map in `crates/oxide-agent-core/Cargo.toml` now uses PRD-style atomic feature names and profile compositions. Heavy dependencies that still compile unconditionally are tracked below and by `scripts/check-cargo-tree-deny.sh`.

## Storage Decision

PRD section 22.7 is treated as the authoritative storage decision:

- `storage-s3-r2` is the only durable storage capability.
- `storage-local-fs` is only transient runtime workspace/cache/staging.
- No durable SQLite or filesystem storage module is planned.
- Stateless recovery from `.env` plus S3/R2 bucket is a hard target.

This supersedes earlier PRD examples that mention durable `storage-local`.

## Feature Profiles

Initial profile features:

- `profile-full`: maximal development/runtime composition.
- `profile-embedded-opencode-local`: Telegram plus OpenCode Go provider and a small non-sandbox tool set, backed by S3/R2 durable storage.
- `profile-lite`: smaller Telegram/OpenCode Go tool set, backed by S3/R2 durable storage.
- `profile-search-only`: web fetch/search plus OpenCode Go, no sandbox/browser/MCP.
- `profile-no-sandbox`: non-sandbox tools only.
- `profile-media-enabled`: media modules only when explicitly selected.

Initial atomic features:

- Transports: `transport-telegram`, `transport-web`, `transport-cli`, `transport-http-api`.
- Storage: `storage-s3-r2`, `storage-local-fs`.
- LLM providers: `llm-chatgpt`, `llm-groq`, `llm-mistral`, `llm-minimax`, `llm-zai`, `llm-nvidia`, `llm-opencode-go`, `llm-openrouter`.
- Tools: `tool-todos`, `tool-compression`, `tool-delegation`, `tool-agents-md`, `tool-reminder`, `tool-wiki-memory`, `tool-webfetch-md`, `tool-tavily`, `tool-searxng`, `tool-browser-use`, `tool-sandbox-fileops`, `tool-sandbox-exec`, `tool-sandbox-recreate`, `tool-file-delivery`, `tool-media-audio`, `tool-media-image`, `tool-media-video`, `tool-ytdlp`, `tool-tts-kokoro`, `tool-tts-silero`, `tool-stack-logs`.
- Sandbox: `sandbox-backend-docker-direct`, `sandbox-backend-sandboxd-client`, `sandbox-daemon`.
- Integrations/manager: `integration-mcp-jira`, `integration-mcp-mattermost`, `integration-ssh-mcp`, `manager-control-plane`.

Removed legacy feature names:

- `tavily` -> `tool-tavily`.
- `searxng` -> `tool-searxng`.
- `browser_use` -> `tool-browser-use`.
- `jira` -> `integration-mcp-jira`.
- `mattermost` -> `integration-mcp-mattermost`.

## Dependency Classification

Core/light dependencies that may remain shared for now:

- `tokio`, `tokio-util`, `futures-util`, `async-trait`.
- `tracing`, `thiserror`, `anyhow`.
- `serde`, `serde_json`, `serde_yaml`, `config`.
- `base64`, `chrono`, `uuid`, `regex`, `url`, `sha2`.
- Test/snapshot helpers when dev-only: `insta`, `mockall`, `proptest`, `tempfile`, `dotenvy`, `tracing-subscriber`.

Optional-heavy or module-owned dependencies to isolate:

- Durable S3/R2 storage: `aws-sdk-s3`, `aws-config`, `aws-credential-types`, `aws-types`.
- Docker sandbox: `bollard`, `tar`.
- Sandbox broker/client protocol: `bincode`, `serde_bytes`.
- MCP child-process integrations: `rmcp`.
- OpenAI-compatible/chat SDK: `async-openai`.
- Former direct Google Gemini SDK code has been removed; Gemini-family routes go through OpenRouter.
- ZAI provider: `zai-rs`.
- MiniMax provider: `claudius`.
- Tavily search: `tavily`.
- Web fetch/search/browser integrations: `reqwest`, `htmd` where used by selected modules.
- Media/ytdlp/TTS-related code and external runtime packages.
- Transport SDKs: `teloxide` for Telegram, `axum`/`tower` for HTTP/web transport.

## Current Leakage Baseline

Known leaks in `oxide-agent-core` after Phase 2f:

- Telegram and web transport dependencies are in separate transport crates, but workspace builds still include those crates unconditionally until binary/profile composition is introduced.
- Some tool registrations are still feature-aware inside the legacy registry rather than generated from capability manifests.

Resolved in Phase 2b:

- `async-openai` compiles only with `llm-groq` or `llm-mistral`.
- Former direct Google Gemini SDK dependency was removed from the project.
- `zai-rs` compiles only with `llm-zai`.
- `claudius` compiles only with `llm-minimax`.

Resolved in Phase 2c:

- AWS SDK crates (`aws-sdk-s3`, `aws-config`, `aws-credential-types`, `aws-types`) compile only with `storage-s3-r2`.
- R2 storage implementation modules and the direct `R2Storage` export are gated behind `storage-s3-r2`.
- Telegram's R2-backed runtime path forwards `storage-s3-r2` through the transport and binary package features.

Resolved in Phase 2d:

- Sandbox Docker dependencies (`bollard`, `tar`, `bytes`, `http-body-util`) compile only with sandbox backend features.
- Sandbox broker protocol dependencies (`bincode`, `serde_bytes`) compile only with sandbox backend features.
- `profile-no-sandbox`, `profile-search-only`, and `llm-opencode-go` leakage checks now fail only on the remaining RMCP dependency.
- `oxide-agent-sandboxd` requires an explicit `sandbox-daemon`/`profile-full` feature, and Docker full-profile builds enable it explicitly.

Resolved in Phase 2e:

- `rmcp` compiles only with `integration-mcp-jira`, `integration-mcp-mattermost`, or `integration-ssh-mcp`.
- SSH manager/preflight approval types remain available without `rmcp` through a no-client stub.
- The CI dependency leakage job now enforces the deny list instead of running as a non-blocking report.

Resolved in Phase 2f:

- `reqwest` is optional and owned by HTTP-using LLM/tool features: ChatGPT, Mistral, ZAI, Nvidia, OpenCode Go, OpenRouter, webfetch, SearXNG, Browser Use, media, and TTS.
- `htmd` is optional and owned only by `tool-webfetch-md`.
- `webfetch_md`, media-file, Kokoro TTS, and Silero TTS provider modules/exports/registrations are gated by their tool features.
- Sub-agent webfetch registration is gated by `tool-webfetch-md`, so `llm-opencode-go` no longer pulls `htmd`.

## Verification Commands

Profile build checks:

```bash
cargo check --workspace --no-default-features --features profile-embedded-opencode-local
cargo check --workspace --no-default-features --features profile-lite
cargo check --workspace --no-default-features --features profile-no-sandbox
cargo check --workspace --no-default-features --features profile-search-only
cargo check --workspace --no-default-features --features profile-media-enabled
cargo check --workspace --no-default-features --features profile-full
```

Dependency leakage checks:

```bash
scripts/check-cargo-tree-deny.sh profile-no-sandbox
scripts/check-cargo-tree-deny.sh profile-search-only
scripts/check-cargo-tree-deny.sh profile-lite
scripts/check-cargo-tree-deny.sh profile-embedded-opencode-local
scripts/check-cargo-tree-deny.sh llm-opencode-go
```

These leakage checks are now expected to pass. Add new deny-list entries as later slices define ownership for additional optional boundaries.

## Next Refactoring Targets

1. Add capability manifests so `cargo tree` checks can be tied to compiled module IDs.
2. Move concrete storage construction out of Telegram runner into application bootstrap per the PRD's final registry model.
3. Refine broker-only sandbox client support so it no longer shares the direct Docker implementation boundary.
4. Continue replacing legacy provider registration with feature-aware capability registration.
5. Split remaining always-compiled low-risk tool modules only when their dependencies or runtime availability justify it.
