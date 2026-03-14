# Проект: Oxide Agent

Этот проект представляет собой Telegram-бота, который интегрируется с различными API больших языковых моделей (LLM) для предоставления пользователям многофункционального ИИ-ассистента. Бот может обрабатывать текстовые, голосовые, видео сообщения и изображения, работать с документами, управлять историей диалога и выполнять сложные задачи в изолированной песочнице.

Бот разработан с использованием **Rust 1.92**, библиотеки `teloxide`, AWS SDK для взаимодействия с Cloudflare R2, и нативной интеграции с провайдерами ИИ (Groq, Mistral AI, Google Gemini, OpenRouter, ZAI/Zhipu AI).

## Branch

The default branch in this repo is `agent-topics`.

## 🏗 Структура проекта

```
crates/
├── oxide-agent-core/                # Ядро: домен, LLM, storage, тесты
│   ├── Cargo.toml
│   ├── src/
│   │   ├── lib.rs
│   │   ├── config.rs                # Конфигурация агента
│   │   ├── storage.rs               # StorageProvider trait + R2 impl + control-plane records/audit
│   │   ├── testing.rs               # TestKit: моки и хелперы
│   │   ├── utils.rs
│   │   ├── agent/                   # Логика агента
│   │   │   ├── mod.rs
│   │   │   ├── runner/              # Цикл исполнения (Loop, Hooks)
│   │   │   ├── loop_detection/      # Детектор зацикливания
│   │   │   ├── prompt/              # Компоновщик промптов (Composer)
│   │   │   ├── providers/           # Tool providers incl. sandbox, todos, manager control-plane
│   │   │   ├── skills/              # Реестр и поиск навыков
│   │   │   ├── recovery.rs          # Восстановление XML/JSON
│   │   │   └── ...
│   │   ├── llm/                     # Интеграции с AI
│   │   │   ├── client.rs            # LlmClient (с поддержкой моков)
│   │   │   ├── providers/           # Groq, Mistral, ZAI, OpenRouter
│   │   │   └── ...
│   │   └── sandbox/                 # Docker-менеджер
│   └── tests/                       # Интеграционные тесты
│       ├── hermetic_agent.rs        # Hermetic logic tests
│       ├── proptest_recovery.rs     # Fuzzing tests
│       ├── snapshot_prompts.rs      # Snapshot tests
│       └── ...
├── oxide-agent-runtime/             # Runtime: сессии и оркестрация
│   ├── src/
│   │   ├── session_registry.rs      # Управление сессиями пользователей
│   │   ├── agent/
│   │   │   └── runtime/             # Реализация AgentRuntime
│   │   └── sandbox/                 # Runtime-компоненты песочницы
├── oxide-agent-transport-telegram/  # Транспорт: Telegram Bot API
│   ├── src/
│   │   ├── runner.rs                # Инициализация бота и DI
│   │   ├── bot/
│   │   │   ├── handlers.rs          # Обработчики команд (chat mode)
│   │   │   ├── agent_handlers.rs    # Обработчики сообщений агенту (Agent Mode)
│   │   │   ├── agent_transport.rs   # Реализация AgentTransport
│   │   │   ├── context.rs           # Context-scoped state management (thread/chat isolation)
│   │   │   ├── topic_route.rs       # Topic-based routing с dynamic bindings
│   │   │   ├── thread.rs            # Thread context extraction и helpers
│   │   │   ├── manager_topic_lifecycle.rs  # Telegram forum topic lifecycle
│   │   │   └── ...
└── oxide-agent-telegram-bot/        # Application Entry Point
    └── src/
        └── main.rs                  # Запуск приложения
sandbox/
└── Dockerfile.sandbox               # Образ песочницы (Ubuntu + Python/Node)
config/
└── local.yaml                       # Локальная конфигурация (не коммитится)
.github/workflows/
└── ci-cd.yml                        # GitHub Actions (Build, Test, Deploy)
docker-compose.yml                   # Локальный запуск
Dockerfile                           # Сборка основного Rust-приложения
```

### Workspace crates
- `oxide-agent-core`: доменная логика агента, LLM-интеграции, хуки, навыки, storage, control-plane CRUD/audit для manager tools. Включает `UserContextConfig` для per-transport контекстов и context-scoped storage API (save/load/clear для контекстов).
- `oxide-agent-runtime`: оркестрация сессий, цикл исполнения, провайдеры инструментов, sandbox, session registry с thread-aware session keys.
- `oxide-agent-transport-telegram`: Telegram transport, UI/handlers, topic routing, thread context management, телеметрия доставки. Включает `context.rs` для context-scoped state management с legacy fallback для DM-чатов.
- `oxide-agent-telegram-bot`: бинарь с конфигурацией и запуском Telegram транспорта.

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
- **Configuration**: Поддержка layered конфигурации через YAML файлы в `config/` (default.yaml, {RUN_MODE}.yaml, local.yaml) + переменные окружения.

Чтобы добавить новый transport (Discord/Slack), создайте `crates/oxide-agent-transport-<name>`, держите SDK и обработчики внутри transport crate, подключите адаптер к runtime, и при необходимости добавьте отдельный бинарь `oxide-agent-<name>-bot` для запуска.

### 2. Operational Workflow
**Tools are enforced by the environment.**
- **Compilation**: Use `cargo check` for quick validation. Only use `cargo build` for final binaries.
- **Dependencies**: Use `cargo add`, `cargo remove`, `cargo update`.
- **Metadata**: Use `workspace info` for project topology and `cargo info` for crate details.

### 3. Code Quality
- **Linting**: Run `cargo clippy` before finishing a task.
- **Formatting**: Run `cargo fmt` before commit
