# Oxide Agent

Oxide Agent is a Telegram bot with Agent Mode on top of multiple LLM providers. It can work with text, voice, images, documents, topic-scoped memory, sandbox tasks, and a manager control plane.

Stack: Rust 1.94, `teloxide`, AWS SDK for Cloudflare R2, native integrations with Groq, Mistral AI, Google Gemini, OpenRouter, MiniMax AI (claude SDK), and ZAI/Zhipu AI.

## Branch

Default branch: `testing`.

## Scale and decision principles

- This project is for personal use and sharing with at most 2-3 people; target load is up to 5 RPS.
- Over-engineering is forbidden: do not add enterprise/distributed complexity, sharding, HA, extra queues, multi-layer abstractions, or heavy observability without a proven need.
- Prefer the simplest maintainable solution for this scale; optimize only after a real bottleneck.

## External Services

### browser_use_bridge (disabled)
- Browser Use is disabled until a cost-effective high-quality vision-agent model is available.
- Code remains in `services/browser_use_bridge/`; enable it with `BROWSER_USE_URL` and see `docs/browser-use.md`.
- Keep scope isolation, `navigation_only` guardrails, and runtime liveness/reconnect behavior when touching the bridge.

## Workspace Overview

### Main crates
- `crates/oxide-agent-core` - agent domain: execution loop, hooks, skills, compaction, storage facade, LLM providers, sandbox facade, wiki memory (store, cache, context, planner, patch), reminder/SSH/manager providers.
- `crates/oxide-agent-runtime` - session runtime orchestration and transport-agnostic progress runtime.
- `crates/oxide-agent-transport-telegram` - Telegram transport: handlers, routing, views, progress rendering, topic/thread integration, resilient messaging.
- `crates/oxide-agent-transport-web` - E2E test web transport: HTTP API (axum), in-memory storage, scripted LLM provider, SSE streaming, latency milestone tracking.
- `crates/oxide-agent-sandboxd` - broker daemon for the sandbox backend; keeps access to Docker and listens on a Unix socket.
- `crates/oxide-agent-telegram-bot` - Telegram bot binary.

### Where code usually lives
- `crates/oxide-agent-core/src/agent/` - executor (slices: config, execution, registry, compaction, policy_hooks, types), runner, hooks, loop detection, skills, compaction, wiki memory (store, cache, context, planner, patch), providers.
- `crates/oxide-agent-core/src/storage/` - storage facade, R2 backend, control-plane records, reminder persistence.
- `crates/oxide-agent-core/src/llm/providers/` - LLM provider implementations.
- `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/` - Agent Mode lifecycle, controls, callbacks, task runner, reminders.
- `crates/oxide-agent-transport-telegram/src/bot/views/agent.rs` - Agent Mode UI.
- `crates/oxide-agent-transport-web/src/` - web transport: HTTP server, session manager, scripted LLM, event log/SSE.
- `docs/` - detailed documentation for rollout, hooks, integrations, and blueprints.
- `skills/` - system skills.

## Architectural invariants

- `oxide-agent-core` and `oxide-agent-runtime` do not depend on transport crates; transport crates depend on core/runtime.
- `teloxide` is used only in `oxide-agent-transport-telegram` and binaries that include it.
- Keep explicit `mod.rs` files and predictable public exports.
- Use `thiserror` for library crates, `anyhow` for app/binary crates.
- Agent Mode and manager/topic functions are designed to be topic-aware and thread-aware.
- Context-scoped storage is mandatory for transport contexts; legacy fallback is allowed only for DM compatibility.
- `Topic AGENTS.md` is stored separately in storage, pinned into flow memory during bootstrap, live-synced after `agents_md_update`, and inherited by sub-agents during delegation; `skills/AGENT.md` is no longer the default source of the system prompt.
- Sandbox runs either directly through the Docker backend or through the broker backend; with `SANDBOX_BACKEND=broker`, access to `docker.sock` stays only with `oxide-agent-sandboxd`.
- Manager CRUD goes through the `manager_control_plane` provider with an audit trail and RBAC at the Telegram transport level (`manager_allowed_users`).
- Wiki memory lives entirely in `crates/oxide-agent-core/src/agent/wiki_memory/`; no separate memory crate exists.

## Key subsystems

### Agent execution model
- Runner lives in `crates/oxide-agent-core/src/agent/runner/`; executor slices live under `agent/executor/`.
- Sessions handle lifecycle, cancellation, loaded skills, hot memory, and transport-independent progress.
- Tool calls can run in parallel; preserve history repair and `tool_call_id` integrity before LLM calls.
- Compaction protects recent tool context, prunes only before the summary boundary, and coalesces identical checkpoints.

### Wiki memory (replaces persistent memory)
- All wiki memory lives in `crates/oxide-agent-core/src/agent/wiki_memory/` — no separate crate.
- Storage is S3/R2 object storage (same as all other durable state); no Postgres dependency.
- Wiki pages are deterministic Markdown objects: `{prefix}/wiki/v1/contexts/{context_id}/pages/{slug}.md`.
- Prompt assembly loads wiki context via `load_wiki_text` from the storage facade.
- Background writer (`planner.rs`) optionally uses an LLM to extract structured memory from conversation.
- Main tools: `wiki_memory_list`, `wiki_memory_read`, `wiki_memory_delete` (blocked for sub-agents).

### Hooks, sub-agents, and skills
- Hooks live in `agent/hooks/`; `completion_check` and `tool_access_policy` are always active. Details: `docs/hooks/`.
- Loop detection has content, tool-sequence, and LLM layers; avoid bypassing it in runner changes.
- Sub-agents use isolated `EphemeralSession`s, inherit topic-scoped `AGENTS.md`, and cannot recurse, send files, mutate topics/control-plane state, use reminders, `stack_logs`, or `recreate_sandbox`.
- Skills live in `agent/skills/` and `skills/`; matching is embedding-based.

### Topic- and flow-scoped state
- Per-transport contexts live in `UserConfig.contexts` through `UserContextConfig`.
- Agent memory uses context-scoped storage APIs: `save_agent_memory_for_context`, `load_agent_memory_for_context`, `clear_agent_memory_for_context`.
- Chat history is isolated via `scoped_chat_storage_id` in the form `"{context_key}/{chat_uuid}"`.
- Topic-scoped flows support attach/detach UX and are stored under the `users/{user_id}/topics/{context_key}/flows/{flow_id}/` prefix.
- `forum_topic_list` is available for memory-independent topic discovery, but blocked for sub-agents.
- Topic-scoped `AGENTS.md` is a separate storage record, pinned during flow bootstrap, live-synced after `agents_md_update`, and inherited by sub-agents.

### Control plane and operational tools
- Manager control plane lives in `agent/providers/manager_control_plane/`; it owns CRUD for topics, bindings, contexts, AGENTS.md, infra, sandboxes, profiles, controls, audit trail, and rollback.
- Stack logs tools read Docker Compose logs, require `topic_infra`, and are blocked for sub-agents.
- Reminders live in `agent/providers/reminder.rs` plus storage records; the in-memory scheduler wakes the original topic/flow.
- SSH approval flow is currently disabled; native upstream SSH file tools are used directly.

### Sandbox and SSH
- Sandbox facade: `crates/oxide-agent-core/src/sandbox/manager.rs`; backends are direct Docker or broker via `sandbox/broker.rs`.
- `SandboxScope` provides stable container identity for persistent sandbox reuse.
- SSH tools: `exec`, `sudo_exec`, `ssh_read_file`, `ssh_apply_file_edit`, `ssh_send_file_to_user`, `check_process`.
- Secret refs support `env:KEY` and `storage:PATH`; secrets must not reach prompts or memory.

### Storage and LLM
- Storage facade and R2 backend are under `crates/oxide-agent-core/src/storage/`; use context-scoped APIs for transport state.
- LLM providers live in `crates/oxide-agent-core/src/llm/providers/`; shared orchestration is in `llm/client.rs`, `llm/capabilities.rs`, `llm/support/`, and `llm/types.rs`.
- Route failover uses weighted `AGENT_MODEL_ROUTES__N__*` / `SUB_AGENT_MODEL_ROUTES__N__*`; persistent 429s quarantine a route and emit `ProviderFailoverActivated`.
- ChatGPT uses OAuth/Codex Responses streaming and must fail over for structured-output/json-mode routes.

### Tool providers
- sandbox, todos, tavily, searxng (self-hosted), crawl4ai, jira-mcp, mattermost-mcp (disabled by default), filehoster, delegation, manager control plane, SSH MCP (native upstream file tools + legacy fallback), yt-dlp, reminders, agents_md, **wiki memory** (list, read, delete), TTS (Kokoro EN + Silero RU), browser-use bridge (disabled), **stack_logs** (Docker Compose logs; disabled by default for topic agents, blocked for sub-agents).
- Extend in `agent/providers/`; keep the transport-agnostic contract.

## Telegram transport

- `crates/oxide-agent-transport-telegram` - handlers, routing, views, progress rendering, topic/thread integration, resilient messaging.
- `bot/agent_handlers/` - lifecycle, controls, callbacks, input, task runner, session, reminders.
- `context.rs`, `topic_route.rs`, `thread.rs`, `session_registry.rs` - context, topic, and thread isolation.
- Rate-limit and provider-failover states are rendered in the UI.

## Web transport (E2E tests)

- `crates/oxide-agent-transport-web` — isolated transport for E2E testing without a dependency on real LLM/Telegram APIs.
- HTTP API (axum): sessions CRUD, task execution, SSE streaming (`/tasks/:id/stream`), timeline, health.
- Scripted LLM provider: `ScriptedResponse::Text` and `ScriptedResponse::ToolCalls` for deterministic responses.
- Latency milestones: `session_ready_ms`, `first_thinking_ms`, `final_response_ms`.

## Configuration

- Layered config: `config/default.yaml`, `config/{RUN_MODE}.yaml`, `config/local.yaml` + environment variables.
- Config files are optional (`required(false)`).
- Key items: `CHATGPT_AUTH_PATH`, search/embedding provider, SearXNG (`SEARXNG_URL`), narrator/sub-agent model, `AGENT_MODEL_ROUTES__N__*`, `AGENT_MODEL_TEMPERATURE`, `COMPACTION_PROTECTED_TOOL_WINDOW_TOKENS`, `SANDBOX_BACKEND`, Jira MCP (`JIRA_URL`, `JIRA_EMAIL`, `JIRA_API_TOKEN`), wiki memory writer (`WIKI_MEMORY_WRITER_ENABLED`, model config).
- Telegram transport config: `ATTACH_DETACH_ENABLED` (default true).

## Development practice

### Build and dependencies
- Use `cargo check` for quick verification; use `cargo build` only when you need the final binary.
- Use `cargo add`, `cargo remove`, `cargo update` for dependencies.
- Use `workspace info` and `cargo info` for workspace metadata.

### Formatting and lint
- Run `cargo clippy` before finishing a task.
- Run `cargo fmt` before committing.

### Testing
- Test helpers: `crates/oxide-agent-core/src/testing.rs` (`mock_llm_simple()`, `mock_storage_noop()`).
- Main categories: hermetic tests, integration tests, snapshot tests (`insta`), property/fuzz tests (`proptest`).
- E2E tests: `crates/oxide-agent-transport-web/tests/e2e.rs` — 6 E2E tests (session lifecycle, task execution, SSE streaming, latency milestones).
- Useful references: `tests/hermetic_agent.rs`, `tests/snapshot_prompts.rs`, `tests/proptest_recovery.rs`.

### Commit style
- Use full commit messages, not short one-line commits.
- Format:
  - `<type>(<scope>): <description>`
  - blank line
  - indented body with `Changes:` and 2-4 concrete bullets
- Types: `feat`, `fix`, `chore`, `docs`, `refactor`, `test`.

Example:

```text
feat(sources): add bybit proof of reserves source

    Changes:
    - Add Bybit proof-of-reserves source using the official frontend reserve ratio JSON endpoint
    - Normalize target asset reserve ratio and missing-asset transparency candidates with source-local tests
    - Wire scheduled checks and refresh source docs
```

## Where to find details

- `docs/hooks/` - hook lifecycle and managed hook behavior.
- `docs/browser-use.md` - disabled browser-use bridge details.
- `README.md` / `README-ru.md` - product overview and user-facing setup notes.
- `config/` and `.env.example` - runtime configuration examples.

## System extension

- Add a new transport as `crates/oxide-agent-transport-<name>`; keep SDK and handlers inside the transport crate.
- Runtime/core must not depend on a specific transport SDK.
- Add a separate `oxide-agent-<name>-bot` binary if needed to run the transport.
