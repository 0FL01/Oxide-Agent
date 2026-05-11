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
> **NOTE**: Browser Use is disabled. A high-quality vision-agent model is required at a reasonable token cost.
> The code and bridge service are kept; enable it by setting `BROWSER_USE_URL` in the config.
> Details: `docs/browser-use.md`.
- Python/FastAPI service in `services/browser_use_bridge/` for browser automation via browser_use.
- Architecture: FastAPI app with slices (`models/`, `services/`, `utils/`), each slice is a self-contained module.
  - `app/main.py` — HTTP endpoints (session lifecycle, screenshot, extract_content, health)
  - `app/config.py` — frozen Settings from env vars with the `BROWSER_USE_BRIDGE_*` prefix
  - `app/services/` — session_manager, profiles, browser_ops, llm_resolver
  - `app/utils/` — browser_utils (liveness probing), json_safe, text, time
- **Profile lifecycle**: `active` → `idle` → `stale` → deleted; TTL pruning (default 7 days), orphan reconciliation on bridge restart.
- **Profile scope isolation**: profiles are scoped (e.g. `topic-a`); cross-scope reuse is rejected with HTTP 409; quota `max_profiles_per_scope` (default 3).
- **Execution modes**: `autonomous` (full browse) vs `navigation_only` (strict steering for follow-up tools, `enable_planning=False`, `max_actions_per_step=1`).
- **Keep-alive**: `navigation_only` runs request `keep_alive=True` for runtime reuse in follow-up tools (`extract_content`/`screenshot`); reconnect attempt on dead runtime.
- **Runtime liveness**: `probe_browser_runtime_ready()`, `probe_browser_session_state()`; transient CDP errors are retried; observability via `browser_runtime_alive`, `browser_reconnect_attempted/succeeded`.
- **LLM resolution**: request-level `BrowserLlmConfig` (with `api_key_ref` like `env:ZAI_API_KEY`) or legacy `BROWSER_USE_BRIDGE_LLM_PROVIDER/MODEL`; providers: `browser_use`, `google`, `anthropic`, `minimax`, `zai`, `openrouter`, `openai_compatible`; schema forcing is relaxed for `zai`/`zhipuai`/`glm`.
- **Visual route guardrails**: configurable guardrails for visual follow-ups; legacy steering wrapper detection (tasks starting with `"Browser Use execution rules for this run:"` → `navigation_only`).
- **Screenshots**: hydrated into the sandbox artifacts dir; available via `/sessions/{id}/artifacts/{artifact_id}`.

## Workspace Overview

### Main crates
- `crates/oxide-agent-core` - agent domain: execution loop, hooks, skills, compaction, storage facade, LLM providers, sandbox facade, persistent memory (classifier, retrieval, embeddings, post-run), reminder/SSH/manager providers.
- `crates/oxide-agent-memory` - domain model, Postgres repository, consolidation, finalization, and hybrid retrieval (lexical + vector) for persistent agent memory.
- `crates/oxide-agent-runtime` - session runtime orchestration and transport-agnostic progress runtime.
- `crates/oxide-agent-transport-telegram` - Telegram transport: handlers, routing, views, progress rendering, topic/thread integration, resilient messaging.
- `crates/oxide-agent-transport-web` - E2E test web transport: HTTP API (axum), in-memory storage, scripted LLM provider, SSE streaming, latency milestone tracking.
- `crates/oxide-agent-sandboxd` - broker daemon for the sandbox backend; keeps access to Docker and listens on a Unix socket.
- `crates/oxide-agent-telegram-bot` - Telegram bot binary.

### Where code usually lives
- `crates/oxide-agent-core/src/agent/` - executor (slices: config, execution, registry, compaction, policy_hooks, types), runner, hooks, loop detection, skills, compaction, persistent memory (classifier, retrieval, embeddings, post-run, coordinator), providers.
- `crates/oxide-agent-memory/src/` - domain types, repository trait, Postgres repo, consolidation, finalization, in-memory harness.
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
- `oxide-agent-memory` defines the domain model and repository trait; `oxide-agent-core` implements orchestration (classifier, coordinator, embeddings, post-run) on top of it.

## Key subsystems

### Agent execution model
- Runner (`agent/runner/`) - execution loop, tool dispatch, response parsing, hook integration, loop detection.
- `AgentSession` - lifecycle tasks, timeout, cancellation, loaded skills, hot memory.
- **Parallel tool execution** - multiple tool calls in one LLM response run concurrently.
- **Fire-and-forget checkpoint** - memory persistence is async and non-blocking; flow checkpoints are coalesced to skip identical snapshots.
- **History repair** - `tool_call_id` validation before LLM calls; orphaned tool results are prevented during compaction.
- **Cold-start tool drift pruning** - removes stale tool calls from persisted memories; configurable via `STARTUP_TOOL_DRIFT_PRUNE_*` env vars.
- Narrator - separate model for thought/narrative summarization.
- **Configurable temperature** - `AGENT_MODEL_TEMPERATURE` env var for overriding main-agent generation.

### Persistent memory
- Code: `agent/persistent_memory/{classifier,coordinator,retrieval,embeddings,post_run,store,behavior}.rs` in core; domain model in `oxide-agent-memory`.
- Postgres backend (pgvector + tsvector); hybrid retrieval (lexical + vector + fusion scoring).
- **LLM Memory Classifier** — 9-class taxonomy (Smalltalk, EpisodeHistory, ExternalFreshFact, ProcedureHowTo, ConstraintPolicy, PreferenceRecall, DecisionRecall, DurableProjectFact, General); returns read/write policy.
- **Post-run memory writer** — LLM-based extraction of reusable memories from a completed task (up to 8 records): Fact, Preference, Procedure, Decision, Constraint.
- **Episode finalizer** — deterministic generation of Thread + Episode + SessionState records.
- **Context consolidator** — importance decay, exact + similarity dedup, TTL expiry, stale session cleanup.
- **Embedding indexer** — async query/document-aware embeddings + backfill; the active embedding profile id includes provider/model/dimensions/prompt-style/prefixes and is used for cache/vector isolation; `openai-base` requires `EMBEDDING_OPENAI_BASE_URL` and `EMBEDDING_OPENAI_API_KEY`.
- **Retrieval advisor hook** — injects contextual cards for memory search suggestion, history, episode.
- **Behavior hooks** — `EpisodicExtractHook` captures tool-derived memory drafts (write_file -> Procedure, failed exec -> Fact, repeated edits -> Preference).
- Memory read tools: `memory_search`, `memory_read_episode`, `memory_read_thread_summary`, `memory_read_thread_window`, `memory_diagnostics`.
- Config: `MEMORY_DATABASE_URL`, `MEMORY_DATABASE_MAX_CONNECTIONS`, `MEMORY_DATABASE_AUTO_MIGRATE`, `EMBEDDING_DIMENSIONS`, `EMBEDDING_OPENAI_BASE_URL`, `EMBEDDING_OPENAI_API_KEY`, `EMBEDDING_PROMPT_STYLE`, `EMBEDDING_QUERY_PREFIX`, `EMBEDDING_DOCUMENT_PREFIX`, classifier env vars.

### Agent Mode compaction
- Pipeline: budget estimation -> classify -> externalize -> prune -> summarize -> rebuild hot context.
- Token-based protected window (configurable via `COMPACTION_PROTECTED_TOOL_WINDOW_TOKENS`).
- Compaction summarization inherits the `AGENT_MODEL_ROUTES`/`SUB_AGENT_MODEL_ROUTES` fallback.
- Prunes only before the summary boundary; delegated results skip externalization.
- **Superseded dedup** - Stage-4 deduplication of identical read-only results (read_file, list_files, agents_md_get, stack_logs). Blocked by mutation between reads (exec, write_file, apply_file_edit).

### Model Route Failover
- Weighted fallback routes via `AGENT_MODEL_ROUTES__N__*` / `SUB_AGENT_MODEL_ROUTES__N__*`.
- Route quarantine after persistent 429s; emits the `ProviderFailoverActivated` event.

### Hooks and loop detection
- Hook system: `agent/hooks/` + integration in `agent/runner/hooks.rs`.
- `completion_check` and `tool_access_policy` are always active; topic-managed hooks: `workload_distributor`, `delegation_guard`, `search_budget`, `timeout_report`.
- Loop detection is three-layered: content detector, tool sequence detector, LLM detector.
- Guide and lifecycle details: `docs/hooks/`.

### Sub-agents
- Delegation is implemented via `DelegationProvider`, `DelegationGuardHook`, `SubAgentSafetyHook`, and a separate `EphemeralSession`; on bootstrap, the sub-agent inherits the topic-scoped `AGENTS.md`.
- Sub-agents have isolated context, separate memory, automatic cleanup, and are forbidden from recursive delegation, sending files to the user, `recreate_sandbox`, reminders, `stack_logs`, and the entire topic-mutation/control-plane tool surface (`topic_*`, `agents_md_*`, `forum_topic_*`, profile/infra mutations).
- Configuration: `sub_agent_model_id`, `sub_agent_model_provider`, `sub_agent_max_tokens`.

### Skills
- Code: `agent/skills/{registry,embeddings,matcher,cache,loader,types}.rs`.
- Skill matching is embedding-based; embedding dimensions can be auto-detected.
- Available system skills live in `skills/`.

### Topic- and flow-scoped state
- Per-transport contexts live in `UserConfig.contexts` through `UserContextConfig`.
- Agent memory uses context-scoped storage APIs: `save_agent_memory_for_context`, `load_agent_memory_for_context`, `clear_agent_memory_for_context`.
- Chat history is isolated via `scoped_chat_storage_id` in the form `"{context_key}/{chat_uuid}"`.
- Topic-scoped flows support attach/detach UX and are stored under the `users/{user_id}/topics/{context_key}/flows/{flow_id}/` prefix.
- `forum_topic_list` is available for memory-independent topic discovery, but blocked for sub-agents.

### Topic-scoped AGENTS.md
- Storage record: `TopicAgentsMdRecord`; flow bootstrap loads the record into pinned system memory, and `agents_md_update` syncs the updated text back into the live session and checkpoint.
- Topic prompt limit: up to 300 lines for `AGENTS.md`, up to 40 lines for `topic_context`.
- Tool surface: `agents_md_get` / `agents_md_update` for topic agent sessions; the manager control plane provides `topic_agents_md_{upsert,get,delete,rollback}` with audit trail and rollback.

### Manager control plane
- Code: `agent/providers/manager_control_plane/`.
- Covers CRUD for forum topics, bindings, contexts, AGENTS.md, infra config, sandboxes, agent profiles, and controls.
- All operations are logged in the audit trail; rollback exists for supported entities.
- Optional `MANAGER_HOME_*` env vars restrict operations to a specific topic when configured.
- Deleting a forum topic automatically cleans up topic memory, chat history, sandboxes, bindings, contexts, AGENTS.md, and infra records.

### Stack logs
- Access Docker Compose logs through `stack_logs_list_sources` and `stack_logs_fetch`.
- Default: 200 entries, max 500; filtering by time, service, pagination.
- Requires `topic_infra` (SSH) access; controlled via the manager control plane (`topic_agent_tools_enable`/`disable`).
- Blocked for sub-agents (operational safety).

### Sandbox and SSH infrastructure
- Sandbox facade: `crates/oxide-agent-core/src/sandbox/manager.rs`; backends are direct Docker or broker via `sandbox/broker.rs`.
- `SandboxScope` provides a stable container identity for persistent sandbox reuse.
- SSH tools: `exec`, `sudo_exec`, `ssh_read_file`, `ssh_apply_file_edit`, `ssh_send_file_to_user`, `check_process`.
- Native upstream file tools (ssh-mcp binary) are used for `ssh_read_file`, `ssh_apply_file_edit`, `ssh_send_file_to_user`; legacy Python fallback for non-absolute paths.
- Secret refs support `env:KEY` and `storage:PATH`; secrets must not reach prompts or memory.
- `recreate_sandbox` - exclusive lock, workspace reset; blocked for sub-agents.

### Approval flow
- Used for sensitive SSH operations.
- Components: `SshApprovalRegistry`, `SshApprovalRequestView`, `SshApprovalGrant`, `ApprovalState`.
- Approval is required for dangerous commands, sensitive paths, and modes from `approval_required_modes`.
- Grants are topic-scoped, single-use, TTL 600s; after approval, the task is automatically re-run.
- **Note**: the SSH approval flow is currently disabled — pending payloads are lost between request and grant; native upstream tools are used directly.

### Reminder system
- Main code: `agent/providers/reminder.rs` + storage records in `storage/reminder.rs` and `r2_reminder.rs`.
- Supported schedules: `Once`, `Interval`, `Cron`.
- Simplified args: `date`, `time`, `every_minutes`, `every_hours`, `timezone`, `weekdays`; partial date/time inputs are supported.
- Main tools: `reminder_schedule`, `reminder_list`, `reminder_cancel`, `reminder_pause`, `reminder_resume`, `reminder_retry`.
- In-memory scheduler queue (bootstrapped from storage); wakes the agent in the original topic/flow.

### Progress and UI
- Progress runtime lives in `oxide-agent-runtime`, transport rendering - in `oxide-agent-transport-telegram/src/bot/progress_render.rs`.
- Agent Mode UI is centered in `views/agent.rs` and `bot/agent_handlers/`.
- The transport layer handles welcome/error/progress UI, inline callbacks, topic controls, media handling, and resilient send/edit wrappers.
- Rate limit status is shown immediately in the UI; it is reset automatically on recovery.

## Storage, LLM, and providers

### Storage
- `storage/mod.rs` - facade and re-exports; the R2 backend is split by topic.
- R2 telemetry: operation counts, cache hit/miss.
- `R2_REGION` env (default `auto`) - MinIO/Wasabi/B2 compatibility.
- Tests: `storage/tests/`.

### LLM
- Providers: `chatgpt`, `gemini`, `groq`, `mistral`, `minimax/`, `nvidia`, `openrouter`, `zai`.
- Browser-use bridge LLM resolution (disabled): separate from core providers; supports `browser_use`, `google`, `anthropic`, `minimax`, `zai`, `openrouter`, `openai_compatible`; schema forcing is relaxed for `zai`/`zhipuai`/`glm`.
- The `chatgpt` provider uses an OAuth auth file (`CHATGPT_AUTH_PATH`) and the Codex/Responses streaming API; structured-output/json-mode routes for it are disabled and must fail over to a non-ChatGPT route.
- HTTP connection pooling + tokenizer caching (~15s startup latency eliminated).
- Embedding dimensions: default 1024, configurable via `EMBEDDING_DIMENSIONS`; Mistral provider skips the dimensions param (auto-handles truncation); custom OpenAI-compatible embeddings are available via `EMBEDDING_PROVIDER=openai-base` + `EMBEDDING_OPENAI_BASE_URL` + `EMBEDDING_OPENAI_API_KEY`.
- Voice transcription: `voxtral` (Mistral) with retry backoff (5 attempts, 3s→48s).
- NVIDIA NIM provider: `nvidia/llama-3.3-nemotron-super-49b-v1`, `nvidia/nemotron-mini`, `minimaxai/minimax-m*`.
- LLM module structure: `capabilities.rs` (model capabilities), `client.rs` (HTTP orchestration), `support/` (backoff, history, http utils), `types.rs` (domain types).

### Tool providers
- sandbox, todos, tavily, searxng (self-hosted), crawl4ai, jira-mcp, mattermost-mcp (disabled by default), filehoster, delegation, manager control plane, SSH MCP (native upstream file tools + legacy fallback), yt-dlp, reminders, agents_md, **persistent memory** (search, episode read, thread summary/window, diagnostics), TTS (Kokoro EN + Silero RU), browser-use bridge (disabled), **stack_logs** (Docker Compose logs; disabled by default for topic agents, blocked for sub-agents).
- Extend in `agent/providers/`; keep the transport-agnostic contract.

## Telegram transport

- `crates/oxide-agent-transport-telegram` - handlers, routing, views, progress rendering, topic/thread integration, resilient messaging.
- `bot/agent_handlers/` - lifecycle, controls, callbacks, input, task runner, session, reminders.
- `context.rs`, `topic_route.rs` - context-scoped state, topic binding resolution.
- `thread.rs`, `session_registry.rs` - thread-aware session isolation.
- Rate limit status is shown in the UI; provider failover notice is shown on switch.

## Web transport (E2E tests)

- `crates/oxide-agent-transport-web` — isolated transport for E2E testing without a dependency on real LLM/Telegram APIs.
- HTTP API (axum): sessions CRUD, task execution, SSE streaming (`/tasks/:id/stream`), timeline, health.
- Scripted LLM provider: `ScriptedResponse::Text` and `ScriptedResponse::ToolCalls` for deterministic responses.
- Latency milestones: `session_ready_ms`, `first_thinking_ms`, `final_response_ms`.

## Configuration

- Layered config: `config/default.yaml`, `config/{RUN_MODE}.yaml`, `config/local.yaml` + environment variables.
- Config files are optional (`required(false)`).
- Key items: `CHATGPT_AUTH_PATH`, search/embedding provider, SearXNG (`SEARXNG_URL`), narrator/sub-agent model, `AGENT_MODEL_ROUTES__N__*`, `AGENT_MODEL_TEMPERATURE`, `COMPACTION_PROTECTED_TOOL_WINDOW_TOKENS`, `SANDBOX_BACKEND`, persistent memory (`MEMORY_DATABASE_URL`, `EMBEDDING_DIMENSIONS`, `EMBEDDING_OPENAI_BASE_URL`, `EMBEDDING_OPENAI_API_KEY`, `EMBEDDING_PROMPT_STYLE`, `EMBEDDING_QUERY_PREFIX`, `EMBEDDING_DOCUMENT_PREFIX`), Jira MCP (`JIRA_URL`, `JIRA_EMAIL`, `JIRA_API_TOKEN`).
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

## System extension

- Add a new transport as `crates/oxide-agent-transport-<name>`; keep SDK and handlers inside the transport crate.
- Runtime/core must not depend on a specific transport SDK.
- Add a separate `oxide-agent-<name>-bot` binary if needed to run the transport.
