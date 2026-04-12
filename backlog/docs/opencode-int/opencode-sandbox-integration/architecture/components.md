# Components

> **Детальное описание компонентов системы**
>
> 📁 **Раздел:** Architecture
> 🎯 **Цель:** Понять каждый компонент в деталях

---

## 📋 Оглавление

- [LLM Agent](#llm-agent)
- [ToolRegistry](#toolregistry)
- [OpencodeToolProvider](#opencodetoolprovider)
- [SandboxProvider](#sandboxprovider)
- [Opencode Server](#opencode-server)
- [Architect Agent](#architect-agent)
- [Subagents](#subagents)
- [Tools](#tools)

---

## LLM Agent

### Назначение

LLM Agent является главным оркестратором системы. Он анализирует запросы пользователя и принимает решения о том, какой инструмент использовать.

### Ответственность

- **Анализ запросов:** Понимание того, что хочет пользователь
- **Выбор инструментов:** Решение между Sandbox и Opencode
- **Формирование команд:** Создание правильных вызовов инструментов
- **Обработка результатов:** Интерпретация ответов от инструментов
- **Управление ошибками:** Обработка сбоев и ошибок

### Интерфейс

```rust
pub struct LLMAgent {
    pub model: String,
    pub registry: ToolRegistry,
    pub session: AgentSession,
}

impl LLMAgent {
    pub async fn process_request(&self, request: &str) -> Result<String, Error> {
        // 1. Анализировать запрос
        let intent = self.analyze_intent(request)?;

        // 2. Выбрать инструмент
        let tool = self.select_tool(&intent)?;

        // 3. Сформировать аргументы
        let args = self.format_arguments(&intent, &tool)?;

        // 4. Выполнить
        let result = self.registry.execute(tool, &args, &progress_tx, None).await?;

        // 5. Обработать результат
        Ok(self.format_response(result))
    }
}
```

### Конфигурация

```toml
[agent]
model = "openai/gpt-4.1"
temperature = 0.3
max_tokens = 4000
tools = ["sandbox", "opencode"]
```

---

## ToolRegistry

### Назначение

ToolRegistry маршрутизирует вызовы инструментов к соответствующим провайдерам. Это центральная точка для всех инструментальных вызовов.

### Ответственность

- **Регистрация провайдеров:** Добавление новых провайдеров (Sandbox, Opencode и др.)
- **Маршрутизация вызовов:** Направление вызовов правильному провайдеру
- **Обработка ошибок:** Единая обработка ошибок от всех провайдеров
- **Прогресс-события:** Отправка событий прогресса вызывающему коду

### Интерфейс

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
    ) -> Result<String, String> {
        // 1. Проверка на opencode tool
        if tool_name == "opencode" {
            return self.handle_opencode(arguments, progress_tx).await;
        }

        // 2. Маршрутизация в провайдеры
        for provider in &self.providers {
            if provider.can_handle(tool_name) {
                return provider.execute(tool_name, arguments, progress_tx, cancellation_token).await;
            }
        }

        Err(format!("Tool not found: {}", tool_name))
    }
}
```

### Events

```rust
pub enum AgentEvent {
    ToolCall {
        name: String,
        input: String,
        command_preview: Option<String>,
    },
    ToolResult {
        name: String,
        output: String,
    },
    FileToSend {
        filename: String,
        content: Vec<u8>,
    },
}
```

---

## OpencodeToolProvider

### Назначение

OpencodeToolProvider - это HTTP клиент для взаимодействия с Opencode Server. Он предоставляет простой интерфейс для создания sessions и отправки prompts.

### Ответственность

- **Создание sessions:** Создание новой session через HTTP API
- **Отправка prompts:** Отправка задач в architect agent
- **Получение результатов:** Извлечение текста из ответов
- **Health checks:** Проверка доступности сервера
- **Обработка ошибок HTTP:** Обработка сетевых ошибок и ошибок API

### Интерфейс

```rust
pub struct OpencodeToolProvider {
    pub base_url: String,
    pub client: reqwest::Client,
    pub timeout: Duration,
}

impl OpencodeToolProvider {
    /// Создать session и отправить задачу
    pub async fn execute_task(&self, task: &str) -> Result<String, OpencodeError> {
        let session = self.create_session(task).await?;
        let response = self.send_prompt(&session.id, task).await?;
        let text = self.extract_text_from_response(&response)?;
        Ok(text)
    }

    /// Создать новую session
    pub async fn create_session(&self, task: &str) -> Result<SessionResponse, OpencodeError>;

    /// Отправить prompt в session
    pub async fn send_prompt(&self, session_id: &str, task: &str) -> Result<PromptResponse, OpencodeError>;

    /// Проверить здоровье сервера
    pub async fn health_check(&self) -> Result<(), OpencodeError>;
}
```

### Конфигурация

```rust
let provider = OpencodeToolProvider::new("http://127.0.0.1:4096".to_string())
    .with_timeout(Duration::from_secs(300)); // 5 минут
```

### API Endpoints

| Endpoint                | Method | Описание              |
| ----------------------- | ------ | --------------------- |
| `/session`              | POST   | Создать новую session |
| `/session/{id}/message` | POST   | Отправить prompt      |
| `/vcs`                  | GET    | Health check          |

---

## SandboxProvider

### Назначение

SandboxProvider управляет Docker контейнером для выполнения команд в изолированной среде.

### Ответственность

- **Управление контейнерами:** Создание, запуск, остановка контейнеров
- **Выполнение команд:** Запуск команд в контейнере
- **Управление файлами:** Чтение и запись файлов в контейнере
- **Очистка ресурсов:** Удаление контейнеров и файлов
- **Безопасность:** Ограничение ресурсов и permissions

### Интерфейс

```rust
pub struct SandboxProvider {
    pub docker: Docker,
    pub container_id: Option<String>,
}

impl SandboxProvider {
    /// Выполнить команду
    pub async fn execute_command(&mut self, cmd: &str) -> Result<String, Error>;

    /// Записать файл
    pub async fn write_file(&mut self, path: &str, content: &[u8]) -> Result<(), Error>;

    /// Прочитать файл
    pub async fn read_file(&mut self, path: &str) -> Result<Vec<u8>, Error>;

    /// Список файлов
    pub async fn list_files(&mut self, path: &str) -> Result<Vec<String>, Error>;
}
```

### Docker конфигурация

```yaml
# docker-compose.yml
services:
  agent-sandbox:
    image: agent-sandbox:latest
    container_name: agent-sandbox-user-{user_id}
    mem_limit: 1g
    cpus: "2"
    volumes:
      - /workspace:/workspace
    command: sleep infinity
```

### Доступные инструменты

| Инструмент | Версия | Использование             |
| ---------- | ------ | ------------------------- |
| Python     | 3.x    | Выполнение скриптов       |
| yt-dlp     | Latest | Скачивание видео          |
| ffmpeg     | Latest | Обработка медиа           |
| curl       | Latest | HTTP запросы              |
| wget       | Latest | Скачивание файлов         |
| jq         | Latest | JSON обработка            |
| git        | Latest | Git операции (если нужно) |

---

## Opencode Server

### Назначение

Opencode Server - это HTTP сервер, который предоставляет API для управления sessions и агентами. Он запускается через `opencode serve`.

### Ответственность

- **HTTP API:** REST API для всех операций
- **Session management:** Создание, удаление, fork сессий
- **Agent orchestration:** Управление architect и subagents
- **Tools:** Предоставление инструментов (bash, edit, grep, glob, etc.)
- **Git integration:** Выполнение git операций через bash tool

### API Structure

```
Opencode Server
├── HTTP API (REST)
│   ├── /session - Sessions
│   ├── /vcs - VCS info
│   ├── /project - Project info
│   ├── /tool - Tools
│   └── /event - SSE events
├── Agent System
│   ├── Architect Agent (primary)
│   └── Subagents (specialized)
└── Tool System
    ├── Bash tool
    ├── Edit tool
    ├── Read tool
    ├── Grep tool
    ├── Glob tool
    ├── Task tool (calls subagents)
    └── Question tool
```

### Конфигурация

```bash
opencode serve \
  --hostname=127.0.0.1 \
  --port=4096 \
  --log-level=info
```

---

## Architect Agent

### Назначение

Architect Agent - это primary agent, который оркестрирует выполнение задач, делегируя работу субагентам.

### Ответственность

- **Анализ задач:** Разбиение сложных задач на подзадачи
- **Делегирование:** Вызов субагентов через Task tool
- **Координация:** Управление порядком выполнения
- **Git операции:** Выполнение commit и push
- **Синтез:** Объединение результатов от субагентов

### Рабочий процесс

```
1. Получает prompt от пользователя
2. Анализирует задачу
3. Разбивает на подзадачи
4. Вызывает субагентов (через Task tool)
5. Мониторит выполнение
6. Выполняет git операции (если нужно)
7. Формирует summary
8. Возвращает результат
```

### Конфигурация

```markdown
---
description: Orchestrates complex multi-step development tasks
mode: primary
permission:
  task:
    "*": "allow"
  edit: "allow"
  bash: "allow"
  read: "allow"
  glob: "allow"
  grep: "allow"
---

You are an architect agent responsible for orchestrating complex multi-step development tasks.

Your role is to:

1. Analyze the user's request and break it down into smaller, manageable tasks
2. Delegate tasks to specialized subagents using the Task tool
3. Coordinate between subagents and consolidate their results
4. Execute git operations when code changes are complete
5. Provide a cohesive summary and next steps

Always:

- Use subagents for specialized work
- Provide clear context when delegating
- Monitor subagent progress and intervene if needed
- Commit and push changes when tasks are complete
```

---

## Subagents

### @explore

**Назначение:** Быстрое исследование кодовой базы (read-only)

**Инструменты:**

- glob - поиск файлов
- grep - поиск кода
- read - чтение файлов
- webfetch - получение веб-страниц
- websearch - поиск в интернете
- codesearch - поиск по коду

**Когда использовать:**

- Найти файлы по паттерну
- Понять структуру проекта
- Найти конкретные функции или классы
- Понять как работает часть кода

**Пример:**

```
task("find API endpoints", "найди все API endpoints в проекте", "explore")
```

---

### @developer

**Назначение:** Реализация кода

**Инструменты:**

- edit - редактирование файлов
- read - чтение файлов
- write - запись файлов
- bash - выполнение команд
- glob - поиск файлов
- grep - поиск кода

**Когда использовать:**

- Создать новые файлы
- Редактировать существующие файлы
- Реализовать новую функцию
- Добавить новые endpoints

**Пример:**

```
task("add logging middleware", "создай logging middleware", "developer")
```

---

### @review

**Назначение:** Code review и проверка качества

**Инструменты:**

- read - чтение файлов
- grep - поиск паттернов
- bash - выполнение команд (lint, tests)
- glob - поиск файлов

**Когда использовать:**

- Проверить изменения кода
- Запустить linting
- Запустить тесты
- Проверить на плохие практики

**Пример:**

```
task("review changes", "проверь добавленный код", "review")
```

---

### @general

**Назначение:** Многошаговые задачи с полным доступом к инструментам

**Инструменты:** Все инструменты доступны

**Когда использовать:**

- Сложные задачи, требующие нескольких типов работы
- Задачи, которые не подходят под других субагентов
- Общие задачи

**Пример:**

```
task("complex task", "выполни сложную задачу", "general")
```

---

### @assist

**Назначение:** Помощник для документации, git и скриптов

**Инструменты:** Ограниченный набор инструментов

**Когда использовать:**

- Написать документацию
- Настроить git
- Создать скрипты

**Пример:**

```
task("write docs", "напиши README", "assist")
```

---

## Tools

### Task Tool

**Назначение:** Вызов субагентов

**Параметры:**

- `description` (string): Короткое описание задачи (3-5 слов)
- `prompt` (string): Детальное описание задачи для субагента
- `subagent_type` (string): Тип субагента ("explore", "developer", "review", etc.)
- `task_id` (string, опционально): ID для возобновления задачи

**Использование:**

```rust
task("find files", "найди все API endpoints", "explore")
```

---

### Bash Tool

**Назначение:** Выполнение shell команд

**Где выполняется:** Opencode Server (НЕ в sandbox Docker контейнере!)

**Использование:**

```rust
bash("git add . && git commit -m 'message'")
bash("npm run lint")
bash("npm test")
```

**Важно:** Bash tool выполняется в файловой системе проекта, НЕ в sandbox!

---

### Edit Tool

**Назначение:** Редактирование файлов

**Параметры:**

- `path` (string): Путь к файлу
- `old_string` (string): Текст для замены
- `new_string` (string): Новый текст

**Использование:**

```rust
edit("src/file.ts", "old code", "new code")
```

---

### Read Tool

**Назначение:** Чтение файлов

**Параметры:**

- `path` (string): Путь к файлу

**Использование:**

```rust
let content = read("src/file.ts")
```

---

### Grep Tool

**Назначение:** Поиск паттернов в файлах

**Параметры:**

- `pattern` (string): Regex паттерн
- `path` (string, опционально): Путь к поиску

**Использование:**

```rust
let matches = grep("router\|endpoint", "src/api/")
```

---

### Glob Tool

**Назначение:** Поиск файлов по паттерну

**Параметры:**

- `pattern` (string): Glob паттерн

**Использование:**

```rust
let files = glob("src/**/*.ts")
```

---

## Сравнение компонентов

| Компонент                | Тип          | Зависимости           | Сложность  |
| ------------------------ | ------------ | --------------------- | ---------- |
| **LLM Agent**            | Main         | ToolRegistry          | ⭐⭐⭐     |
| **ToolRegistry**         | Orchestrator | Providers             | ⭐⭐       |
| **OpencodeToolProvider** | Provider     | HTTP, Opencode Server | ⭐⭐⭐     |
| **SandboxProvider**      | Provider     | Docker                | ⭐⭐⭐⭐   |
| **Opencode Server**      | Server       | Опенкод               | ⭐⭐⭐⭐⭐ |
| **Architect Agent**      | LLM Agent    | Subagents, Tools      | ⭐⭐⭐⭐   |
| **Subagents**            | LLM Agents   | Tools                 | ⭐⭐⭐     |
| **Tools**                | Executors    | N/A                   | ⭐⭐       |

---

## Следующие шаги

- [ ] Изучить [examples/basic_usage.md](../examples/basic_usage.md) - Базовые примеры
- [ ] Изучить [implementation/](../implementation/) - Код реализации
- [ ] Перейти к [testing/](../testing/) - Тестирование

---

**Связанные документы:**

- [architecture/overview.md](./overview.md) - Обзор архитектуры
- [architecture/flow.md](./flow.md) - Потоки выполнения
- [implementation/](../implementation/) - Код реализации
