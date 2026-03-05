# Architecture Overview

> **Обзор архитектуры системы интеграции Opencode с Sandbox**
>
> 📁 **Раздел:** Architecture
> 🎯 **Цель:** Понять как компоненты взаимодействуют

---

## 📋 Оглавление

- [Высокоуровневая архитектура](#высокоуровневая-архитектура)
- [Компоненты](#компоненты)
- [Взаимодействие компонентов](#взаимодействие-компонентов)
- [Разделение ответственности](#разделение-ответственности)
- [Безопасность и изоляция](#безопасность-и-изоляция)

---

## Высокоуровневая архитектура

### Диаграмма системы

```
┌─────────────────────────────────────────────────────────────────────┐
│                         Пользователь                            │
│                    (Telegram, API, CLI)                        │
└────────────────────────┬────────────────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────────────────┐
│                        LLM Агент                               │
│                                                               │
│  • Анализирует запрос пользователя                              │
│  • Принимает решения о том, какой инструмент использовать          │
│  • Оркестрирует выполнение задач                               │
│  • Обрабатывает результаты и ошибки                             │
└──────────────────┬──────────────────────────────────────────────────┘
                   │
        ┌──────────┴──────────┐
        │                     │
        ▼                     ▼
┌───────────────────┐    ┌───────────────────┐
│  ToolRegistry     │    │  Opencode         │
│                  │    │  ToolProvider      │
│  Маршрутизация   │    │                   │
│  инструментов     │    │  HTTP клиент для   │
│                  │    │  Opencode Server   │
└────────┬──────────┘    └────────┬──────────┘
         │                        │
         ▼                        ▼
┌───────────────────┐    ┌───────────────────┐
│  Sandbox         │    │  Opencode Server  │
│  Provider        │    │                  │
│                  │    │  opencode serve   │
│  Управление      │    │                  │
│  Docker          │    │  ┌─────────────┐ │
│                  │    │  │ Architect    │ │
│  ┌───────────┐  │    │  │ Agent       │ │
│  │   Docker   │  │    │  └──────┬──────┘ │
│  │ Container │  │    │         │         │
│  │           │  │    │         ▼         │
│  │ - Python  │  │    │  ┌─────────────┐ │
│  │ - yt-dlp  │  │    │  │ @explore    │ │
│  │ - ffmpeg  │  │    │  └─────────────┘ │
│  │ - Debian  │  │    │                  │
│  │           │  │    │  ┌─────────────┐ │
│  └───────────┘  │    │  │ @developer  │ │
│                  │    │  └─────────────┘ │
└───────────────────┘    │                  │
                       │  ┌─────────────┐ │
                       │  │ @review     │ │
                       │  └─────────────┘ │
                       │                  │
                       │  ┌─────────────┐ │
                       │  │ Bash tool   │ │
                       │  │ (git)       │ │
                       │  └─────────────┘ │
                       └───────────────────┘
```

### Ключевые потоки

#### Поток 1: Обработка данных (Sandbox)

```
User → LLM Agent → ToolRegistry → Sandbox Provider → Docker Container
```

**Используются для:**

- Скачивания файлов (yt-dlp)
- Обработки медиа (ffmpeg)
- Выполнения Python скриптов
- Обработки данных

#### Поток 2: Разработка кода (Opencode)

```
User → LLM Agent → ToolRegistry → Opencode Provider → Opencode Server
                                                                  ↓
                                                        Architect Agent
                                                                  ↓
                                                    @explore + @developer + @review
                                                                  ↓
                                                       Git (Bash tool)
```

**Используются для:**

- Разработки кода
- Рефакторинга
- Исправления багов
- Code review
- Git операций

---

## Компоненты

### 1. LLM Агент

**Назначение:** Основный оркестратор системы

**Ответственность:**

- Анализ запросов пользователя
- Выбор подходящего инструмента
- Формирование команд для инструментов
- Обработка результатов
- Управление ошибками

**Интерфейс:**

```rust
pub struct LLMAgent {
    pub model: String,
    pub registry: ToolRegistry,
}

impl LLMAgent {
    pub async fn process_request(&self, request: &str) -> Result<String, Error>;
}
```

---

### 2. ToolRegistry

**Назначение:** Маршрутизатор для инструментов

**Ответственность:**

- Регистрация инструментов
- Маршрутизация вызовов
- Обработка ошибок
- Прогресс-события

**Интерфейс:**

```rust
pub struct ToolRegistry {
    pub providers: Vec<Box<dyn ToolProvider>>,
    pub opencode_provider: OpencodeToolProvider,
}

impl ToolRegistry {
    pub async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        progress_tx: &Sender<AgentEvent>,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String, String>;
}
```

---

### 3. OpencodeToolProvider

**Назначение:** HTTP клиент для Opencode Server

**Ответственность:**

- Создание sessions
- Отправка prompts
- Получение результатов
- Обработка ошибок HTTP

**Интерфейс:**

```rust
pub struct OpencodeToolProvider {
    pub base_url: String,
    pub client: reqwest::Client,
}

impl OpencodeToolProvider {
    pub async fn execute_task(&self, task: &str) -> Result<String, OpencodeError>;
    pub async fn health_check(&self) -> Result<(), OpencodeError>;
}
```

---

### 4. SandboxProvider

**Назначение:** Управление Docker контейнером

**Ответственность:**

- Создание контейнеров
- Выполнение команд
- Управление файлами
- Очистка ресурсов

**Интерфейс:**

```rust
pub struct SandboxProvider {
    pub docker: Docker,
}

impl SandboxProvider {
    pub async fn execute_command(&mut self, cmd: &str) -> Result<String, Error>;
    pub async fn write_file(&mut self, path: &str, content: &[u8]) -> Result<(), Error>;
}
```

---

### 5. Opencode Server

**Назначение:** Сервер для архитектора и субагентов

**Ответственность:**

- HTTP API для sessions
- Architect agent оркестрация
- Субагенты (@explore, @developer, @review)
- Bash tool для git операций

**Компоненты:**

- HTTP API (REST)
- Architect Agent (LLM)
- Subagents (LLM)
- Tools (bash, edit, read, grep, glob)
- Git integration

---

## Взаимодействие компонентов

### Сценарий: Разработка новой функции

```
1. User → LLM Agent
   Запрос: "добавь логирование всех API запросов"

2. LLM Agent → ToolRegistry
   Вызов: execute("opencode", r#"{"task": "..."}"#, ...)

3. ToolRegistry → OpencodeToolProvider
   Маршрутизация: tool_name == "opencode"

4. OpencodeToolProvider → Opencode Server
   POST /session (создать session)
   POST /session/{id}/message (отправить prompt)

5. Opencode Server → Architect Agent
   Обработка prompt

6. Architect Agent → @explore
   Task: "найди все API endpoints"

7. @explore → Tools
   glob("src/**/*.ts")
   grep("router\|endpoint")

8. Architect Agent → @developer
   Task: "добавь логирование middleware"

9. @developer → Tools
   read("src/middleware/logger.ts")
   edit("src/middleware/logger.ts")
   edit("src/server.ts")

10. Architect Agent → @review
    Task: "проверь код"

11. @review → Tools
    read("src/middleware/logger.ts")
    bash("npm run lint")

12. Architect Agent → Bash tool
    Command: "git add . && git commit -m 'feat: add logging'"
    Command: "git push"

13. Opencode Server → OpencodeToolProvider
    Response: результат выполнения

14. OpencodeToolProvider → ToolRegistry
    Return: строка с результатом

15. ToolRegistry → LLM Agent
    Result: результат выполнения

16. LLM Agent → User
    Response: "✅ Логирование добавлено"
```

---

## Разделение ответственности

### Sandbox (Docker) - Обработка данных

| Тип задач         | Инструменты               | Примеры                       |
| ----------------- | ------------------------- | ----------------------------- |
| Скачивание файлов | yt-dlp, curl, wget        | YouTube видео, файлы          |
| Обработка медиа   | ffmpeg                    | Видео → GIF, аудио извлечение |
| Анализ данных     | Python, Python библиотеки | Pandas, NumPy                 |
| Генерация данных  | Python скрипты            | CSV, JSON                     |
| Веб-скрапинг      | curl, wget, Python        | Парсинг сайтов                |

### Opencode - Разработка кода

| Тип задач    | Инструменты            | Примеры                        |
| ------------ | ---------------------- | ------------------------------ |
| Анализ кода  | @explore, grep, glob   | Поиск файлов, поиск паттернов  |
| Реализация   | @developer, edit, read | Создание/редактирование файлов |
| Проверка     | @review, bash (lint)   | Code review, тесты             |
| Git операции | Bash tool              | Commit, push, pull             |
| Документация | @general, write        | README, API docs               |

---

## Безопасность и изоляция

### Изоляция Sandbox

✅ **Docker контейнер:**

- Изолированная файловая система
- Ограниченные ресурсы (1GB RAM, 2 CPU)
- Network изоляция
- Ограниченные permissions

✅ **Временные файлы:**

- Файлы остаются только в контейнере
- Автоматическая очистка при остановке
- Нет доступа к хосту

### Изоляция Opencode

✅ **Отдельный процесс:**

- Opencode Server запущен отдельно
- Изолирован от Sandbox
- Контролируется через HTTP API

✅ **Git репозиторий:**

- Отдельный репозиторий
- Нет прямого доступа к Sandbox файлам
- Git операции изолированы

✅ **Permissions:**

- Agent permissions ограничены
- File access через permissions
- Subagent restrictions

---

## Преимущества архитектуры

### 1. Разделение ответственности

- **Sandbox** = Данные (Python, yt-dlp, ffmpeg)
- **Opencode** = Код (git, разработка, review)
- Четкое разделение упрощает поддержку

### 2. Масштабируемость

- Добавить новый провайдер легко
- Можно добавить несколько sandbox providers
- Opencode не зависит от конкретного песочницы

### 3. Гибкость

- LLM сам выбирает инструмент
- Можно комбинировать sandbox + opencode
- Легко расширять новые инструменты

### 4. Безопасность

- Полная изоляция между компонентами
- Docker контейнер для небезопасных операций
- Нет прямого доступа к системе

### 5. Тестируемость

- Каждый компонент можно тестировать отдельно
- Mock-и для зависимостей
- Интеграционные тесты для всей системы

---

## Следующие шаги

- [ ] Изучить [flow.md](./flow.md) - Детальные потоки выполнения
- [ ] Изучить [components.md](./components.md) - Детальное описание компонентов
- [ ] Перейти к [implementation/](../implementation/) - Код реализации

---

**Связанные документы:**

- [architecture/flow.md](./flow.md) - Потоки выполнения
- [architecture/components.md](./components.md) - Компоненты
- [README.md](../README.md) - Главная документация
