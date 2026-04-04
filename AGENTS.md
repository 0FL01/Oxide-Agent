# Проект: Oxide Agent

Oxide Agent - Telegram-бот с Agent Mode поверх нескольких LLM-провайдеров. Бот умеет работать с текстом, голосом, изображениями, документами, topic-scoped памятью, sandbox-задачами и менеджерским control plane.

Стек: Rust 1.94, `teloxide`, AWS SDK для Cloudflare R2, нативные интеграции с Groq, Mistral AI, Google Gemini, OpenRouter, MiniMax AI (claude SDK) и ZAI/Zhipu AI.

## Branch

Default branch: `testing`.

## External Services

### browser_use_bridge (disabled)
> **NOTE**: Browser Use отключён. Требуется качественная vision-агентная модель за вменяемую цену за токен.
> Код и bridge-сервис сохранены; для включения нужно задать `BROWSER_USE_URL` в конфиге.
> Подробности: `docs/browser-use.md`.
- Python/FastAPI сервис в `services/browser_use_bridge/` для browser automation через browser_use.
- Архитектура: FastAPI app с slices (`models/`, `services/`, `utils/`), каждый slice — self-contained модуль.
  - `app/main.py` — HTTP endpoints (session lifecycle, screenshot, extract_content, health)
  - `app/config.py` — frozen Settings из env vars с префиксом `BROWSER_USE_BRIDGE_*`
  - `app/services/` — session_manager, profiles, browser_ops, llm_resolver
  - `app/utils/` — browser_utils (liveness probing), json_safe, text, time
- **Profile lifecycle**: `active` → `idle` → `stale` → deleted; TTL pruning (default 7 days), orphan reconciliation при bridge restart.
- **Profile scope isolation**: профили scoped (e.g., `topic-a`); cross-scope reuse отклоняется с HTTP 409; quota `max_profiles_per_scope` (default 3).
- **Execution modes**: `autonomous` (full browse) vs `navigation_only` (strict steering для follow-up tools, `enable_planning=False`, `max_actions_per_step=1`).
- **Keep-alive**: `navigation_only` runs запрашивают `keep_alive=True` для reuse runtime в follow-up tools (`extract_content`/`screenshot`); reconnect attempt при dead runtime.
- **Runtime liveness**: probe_browser_runtime_ready(), probe_browser_session_state(); transient CDP errors retried; observability via `browser_runtime_alive`, `browser_reconnect_attempted/succeeded`.
- **LLM resolution**: request-level `BrowserLlmConfig` (с `api_key_ref` типа `env:ZAI_API_KEY`) или legacy `BROWSER_USE_BRIDGE_LLM_PROVIDER/MODEL`; провайдеры: `browser_use`, `google`, `anthropic`, `minimax`, `zai`, `openrouter`, `openai_compatible`; schema forcing relaxed для `zai`/`zhipuai`/`glm`.
- **Visual route guardrails**: configurable guardrails для visual follow-ups; legacy steering wrapper detection (tasks starting с `"Browser Use execution rules for this run:"` → `navigation_only`).
- **Screenshots**: hydrate в sandbox artifacts dir; доступны через `/sessions/{id}/artifacts/{artifact_id}`.

## Workspace Overview

### Основные crate'ы
- `crates/oxide-agent-core` - домен агента: execution loop, hooks, skills, compaction, storage facade, LLM providers, sandbox facade, reminder/SSH/manager providers.
- `crates/oxide-agent-runtime` - runtime-оркестрация сессий и transport-agnostic progress runtime.
- `crates/oxide-agent-transport-telegram` - Telegram transport: handlers, routing, views, progress rendering, topic/thread integration, resilient messaging.
- `crates/oxide-agent-transport-web` - E2E test web transport: HTTP API (axum), in-memory storage, scripted LLM provider, SSE streaming, latency milestone tracking.
- `crates/oxide-agent-sandboxd` - broker daemon для sandbox backend; держит доступ к Docker и слушает Unix socket.
- `crates/oxide-agent-telegram-bot` - бинарь запуска Telegram-бота.

### Где обычно искать код
- `crates/oxide-agent-core/src/agent/` - executor, runner, hooks, loop detection, skills, compaction, providers.
- `crates/oxide-agent-core/src/storage/` - storage facade, R2 backend, control-plane records, reminder persistence.
- `crates/oxide-agent-core/src/llm/providers/` - реализации LLM-провайдеров.
- `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/` - lifecycle Agent Mode, controls, callbacks, task runner, reminders.
- `crates/oxide-agent-transport-telegram/src/bot/views/agent.rs` - UI Agent Mode.
- `crates/oxide-agent-transport-web/src/` - web transport: HTTP server, session manager, scripted LLM, event log/SSE.
- `docs/` - подробная документация по rollout, hooks, интеграциям и blueprint'ам.
- `skills/` - системные навыки агента.

## Архитектурные инварианты

- `oxide-agent-core` и `oxide-agent-runtime` не зависят от transport crate; transport crate зависят от core/runtime.
- `teloxide` используется только в `oxide-agent-transport-telegram` и бинарях, которые его подключают.
- В crate сохраняем явные `mod.rs` и предсказуемые публичные экспорты.
- Для library crate используем `thiserror`, для app/binary crate - `anyhow`.
- Agent Mode и manager/topic-функции проектируются как topic-aware и thread-aware.
- Context-scoped storage обязателен для transport-контекстов; legacy fallback допустим только для DM-совместимости.
- `Topic AGENTS.md` хранится отдельно в storage и инжектится prompt composer'ом; `skills/AGENT.md` больше не является дефолтным источником системного промпта.
- Sandbox работает либо напрямую через Docker backend, либо через broker backend; при `SANDBOX_BACKEND=broker` доступ к `docker.sock` остается только у `oxide-agent-sandboxd`.
- Manager CRUD идет через provider `manager_control_plane` с audit trail и RBAC на уровне Telegram transport (`manager_allowed_users`).

## Ключевые подсистемы

### Agent execution model
- Runner (`agent/runner/`) - execution loop, tool dispatch, response parsing, hook integration, loop detection.
- `AgentSession` - lifecycle tasks, timeout, cancellation, loaded skills, hot-memory.
- **Parallel tool execution** - multiple tool calls in one LLM response run concurrently.
- **Fire-and-forget checkpoint** - memory persistence is async, non-blocking; flow checkpoints coalesced to skip identical snapshots.
- **History repair** - tool_call_id validation before LLM calls; orphaned tool results prevented during compaction.
- **Cold-start tool drift pruning** - removes stale tool calls from persisted memories; configurable via `STARTUP_TOOL_DRIFT_PRUNE_*` env vars.
- Narrator - separate model for thought/narrative summarization.

### Agent Mode compaction
- Pipeline: budget estimation -> classify -> externalize -> prune -> summarize -> rebuild hot context.
- Token-based protected window (configurable via `COMPACTION_PROTECTED_TOOL_WINDOW_TOKENS`).
- Compaction summarization inherits `AGENT_MODEL_ROUTES`/`SUB_AGENT_MODEL_ROUTES` fallback.
- Prunes only before summary boundary; delegate results skip externalization.
- **Superseded dedup** - Stage-4 дедупликация идентичных read-only результатов (read_file, list_files, agents_md_get, stack_logs). Блокируется при mutation между чтениями (exec, write_file, apply_file_edit).

### Model Route Failover
- Weighted fallback routes via `AGENT_MODEL_ROUTES__N__*` / `SUB_AGENT_MODEL_ROUTES__N__*`.
- Route quarantine after persistent 429s; emits `ProviderFailoverActivated` event.

### Hooks и loop detection
- Hook system: `agent/hooks/` + интеграция в `agent/runner/hooks.rs`.
- Всегда активны `completion_check` и `tool_access_policy`; topic-managed hooks: `workload_distributor`, `delegation_guard`, `search_budget`, `timeout_report`.
- Loop detection трехслойный: content detector, tool sequence detector, LLM detector.
- Справка и детали жизненного цикла: `docs/hooks/`.

### Sub-agents
- Делегация реализована через `DelegationProvider`, `DelegationGuardHook`, `SubAgentSafetyHook` и отдельную `EphemeralSession`.
- У sub-agent'ов изолированный контекст, отдельная память, автоматическая очистка и запрет на рекурсивную делегацию, отправку файлов пользователю, reminder tools и stack_logs (операционные логи инфраструктуры).
- Конфигурация: `sub_agent_model_id`, `sub_agent_model_provider`, `sub_agent_max_tokens`.

### Skills
- Код: `agent/skills/{registry,embeddings,matcher,cache,loader,types}.rs`.
- Матчинг навыков основан на embeddings; размерность embeddings может определяться автоматически.
- Доступные системные навыки лежат в `skills/`.

### Topic- and flow-scoped state
- Per-transport контексты живут в `UserConfig.contexts` через `UserContextConfig`.
- Для памяти агента используются context-scoped storage API: `save_agent_memory_for_context`, `load_agent_memory_for_context`, `clear_agent_memory_for_context`.
- Chat history изолируется через `scoped_chat_storage_id` формата `"{context_key}/{chat_uuid}"`.
- Topic-scoped flows поддерживают attach/detach UX и хранятся по префиксу `users/{user_id}/topics/{context_key}/flows/{flow_id}/`.
- `forum_topic_list` доступен для memory-independent topic discovery, но заблокирован у sub-agent'ов.

### Topic-scoped AGENTS.md
- Storage record: `TopicAgentsMdRecord`; orchestration via storage API and `prompt/composer.rs`.
- Topic prompt ограничен: до 300 строк для `AGENTS.md`, до 40 строк для `topic_context`.
- Self-editing tools: `agents_md_get`, `agents_md_update` (top-level agents only).

### Manager control plane
- Код: `agent/providers/manager_control_plane/`.
- Покрывает CRUD для forum topics, bindings, contexts, AGENTS.md, infra config, sandboxes, agent profiles и controls.
- Все операции журналируются в audit trail; есть rollback для поддерживаемых сущностей.
- Optional `MANAGER_HOME_*` env vars restrict operations к specific topic when configured.
- При удалении forum topic автоматически чистятся topic memory, chat history, sandboxes, bindings, contexts, AGENTS.md и infra records.

### Stack logs
- Доступ к логам Docker Compose через `stack_logs_list_sources` и `stack_logs_fetch`.
- Default: 200 entries, max 500; фильтрация по времени, сервису, pagination.
- Требует `topic_infra` (SSH) для доступа; управляется через manager control plane (`topic_agent_tools_enable`/`disable`).
- Заблокированы для sub-agent'ов (операционная безопасность).

### Sandbox и SSH infrastructure
- Sandbox facade: `crates/oxide-agent-core/src/sandbox/manager.rs`; backends - direct Docker или broker через `sandbox/broker.rs`.
- `SandboxScope` дает стабильную идентичность контейнера для persistent sandbox reuse.
- SSH tools: `exec`, `sudo_exec`, `read_file`, `apply_file_edit`, `check_process`, `ssh_send_file_to_user`.
- Secret refs поддерживают `env:KEY` и `storage:PATH`; секреты не должны попадать в prompts или memory.
- `recreate_sandbox` - exclusive lock, reset workspace; blocked для sub-agents.

### Approval flow
- Используется для чувствительных SSH-операций.
- Компоненты: `SshApprovalRegistry`, `SshApprovalRequestView`, `SshApprovalGrant`, `ApprovalState`.
- Approval нужен для dangerous commands, чувствительных путей и режимов из `approval_required_modes`.
- Гранты topic-scoped, single-use, TTL 600s; после approve задача автоматически переигрывается.

### Reminder system
- Основной код: `agent/providers/reminder.rs` + storage records в `storage/reminder.rs` и `r2_reminder.rs`.
- Поддерживаются `Once`, `Interval`, `Cron` расписания.
- Simplified args: `date`, `time`, `every_minutes`, `every_hours`, `timezone`, `weekdays`; partial date/time inputs supported.
- Основные tools: `reminder_schedule`, `reminder_list`, `reminder_cancel`, `reminder_pause`, `reminder_resume`, `reminder_retry`.
- In-memory scheduler queue (bootstrap из storage); просыпает агента в исходном topic/flow.

### Progress и UI
- Progress runtime живет в `oxide-agent-runtime`, transport rendering - в `oxide-agent-transport-telegram/src/bot/progress_render.rs`.
- UI Agent Mode сосредоточен в `views/agent.rs` и `bot/agent_handlers/`.
- Transport слой отвечает за welcome/error/progress UI, inline callbacks, topic controls, media handling и resilient send/edit wrappers.
- Rate limit status отображается сразу в UI; автоматически сбрасывается при восстановлении.

## Storage, LLM и providers

### Storage
- `storage/mod.rs` - facade и реэкспорты; R2 backend разнесен по темам.
- R2 telemetry: operation counts, cache hit/miss.
- `R2_REGION` env (default `auto`) - MinIO/Wasabi/B2 compatibility.
- Tests: `storage/tests/`.

### LLM
- Providers: `gemini`, `groq`, `mistral`, `minimax/`, `nvidia`, `openrouter`, `zai`.
- Browser-use bridge LLM resolution (disabled): отдельный от core провайдеров; поддерживает `browser_use`, `google`, `anthropic`, `minimax`, `zai`, `openrouter`, `openai_compatible`; schema forcing relaxed для `zai`/`zhipuai`/`glm`.
- HTTP connection pooling + tokenizer caching (~15s startup latency eliminated).
- Voice transcription: `voxtral` (Mistral) с retry backoff (5 attempts, 3s→48s).
- NVIDIA NIM provider: `nvidia/llama-3.3-nemotron-super-49b-v1`, `nvidia/nemotron-mini`, `minimaxai/minimax-m*`.
- LLM module structure: `capabilities.rs` (model capabilities), `client.rs` (HTTP orchestration), `support/` (backoff, history, http utils), `types.rs` (domain types).

### Tool providers
- sandbox, todos, tavily, searxng (self-hosted), crawl4ai, jira-mcp, mattermost-mcp (disabled by default), filehoster, delegation, manager control plane, SSH MCP (включая `ssh_send_file_to_user`), yt-dlp, reminders, agents_md, TTS (Kokoro EN + Silero RU), browser-use bridge (disabled), **stack_logs** (Docker Compose логи; disabled by default для topic agents, blocked для sub-agents).
- Расширяй в `agent/providers/`; сохраняй transport-agnostic контракт.

## Telegram transport

- `crates/oxide-agent-transport-telegram` - handlers, routing, views, progress rendering, topic/thread integration, resilient messaging.
- `bot/agent_handlers/` - lifecycle, controls, callbacks, input, task runner, session, reminders.
- `context.rs`, `topic_route.rs` - context-scoped state, topic binding resolution.
- `thread.rs`, `session_registry.rs` - thread-aware session isolation.
- Rate limit status отображается в UI; provider failover notice показывается при переключении.

## Web transport (E2E tests)

- `crates/oxide-agent-transport-web` — изолированный transport для E2E-тестирования без зависимости от реальных LLM/Telegram API.
- HTTP API (axum): sessions CRUD, task execution, SSE streaming (`/tasks/:id/stream`), timeline, health.
- Scripted LLM provider: `ScriptedResponse::Text` и `ScriptedResponse::ToolCalls` для deterministic responses.
- Latency milestones: `session_ready_ms`, `first_thinking_ms`, `final_response_ms`.

## Конфигурация

- Layered config: `config/default.yaml`, `config/{RUN_MODE}.yaml`, `config/local.yaml` + environment variables.
- Конфигурационные файлы опциональны (`required(false)`).
- Ключевые: search/embedding provider, SearXNG (`SEARXNG_URL`), narrator/sub-agent model, `AGENT_MODEL_ROUTES__N__*`, `COMPACTION_PROTECTED_TOOL_WINDOW_TOKENS`, `SANDBOX_BACKEND`, Jira MCP (`JIRA_URL`, `JIRA_EMAIL`, `JIRA_API_TOKEN`).

## Практика разработки

### Сборка и зависимости
- Для быстрой проверки используй `cargo check`; `cargo build` - только когда нужен итоговый бинарь.
- Для зависимостей используй `cargo add`, `cargo remove`, `cargo update`.
- Для метаданных workspace используй `workspace info` и `cargo info`.

### Форматирование и lint
- Перед завершением задачи запускай `cargo clippy`.
- Перед коммитом запускай `cargo fmt`.

### Тестирование
- Test helpers: `crates/oxide-agent-core/src/testing.rs` (`mock_llm_simple()`, `mock_storage_noop()`).
- Основные категории: hermetic tests, integration tests, snapshot tests (`insta`), property/fuzz tests (`proptest`).
- E2E tests: `crates/oxide-agent-transport-web/tests/e2e.rs` — 6 E2E тестов (session lifecycle, task execution, SSE streaming, latency milestones).
- Полезные ориентиры: `tests/hermetic_agent.rs`, `tests/snapshot_prompts.rs`, `tests/proptest_recovery.rs`.

## Расширение системы

- Новый transport добавляй как `crates/oxide-agent-transport-<name>`; SDK и handlers держи внутри transport crate.
- Runtime/core не должны получать зависимость на конкретный transport SDK.
- При необходимости добавляй отдельный бинарь `oxide-agent-<name>-bot` для запуска transport-а.