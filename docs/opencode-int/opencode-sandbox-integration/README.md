# Opencode + Sandbox Integration - Complete Documentation

> **Полная документация для интеграции OpenCode архитектора с существующей системой песочницы (Docker)**
>
> 📁 **Версия:** 1.0.0
> 📅 **Дата:** 2026-03-05
> 🚀 **Статус:** Production Ready

---

## 📋 Оглавление

- [Что это такое?](#что-это-такое)
- [Для кого это?](#для-кого-это)
- [Архитектура](#архитектура)
- [Быстрый старт](#быстрый-старт)
- [Документация](#документация)
- [Примеры использования](#примеры-использования)
- [Требования](#требования)

---

## Что это такое?

Эта интеграция позволяет LLM агенту использовать **два типа инструментов**:

### 1. **Sandbox Tools** (Docker контейнер)

- Python 3
- yt-dlp (YouTube downloader)
- ffmpeg (media processing)
- Стандартные Unix инструменты

**Использование:** Обработка данных, скачивание файлов, работа с медиа.

### 2. **Opencode Tools** (Разработка кода)

- @explore - Анализ кодовой базы
- @developer - Реализация кода
- @review - Code review
- Bash tool - Git операции (commit, push)

**Использование:** Разработка кода, рефакторинг, исправление багов.

---

## Для кого это?

✅ **Разработчики агентов** - добавляют инструменты в свои агенты
✅ **ML инженеры** - создают multi-tool системы
✅ **DevOps** - интегрируют с CI/CD
✅ **Исследователи** - изучают архитектуру агентов

---

## Архитектура

```
┌─────────────────────────────────────────────────────────────────┐
│                        Telegram Bot                         │
└────────────────────────┬────────────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────────────┐
│                         LLM Agent                           │
│                        (Ваш агент)                          │
│                                                             │
│  • Анализирует запрос                                        │
│  • Выбирает инструмент                                      │
│  • Контролирует поток                                       │
└──────────────────┬──────────────────────────────────────────────┘
                   │
        ┌──────────┴──────────┐
        │                     │
        ▼                     ▼
┌──────────────┐      ┌──────────────────┐
│   Sandbox    │      │     Opencode      │
│   Provider   │      │     Provider      │
└──────┬───────┘      └────────┬─────────┘
       │                       │
       ▼                       ▼
┌──────────────┐      ┌──────────────────────┐
│   Docker     │      │   Opencode Server     │
│   Container  │      │   (opencode serve)    │
│              │      │                       │
│  - Python    │      │  - Architect Agent    │
│  - yt-dlp    │      │  - @explore           │
│  - ffmpeg    │      │  - @developer         │
│  - Debian    │      │  - @review            │
│              │      │  - Bash tool          │
└──────────────┘      │  - Edit tool          │
                      │  - Git repo           │
                      └──────────────────────┘
```

---

## Быстрый старт

### 1. Настройка (5 минут)

```bash
# Клонировать или скопировать эту документацию
cd opencode-sandbox-integration

# Запустить скрипт настройки
chmod +x configuration/setup.sh
./configuration/setup.sh
```

### 2. Копирование кода (2 минуты)

```bash
# Скопировать файлы реализации
cp implementation/opencode_provider.rs oxide-agent-core/src/agent/providers/
cp implementation/registry_integration.rs oxide-agent-core/src/agent/
cp implementation/session.rs oxide-agent-core/src/agent/

# Скопировать примеры
cp examples/integration_examples.rs oxide-agent-core/src/agent/examples/
```

### 3. Обновление зависимостей (1 минута)

Добавить в `Cargo.toml`:

```toml
[dependencies]
reqwest = { version = "0.11", features = ["json"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
tokio = { version = "1.0", features = ["full"] }
async-trait = "0.1"
```

### 4. Обновление LLM prompt (1 минута)

Скопировать содержимое `configuration/llm_prompt.md` в system prompt вашего LLM агента.

### 5. Тестирование (2 минуты)

```bash
# Проверить Opencode сервер
curl http://127.0.0.1:4096/vcs

# Запустить тесты
cargo test example -- --ignored
```

**Итого: 11 минут** ⏱️

---

## Документация

### 📖 Основная документация

| Документ                                                   | Описание                            | Ссылка           |
| ---------------------------------------------------------- | ----------------------------------- | ---------------- |
| [INDEX.md](./INDEX.md)                                     | Индекс всех файлов и быстрые ссылки | **Начать здесь** |
| [architecture/overview.md](./architecture/overview.md)     | Обзор архитектуры                   | Читать           |
| [architecture/flow.md](./architecture/flow.md)             | Потоки выполнения                   | Читать           |
| [architecture/components.md](./architecture/components.md) | Компоненты системы                  | Читать           |

### 💻 Реализация

| Документ                                                                           | Описание                  | Ссылка |
| ---------------------------------------------------------------------------------- | ------------------------- | ------ |
| [implementation/opencode_provider.rs](./implementation/opencode_provider.rs)       | HTTP клиент для Opencode  | Код    |
| [implementation/registry_integration.rs](./implementation/registry_integration.rs) | Интеграция в ToolRegistry | Код    |
| [implementation/session.rs](./implementation/session.rs)                           | Управление сессиями       | Код    |

### 📚 Примеры

| Документ                                                               | Описание                 | Ссылка |
| ---------------------------------------------------------------------- | ------------------------ | ------ |
| [examples/basic_usage.md](./examples/basic_usage.md)                   | Базовое использование    | Читать |
| [examples/advanced_workflow.md](./examples/advanced_workflow.md)       | Сложные рабочие процессы | Читать |
| [examples/integration_examples.rs](./examples/integration_examples.rs) | 6 практических примеров  | Код    |

### ⚙️ Конфигурация

| Документ                                                                           | Описание              | Ссылка       |
| ---------------------------------------------------------------------------------- | --------------------- | ------------ |
| [configuration/llm_prompt.md](./configuration/llm_prompt.md)                       | System prompt для LLM | Использовать |
| [configuration/setup.sh](./configuration/setup.sh)                                 | Скрипт настройки      | Запустить    |
| [configuration/environment_variables.md](./configuration/environment_variables.md) | Переменные окружения  | Читать       |

### 🧪 Тестирование

| Документ                                                       | Описание             | Ссылка |
| -------------------------------------------------------------- | -------------------- | ------ |
| [testing/unit_tests.md](./testing/unit_tests.md)               | Unit тесты           | Читать |
| [testing/integration_tests.md](./testing/integration_tests.md) | Интеграционные тесты | Читать |
| [testing/troubleshooting.md](./testing/troubleshooting.md)     | Устранение проблем   | Читать |

### 🚀 Деплой

| Документ                                                                   | Описание                 | Ссылка       |
| -------------------------------------------------------------------------- | ------------------------ | ------------ |
| [deployment/production_checklist.md](./deployment/production_checklist.md) | Чек-лист для продакшена  | Использовать |
| [deployment/monitoring.md](./deployment/monitoring.md)                     | Мониторинг и логирование | Читать       |

---

## Примеры использования

### Пример 1: Скачать видео с YouTube

**Запрос:** "скачай видео с YouTube https://youtube.com/watch?v=xxx"

**LLM выбирает:** Sandbox (требуется yt-dlp)

```rust
// LLM вызовет:
{
  "tool": "execute_command",
  "command": "yt-dlp -f best https://youtube.com/watch?v=xxx -o video.mp4"
}
```

**Результат:** Видео скачано в Docker контейнер.

---

### Пример 2: Добавить логирование API

**Запрос:** "добавь логирование всех API запросов"

**LLM выбирает:** Opencode (требуется разработка кода)

```rust
// LLM вызовет:
{
  "tool": "opencode",
  "task": "add request logging for all API endpoints"
}
```

**Результат:**

- @explore находит API endpoints
- @developer создает logging middleware
- @review проверяет код
- Bash tool выполняет git commit + push

---

### Пример 3: Многошаговая задача

**Запрос:** "скачай видео, извлеки аудио и добавь поддержку аудио в наш API"

**LLM выполняет:**

1. **Sandbox:** Скачать видео (yt-dlp)
2. **Sandbox:** Извлечь аудио (ffmpeg)
3. **Opencode:** Добавить API endpoint для аудио
4. **Opencode:** Написать тесты
5. **Opencode:** Git commit + push

---

## Требования

### Системные требования

| Компонент | Минимум               | Рекомендуется  |
| --------- | --------------------- | -------------- |
| **OS**    | Linux, macOS, Windows | Linux (Docker) |
| **RAM**   | 2GB                   | 4GB            |
| **CPU**   | 2 cores               | 4 cores        |
| **Disk**  | 1GB                   | 5GB            |

### Программные требования

| Компонент    | Версия |
| ------------ | ------ |
| **Rust**     | 1.70+  |
| **Bun**      | 1.0+   |
| **Docker**   | 20.10+ |
| **Opencode** | Latest |
| **Git**      | 2.0+   |

### Зависимости Rust

```toml
[dependencies]
reqwest = { version = "0.11", features = ["json"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
tokio = { version = "1.0", features = ["full"] }
async-trait = "0.1"
tracing = "0.1"
```

---

## Дополнительные ресурсы

### Внешняя документация

- [Opencode Documentation](https://opencode.ai/docs)
- [Opencode SDK](https://opencode.ai/docs/sdk)
- [Opencode Agents](https://opencode.ai/docs/agents)

### Связанные проекты

- [Telegram Bot Integration](https://github.com/your-repo/telegram-bot)
- [Sandbox Provider](https://github.com/your-repo/sandbox)

---

## Поддержка

### Вопросы и проблемы

1. Проверьте [testing/troubleshooting.md](./testing/troubleshooting.md)
2. Изучите [examples/basic_usage.md](./examples/basic_usage.md)
3. Проверьте [INDEX.md](./INDEX.md) для поиска конкретного файла

### Сообщество

- GitHub Issues: https://github.com/your-repo/issues
- Discord: https://discord.gg/your-server

---

## Лицензия

Эта документация предоставляется как есть для интеграции.

---

**Начните здесь:** [INDEX.md](./INDEX.md)
