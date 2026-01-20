# Проект: Oxide Agent

Этот проект представляет собой Telegram-бота, который интегрируется с различными API больших языковых моделей (LLM) для предоставления пользователям многофункционального ИИ-ассистента. Бот может обрабатывать текстовые, голосовые, видео сообщения и изображения, работать с документами, управлять историей диалога и выполнять сложные задачи в изолированной песочнице.

Бот разработан с использованием **Rust 1.92**, библиотеки `teloxide`, AWS SDK для взаимодействия с Cloudflare R2, и нативной интеграции с провайдерами ИИ (Groq, Mistral AI, Google Gemini, OpenRouter, ZAI/Zhipu AI).

## Branch

The default branch in this repo is `testing`.

## 🏗 Структура проекта

```
crates/
├── oxide-agent-core/                # ядро агента, LLM, хуки, навыки, storage
├── oxide-agent-runtime/             # цикл исполнения, провайдеры, sandbox, сессии
├── oxide-agent-transport-telegram/  # Telegram transport + handlers (teloxide)
└── oxide-agent-telegram-bot/         # бинарь Telegram-бота и сборка приложения
skills/                               # определения навыков в формате markdown
docs/                                 # документация и спецификации
backlog/                              # планы и завершенные задачи
sandbox/                              # Docker-конфигурация песочницы
.github/workflows/                    # GitHub Actions CI/CD
Dockerfile                            # Dockerfile основного приложения
docker-compose.yml
```

### Workspace crates
- `oxide-agent-core`: доменная логика агента, LLM-интеграции, хуки, навыки, storage.
- `oxide-agent-runtime`: оркестрация сессий, цикл исполнения, провайдеры инструментов, sandbox.
- `oxide-agent-transport-telegram`: Telegram transport, UI/handlers, телеметрия доставки.
- `oxide-agent-telegram-bot`: бинарь с конфигурацией и запуском Telegram транспорта.

## 🦀 Rust Architecture & Workflow

### 1. Architecture & Structure
- **Feature Isolation**: `oxide-agent-core` и `oxide-agent-runtime` не должны зависеть от транспортных crate; транспорты зависят от core/runtime.
- **Transport Boundaries**: `teloxide` используется только в `oxide-agent-transport-telegram` (и бинарях, которые ее подключают).
- **Module Hierarchy**: В каждом crate сохраняем явные `mod.rs` и публичные экспорты модулей.
- **Error Handling**: Use `thiserror` for libraries and `anyhow` for apps.
  > *Note: `unwrap()`, `expect()` are strictly blocked by system hooks.*

Чтобы добавить новый transport (Discord/Slack), создайте `crates/oxide-agent-transport-<name>`, держите SDK и обработчики внутри transport crate, подключите адаптер к runtime, и при необходимости добавьте отдельный бинарь `oxide-agent-<name>-bot` для запуска.

### 2. Operational Workflow
**Tools are enforced by the environment.**
- **Compilation**: Use `cargo check` for quick validation. Only use `cargo build` for final binaries.
- **Dependencies**: Use `cargo add`, `cargo remove`, `cargo update`.
- **Metadata**: Use `workspace info` for project topology and `cargo info` for crate details.

### 3. Code Quality
- **Linting**: Run `cargo clippy` before finishing a task.
- **Formatting**: **Automatic.** The system auto-formats on save. Do not run `cargo fmt` manually.