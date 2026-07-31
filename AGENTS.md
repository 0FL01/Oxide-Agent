# Oxide Agent

Oxide Agent is a Telegram bot with Agent Mode on top of multiple LLM providers. It handles text, voice, images, documents, topic-scoped memory, sandbox tasks, a web console (Life Mode), and a manager control plane.

Stack: Rust 2024 edition, `teloxide`, SQLx/Postgres durable storage, Leptos, native integrations with OpenRouter, Anthropic Messages (generic, covers MiniMax), OpenAI-Base (covers ZAI/Mistral/custom), ChatGPT/Codex OAuth, and OpenCode Go/Zen. Gemini-family models are accessed through OpenRouter routes, not a direct Google Gemini provider.

## Branch

Default branch: `main`.

## Scale and decision principles

- Personal use, up to 2-3 people; target load up to 5 RPS.
- No sharding, HA, extra queues, multi-layer abstractions, or heavy observability without proven need.
- Optimize only after a real bottleneck; if a fix leaves a class of problems open, redesign the root cause rather than preserve a broken architecture.

## Implementation bias

- Smallest fundamentally correct change; preserve architecture only when it is sound.
- Boring, explicit, locally understandable code over generic frameworks.
- No new crates, services, queues, caches, storage backends, protocols, or abstraction layers unless clearly required.
- Add abstraction only after real duplication or multiple call sites exist.
- Document known limitations instead of building generalized designs for hypothetical needs.

## Workspace Overview

### Main crates
- `oxide-agent-core` - agent domain: execution, hooks, compaction, storage facade, LLM providers, sandbox, reminder/SSH/manager providers.
- `oxide-agent-runtime` - session runtime orchestration and transport-agnostic progress.
- `oxide-agent-transport-telegram` - Telegram transport: handlers, routing, views, progress, topic/thread integration.
- `oxide-agent-transport-web` - Web console backend and E2E test transport: HTTP API (axum), scripted LLM, SSE streaming, Life Mode executor/routes.
- `oxide-agent-web-contracts` - Shared web API types: auth, config, events, sessions, tasks, life.
- `oxide-agent-web-ui` - Leptos web console frontend: components, SSE streaming, markdown rendering, dark theme, Life Mode UI.
- `oxide-agent-sandboxd` - Docker sandbox broker daemon; Unix socket, Docker access.
- `oxide-agent-telegram-bot` - Telegram bot binary.
- `oxide-browser-contracts` - Shared REST contract types between the native browser sidecar and the core browser-live client; independent of core/runtime internals.
- `oxide-browser-sidecar` - Native Rust Chromium sidecar binary; talks CDP directly, serves the browser-live REST API (sessions, actions, snapshots, screenshots).

## Architectural invariants

- `oxide-agent-core` and `oxide-agent-runtime` do not depend on transport crates; transport crates depend on core/runtime.
- `teloxide` is used only in `oxide-agent-transport-telegram` and binaries that include it.
- Build and runtime composition are capability-module based. The declarative module registry at `crates/oxide-agent-core/module_registry.toml` is the single source of truth for module IDs, Cargo features, profiles, provided/required capabilities, and profile membership. Manifests in `crates/oxide-agent-core/src/capabilities/`; tool registration in `tool_runtime/`.
- Cargo `default` features are intentionally empty. Use profile features: `profile-embedded-opencode-local`, `profile-web-embedded-opencode-local`, `profile-full`.
- Keep explicit `mod.rs` files and predictable public exports.
- Use `thiserror` for library crates, `anyhow` for app/binary crates.
- Topic-aware and thread-aware by default for agent mode and manager functions.
- Context-scoped storage is mandatory for transport contexts; legacy fallback only for DM compatibility.
- Topic-scoped `AGENTS.md` is stored separately, pinned during flow bootstrap, live-synced after `agents_md_update`, inherited by sub-agents.
- Sandbox backend is always the sandboxd broker for agent processes. `DockerSandboxManager` exists only inside the sandboxd daemon; agent processes connect via Unix socket to the daemon.
- Manager CRUD goes through `manager_control_plane` provider with audit trail and RBAC (`manager_allowed_users`).
- `storage-sqlx` is the production durable storage. Local filesystem is transient only.
- Gemini models are valid only through OpenRouter; no direct Gemini provider is maintained.

## Key Subsystems

### Agent execution
- Runner in `agent/runner/`; executor slices in `agent/executor/`.
- Tool calls run in parallel; preserve history repair and `tool_call_id` integrity before LLM calls.
- Compaction is runner-integrated with typed message classes, budget estimator, hot-memory classifier, externalized large tool payloads, and LLM summarization sidecar. Legacy staged pipeline (classifier/prune/rebuild/summarizer) has been removed.

### Hooks and sub-agents
- Hooks in `agent/hooks/`. Always active: `completion_check`, `tool_access_policy`, `hot_context_health`, `search_budget`, `timeout_report`. Sub-agent safety hook enforces delegation restrictions. Details: `docs/hooks/`.
- Loop detection has content, tool-sequence, and LLM layers; do not bypass in runner changes.
- Sub-agents: isolated `EphemeralSession`s, inherit topic-scoped `AGENTS.md`, cannot recurse/send files/mutate topics/control-plane/use reminders/`stack_logs`/`recreate_sandbox`. Browser tools available via `allowed_tools` whitelist with RAII cleanup on run end.
- Do not reintroduce embedding-selected skills.

### Compaction
- Runner-integrated: typed message classes, budget estimator, hot-memory classifier, externalized large tool payloads, LLM summarization sidecar.
- Architecture details: `docs/compaction-architecture.md`.

### Prompt cache hit
- Static prefix + dynamic suffix; provider-specific details and smoke test in `docs/tips/cache-hit.md`.

### Topic- and flow-scoped state
- Contexts in `UserConfig.contexts` via `UserContextConfig`. Memory uses context-scoped APIs.
- Chat history isolated via `scoped_chat_storage_id`.
- Flows support attach/detach UX; `forum_topic_list` available for topic discovery (blocked for sub-agents).

### Control plane and operations
- Manager control plane in `agent/providers/manager_control_plane/`; CRUD for topics, bindings, contexts, AGENTS.md, infra, sandboxes, profiles, controls, audit trail, rollback.
- Stack logs: Docker Compose log access, requires `topic_infra`, blocked for sub-agents.
- Reminders: `agent/providers/reminder.rs` + storage; in-memory scheduler wakes the original topic/flow.

### Sandbox and SSH
- Facade: `sandbox/manager.rs` → `BrokerSandboxManager` → `SandboxBrokerClient` (Unix socket to sandboxd daemon). `DockerSandboxManager` is daemon-internal only.
- `SandboxScope` provides stable identity for persistent sandbox reuse.
- SSH: external `ssh-mcp` binary spawned as a child process, communicated with via MCP protocol (rmcp, stdio). Binary path: `OXIDE_SSH_MCP_BINARY` (default `/usr/local/bin/ssh-mcp`). Session is lazy, reused across tool calls, killed on timeout/error/cancel. Approval flow disabled (YOLO mode).
- SSH tools: `ssh_exec`, `ssh_sudo_exec`, `ssh_read_file`, `ssh_apply_file_edit`, `ssh_check_process`, `ssh_send_file_to_user`.
- Secret refs: `env:KEY`, `storage:PATH`; secrets must not reach prompts or memory.

### Browser Live
- Provider in `agent/providers/browser_live/`; sidecar binary in `oxide-browser-sidecar`, shared REST types in `oxide-browser-contracts`.
- Tools: `browser_start`/`browser_observe`/`browser_execute`/`browser_extract`/`browser_debug`/`browser_save_screenshot`/`browser_close` over a CDP WebSocket to a headless Chromium.
- Runs in Yolo mode (agent may type secrets and submit forms); disabled by default via `BROWSER_AGENT_ENABLED`. Details: `docs/browser-live.md`.
- Available to sub-agents via `allowed_tools` whitelist in `spawn_sub_agents`; inherits parent's `browser_live_context` for artifact storage scope.
- RAII session cleanup: `close_all_sessions` runs after every agent run end (parent and sub-agent) on any outcome (success/timeout/cancel/error) to prevent Chromium process leaks.
- Sidecar session cap: `BROWSER_AGENT_SIDECAR_MAX_SESSIONS` (default 8) rejects new sessions at capacity with `sidecar_at_capacity` error.

### Life Mode
- Permanent chat console with stable memory scope `(principal, "life", "main")`; runs reuse the same `AgentExecutor` path as ordinary web sessions.
- Contracts in `oxide-agent-web-contracts/src/life.rs`; executor/routes in `oxide-agent-transport-web/src/server/life_*.rs`; UI in `oxide-agent-web-ui/src/life/`.
- Sensitivity gate: `ApiLifeInputSensitivity` controls whether input is stored durably.
- Delivery: life runs can deliver files and messages back to Telegram via `life_telegram_delivery.rs`.

### Storage and LLM
- Storage facade and SQLx/Postgres backend in `storage/`; context-scoped APIs for transport state.
- LLM providers in `llm/providers/`; shared orchestration: `llm/client.rs`, `llm/capabilities.rs`, `llm/support/` (backoff, HTTP pooling, OpenAI compat), `llm/types.rs`.
- Six provider modules: `openai-chatgpt` (OAuth/Codex Responses streaming), `anthropic` (generic Anthropic Messages, covers MiniMax), `openai-base` (OpenAI-compatible, covers ZAI/Mistral/custom), `opencode-go`, `opencode-zen`, `openrouter`. `opencode-go` and `opencode-zen` share one cargo feature (`llm-opencode-go`).
- Anthropic Messages transport (`messages/`) is shared by `anthropic` and `opencode_go` providers.
- ChatGPT: OAuth/Codex Responses streaming; structured-output/json-mode agent requests require a compatible selected model.
- `AGENT_MODEL_ROUTES__N__*` / `SUB_AGENT_MODEL_ROUTES__N__*` define ordered catalogs for manual model selection; the first configured route is the default. One execution uses one model and never fails over automatically.

### Tool providers

- Extend in `agent/providers/`; keep the transport-agnostic contract. Feature-gated via the module registry.
- Sandbox: `sandbox-fileops` (`read_file`/`write_file`/`apply_file_edit`/`list_files`), `sandbox-exec` (`execute_command`), `sandbox-recreate` (`recreate_sandbox`).
- Search: `web_search` (one tool, multiple backends: CRW, Tavily, Brave), `crw` (scrape), `retrieve-tools` (tool group activation).
- Fetch: `webfetch_md` (feature-controlled, registered by default). `OXIDE_WEB_CRAWLER_MERGE=true` hides the split tool and exposes `web_crawler` with `render:"http"` (default), `render:"lightpanda"`, and `render:"playwright"`. HTTP mode falls back once to Lightpanda for anti-bot, 403, or 429 failures; explicit rendered modes do not cascade. `docker-compose.web.yml` defaults merge mode to true.
- Browser: `browser_live` tools (see Browser Live above).
- Media: `audio-stt` (transcription), `vision-image` (description), `vision-video` (description).
- Integrations: `jira-mcp`, `mattermost-mcp` (runtime-disabled, enabled via `topic_agent_tools_enable`), `ssh-mcp`.
- Agent ops: `delegation`, `manager_control_plane`, `reminder`, `agents_md`, `stack_logs` (disabled for topic agents, blocked for sub-agents), `compression`, `file_delivery`, `path`, `todos`, `tts` (Kokoro EN + Silero RU), `ytdlp`.

## Configuration

- Layered: optional `config/{RUN_MODE}.yaml`, `config/local.yaml` + env vars. Config files optional (`required(false)`).
- Provider secrets in `modules.<module-id>` with env fallbacks.
- Key runtime: DuckDuckGo, model routes, temperature, compaction budget, Jira MCP.
- Docker Compose split: `docker-compose.yml` (root), `docker-compose.telegram.yml`, `docker-compose.web.yml`. Optional local CRW/Postgres overlays: `docker-compose.telegram.local-services.yml`, `docker-compose.web.local-services.yml`. Build assets and service config in `docker/` (`Dockerfile.*`, `searxng/`, `consent-rules*`) are referenced by the root compose files.

## Development Practices

### Build
- `cargo check` for quick verification; `cargo build` only for final binary.
- Embedded: `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`.
- Full: `cargo build --release --no-default-features --features profile-full`.
- Other profiles: `profile-web-embedded-opencode-local`.
- Capability output (swap `<PROFILE>` and `<profile-name>`):
  - `cargo run -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features <PROFILE> -- capabilities --compiled --json`
  - `cargo run -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features <PROFILE> -- capabilities --enabled --json`
  - `cargo run -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features <PROFILE> -- config schema --compiled --json`
  - `cargo run -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features <PROFILE> -- config example --profile <profile-name> --json`
- Dependencies: `cargo add`, `cargo remove`, `cargo update`. Metadata: `workspace info`, `cargo info`.

### Module registry

- `crates/oxide-agent-core/module_registry.toml` is the single source of truth for module IDs, Cargo features, profiles, and capability provides/requires.
- `cargo run -p xtask -- module-registry check` — verifies Cargo profile feature lists, transport forwarding, `profiles/*.toml`, and `compiled.rs` declarations match the registry. Run before committing profile or module changes.
- `cargo run -p xtask -- module-registry generate` — regenerates the marked Cargo profile section and `profiles/*.toml` from the registry. Generated artifacts are checked in; `check` fails if they are stale.
- `crates/oxide-agent-core/build.rs` emits `oxide_module_<id>` cfg aliases (e.g. `oxide_module_tool_todos`) from the registry. Tests should gate on `#[cfg(oxide_module_<id>)]` instead of raw `#[cfg(feature = "<feature>")]`. Profile features (`profile-full` etc.) remain raw Cargo feature gates.
- One Cargo feature can map to multiple module IDs (e.g. `llm-opencode-go` → `llm-provider/opencode-go` and `llm-provider/opencode-zen`). The registry models this as separate module records sharing one `cargo_feature`.

### Format and lint
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all -- --check` must both pass before finishing. CI enforces both.
- When touching `oxide-agent-web-ui`, also verify the wasm target: `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`. Leptos `view!` macro expansion differs between native and wasm; native-only checks do not catch ownership/move errors that surface under the real trunk build. For a full frontend gate, run `trunk build --release` from `crates/oxide-agent-web-ui`.

### Testing
- Helpers: `crates/oxide-agent-core/src/testing.rs` (`mock_llm_simple()`, `mock_storage_noop()`, `test_set_env()`, `test_remove_env()`).
- Categories: hermetic, integration, snapshot (`insta`), property/fuzz (`proptest`).
- E2E: `crates/oxide-agent-transport-web/tests/e2e.rs`.
- Transport-specific profiles (e.g. `profile-web-embedded-opencode-local`) do not activate features in unrelated crates. `cargo test --workspace` will fail on crates whose modules are behind different feature gates. Use scoped `-p` for such profiles: `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`. Full and lite profiles work with `--workspace`.
- The legacy modular shell guard layer was removed; use focused `cargo check`, `cargo test`, and Docker build checks for touched areas.
- Gate tests on module availability using `#[cfg(oxide_module_<id>)]` aliases emitted by `build.rs`, not raw `#[cfg(feature = "...")]`. Profile-level test gating (`#![cfg(any(feature = "profile-..."))]`) remains raw Cargo features.

### Commit style
- `<type>(<scope>): <description>` + blank line + indented `Changes:` with 2-4 bullets.
- Types: `feat`, `fix`, `chore`, `docs`, `refactor`, `test`.

```text
feat(sources): add bybit proof of reserves source

    Changes:
    - Add Bybit proof-of-reserves source using the official frontend reserve ratio JSON endpoint
    - Normalize target asset reserve ratio and missing-asset transparency candidates with source-local tests
    - Wire scheduled checks and refresh source docs
```

## Where to find details

- `docs/tips/cache-hit.md` - prompt cache hit analysis: architecture, assembly order, telemetry, production validation.
- `docs/hooks/` - hook lifecycle and managed hook behavior.
- `docs/compaction-architecture.md` - compaction architecture: engine, admission, renderer, blocks.
- `docs/silero-tts-api.md` - Silero TTS integration for Russian voice.
- `docs/context-window-tracking.md` - token budget and context window management.
- `docs/stack-logs.md` - stack logs tool: Docker Compose log access.
- `docs/deploy.md` - concise deploy guide, optional external services, local service overlays, operations.
- `docs/browser-live.md` - Browser Live agent: sidecar setup, REST/CDP contract, actions, security model.
- `docs/lazy-tool-migration.md` - lazy tool registration migration guide.
- `README.md` - product overview and user-facing setup notes.
- `.env.example` - runtime configuration examples.

## System extension

- New transport: `crates/oxide-agent-transport-<name>`; SDK and handlers inside the transport crate.
- Runtime/core must not depend on a specific transport SDK.
- Separate `oxide-agent-<name>-bot` binary if needed.
