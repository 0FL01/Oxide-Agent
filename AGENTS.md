# Проект: Oxide Agent

Oxide Agent - Telegram-бот с Agent Mode поверх нескольких LLM-провайдеров. Бот умеет работать с текстом, голосом, изображениями, документами, topic-scoped памятью, sandbox-задачами и менеджерским control plane.

Стек: Rust 1.94, `teloxide`, AWS SDK для Cloudflare R2, нативные интеграции с Groq, Mistral AI, Google Gemini, OpenRouter, MiniMax AI (claude SDK) и ZAI/Zhipu AI.

## Branch

Default branch: `agent-topics`.

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
- Базовые точки входа: `executor.rs`, `session.rs`, `memory.rs`, `context.rs`, `profile.rs`, `tool_bridge.rs`, `structured_output.rs`, `recovery.rs`.
- Runner собран в `agent/runner/` и отвечает за execution loop, dispatch tool calls, response parsing, hook integration и loop detection wiring.
- `AgentSession` хранит lifecycle задачи, timeout, cancellation, loaded skills и hot-memory; compaction запускается orchestration layer'ом, а не как side effect `AgentMemory`.
- Narrator (`narrator.rs`) использует отдельную модель для thought summarization и narrative summary.

### Agent Mode compaction
- Код: `crates/oxide-agent-core/src/agent/compaction/`.
- Pipeline: budget estimation -> classify -> externalize -> prune -> summarize -> rebuild hot context -> optional archive refs.
- Сохраняются base system context, topic `AGENTS.md`, текущая задача, todos, runtime injections, approvals и recent working set.
- Крупные tool outputs сначала externalize/prune, потом попадают в LLM compaction.

### Hooks и loop detection
- Hook system: `agent/hooks/` + интеграция в `agent/runner/hooks.rs`.
- Всегда активны `completion_check` и `tool_access_policy`; topic-managed hooks: `workload_distributor`, `delegation_guard`, `search_budget`, `timeout_report`.
- Loop detection трехслойный: content detector, tool sequence detector, LLM detector.
- Справка и детали жизненного цикла: `docs/hooks/`.

### Sub-agents
- Делегация реализована через `DelegationProvider`, `DelegationGuardHook`, `SubAgentSafetyHook` и отдельную `EphemeralSession`.
- У sub-agent'ов изолированный контекст, отдельная память, автоматическая очистка и запрет на рекурсивную делегацию, отправку файлов пользователю и reminder tools.
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
- Storage record: `TopicAgentsMdRecord`; orchestration - через storage API и `prompt/composer.rs`.
- Topic prompt ограничен по размеру: до 300 строк для полного `AGENTS.md`, до 40 строк для `topic_context`.
- Topic `AGENTS.md` сохраняется при compaction и может быть управляем через manager CRUD с rollback/audit trail.

### Manager control plane
- Код: `agent/providers/manager_control_plane/`.
- Покрывает CRUD для forum topics, bindings, contexts, AGENTS.md, infra config, sandboxes, agent profiles и controls.
- Все операции журналируются в audit trail; есть rollback для поддерживаемых сущностей.
- При удалении forum topic автоматически чистятся topic memory, chat history, sandboxes, bindings, contexts, AGENTS.md и infra records.

### Sandbox и SSH infrastructure
- Sandbox facade: `crates/oxide-agent-core/src/sandbox/manager.rs`; backends - direct Docker или broker через `sandbox/broker.rs`.
- `SandboxScope` дает стабильную идентичность контейнера для persistent sandbox reuse.
- SSH infrastructure topic-scoped: `ssh_mcp.rs`, `TopicInfraConfigRecord`, `TopicInfraAuthMode`, `TopicInfraToolMode`.
- Secret refs поддерживают `env:KEY` и `storage:PATH`; секреты не должны попадать в prompts или memory.
- Разрешенные SSH tool modes включают `exec`, `sudo_exec`, `read_file`, `apply_file_edit`, `check_process`.

### Approval flow
- Используется для чувствительных SSH-операций.
- Компоненты: `SshApprovalRegistry`, `SshApprovalRequestView`, `SshApprovalGrant`, `ApprovalState`.
- Approval нужен для dangerous commands, чувствительных путей и режимов из `approval_required_modes`.
- Гранты topic-scoped, single-use, TTL 600s; после approve задача автоматически переигрывается.

### Reminder system
- Основной код: `agent/providers/reminder.rs` + storage records в `storage/reminder.rs` и `r2_reminder.rs`.
- Поддерживаются `Once`, `Interval`, `Cron` расписания.
- Основные tools: `reminder_schedule`, `reminder_list`, `reminder_cancel`, `reminder_pause`, `reminder_resume`, `reminder_retry`.
- Scheduler просыпает агента в исходном topic/flow; storage использует lease-based claiming.

### Progress и UI
- Progress runtime живет в `oxide-agent-runtime`, transport rendering - в `oxide-agent-transport-telegram/src/bot/progress_render.rs`.
- UI Agent Mode сосредоточен в `views/agent.rs` и `bot/agent_handlers/`.
- Transport слой отвечает за welcome/error/progress UI, inline callbacks, topic controls, media handling и resilient send/edit wrappers.

## Storage, LLM и providers

### Storage
- `storage/mod.rs` - публичный facade и реэкспорты.
- R2 backend разнесен по темам: base primitives, user/history, memory/flows, control plane, reminders.
- Storage tests сгруппированы по тематике в `storage/tests/`.

### LLM
- Базовый вход: `llm/mod.rs`, `common.rs`, `embeddings.rs`, `http_utils.rs`, `openai_compat.rs`.
- Провайдеры: `gemini`, `groq`, `mistral`, `minimax/` (folder structure: client, messages, tools, response), `openrouter`, `zai`.

### Tool providers
- Основные provider'ы: sandbox, todos, tavily, crawl4ai, filehoster, delegation, manager control plane, SSH MCP, yt-dlp, reminders.
- При расширении экосистемы провайдеров добавляй код в `agent/providers/` и сохраняй transport-agnostic контракт на уровне core.

## Telegram transport

- Весь Telegram-specific код держим в `crates/oxide-agent-transport-telegram`.
- `bot/agent_handlers/` разбит по ролям: lifecycle, controls, callbacks, input, task runner, session, reminders, shared helpers.
- `context.rs` и `topic_route.rs` отвечают за context-scoped state и topic binding resolution.
- `thread.rs` и `session_registry.rs` обеспечивают thread-aware session isolation.
- `agent/media.rs`, `messaging.rs`, `resilient.rs`, `unauthorized_cache.rs` закрывают медиа, длинные сообщения, retry/edit и защиту от неавторизованного доступа.

## Web transport (E2E tests)

- Crate: `crates/oxide-agent-transport-web` — изолированный transport для E2E-тестирования без зависимости от реальных LLM/Telegram API.
- HTTP API (axum): `POST /sessions`, `GET /sessions/:id`, `DELETE /sessions/:id`, `POST /sessions/:session_id/tasks`, `GET /tasks/:task_id/progress`, `GET /tasks/:task_id/events`, `GET /tasks/:task_id/stream` (SSE), `GET /tasks/:task_id/timeline`, `POST /tasks/:task_id/cancel`, `GET /health`.
- Scripted LLM provider (`src/scripted_llm.rs`): детерминированные ответы через `ScriptedResponse::Text` и `ScriptedResponse::ToolCalls`. `chat_with_tools()` возвращает валидный JSON (важно для structured output).
- `TaskEventLog` (`src/web_transport.rs`): буферизация событий в памяти + broadcast-канал для SSE. Методы: `push()`, `subscribe()`, `close()`, `snapshot()`, `drain()`.
- `collect_events()`: собирает `AgentEvent` из mpsc-канала, возвращает `(ProgressState, MilestoneTimestamps)`. Отслеживает `first_thinking_at` и `finished_at`.
- Latency milestones: `session_ready_ms` (HTTP → executor ready), `first_thinking_ms` (agent start → first Thinking), `final_response_ms` (agent start → Finished/Error/Cancelled).
- SSE endpoint: `stream!` macro + `tokio::select!` на broadcast-канале для непрерывной доставки событий.
- Task tracking: `AppState` хранит `task_handles: Arc<RwLock<HashMap<String, Arc<JoinHandle<()>>>>>` для abort при cancel.
- `yield_now()` после `tokio::spawn` в HTTP-handler — чтобы runtime драйвил внутренние spawned-задачи.

## Конфигурация

- Layered config: `config/default.yaml`, `config/{RUN_MODE}.yaml`, `config/local.yaml` + environment variables.
- Конфигурационные файлы опциональны (`required(false)`).
- Ключевые настройки: search provider, embedding provider/model, narrator/sub-agent model settings, media/chat/agent overrides, `SANDBOX_BACKEND`, `SANDBOXD_SOCKET`, `SANDBOX_IMAGE`.
- Telegram-специфика включает `topic_configs`, `manager_allowed_users_str` и cooldown-настройки для unauthorized access protection.

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

## Где искать подробности

- `docs/HANDOVER-NOTE.txt` - актуальный handover по rollout и operational detail'ам.
- `docs/hooks/` - документация по hooks и sub-agent lifecycle.
- `docs/AGENT-TOPICS-BLUEPRINT.md` - blueprint topic/platform дизайна.
- `docs/KOKORO-voice.md` - локальная TTS интеграция.
- `docs/sdk-third-party-api-examples.md` - примеры внешних SDK/API.
