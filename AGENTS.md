# Oxide Agent

Oxide Agent is a Telegram bot with Agent Mode on top of multiple LLM providers. It handles text, voice, images, documents, topic-scoped memory, sandbox tasks, a web console, and a manager control plane.

Stack: Rust 1.94, `teloxide`, SQLx/Postgres durable storage, Leptos, native integrations with Mistral AI, OpenRouter, MiniMax AI (claude SDK), ZAI/Zhipu AI, NVIDIA NIM, ChatGPT/Codex OAuth, and OpenCode Go. Gemini-family models are accessed through OpenRouter routes, not a direct Google Gemini provider.

## Branch

Default branch: `dev`.

## Scale and decision principles

- Personal use, up to 2-3 people; target load up to 5 RPS.
- Over-engineering is forbidden: no sharding, HA, extra queues, multi-layer abstractions, or heavy observability without proven need.
- Prefer the simplest maintainable solution; optimize only after a real bottleneck.

## Implementation bias

- Smallest working change that preserves current architecture.
- Boring, explicit, locally understandable code over generic frameworks.
- No new crates, services, queues, caches, storage backends, protocols, or abstraction layers unless clearly required.
- Add abstraction only after real duplication or multiple call sites exist.
- Document known limitations instead of building generalized designs for hypothetical needs.

## Workspace Overview

### Main crates
- `oxide-agent-core` - agent domain: execution, hooks, compaction, storage facade, LLM providers, sandbox, wiki memory, reminder/SSH/manager providers.
- `oxide-agent-runtime` - session runtime orchestration and transport-agnostic progress.
- `oxide-agent-transport-telegram` - Telegram transport: handlers, routing, views, progress, topic/thread integration.
- `oxide-agent-transport-web` - Web console backend and E2E test transport: HTTP API (axum), scripted LLM, SSE streaming.
- `oxide-agent-web-contracts` - Shared web API types: auth, config, events, sessions, tasks.
- `oxide-agent-web-ui` - Leptos web console frontend: components, SSE streaming, markdown rendering, dark theme.
- `oxide-agent-sandboxd` - Docker sandbox broker daemon; Unix socket, Docker access.
- `oxide-agent-telegram-bot` - Telegram bot binary.

## Where To Look

- `crates/oxide-agent-core/src/agent/` - executor, runner, hooks, compaction, wiki memory, providers, prompt composition.
- `crates/oxide-agent-core/src/storage/` - storage facade, SQLx backend, domain records (control-plane, reminders, flows).
- `crates/oxide-agent-core/src/llm/providers/` - LLM provider implementations.
- `crates/oxide-agent-core/src/sandbox/` - sandbox facade; backends: direct Docker, broker, Bubblewrap (`bwrap/`).
- `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/` - Agent Mode lifecycle, controls, callbacks, task runner, reminders.
- `crates/oxide-agent-transport-web/src/server/` - web console backend; `mod.rs` is a thin hub, `router.rs` owns route table/serve, route slices live in `*_routes.rs`, with `sse.rs`, `static_assets.rs`, `task_executor.rs`, and `types.rs` for streaming/assets/execution/state.
- `crates/oxide-agent-web-ui/src/` - Leptos frontend: components, routes, SSE client; CSS entrypoint is `styles.css`, with maintained slices in `styles/` (`00-tokens.css` through `10-responsive.css`).
- `crates/oxide-agent-core/src/capabilities/` - compiled module and capability manifests.
- `crates/oxide-agent-core/src/agent/tool_runtime/` - typed tool registration and execution.
- `docs/` - detailed documentation for hooks, integrations, sandbox, wiki memory, and TTS.

## Architectural invariants

- `oxide-agent-core` and `oxide-agent-runtime` do not depend on transport crates; transport crates depend on core/runtime.
- `teloxide` is used only in `oxide-agent-transport-telegram` and binaries that include it.
- Build and runtime composition are capability-module based. Manifests in `crates/oxide-agent-core/src/capabilities/`; tool registration in `tool_runtime/`.
- Cargo `default` features are intentionally empty. Use profile features: `profile-embedded-opencode-local`, `profile-web-embedded-opencode-local`, `profile-lite`, `profile-search-only`, `profile-no-sandbox`, `profile-media-enabled`, `profile-host-bwrap`, `profile-full`.
- Keep explicit `mod.rs` files and predictable public exports.
- Use `thiserror` for library crates, `anyhow` for app/binary crates.
- Topic-aware and thread-aware by default for agent mode and manager functions.
- Context-scoped storage is mandatory for transport contexts; legacy fallback only for DM compatibility.
- Topic-scoped `AGENTS.md` is stored separately, pinned during flow bootstrap, live-synced after `agents_md_update`, inherited by sub-agents.
- Sandbox backends are explicit: direct Docker (`SANDBOX_BACKEND=docker`), broker (`SANDBOX_BACKEND=broker`), or Bubblewrap (`SANDBOX_BACKEND=bwrap`). Default Compose stays on broker; bwrap must not require Docker.
- Manager CRUD goes through `manager_control_plane` provider with audit trail and RBAC (`manager_allowed_users`).
- `storage-sqlx` is the production durable storage. Local filesystem is transient only.
- Direct Google Gemini provider code must stay absent. Gemini models are valid only through OpenRouter.

## Key Subsystems

### Agent execution
- Runner in `agent/runner/`; executor slices in `agent/executor/`.
- Runner modules: `execution.rs`, `llm_calls.rs`, `model_routes.rs`, `response_dispatch.rs`, `runtime_compaction.rs`, `token_snapshots.rs`, `hooks.rs`, `loop_detection.rs`, `tools.rs`.
- Tool calls run in parallel; preserve history repair and `tool_call_id` integrity before LLM calls.
- Compaction is runner-integrated with typed message classes, budget estimator, hot-memory classifier, externalized large tool payloads, and LLM summarization sidecar. Legacy staged pipeline (classifier/prune/rebuild/summarizer) has been removed.

### Wiki memory
- Lives in `agent/wiki_memory/` -- no separate crate. Storage: SQLx/Postgres via the storage facade.
- Pages are deterministic Markdown: `{prefix}/wiki/v1/contexts/{context_id}/pages/{slug}.md`.
- Background planner (`planner.rs`) optionally uses LLM to extract structured memory.
- Tools: `wiki_memory_list`, `wiki_memory_read`, `wiki_memory_delete` (blocked for sub-agents).

### Hooks and sub-agents
- Hooks in `agent/hooks/`. Always active: `completion_check`, `tool_access_policy`, `hot_context_health`, `search_budget`, `timeout_report`. Memory hooks (`episodic_extract`, `retrieval_advisor`) activate when wiki memory writer is enabled. Sub-agent safety hook enforces delegation restrictions. Details: `docs/hooks/`.
- Loop detection has content, tool-sequence, and LLM layers; do not bypass in runner changes.
- Sub-agents: isolated `EphemeralSession`s, inherit topic-scoped `AGENTS.md`, cannot recurse/send files/mutate topics/control-plane/use reminders/`stack_logs`/`recreate_sandbox`.
- Do not reintroduce embedding-selected skills.

### Prompt cache hit
- **Static prefix + dynamic suffix** — все динамические блоки (date/time, wiki context) строго в конце system prompt. Стабильные блоки (fallback, workflow, structured output, topic AGENTS.md) в начале формируют cacheable prefix.
- **Assembly order**: `[fallback + profile + workflow_guidance + structured_output] + [wiki_context] + [date_context]`. Дата и wiki — всегда в конце.
- **Fold system messages** (`history.rs`): stable (`[TOPIC_AGENTS_MD]`, `[OXIDE_COMPACTED_SUMMARY_V1]`) идут перед `date_suffix` в cacheable prefix; volatile (retry notes, temporal context, infra status) — после `date_suffix`.
- **Tool schemas**: в prompt только compact sorted tool-name list (`~98 bytes`); полные JSON schemas — исключительно через native `tools[]` payload.
- **Compacted summary**: в prompt-visible текст только `generation` + `wiki_memory_lookup_available`; `created_at`, provider, route, token counts — только в логах.
- **Budget guard**: `compress` tool blocked при <85% context utilization, предотвращая premature compaction и сброс кэша.
- **Cache telemetry**: `TokenUsage` содержит `cached_tokens`, `cache_creation_tokens`, метод `cache_hit_rate()`. Парсится у всех 9 production providers.
- Детали: `docs/tips/cache-hit.md` — полный анализ, provider-specific механизмы, production validation, smoke test.

### Topic- and flow-scoped state
- Contexts in `UserConfig.contexts` via `UserContextConfig`. Memory uses context-scoped APIs.
- Chat history isolated via `scoped_chat_storage_id`.
- Flows support attach/detach UX; `forum_topic_list` available for topic discovery (blocked for sub-agents).

### Control plane and operations
- Manager control plane in `agent/providers/manager_control_plane/`; CRUD for topics, bindings, contexts, AGENTS.md, infra, sandboxes, profiles, controls, audit trail, rollback.
- Stack logs: Docker Compose log access, requires `topic_infra`, blocked for sub-agents.
- Reminders: `agent/providers/reminder.rs` + storage; in-memory scheduler wakes the original topic/flow.
- SSH: native upstream tools used directly; approval flow disabled.

### Sandbox and SSH
- Facade: `sandbox/manager.rs`; backends: direct Docker, broker (`broker.rs`), Bubblewrap (`bwrap/` -- 13 modules).
- `SandboxScope` provides stable identity for persistent sandbox reuse.
- SSH tools: `exec`, `sudo_exec`, `ssh_read_file`, `ssh_apply_file_edit`, `ssh_send_file_to_user`, `check_process`.
- Secret refs: `env:KEY`, `storage:PATH`; secrets must not reach prompts or memory.

### Storage and LLM
- Storage facade and SQLx/Postgres backend in `storage/`; context-scoped APIs for transport state.
- LLM providers in `llm/providers/`; shared orchestration: `llm/client.rs`, `llm/capabilities.rs`, `llm/support/` (backoff, HTTP pooling, OpenAI compat), `llm/types.rs`.
- Route failover: weighted `AGENT_MODEL_ROUTES__N__*` / `SUB_AGENT_MODEL_ROUTES__N__*`; persistent 429s quarantine a route.
- ChatGPT: OAuth/Codex Responses streaming; must fail over for structured-output/json-mode routes.
### Tool providers

- Extend in `agent/providers/`; keep the transport-agnostic contract. Feature-gated: sandbox, todos, tavily, duckduckgo, webfetch_md, crawl4ai-markdown, searxng, jira-mcp, mattermost-mcp (disabled), filehoster, delegation, manager_control_plane, ssh_mcp, yt-dlp, reminders, agents_md, wiki_memory, tts (Kokoro EN + Silero RU), stack_logs (disabled for topic agents, blocked for sub-agents), compression, file_delivery, path.
- `webfetch_md` and `crawl4ai_markdown` are mutually exclusive at runtime: if `OXIDE_CRAWL4AI_BASE_URL` is set (or `OXIDE_CRAWL4AI_ENABLED=true`), only `crawl4ai_markdown` is registered; otherwise only `webfetch_md` is registered. Override with `WEBFETCH_MD_ENABLED=false` as a belt-and-braces disable.

## Configuration

- Layered: optional `config/{RUN_MODE}.yaml`, `config/local.yaml` + env vars. Config files optional (`required(false)`).
- Provider secrets in `modules.<module-id>` with env fallbacks.
- Key runtime: DuckDuckGo, model routes, temperature, compaction budget, sandbox backend (`SANDBOX_BACKEND`, `BWRAP_*`), Jira MCP, wiki memory writer.
- Docker Compose split: `docker-compose.yml` (root), `docker-compose.telegram.yml`, `docker-compose.web.yml`. Optional local SearXNG/Crawl4AI overlays: `docker-compose.telegram.local-services.yml`, `docker-compose.web.local-services.yml`. Profile overlays in `docker/`.

## Development Practices

### Build
- `cargo check` for quick verification; `cargo build` only for final binary.
- Embedded: `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`.
- Full: `cargo build --release --no-default-features --features profile-full`.
- Bwrap: `cargo check --workspace --no-default-features --features profile-host-bwrap`.
- Other profiles: `profile-lite`, `profile-search-only`, `profile-no-sandbox`, `profile-media-enabled`, `profile-web-embedded-opencode-local`.
- Capability output (swap `<PROFILE>` and `<profile-name>`):
  - `cargo run -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features <PROFILE> -- capabilities --compiled --json`
  - `cargo run -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features <PROFILE> -- capabilities --enabled --json`
  - `cargo run -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features <PROFILE> -- config schema --compiled --json`
  - `cargo run -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features <PROFILE> -- config example --profile <profile-name> --json`
- Dependencies: `cargo add`, `cargo remove`, `cargo update`. Metadata: `workspace info`, `cargo info`.

### Format and lint
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all -- --check` must both pass before finishing. CI enforces both.

### Testing
- Helpers: `crates/oxide-agent-core/src/testing.rs` (`mock_llm_simple()`, `mock_storage_noop()`, `test_set_env()`, `test_remove_env()`).
- Categories: hermetic, integration, snapshot (`insta`), property/fuzz (`proptest`).
- E2E: `crates/oxide-agent-transport-web/tests/e2e.rs`.
- Transport-specific profiles (e.g. `profile-web-embedded-opencode-local`) do not activate features in unrelated crates. `cargo test --workspace` will fail on crates whose modules are behind different feature gates. Use scoped `-p` for such profiles: `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`. Full and lite profiles work with `--workspace`.
- The legacy modular shell guard layer was removed; use focused `cargo check`, `cargo test`, and Docker build checks for touched areas.

### Bwrap rootfs
- `scripts/build-bwrap-rootfs-debian.sh`, `build-bwrap-rootfs-host-smoke.sh`, `import-bwrap-rootfs-tar.sh`, `smoke-bwrap.sh`.

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
- `docs/wiki-memory.md` - wiki memory system: storage, planner, context assembly.
- `docs/bwrap-sandbox.md` - Bubblewrap sandbox backend: setup, rootfs, execution.
- `docs/silero-tts-api.md` - Silero TTS integration for Russian voice.
- `docs/context-window-tracking.md` - token budget and context window management.
- `docs/stack-logs-stage0.md` - stack logs tool: Docker Compose log access.
- `docs/deploy.md` - concise deploy guide, optional external services, local service overlays, operations.
- `README.md` - product overview and user-facing setup notes.
- `config/` and `.env.example` - runtime configuration examples.

## System extension

- New transport: `crates/oxide-agent-transport-<name>`; SDK and handlers inside the transport crate.
- Runtime/core must not depend on a specific transport SDK.
- Separate `oxide-agent-<name>-bot` binary if needed.
