# Проект: Oxide Agent

Этот проект представляет собой Telegram-бота, который интегрируется с различными API больших языковых моделей (LLM) для предоставления пользователям многофункционального ИИ-ассистента. Бот может обрабатывать текстовые, голосовые, видео сообщения и изображения, работать с документами, управлять историей диалога и выполнять сложные задачи в изолированной песочнице.

Бот разработан с использованием **Rust 1.92**, библиотеки `teloxide`, AWS SDK для взаимодействия с Cloudflare R2, и нативной интеграции с провайдерами ИИ (Groq, Mistral AI, Google Gemini, OpenRouter, ZAI/Zhipu AI).

## 🏗 Структура проекта

```
src/
├── main.rs                    # точка входа
├── lib.rs                     # библиотечный корень
├── agent/                     # ядро агента и логика выполнения
│   ├── mod.rs
│   ├── executor.rs
│   ├── context.rs             # контекст выполнения агента
│   ├── recovery.rs            # восстановление malformed ответов
│   ├── structured_output.rs    # парсинг и валидация структурированных ответов
│   ├── tool_bridge.rs         # мост исполнения инструментов
│   ├── session_registry.rs    # реестр сессий агентов
│   ├── thoughts.rs            # генерация мыслей агента
│   ├── prompt/                # сборка системных промптов
│   │   ├── mod.rs
│   │   └── composer.rs
│   ├── loop_detection/        # детектирование зацикливаний
│   │   ├── mod.rs
│   │   ├── config.rs
│   │   ├── content_detector.rs
│   │   ├── llm_detector.rs
│   │   ├── service.rs
│   │   ├── tool_detector.rs
│   │   └── types.rs
│   ├── runner/                # вспомогательные модули исполнения
│   │   ├── mod.rs
│   │   ├── execution.rs
│   │   ├── hooks.rs
│   │   ├── loop_detection.rs
│   │   ├── responses.rs
│   │   ├── tools.rs
│   │   └── types.rs
│   ├── skills/                # подсистема навыков (RAG/embeddings)
│   │   ├── mod.rs
│   │   ├── cache.rs
│   │   ├── embeddings.rs
│   │   ├── loader.rs
│   │   ├── matcher.rs
│   │   ├── registry.rs
│   │   └── types.rs
│   ├── session.rs
│   ├── memory.rs
│   ├── preprocessor.rs
│   ├── progress.rs
│   ├── narrator.rs            # генерация нарратива статусов
│   ├── provider.rs
│   ├── registry.rs
│   ├── hooks/                 # хуки выполнения (completion и др.)
│   │   ├── mod.rs
│   │   ├── completion.rs
│   │   ├── delegation_guard.rs # защита делегирования
│   │   ├── registry.rs
│   │   ├── sub_agent_safety.rs # проверка безопасности делегирования
│   │   ├── types.rs
│   │   └── workload.rs         # управление рабочей нагрузкой
│   └── providers/             # провайдеры инструментов (Sandbox, Tavily, и т.д.)
│       ├── mod.rs
│       ├── delegation.rs      # делегирование под-агентам
│       ├── filehoster.rs
│       ├── path.rs
│       ├── sandbox.rs
│       ├── crawl4ai.rs
│       ├── tavily.rs
│       ├── todos.rs
│       └── ytdlp.rs
├── bot/                       # логика Telegram-бота и хендлеры
│   ├── mod.rs
│   ├── handlers.rs
│   ├── agent_handlers.rs
│   ├── messaging.rs           # отправка и разбиение сообщений
│   ├── resilient.rs           # устойчивая отправка с ретраями
│   ├── state.rs
│   ├── unauthorized_cache.rs
│   ├── views/                 # шаблоны сообщений и UI
│   │   ├── mod.rs
│   │   └── agent.rs
│   └── agent/                 # бот-специфичная логика агента
│       ├── mod.rs
│       └── media.rs
├── llm/                       # интеграции с провайдерами LLM
│   ├── mod.rs
│   ├── common.rs
│   ├── embeddings.rs          # векторные представления
│   ├── http_utils.rs
│   ├── openai_compat.rs
│   └── providers/
│       ├── mod.rs
│       ├── gemini.rs
│       ├── groq.rs
│       ├── mistral.rs
│       ├── openrouter.rs
│       ├── openrouter/
│       │   └── helpers.rs
│       ├── zai.rs
│       └── zai/
│           └── stream.rs
├── sandbox/                   # управление изолированной средой
│   ├── mod.rs
│   └── manager.rs
├── storage.rs
├── config.rs
└── utils.rs

skills/                       # определения навыков в формате markdown
├── core.md
├── delegation_manager.md      # управление делегированием
├── ffmpeg-conversion.md
├── file-hosting.md
├── file-management.md
├── html-report.md
├── task-planning.md
├── video-processing.md
└── web-search.md

tests/                        # интеграционные и функциональные тесты
├── agent_xml_leak_prevention.rs
├── cancellation_respected.rs
├── integration_validation.rs
└── sub_agent_delegation.rs

backlog/                      # документация и планы
├── blueprints/
├── bugs/
├── docs/                     # спецификации компонентов
└── done/                     # завершенные задачи

sandbox/                      # конфигурация Docker для песочницы
└── Dockerfile.sandbox

Dockerfile                     # Dockerfile основного приложения
docker-compose.yml
```

## 🦀 Rust Architecture & Workflow

### 1. Architecture & Structure
- **Feature Isolation**: Maintain feature-based directory structure. `agent/` modules must not depend on `bot/`.
- **Module Hierarchy**: Every directory must have a `mod.rs` defining clear public exports.
- **Error Handling**: Use `thiserror` for libraries and `anyhow` for apps.
  > *Note: `unwrap()`, `expect()`, and files >300 lines are strictly blocked by system hooks.*

### 2. Operational Workflow
**Tools are enforced by the environment.**
- **Compilation**: Use `cargo-check` for quick validation. Only use `cargo-build` for final binaries.
- **Dependencies**: Use `cargo-add`, `cargo-remove`, `cargo-update`.
- **Metadata**: Use `workspace-info` for project topology and `cargo-info` for crate details.
- **Cleanup**: Periodically run `cargo-machete`.

### 3. Debugging Strategy
1. **Analyze**: If compiler throws an error code (e.g., E0308), run `rustc-explain E0308` FIRST.
2. **Search**: Use `tavily-search` -> `tavily-extract` for external docs/errors.
3. **Test**: Use `cargo-test` for logic and `cargo-hack` for feature flag combinations.

### 4. Code Quality
- **Linting**: Run `cargo-clippy` before finishing a task.
- **Formatting**: **Automatic.** The system auto-formats on save. Do not run `cargo fmt` manually.
- **Security**: Run `cargo-deny-check` for audits.

## ⚡ Tool Intent Map
| Intent | Tool |
| :--- | :--- |
| "Check syntax/types" | `cargo-check` |
| "Check crate features" | `cargo-info [crate]` |
| "Understand error" | `rustc-explain [code]` |
| "Find docs/solutions" | `tavily-search` |
