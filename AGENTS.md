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
│   │   │   ├── compaction/          # Staged Agent Mode compaction pipeline
│   │   │   │   ├── mod.rs           # Public exports and orchestration
│   │   │   │   ├── budget.rs        # Budget estimation and token counting
│   │   │   │   ├── classifier.rs    # Message classification stage
│   │   │   │   ├── externalize.rs   # Externalize large messages to storage
│   │   │   │   ├── prune.rs         # Prune redundant/compacted content
│   │   │   │   ├── prompt.rs        # Summarization prompt building
│   │   │   │   ├── summarizer.rs    # LLM-based summarization stage
│   │   │   │   ├── rebuild.rs       # Rebuild hot context from summary
│   │   │   │   ├── archive.rs       # Archive old conversation segments
│   │   │   │   ├── service.rs       # Main compaction service orchestrator
│   │   │   │   ├── types.rs         # Compaction types and data structures
│   │   │   │   └── tests/           # Compaction test suite
│   │   │   │       ├── mod.rs
│   │   │   │       ├── fixtures.rs
│   │   │   │       ├── budget_boundaries.rs
│   │   │   │       ├── cleanup_stages.rs
│   │   │   │       ├── recent_window.rs
│   │   │   │       ├── rebuild_archive.rs
│   │   │   │       └── summary_paths.rs
│   │   │   ├── hooks/               # Hook system (9 hooks)
│   │   │   │   ├── mod.rs           # Hook runner and public exports
│   │   │   │   ├── registry.rs      # Hook registration and management
│   │   │   │   ├── types.rs         # Hook trait definitions and types
│   │   │   │   ├── completion.rs    # CompletionCheckHook
│   │   │   │   ├── delegation_guard.rs  # DelegationGuardHook
│   │   │   │   ├── search_budget.rs # SearchBudgetHook
│   │   │   │   ├── sub_agent_safety.rs  # SubAgentSafetyHook
│   │   │   │   ├── timeout_report.rs    # TimeoutReportHook
│   │   │   │   ├── tool_access.rs   # ToolAccessPolicyHook
│   │   │   │   └── workload.rs      # WorkloadDistributorHook
│   │   │   ├── loop_detection/      # Детектор зацикливания
│   │   │   │   ├── mod.rs           # Public exports
│   │   │   │   ├── config.rs        # Loop detection configuration
│   │   │   │   ├── service.rs       # Main loop detection service
│   │   │   │   ├── types.rs         # Loop detection types
│   │   │   │   ├── content_detector.rs  # Content pattern detection
│   │   │   │   ├── tool_detector.rs # Repetitive tool sequence detection
│   │   │   │   └── llm_detector.rs  # LLM-based loop detection
│   │   │   ├── providers/           # Tool providers (sandbox, todos, search, etc)
│   │   │   │   ├── mod.rs           # Provider module exports
│   │   │   │   ├── sandbox.rs       # Sandbox execution provider
│   │   │   │   ├── todos.rs         # Todo/task management provider
│   │   │   │   ├── tavily.rs        # Tavily search provider
│   │   │   │   ├── ytdlp.rs         # yt-dlp media download provider
│   │   │   │   ├── filehoster.rs    # File hosting operations
│   │   │   │   ├── path.rs          # Path utilities for sandbox
│   │   │   │   ├── ssh_mcp.rs       # SSH MCP provider with approval flow
│   │   │   │   ├── delegation.rs    # Sub-agent delegation provider
│   │   │   │   ├── reminder.rs      # Reminder scheduling provider
│   │   │   │   ├── crawl4ai/        # Crawl4AI web scraping
│   │   │   │   │   ├── mod.rs
│   │   │   │   │   ├── response.rs
│   │   │   │   │   └── tests.rs
│   │   │   │   └── manager_control_plane/  # Manager CRUD operations
│   │   │   │       ├── mod.rs       # Public surface and dispatch
│   │   │   │       ├── audit.rs     # Audit persistence and rollback
│   │   │   │       ├── bindings.rs  # Topic binding CRUD + rollback
│   │   │   │       ├── contexts.rs  # Topic context CRUD + rollback
│   │   │   │       ├── agents_md.rs # Topic AGENTS.md CRUD + rollback
│   │   │   │       ├── infra.rs     # Topic infrastructure CRUD
│   │   │   │       ├── profiles.rs  # Agent profile CRUD + rollback
│   │   │   │       ├── agent_controls.rs   # Topic agent tools/hooks controls
│   │   │   │       ├── forum_topics.rs     # Forum lifecycle and catalog
│   │   │   │       ├── sandboxes.rs # Topic sandbox inventory
│   │   │   │       ├── shared.rs    # Validation and serialization helpers
│   │   │   │       └── tests/mod.rs # Manager control-plane tests
│   │   │   ├── runner/              # Цикл исполнения агента
│   │   │   │   ├── mod.rs           # Runner public exports
│   │   │   │   ├── execution.rs     # Core execution loop logic
│   │   │   │   ├── tools.rs         # Tool call handling and dispatch
│   │   │   │   ├── responses.rs     # Response processing and parsing
│   │   │   │   ├── hooks.rs         # Hook integration in runner
│   │   │   │   ├── loop_detection.rs    # Loop detection integration
│   │   │   │   └── types.rs         # Runner types and state
│   │   │   ├── skills/              # Реестр и поиск навыков
│   │   │   │   ├── mod.rs           # Skills module exports
│   │   │   │   ├── registry.rs      # Skill registry management
│   │   │   │   ├── embeddings.rs    # Embedding generation and matching
│   │   │   │   ├── matcher.rs       # Skill matching logic
│   │   │   │   ├── cache.rs         # Embedding cache
│   │   │   │   ├── loader.rs        # Skill loading from files
│   │   │   │   └── types.rs         # Skill type definitions
│   │   │   ├── prompt/              # Prompt building and composition
│   │   │   │   ├── mod.rs           # Prompt module exports
│   │   │   │   └── composer.rs      # Prompt composer with AGENTS.md injection
│   │   │   ├── executor.rs          # Core agent execution logic
│   │   │   ├── session.rs           # AgentSession lifecycle management
│   │   │   ├── memory.rs            # Memory storage model and typed messages
│   │   │   ├── context.rs           # Agent context and state
│   │   │   ├── identity.rs          # Agent identity and persona
│   │   │   ├── progress.rs          # Agent progress events
│   │   │   ├── provider.rs          # Tool Provider trait
│   │   │   ├── registry.rs          # Tool Registry
│   │   │   ├── tool_bridge.rs       # Tool execution bridge
│   │   │   ├── structured_output.rs # Structured output parsing
│   │   │   ├── thoughts.rs          # Agent thought inference
│   │   │   ├── narrator.rs          # Narrator for thought summarization
│   │   │   ├── preprocessor.rs      # Input processing (voice/images)
│   │   │   ├── profile.rs           # Agent profiles & policies
│   │   │   └── recovery.rs          # Восстановление XML/JSON
│   │   ├── llm/                     # Интеграции с AI
│   │   │   ├── mod.rs               # LlmClient and public exports
│   │   │   ├── common.rs            # Common utilities
│   │   │   ├── embeddings.rs        # Embedding provider interface
│   │   │   ├── http_utils.rs        # HTTP utilities for LLM calls
│   │   │   ├── openai_compat.rs     # OpenAI-compatible API format
│   │   │   └── providers/           # LLM provider implementations
│   │   │       ├── mod.rs           # Provider module exports
│   │   │       ├── gemini.rs        # Google Gemini provider
│   │   │       ├── groq.rs          # Groq provider
│   │   │       ├── mistral.rs       # Mistral AI provider
│   │   │       ├── openrouter.rs    # OpenRouter provider
│   │   │       ├── openrouter/      # OpenRouter helpers
│   │   │       │   └── helpers.rs
│   │   │       ├── zai.rs           # ZAI/Zhipu AI provider
│   │   │       └── zai/             # ZAI SDK internals
│   │   │           ├── sdk.rs       # ZAI SDK client
│   │   │           └── sdk/
│   │   │               ├── messages.rs  # Message handling
│   │   │               └── stream.rs    # Streaming support
│   │   ├── sandbox/                 # Sandbox facade and backends
│   │   │   ├── mod.rs               # Sandbox module exports
│   │   │   ├── manager.rs           # SandboxManager facade + Docker backend
│   │   │   ├── broker.rs            # Unix socket broker protocol
│   │   │   └── scope.rs             # SandboxScope stable identity
│   │   ├── config.rs                # Configuration structures
│   │   ├── storage/                 # Storage facade, contracts, R2 backend, tests
│   │   │   ├── mod.rs               # Public storage facade and re-exports
│   │   │   ├── error.rs             # Storage errors
│   │   │   ├── keys.rs              # Storage key builders and prefixes
│   │   │   ├── provider.rs          # StorageProvider trait and mock support
│   │   │   ├── user.rs              # User config/message domain types
│   │   │   ├── flows.rs             # Agent flow record types
│   │   │   ├── control_plane.rs     # Control-plane records and validation helpers
│   │   │   ├── reminder.rs          # Reminder records and schedule helpers
│   │   │   ├── schema.rs            # Storage schema version constants
│   │   │   ├── builders.rs          # Record construction/version helpers
│   │   │   ├── utils.rs             # Shared retry/time/audit-page helpers
│   │   │   ├── r2.rs                # R2Storage struct
│   │   │   ├── r2_provider.rs       # StorageProvider impl for R2Storage
│   │   │   ├── r2_base.rs           # Shared R2 primitives, cache, conditional writes, locks
│   │   │   ├── r2_user.rs           # User config/history R2 operations
│   │   │   ├── r2_memory.rs         # Agent memory and flow R2 operations
│   │   │   ├── r2_control_plane.rs  # Control-plane and secret R2 operations
│   │   │   ├── r2_reminder.rs       # Reminder R2 operations
│   │   │   └── tests/               # Storage unit tests by topic
│   │   │       ├── mod.rs
│   │   │       ├── keys_and_user.rs
│   │   │       ├── reminders.rs
│   │   │       ├── prompts.rs
│   │   │       ├── builders.rs
│   │   │       ├── bindings.rs
│   │   │       └── utils.rs
│   │   ├── testing.rs               # TestKit: моки и хелперы
│   │   ├── utils.rs                 # General utilities
│   │   └── lib.rs                   # Core library exports
│   └── tests/                       # Интеграционные и lifecycle тесты
├── oxide-agent-runtime/             # Runtime: сессии и оркестрация
│   └── src/
│       ├── agent/
│       │   ├── mod.rs               # Runtime agent module
│       │   └── runtime/
│       │       ├── mod.rs           # Progress runtime exports
│       │       └── progress.rs      # Progress runtime implementation
│       ├── session_registry.rs      # Управление сессиями
│       └── lib.rs                   # Runtime library exports
├── oxide-agent-sandboxd/            # Sandbox broker daemon with Docker access
│   └── src/
│       └── main.rs                  # Unix socket broker entry point
├── oxide-agent-transport-telegram/  # Транспорт: Telegram Bot API
│   ├── src/
│   │   ├── bot/
│   │   │   ├── agent_handlers/      # Agent Mode handlers (modularized)
│   │   │   │   ├── mod.rs           # Thin facade and re-exports
│   │   │   │   ├── lifecycle.rs     # Agent mode activation/orchestration
│   │   │   │   ├── controls.rs      # Control commands and exit flow
│   │   │   │   ├── callbacks.rs     # Inline callback routing
│   │   │   │   ├── input.rs         # Text and multimodal input handling
│   │   │   │   ├── task_runner.rs   # Task execution and result delivery
│   │   │   │   ├── session.rs       # Session lifecycle and registry
│   │   │   │   ├── execution_config.rs  # Execution profile wiring
│   │   │   │   ├── reminders.rs     # Reminder scheduler handling
│   │   │   │   ├── shared.rs        # Shared helpers and state maps
│   │   │   │   └── tests.rs         # Agent handler unit tests
│   │   │   ├── agent/               # Agent-specific utilities
│   │   │   │   ├── mod.rs           # Agent utilities exports
│   │   │   │   └── media.rs         # Media handling for agents
│   │   │   ├── views/               # UI component views
│   │   │   │   ├── mod.rs           # Views module exports
│   │   │   │   └── agent.rs         # Agent Mode UI components
│   │   │   ├── handlers.rs          # Top-level Telegram handlers
│   │   │   ├── agent_transport.rs   # Transport adapter for progress
│   │   │   ├── context.rs           # Context-scoped transport state
│   │   │   ├── topic_route.rs       # Topic routing and binding resolution
│   │   │   ├── thread.rs            # Telegram thread/topic helpers
│   │   │   ├── manager_topic_lifecycle.rs  # Manager topic provisioning
│   │   │   ├── messaging.rs         # Long-message delivery helpers
│   │   │   ├── resilient.rs         # Resilient send/edit wrappers
│   │   │   ├── progress_render.rs   # HTML progress rendering
│   │   │   ├── unauthorized_cache.rs    # Unauthorized access cache
│   │   │   ├── state.rs             # Dialogue state machine
│   │   │   └── mod.rs               # Bot module exports
│   │   ├── runner.rs                # Bot initialization
│   │   ├── config.rs                # Telegram-specific config
│   │   └── lib.rs                   # Telegram transport exports
│   └── tests/
└── oxide-agent-telegram-bot/        # Application Entry Point
    └── src/
        └── main.rs                  # Binary entry point
skills/                              # Документация навыков агента (9 skills)
docs/                                # Комплексная документация
├── HANDOVER-NOTE.txt                # Текущий handover по compaction rollout
├── hooks/                           # Hook system documentation
│   └── sub-agents/                  # Sub-agent delegation lifecycle
├── opencode-int/                    # OpenCode sandbox integration
│   └── opencode-sandbox-integration/
├── AGENT-TOPICS-BLUEPRINT.md
├── KOKORO-voice.md                  # Local Kokoro TTS API reference
└── sdk-third-party-api-examples.md
sandbox/
└── Dockerfile.sandbox
```

### Workspace crates
- `oxide-agent-core`: доменная логика агента, staged compaction pipeline для Agent Mode, LLM-интеграции (включая `http_utils.rs`, `openai_compat.rs`), хуки (9 hooks с `registry.rs`, `types.rs`), навыки (с `cache.rs`, `loader.rs`, `matcher.rs`, `types.rs`), runner (с `execution.rs`, `tools.rs`, `responses.rs`, `hooks.rs`, `types.rs`), модульный storage facade в `storage/mod.rs` с вынесенными domain/contracts/R2 helper-модулями и тематическими storage tests, control-plane CRUD/audit для manager tools. Включает `UserContextConfig` для per-transport контекстов, context-scoped storage API, `AgentExecutionProfile` с `ToolAccessPolicy`, `context.rs`, `identity.rs` для агентского контекста и персон, topic-scoped prompts/configs, `utils.rs` для общих утилит и SSH MCP provider с approval flow.
- `oxide-agent-runtime`: оркестрация сессий, прогресс-рендеринг, session registry с thread-aware session keys.
- `oxide-agent-sandboxd`: отдельный broker daemon для sandbox. Слушает Unix socket (`SANDBOXD_SOCKET`), владеет `docker.sock`, принимает узкий sandbox protocol и выполняет Docker operations от имени основного агента.
- `oxide-agent-transport-telegram`: Telegram transport, UI/handlers, topic routing, thread context management, resilient messaging, progress rendering, unauthorized access protection, телеметрия доставки. Включает модульный `bot/agent_handlers/` (facade + lifecycle/controls/callbacks/input/task_runner/session/execution_config/reminders/shared/tests), `context.rs` для context-scoped state management с legacy fallback для DM-чатов, `agent/media.rs` для обработки медиа и views module для UI компонентов.
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

**Components**: `LoopDetectionConfig`, `LoopDetectionService`, `content_detector.rs`, `tool_detector.rs`, `llm_detector.rs`, `config.rs`, `service.rs`, `types.rs`.

Integration via `LoopDetectionHook` in agent execution loop.

## 🎬 Agent Session Management

Task lifecycle tracking with timeout control, cancellation support, and sandbox persistence.

**Components**: `AgentSession`, `AgentStatus`, `session.rs`

**Features**: Task lifecycle tracking, 30-minute timeout, cancellation tokens, loaded skills tracking, typed hot-memory storage. Compaction запускается orchestration layer через `agent/compaction/`, а не как side effect `AgentMemory`.

## 🗜 Agent Mode Compaction

Staged compaction pipeline only for Agent Mode.

**Components**: `agent/compaction/{budget,classifier,externalize,prune,prompt,summarizer,rebuild,archive,service,types}.rs`

**Flow**: budget estimation → classify → externalize → prune → summarize with separate model → rebuild hot context → optional archive refs.

**Guarantees**: сохраняет base system context, topic `AGENTS.md`, current task, todos, runtime injections, approvals и recent raw working set; крупные tool outputs выносятся или pruning-ятся до LLM compaction.

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

**Components**: `SkillRegistry`, `embeddings.rs`, `matcher.rs`, `cache.rs`, `loader.rs`, `types.rs`.

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

### Testing Documentation
Hermetic testing patterns (`tests/hermetic_agent.rs`), snapshot testing with insta (`tests/snapshot_prompts.rs`), property-based testing with proptest (`tests/proptest_recovery.rs`).
