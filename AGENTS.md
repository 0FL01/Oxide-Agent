# Проект: Oxide Agent

Этот проект представляет собой Telegram-бота, который интегрируется с различными API больших языковых моделей (LLM) для предоставления пользователям многофункционального ИИ-ассистента. Бот может обрабатывать текстовые, голосовые, видео сообщения и изображения, работать с документами, управлять историей диалога и выполнять сложные задачи в изолированной песочнице.

Бот разработан с использованием **Rust 1.94**, библиотеки `teloxide`, AWS SDK для взаимодействия с Cloudflare R2, и нативной интеграции с провайдерами ИИ (Groq, Mistral AI, Google Gemini, OpenRouter, ZAI/Zhipu AI).

## Branch

The default branch in this repo is `agent-topics`.

## 🏗 Структура проекта

```
crates/
├── oxide-agent-core/                # Ядро: домен, LLM, storage, тесты
│   ├── src/
│   │   ├── agent/                   # Логика агента
│   │   │   ├── executor.rs          # Core agent execution logic
│   │   │   ├── narrator.rs          # Dialogue management
│   │   │   ├── preprocessor.rs      # Input processing (voice/images)
│   │   │   ├── tool_bridge.rs       # Tool execution bridge
│   │   │   ├── structured_output.rs # Structured output parsing
│   │   │   ├── thoughts.rs          # Agent thought inference
│   │   │   ├── memory.rs            # Memory management with auto-compaction
│   │   │   ├── progress.rs          # Agent events
│   │   │   ├── session.rs           # AgentSession lifecycle management
│   │   │   ├── provider.rs          # Tool Provider trait
│   │   │   ├── registry.rs          # Tool Registry
│   │   │   ├── runner/              # Цикл исполнения
│   │   │   ├── hooks/               # Hook system (9 hooks)
│   │   │   │   └── tool_access.rs   # Tool policy enforcement
│   │   │   ├── loop_detection/      # Детектор зацикливания
│   │   │   ├── providers/           # Tool providers (sandbox, todos, manager, search, ssh_mcp)
│   │   │   │   └── manager_control_plane/
│   │   │   │       ├── mod.rs       # Public surface, provider wiring, dispatch
│   │   │   │       ├── audit.rs     # Audit persistence and rollback lookup helpers
│   │   │   │       ├── bindings.rs  # Topic binding CRUD + rollback
│   │   │   │       ├── contexts.rs  # Topic context CRUD + rollback
│   │   │   │       ├── agents_md.rs # Topic AGENTS.md CRUD + rollback
│   │   │   │       ├── infra.rs     # Topic infra CRUD, preview, preflight
│   │   │   │       ├── profiles.rs  # Agent profile CRUD + rollback
│   │   │   │       ├── agent_controls.rs # Topic agent tools/hooks controls
│   │   │   │       ├── forum_topics.rs   # Forum lifecycle, catalog, SSH provisioning
│   │   │   │       ├── sandboxes.rs # Topic sandbox inventory and lifecycle
│   │   │   │       ├── shared.rs    # Generic validation and serialization helpers
│   │   │   │       └── tests/mod.rs # Manager control-plane test suite
│   │   │   ├── skills/              # Реестр и поиск навыков (embeddings)
│   │   │   ├── profile.rs           # Agent profiles & policies
│   │   │   └── recovery.rs          # Восстановление XML/JSON
│   │   ├── llm/                     # Интеграции с AI
│   │   │   ├── mod.rs               # LlmClient struct
│   │   │   ├── common.rs            # Common utilities
│   │   │   ├── embeddings.rs        # Embedding provider
│   │   │   └── providers/           # Groq, Mistral, Gemini, OpenRouter, ZAI
│   │   │       └── zai/             # ZAI SDK internals (messages, stream transport)
│   │   ├── sandbox/                 # Sandbox facade, Docker backend, Unix-socket broker
│   │   │   ├── manager.rs           # SandboxManager facade + Docker backend implementation
│   │   │   ├── broker.rs            # Sandbox broker protocol/client/server over Unix socket
│   │   │   └── scope.rs             # SandboxScope stable container identity
│   │   ├── config.rs
│   │   ├── storage.rs
│   │   └── testing.rs               # TestKit: моки и хелперы
│   └── tests/                       # Интеграционные тесты
├── oxide-agent-runtime/             # Runtime: сессии и оркестрация
│   └── src/
│       ├── session_registry.rs      # Управление сессиями
│       └── agent/runtime/           # Progress runtime
├── oxide-agent-sandboxd/            # Sandbox broker daemon with Docker access
│   └── src/main.rs                  # Unix socket broker entry point
├── oxide-agent-transport-telegram/  # Транспорт: Telegram Bot API
│   ├── src/
│   │   ├── runner.rs                # Инициализация бота
│   │   ├── bot/
│   │   │   ├── handlers.rs          # Top-level Telegram handlers and menus
│   │   │   ├── agent_handlers/      # Agent Mode facade + modularized handler slices
│   │   │   │   ├── mod.rs           # Thin facade and re-exports
│   │   │   │   ├── lifecycle.rs     # Agent mode activation/message orchestration
│   │   │   │   ├── controls.rs      # Control commands, confirmations, exit flow
│   │   │   │   ├── callbacks.rs     # Inline callback routing and approvals
│   │   │   │   ├── input.rs         # Batched text and multimodal input handling
│   │   │   │   ├── task_runner.rs   # Task execution, progress, result delivery
│   │   │   │   ├── session.rs       # Session lifecycle, compat keys, registry helpers
│   │   │   │   ├── execution_config.rs # Execution profile, infra, reminder context wiring
│   │   │   │   ├── reminders.rs     # Reminder scheduler wake-up handling
│   │   │   │   ├── shared.rs        # Shared helpers and pending state maps
│   │   │   │   └── tests.rs         # Agent handler unit tests
│   │   │   ├── agent_transport.rs   # Transport adapter for progress/task updates
│   │   │   ├── context.rs           # Context-scoped transport state
│   │   │   ├── topic_route.rs       # Topic routing and dynamic binding resolution
│   │   │   ├── thread.rs            # Telegram thread/topic helpers
│   │   │   ├── manager_topic_lifecycle.rs # Manager topic provisioning helpers
│   │   │   ├── messaging.rs         # Long-message delivery helpers
│   │   │   ├── resilient.rs         # Resilient Telegram send/edit wrappers
│   │   │   ├── progress_render.rs   # HTML progress rendering
│   │   │   ├── unauthorized_cache.rs # Unauthorized access cooldown cache
│   │   │   ├── state.rs             # Dialogue state machine
│   │   │   └── views/               # UI component views
│   │   │       └── agent.rs         # Agent Mode UI components
│   │   └── tests/
└── oxide-agent-telegram-bot/        # Application Entry Point
    └── src/main.rs
skills/                              # Документация навыков агента (9 skills)
docs/                                # Комплексная документация
├── hooks/                           # Hook system documentation
│   └── sub-agents/                  # Sub-agent delegation lifecycle
├── opencode-int/                    # OpenCode sandbox integration
│   └── opencode-sandbox-integration/ # architecture, configuration, deployment, examples, testing
├── AGENT-TOPICS-BLUEPRINT.md
├── KOKORO-voice.md                  # Local Kokoro TTS API reference and ffmpeg usage
└── sdk-third-party-api-examples.md
sandbox/
└── Dockerfile.sandbox
```

### Workspace crates
- `oxide-agent-core`: доменная логика агента, LLM-интеграции, хуки, навыки, storage, control-plane CRUD/audit для manager tools. Включает `UserContextConfig` для per-transport контекстов и context-scoped storage API, embeddings support, hook system с manageable/protected hooks, `AgentExecutionProfile` с `ToolAccessPolicy`, `TopicContextRecord`, `TopicInfraConfigRecord`, `TopicAgentsMdRecord` для topic-scoped системных промптов, SSH MCP provider с approval flow.
- `oxide-agent-runtime`: оркестрация сессий, прогресс-рендеринг, session registry с thread-aware session keys.
- `oxide-agent-sandboxd`: отдельный broker daemon для sandbox. Слушает Unix socket (`SANDBOXD_SOCKET`), владеет `docker.sock`, принимает узкий sandbox protocol и выполняет Docker operations от имени основного агента.
- `oxide-agent-transport-telegram`: Telegram transport, UI/handlers, topic routing, thread context management, resilient messaging, progress rendering, unauthorized access protection, телеметрия доставки. Включает модульный `bot/agent_handlers/` (facade + lifecycle/controls/callbacks/input/task_runner/session/execution_config/reminders/shared/tests), `context.rs` для context-scoped state management с legacy fallback для DM-чатов и views module для UI компонентов.
- `oxide-agent-telegram-bot`: бинарь с конфигурацией и запуском Telegram транспорта.

## 🧪 Testing Infrastructure

### TestKit (`testing.rs`)
- `mock_llm_simple()`, `mock_storage_noop()` - Mock providers for isolated tests

### Test Categories
- **Hermetic Tests** - Isolated logic tests with mock dependencies
- **Fuzzing Tests** - Property-based testing with proptest
- **Snapshot Tests** - Prompt regression testing with insta
- **Integration Tests** - Cross-component testing (cancellation, providers, delegation, routing, XML prevention, manager lifecycle)

### Testing Dependencies
- `mockall` - Trait-based mocking, `insta` - Snapshot testing, `proptest` - Property-based testing

## 🦀 Rust Architecture & Workflow

### 1. Architecture & Structure
- **Feature Isolation**: `oxide-agent-core` и `oxide-agent-runtime` не должны зависеть от транспортных crate; транспорты зависят от core/runtime.
- **Transport Boundaries**: `teloxide` используется только в `oxide-agent-transport-telegram` (и бинарях, которые ее подключают).
- **Module Hierarchy**: В каждом crate сохраняем явные `mod.rs` и публичные экспорты модулей.
- **Error Handling**: Use `thiserror` for libraries and `anyhow` for apps.
- **Manager Control Plane**: manager CRUD идет через tool provider `manager_control_plane`, user-scoped storage records и audit trail; RBAC включается на уровне Telegram transport через `manager_allowed_users`.
- **Session Safety**: Для threaded AgentMode reuse/refresh опираемся на `SessionRegistry` safe APIs (`remove_if_idle`) и не удаляем running session из реестра.
- **Topic/Thread Routing**: Поддержка Telegram Forum Topics с per-topic конфигурацией, dynamic runtime bindings с expiry/activity tracking, и thread-aware session isolation.
- **Context-Scoped Storage**: Per-transport контексты используют `UserContextConfig` в `UserConfig.contexts`, context-scoped storage API для памяти агента (`save_agent_memory_for_context`, `load_agent_memory_for_context`, `clear_agent_memory_for_context`), и chat history isolation через `scoped_chat_storage_id` (format: `"{context_key}/{chat_uuid}"`). Legacy fallback для DM-чатов сохраняет обратную совместимость.
- **Configuration**: Поддержка layered конфигурации через YAML файлы в `config/` ({RUN_MODE}.yaml, local.yaml) + переменные окружения. Config files are optional (`required(false)`).
- **Sandbox Isolation Boundary**: при `SANDBOX_BACKEND=broker` основной `oxide_agent` больше не требует прямой доступ к `/var/run/docker.sock`; sandbox operations идут через `oxide-agent-sandboxd` по Unix socket, а Docker access остается только у broker service.

Чтобы добавить новый transport (Discord/Slack), создайте `crates/oxide-agent-transport-<name>`, держите SDK и обработчики внутри transport crate, подключите адаптер к runtime, и при необходимости добавьте отдельный бинарь `oxide-agent-<name>-bot` для запуска.

### 2. Operational Workflow
**Tools are enforced by the environment.**
- **Compilation**: Use `cargo check` for quick validation. Only use `cargo build` for final binaries.
- **Dependencies**: Use `cargo add`, `cargo remove`, `cargo update`.
- **Metadata**: Use `workspace info` for project topology and `cargo info` for crate details.

### 3. Code Quality
- **Linting**: Run `cargo clippy` before finishing a task.
- **Formatting**: Run `cargo fmt` before commit.

## 🔁 Loop Detection System

Three levels of protection against infinite loops: Content Detector (pattern matching), Tool Detector (repetitive sequences), LLM Detector (AI-based recognition).

**Components**: `LoopDetectionConfig`, `LoopDetectionService`, `content_detector.rs`, `tool_detector.rs`, `llm_detector.rs`, `types.rs`.

Integration via `LoopDetectionHook` in agent execution loop.

## 🎬 Agent Session Management

Task lifecycle tracking with timeout control, cancellation support, and sandbox persistence.

**Components**: `AgentSession`, `AgentStatus`, `session.rs`

**Features**: Task lifecycle tracking, 30-minute timeout, cancellation tokens, loaded skills tracking, memory management with auto-compaction.

**SandboxScope**: Stable container identity via FNV-1a hashing for persistent Docker containers across sessions.

**Sandbox Backends**: `SandboxManager` в `sandbox/manager.rs` является facade и поддерживает direct Docker backend и broker backend через Unix socket protocol (`sandbox/broker.rs`). Это сохраняет текущий API tool providers и выносит `docker.sock` из основного runtime container.

## 👥 Sub-Agent Architecture

**EphemeralSession**: Isolated context for sub-agent tasks, automatic cleanup, blocked tools (`delegate_to_sub_agent`, `send_file_to_user`, all reminder tools), session-scoped storage and memory.

**Delegation Flow**: `Main Agent → DelegationGuardHook → EphemeralSession → Sub-Agent → Result → Main Agent`

**Components**: `DelegationProvider`, `DelegationGuardHook`, `SubAgentSafetyHook`, `executor.rs`.

**Configuration**: `sub_agent_model_id`, `sub_agent_model_provider`, `sub_agent_max_tokens`.

## 🔄 Flow Storage & Attach/Detach

Topic-scoped agent flows with persistent memory isolation within forum topics.

**Components**: `AgentFlowRecord`, `UserContextConfig.current_agent_flow_id`, `context.rs`, `views/agent.rs`

**Features**: Multiple flows per topic, Attach/Detach UI controls (inline keyboard), flow-scoped storage API, automatic cleanup of abandoned empty flows.

**Storage**: `users/{user_id}/topics/{context_key}/flows/{flow_id}/` - flow-scoped memory and metadata in R2/S3.

**Delegation**: `forum_topic_list` tool is blocked for sub-agents.

## 📋 Topic-Scoped AGENTS.md

S3-backed system prompts per topic with manager CRUD operations.

**Components**: `TopicAgentsMdRecord`, storage API (`upsert_topic_agents_md`, `get_topic_agents_md`, `delete_topic_agents_md`), `composer.rs` injection logic.

**Features**: Per-topic system prompt storage (300-line limit, 40-line limit for topic_context), strict validation with duplicate content detection, pinned message injection on flow creation, preservation during memory compaction, manager tools (`topic_agents_md_upsert/get/delete/rollback`) with audit trail.

**Breaking Change**: `skills/AGENT.md` no longer used as default prompt source.

## 🎭 Narrator System

Separate LLM model for summarizing agent thoughts and generating narrative summaries.

**Components**: `narrator.rs`, `NarratorProvider`, `narrator_model_id`, `narrator_model_provider`.

**Features**: Thought summarization, narrative generation for multi-step tasks, enhanced dialogue management with context compression.

## 📊 Progress Rendering

Transport-agnostic progress reporting system.

**Components**: `ProgressState`, `Step`, `ProgressRuntime`, `ProgressRuntimeConfig`, `progress.rs` (runtime), `progress_render.rs` (transport).

**Transport Adaptation**: `AgentTransport::send_progress()`, views module for consistent UI, multi-step operations with status updates.

## 🖼 Views Module

UI component system for Agent Mode with localization support and transport-agnostic design.

**Components**: `AgentView` trait, `DefaultAgentView`, `views/agent.rs`

**Features**: 
- Text messages for all agent states (welcome, processing, errors, confirmations)
- Keyboard markups (resize keyboards for DM, inline keyboards for forum topics)
- 18 callback constants for user actions (cancel, clear memory, recreate container, attach/detach, exit, ssh approve/reject)
- Loop detection action keyboards

## 🔧 Hook System

Centralized hook system for agent behavior modification.

**Available Hooks**: `CompletionCheckHook`, `DelegationGuardHook`, `SearchBudgetHook`, `SubAgentSafetyHook`, `TimeoutReportHook`, `ToolAccessPolicyHook`, `WorkloadDistributorHook`.

**Management**: `HookRegistry`, `hooks.rs` (runner), `types.rs`. See `docs/hooks/` for comprehensive documentation.

**Hook Categories**:
- Manageable: `workload_distributor`, `delegation_guard`, `search_budget`, `timeout_report` (can be enabled/disabled per topic)
- Protected: `completion_check`, `tool_access_policy` (always active)

## 🧩 Manager Control Plane

CRUD operations for forum topics, agent profiles, topic contexts, infrastructure configs, AGENTS.md storage, and bindings with full audit trail.

**Components**: `ManagerControlPlaneProvider`, `manager_control_plane/mod.rs`, `audit.rs`, `bindings.rs`, `contexts.rs`, `agents_md.rs`, `infra.rs`, `profiles.rs`, `agent_controls.rs`, `forum_topics.rs`, `sandboxes.rs`, `shared.rs`, `AuditEventRecord`, `TopicBindingRecord`, `TopicBindingKind`, `TopicAgentsMdRecord`.

**Features**: Forum topic lifecycle, topic binding management, agent profile CRUD, topic context CRUD, topic AGENTS.md CRUD, infrastructure config CRUD, tool/hook control for topic agents, atomic topic provisioning, complete audit trail, RBAC via `manager_allowed_users`, reminder provider in topic agent catalog with aliases "reminder"/"wakeups".

**Forum Topic Catalog**: `forum_topic_list` tool for memory-independent topic discovery. Catalog entries persist topic metadata (name, icon, closed status) in S3 with automatic cleanup on topic deletion.

**Cleanup on Delete**: Automatic cleanup of agent memory, chat history, Docker containers, topic bindings, topic contexts, topic AGENTS.md, and infrastructure configs when forum topic is deleted.

**Sandbox Cleanup Path**: cleanup topic sandboxes по-прежнему строится вокруг `SandboxScope`, но фактическое удаление контейнера может идти либо напрямую через Docker backend, либо через broker client depending on `SANDBOX_BACKEND`.

**Storage**: User-scoped storage records, audit events logged to R2/S3, thread-aware isolation, private secret namespace for infrastructure credentials.

## 🎯 Skills System

Embedding-based skill matching and retrieval.

**Components**: `SkillRegistry`, `embeddings.rs`, `matcher.rs`, `cache.rs`, `loader.rs`.

**Available Skills** (9 skills in `skills/`): core, delegation_manager, ffmpeg-conversion, file-hosting, file-management, html-report, task-planning, video-processing, web-search.

**Configuration**: `embedding_provider`, `embedding_model_id`, auto-probing for embedding dimensions.

## 🔐 Topic Infrastructure Layer

Topic-scoped SSH infrastructure configuration with secure secret resolution and approval gating.

**Components**: `TopicInfraConfigRecord`, `TopicInfraAuthMode`, `TopicInfraToolMode`, `SshMcpProvider`, secret resolution (`env:KEY` or `storage:PATH` refs).

**Features**:
- SSH target configuration: host, port, remote_user, auth_mode (None/Password/PrivateKey)
- Allowed tool modes: exec, sudo_exec, read_file, apply_file_edit, check_process
- Secret refs resolved from private storage namespace or environment (never in prompts/memory)
- Upstream SSH-MCP binary via rmcp protocol with persistent connections
- Manager tools: `topic_infra_upsert/get/delete/rollback` with audit trail
- Automatic cleanup on forum topic deletion

---

## ✅ Approval Flow System

Short-lived approval gating for sensitive SSH operations with transport integration.

**Components**: `SshApprovalRegistry`, `SshApprovalRequestView`, `SshApprovalGrant`, `ApprovalState`.

**Flow**: Tool request → Approval registry → Transport UI (approve/reject) → System message injection → Replay with token → Execute.

**Approval Triggers**: Modes in `approval_required_modes`, dangerous commands (rm -rf, shutdown, terraform), sensitive paths (/etc/, /root/, .ssh).

**Features**: 600s TTL, topic-scoped, single-use tokens, automatic task retry after approval.

## ⏰ Reminder System

Scheduled wake-up tasks with background scheduler for deferred agent execution.

**Components**: `ReminderProvider`, `ReminderJobRecord`, `spawn_reminder_scheduler`.

**Schedule Types**: `Once` (timestamp/delay), `Interval` (recurring), `Cron` (timezone-aware expression).

**Tools**: `reminder_schedule`, `reminder_list`, `reminder_cancel`, `reminder_pause`, `reminder_resume`, `reminder_retry`.

**Storage**: `users/{user_id}/control_plane/reminders/{reminder_id}.json` with lease-based claiming.

**Scheduler**: 5s polling, 16 batch limit, 300s lease, wakes agent in original topic/flow.

---

## 🔌 Provider Ecosystem

### Tool Providers
`sandbox.rs`, `todos.rs`, `tavily.rs`, `crawl4ai/`, `filehoster.rs`, `delegation.rs`, `manager_control_plane/`, `ssh_mcp.rs`, `ytdlp.rs`, `reminder.rs`.

### Sandbox Stack
`sandbox/manager.rs` - facade and Docker backend, `sandbox/broker.rs` - Unix socket protocol/client/server, `sandbox/scope.rs` - stable sandbox naming/labels, `oxide-agent-sandboxd` - standalone broker binary.

### LLM Providers
`gemini.rs`, `groq.rs`, `mistral.rs`, `openrouter.rs`, `zai.rs`.

### Features
`tavily` - Tavily search provider, `crawl4ai` - Crawl4AI web scraping, `zai-rs` - ZAI SDK integration.

## ⚙️ Configuration System

### Layered Configuration
`config/default.yaml`, `config/{RUN_MODE}.yaml`, `config/local.yaml` + environment variables (all optional).

### Key Settings
`crawl4ai_url`, `crawl4ai_timeout_secs`, `search_provider` (tavily/crawl4ai), `embedding_provider`, `embedding_model_id`, `narrator_model_id`, `sub_agent_model_id`, model overrides (chat, agent, sub_agent, media, narrator), `SANDBOX_BACKEND` (`docker` or `broker`), `SANDBOXD_SOCKET`, `SANDBOX_IMAGE`.

### Telegram Settings
`topic_configs`, `manager_allowed_users_str`, cooldown constants for unauthorized access protection.

## 📚 Documentation

### Core Documentation
- `docs/hooks/` - Complete hook system documentation
- `docs/hooks/sub-agents/` - Sub-agent delegation lifecycle
- `docs/opencode-int/` - OpenCode sandbox integration
- `docs/AGENT-TOPICS-BLUEPRINT.md` - Topic-based routing design
- `docs/KOKORO-voice.md` - Local Kokoro TTS API reference
- `docs/sdk-third-party-api-examples.md` - API examples
- `skills/` - Skill documentation (9 skills)

### Testing Documentation
Hermetic testing patterns (`tests/hermetic_agent.rs`), snapshot testing with insta (`tests/snapshot_prompts.rs`), property-based testing with proptest (`tests/proptest_recovery.rs`).
