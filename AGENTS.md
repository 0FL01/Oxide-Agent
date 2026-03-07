# Проект: Oxide Agent

Этот проект представляет собой Telegram-бота, который интегрируется с различными API больших языковых моделей (LLM) для предоставления пользователям многофункционального ИИ-ассистента. Бот может обрабатывать текстовые, голосовые, видео сообщения и изображения, работать с документами, управлять историей диалога и выполнять сложные задачи в изолированной песочнице.

Бот разработан с использованием **Rust 1.93**, библиотеки `teloxide`, AWS SDK для взаимодействия с Cloudflare R2, и нативной интеграции с провайдерами ИИ (Groq, Mistral AI, Google Gemini, OpenRouter, ZAI/Zhipu AI).

## Branch

The default branch in this repo is `testing`.

## 🏗 Структура проекта

```
crates/
├── oxide-agent-core/                # Ядро: домен, LLM, storage, тесты
│   ├── Cargo.toml
│   ├── src/
│   │   ├── lib.rs
│   │   ├── config.rs                # Конфигурация агента
│   │   ├── storage.rs               # StorageProvider trait + R2 impl
│   │   ├── testing.rs               # TestKit: моки и хелперы
│   │   ├── utils.rs
│   │   ├── agent/                   # Логика агента
│   │   │   ├── mod.rs
│   │   │   ├── task.rs              # Task domain: TaskId/TaskState/Snapshot/TaskEvent
│   │   │   ├── runner/              # Цикл исполнения (Loop, Hooks)
│   │   │   ├── loop_detection/      # Детектор зацикливания
│   │   │   ├── prompt/              # Компоновщик промптов (Composer)
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
│   │   ├── lib.rs
│   │   ├── session_registry.rs      # Управление сессиями пользователей
│   │   ├── task_registry.rs         # Runtime registry для TaskId и cancellation tokens
│   │   ├── task_executor.rs         # Detached task execution + checkpoint persistence
│   │   ├── task_events.rs           # Transport-agnostic task event publishing
│   │   ├── task_recovery.rs         # Boot-time reconciliation and stale snapshot repair
│   │   ├── worker_manager.rs        # TaskId -> JoinHandle tracking and worker limits
│   │   ├── agent/
│   │   │   └── runtime/             # Реализация AgentRuntime
├── oxide-agent-transport-telegram/  # Транспорт: Telegram Bot API
│   ├── src/
│   │   ├── runner.rs                # Инициализация бота и DI
│   │   ├── bot/
│   │   │   ├── context.rs           # TelegramHandlerContext shared DI bundle
│   │   │   ├── handlers.rs          # Обработчики команд
│   │   │   ├── agent_handlers.rs    # Обработчики сообщений агенту
│   │   │   ├── agent_transport.rs   # Реализация AgentTransport
│   │   │   └── ...
└── oxide-agent-telegram-bot/        # Application Entry Point
    └── src/
        └── main.rs                  # Запуск приложения
sandbox/
└── Dockerfile.sandbox               # Образ песочницы (Ubuntu + Python/Node)
.github/workflows/
└── ci-cd.yml                        # GitHub Actions (Build, Test, Deploy)
docker-compose.yml                   # Локальный запуск
Dockerfile                           # Сборка основного Rust-приложения
```

### Workspace crates
- `oxide-agent-core`: доменная логика агента, LLM-интеграции, хуки, навыки, storage, task domain и persistence contract.
- `oxide-agent-runtime`: оркестрация сессий, worker manager, detached task executor, task recovery, task registry, task event publishing и runtime-компоненты.
- `oxide-agent-transport-telegram`: Telegram transport, UI/handlers, runtime-aware Agent Mode routing, телеметрия доставки.
- `oxide-agent-telegram-bot`: бинарь с конфигурацией и запуском Telegram транспорта.

## 🦀 Rust Architecture & Workflow

### 1. Architecture & Structure
- **Feature Isolation**: `oxide-agent-core` и `oxide-agent-runtime` не должны зависеть от транспортных crate; транспорты зависят от core/runtime.
- **Transport Boundaries**: `teloxide` используется только в `oxide-agent-transport-telegram` (и бинарях, которые ее подключают).
- **Module Hierarchy**: В каждом crate сохраняем явные `mod.rs` и публичные экспорты модулей.
- **Error Handling**: Use `thiserror` for libraries and `anyhow` for apps.

Чтобы добавить новый transport (Discord/Slack), создайте `crates/oxide-agent-transport-<name>`, держите SDK и обработчики внутри transport crate, подключите адаптер к runtime, и при необходимости добавьте отдельный бинарь `oxide-agent-<name>-bot` для запуска.

### 2. Operational Workflow
**Tools are enforced by the environment.**
- **Compilation**: Use `cargo check` for quick validation. Only use `cargo build` for final binaries.
- **Dependencies**: Use `cargo add`, `cargo remove`, `cargo update`.
- **Metadata**: Use `workspace info` for project topology and `cargo info` for crate details.

### 3. Code Quality
- **Linting**: Run `cargo clippy` before finishing a task.
- **Formatting**: Run `cargo fmt` before commit
