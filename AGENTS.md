# Проект: Oxide Agent

Этот проект представляет собой Telegram-бота, который интегрируется с различными API больших языковых моделей (LLM) для предоставления пользователям многофункционального ИИ-ассистента. Бот может обрабатывать текстовые, голосовые, видео сообщения и изображения, работать с документами, управлять историей диалога и выполнять сложные задачи в изолированной песочнице.

Бот разработан с использованием **Rust 1.92**, библиотеки `teloxide`, AWS SDK для взаимодействия с Cloudflare R2, и нативной интеграции с провайдерами ИИ (Groq, Mistral AI, Google Gemini, OpenRouter, ZAI/Zhipu AI).

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
│   │   │   ├── provider.rs          # Tool Provider trait
│   │   │   ├── registry.rs          # Tool Registry
│   │   │   ├── runner/              # Цикл исполнения
│   │   │   ├── hooks/               # Hook system (9 hooks)
│   │   │   ├── loop_detection/      # Детектор зацикливания
│   │   │   ├── providers/           # Tool providers (sandbox, todos, manager, search)
│   │   │   ├── skills/              # Реестр и поиск навыков (embeddings)
│   │   │   └── recovery.rs          # Восстановление XML/JSON
│   │   ├── llm/                     # Интеграции с AI
│   │   │   ├── mod.rs               # LlmClient struct
│   │   │   ├── common.rs            # Common utilities
│   │   │   ├── embeddings.rs        # Embedding provider
│   │   │   └── providers/           # Groq, Mistral, Gemini, OpenRouter, ZAI
│   │   ├── sandbox/                 # Docker-менеджер
│   │   ├── config.rs
│   │   ├── storage.rs
│   │   └── testing.rs               # TestKit: моки и хелперы
│   └── tests/                       # Интеграционные тесты
├── oxide-agent-runtime/             # Runtime: сессии и оркестрация
│   └── src/
│       ├── session_registry.rs      # Управление сессиями
│       └── agent/runtime/           # Progress runtime
├── oxide-agent-transport-telegram/  # Транспорт: Telegram Bot API
│   ├── src/
│   │   ├── runner.rs                # Инициализация бота
│   │   ├── bot/
│   │   │   ├── handlers.rs
│   │   │   ├── agent_handlers.rs
│   │   │   ├── agent_transport.rs
│   │   │   ├── context.rs
│   │   │   ├── topic_route.rs
│   │   │   ├── thread.rs
│   │   │   ├── manager_topic_lifecycle.rs
│   │   │   ├── messaging.rs
│   │   │   ├── resilient.rs
│   │   │   ├── progress_render.rs
│   │   │   ├── unauthorized_cache.rs
│   │   │   ├── state.rs
│   │   │   └── views/               # UI component views
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
└── sdk-third-party-api-examples.md
sandbox/
└── Dockerfile.sandbox
```

### Workspace crates
- `oxide-agent-core`: доменная логика агента, LLM-интеграции, хуки, навыки, storage, control-plane CRUD/audit для manager tools. Включает `UserContextConfig` для per-transport контекстов и context-scoped storage API (save/load/clear для контекстов), embeddings support, и полный hook system.
- `oxide-agent-runtime`: оркестрация сессий, прогресс-рендеринг, session registry с thread-aware session keys.
- `oxide-agent-transport-telegram`: Telegram transport, UI/handlers, topic routing, thread context management, resilient messaging, progress rendering, unauthorized access protection, телеметрия доставки. Включает `context.rs` для context-scoped state management с legacy fallback для DM-чатов и views module для UI компонентов.
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

## 👥 Sub-Agent Architecture

**EphemeralSession**: Isolated context for sub-agent tasks, automatic cleanup, blocked tools (`delegate_to_sub_agent`, `send_file_to_user`), session-scoped storage and memory.

**Delegation Flow**: `Main Agent → DelegationGuardHook → EphemeralSession → Sub-Agent → Result → Main Agent`

**Components**: `DelegationProvider`, `DelegationGuardHook`, `SubAgentSafetyHook`, `executor.rs`.

**Configuration**: `sub_agent_model_id`, `sub_agent_model_provider`, `sub_agent_max_tokens`.

## 🎭 Narrator System

Separate LLM model for summarizing agent thoughts and generating narrative summaries.

**Components**: `narrator.rs`, `NarratorProvider`, `narrator_model_id`, `narrator_model_provider`.

**Features**: Thought summarization, narrative generation for multi-step tasks, enhanced dialogue management with context compression.

## 📊 Progress Rendering

Transport-agnostic progress reporting system.

**Components**: `ProgressState`, `Step`, `ProgressRuntime`, `ProgressRuntimeConfig`, `progress.rs` (runtime), `progress_render.rs` (transport).

**Transport Adaptation**: `AgentTransport::send_progress()`, views module for consistent UI, multi-step operations with status updates.

## 🔧 Hook System

Centralized hook system for agent behavior modification.

**Available Hooks**: `CompletionCheckHook`, `DelegationGuardHook`, `SearchBudgetHook`, `SubAgentSafetyHook`, `TimeoutReportHook`, `WorkloadDistributorHook`.

**Management**: `HookRegistry`, `hooks.rs` (runner), `types.rs`. See `docs/hooks/` for comprehensive documentation.

## 🧩 Manager Control Plane

CRUD operations for forum topics and manager tasks with full audit trail.

**Components**: `ManagerControlPlaneProvider`, `manager_control_plane.rs` (104KB), `AuditEventRecord`, `TopicBindingRecord`, `TopicBindingKind`.

**Features**: Forum topic creation/deletion, topic binding management, task assignment and tracking, complete audit trail, RBAC via `manager_allowed_users`.

**Storage**: User-scoped storage records, audit events logged to R2/S3, thread-aware isolation.

## 🎯 Skills System

Embedding-based skill matching and retrieval.

**Components**: `SkillRegistry`, `embeddings.rs`, `matcher.rs`, `cache.rs`, `loader.rs`.

**Available Skills** (9 skills in `skills/`): core, delegation_manager, ffmpeg-conversion, file-hosting, file-management, html-report, task-planning, video-processing, web-search.

**Configuration**: `embedding_provider`, `embedding_model_id`, auto-probing for embedding dimensions.

## 🔌 Provider Ecosystem

### Tool Providers
`sandbox.rs`, `todos.rs`, `tavily.rs`, `crawl4ai/`, `filehoster.rs`, `delegation.rs`, `manager_control_plane.rs`.

### LLM Providers
`gemini.rs`, `groq.rs`, `mistral.rs`, `openrouter.rs`, `zai.rs`.

### Features
`tavily` - Tavily search provider, `crawl4ai` - Crawl4AI web scraping, `zai-rs` - ZAI SDK integration.

## ⚙️ Configuration System

### Layered Configuration
`config/default.yaml`, `config/{RUN_MODE}.yaml`, `config/local.yaml` + environment variables (all optional).

### Key Settings
`crawl4ai_url`, `crawl4ai_timeout_secs`, `search_provider` (tavily/crawl4ai), `embedding_provider`, `embedding_model_id`, `narrator_model_id`, `sub_agent_model_id`, model overrides (chat, agent, sub_agent, media, narrator).

### Telegram Settings
`topic_configs`, `manager_allowed_users_str`, cooldown constants for unauthorized access protection.

## 📚 Documentation

### Core Documentation
- `docs/hooks/` - Complete hook system documentation
- `docs/hooks/sub-agents/` - Sub-agent delegation lifecycle
- `docs/opencode-int/` - OpenCode sandbox integration
- `docs/AGENT-TOPICS-BLUEPRINT.md` - Topic-based routing design
- `docs/sdk-third-party-api-examples.md` - API examples
- `skills/` - Skill documentation (9 skills)

### Testing Documentation
Hermetic testing patterns (`tests/hermetic_agent.rs`), snapshot testing with insta (`tests/snapshot_prompts.rs`), property-based testing with proptest (`tests/proptest_recovery.rs`).
