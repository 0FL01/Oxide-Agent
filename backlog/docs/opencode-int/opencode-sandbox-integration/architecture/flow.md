# Execution Flow

> **Детальное описание потоков выполнения с примерами**
>
> 📁 **Раздел:** Architecture
> 🎯 **Цель:** Понять как работает система пошагово

---

## 📋 Оглавление

- [Потоки высокого уровня](#потоки-высокого-уровня)
- [Детальный поток: Sandbox задача](#детальный-поток-sandbox-задача)
- [Детальный поток: Opencode задача](#детальный-поток-opencode-задача)
- [Многошаговые потоки](#многошаговые-потоки)
- [Обработка ошибок](#обработка-ошибок)

---

## Потоки высокого уровня

### Flow 1: Sandbox-only задача

```
User Request
    ↓
LLM Analysis (needs: data processing)
    ↓
ToolRegistry.execute("execute_command", ...)
    ↓
SandboxProvider.execute_command()
    ↓
Docker Container (yt-dlp, ffmpeg, Python)
    ↓
Result
    ↓
LLM → User
```

**Пример:** Скачать видео с YouTube

### Flow 2: Opencode-only задача

```
User Request
    ↓
LLM Analysis (needs: code development)
    ↓
ToolRegistry.execute("opencode", ...)
    ↓
OpencodeToolProvider.execute_task()
    ↓
Opencode Server
    ↓
Architect Agent
    ↓
Subagents (@explore, @developer, @review)
    ↓
Bash tool (git)
    ↓
Result
    ↓
LLM → User
```

**Пример:** Добавить логирование API

### Flow 3: Комбинированная задача

```
User Request
    ↓
LLM Analysis (needs: data + code)
    ↓
ToolRegistry.execute("execute_command", ...)  # Шаг 1
    ↓
Docker Container (download data)
    ↓
Result 1
    ↓
ToolRegistry.execute("opencode", ...)       # Шаг 2
    ↓
Opencode Server (process data in code)
    ↓
Result 2
    ↓
LLM → User
```

**Пример:** Скачать видео, извлечь аудио, добавить API для аудио

---

## Детальный поток: Sandbox задача

### Сценарий: Скачать YouTube видео

**Запрос:** "скачай видео с YouTube https://youtube.com/watch?v=xxx"

#### Шаг 1: User → LLM Agent

```
Input:
  User: "скачай видео с YouTube https://youtube.com/watch?v=xxx"

LLM Thinking:
  - Требуется скачивание видео
  - Нужен yt-dlp (в Sandbox)
  - Инструмент: execute_command

Decision:
  Использовать Sandbox tool
```

#### Шаг 2: LLM Agent → ToolRegistry

```
LLM Agent вызывает:
  ToolRegistry.execute(
    tool_name: "execute_command",
    arguments: r#"{"command": "yt-dlp -f best https://youtube.com/watch?v=xxx -o video.mp4"}"#,
    progress_tx: &Sender<AgentEvent>,
    cancellation_token: None,
  )
```

#### Шаг 3: ToolRegistry → SandboxProvider

```
ToolRegistry видит: tool_name == "execute_command"
    ↓
Маршрутизирует в: SandboxProvider
    ↓
SandboxProvider.execute_command(
    "yt-dlp -f best https://youtube.com/watch?v=xxx -o video.mp4"
)
```

#### Шаг 4: SandboxProvider → Docker Container

```
SandboxProvider.exec_command(cmd)
    ↓
Docker.create_exec(
    container_id: "agent-sandbox-user-123",
    cmd: ["sh", "-c", "yt-dlp -f best https://youtube.com/watch?v=xxx -o video.mp4"],
    working_dir: "/workspace"
)
    ↓
Docker.start_exec(exec_id)
    ↓
Output streaming:
  - yt-dlp запускается
  - Прогресс скачивания
  - Video downloaded: video.mp4 (100%)
    ↓
ExecResult {
    stdout: "[download] 100% of 50MiB\n[download] Destination: video.mp4",
    stderr: "",
    exit_code: 0,
}
```

#### Шаг 5: Результат → LLM Agent → User

```
SandboxProvider возвращает:
  Ok("[download] 100% of 50MiB\n[download] Destination: video.mp4")
    ↓
ToolRegistry отправляет прогресс:
  AgentEvent::ToolResult {
    name: "execute_command",
    output: "[download] 100% of 50MiB\n[download] Destination: video.mp4"
  }
    ↓
LLM Agent получает результат
    ↓
LLM форматирует ответ:
  "✅ Видео успешно скачано!

   Файл: video.mp4
   Размер: 50MiB
   Местоположение: /workspace/video.mp4"
    ↓
User получает сообщение
```

---

## Детальный поток: Opencode задача

### Сценарий: Добавить логирование API

**Запрос:** "добавь логирование всех API запросов"

#### Шаг 1: User → LLM Agent

```
Input:
  User: "добавь логирование всех API запросов"

LLM Thinking:
  - Требуется разработка кода
  - Нужен Architect Agent (@explore, @developer, @review)
  - Инструмент: opencode

Decision:
  Использовать Opencode tool
```

#### Шаг 2: LLM Agent → ToolRegistry

```
LLM Agent вызывает:
  ToolRegistry.execute(
    tool_name: "opencode",
    arguments: r#"{"task": "add request logging for all API endpoints"}"#,
    progress_tx: &Sender<AgentEvent>,
    cancellation_token: None,
  )
```

#### Шаг 3: ToolRegistry → OpencodeToolProvider

```
ToolRegistry видит: tool_name == "opencode"
    ↓
Маршрутизирует в: OpencodeToolProvider
    ↓
OpencodeToolProvider.execute_task("add request logging for all API endpoints")
```

#### Шаг 4: OpencodeToolProvider → Opencode Server (Create Session)

```
OpencodeToolProvider.create_session(task)
    ↓
HTTP POST http://localhost:4096/session
Body: {
  "title": "Sandbox: add request logging for all API endpoints",
  "agent": "architect"
}
    ↓
HTTP 200 OK
Response: {
  "id": "session-abc123",
  "title": "Sandbox: add request logging for all API endpoints",
  "projectID": "project-xyz",
  "directory": "/path/to/project"
}
```

#### Шаг 5: OpencodeToolProvider → Opencode Server (Send Prompt)

```
OpencodeToolProvider.send_prompt(session_id, task)
    ↓
HTTP POST http://localhost:4096/session/session-abc123/message
Body: {
  "agent": "architect",
  "parts": [{
    "type": "text",
    "text": "add request logging for all API endpoints"
  }]
}
    ↓
Opencode Server передает prompt в Architect Agent
```

#### Шаг 6: Architect Agent → @explore

```
Architect Agent получает prompt:
  "add request logging for all API endpoints"
    ↓
Architect разбивает задачу:
  1. Найти API endpoints (@explore)
  2. Создать logging middleware (@developer)
  3. Проверить код (@review)
  4. Commit + push (bash)
    ↓
Architect вызывает Task tool:
  Task(
    description: "find API endpoints",
    prompt: "найди все API endpoints в проекте",
    subagent_type: "explore"
  )
```

#### Шаг 7: @explore → Tools

```
@explore executes:
  1. glob("src/**/*.ts")
     → Файлы: src/api/routes.ts, src/server.ts, src/middleware/logger.ts
     ↓
  2. grep("router\|endpoint", "src/api/routes.ts")
     → Найдено: /api/users, /api/posts, /api/auth
     ↓
  3. read("src/api/routes.ts")
     → Файл содержит Express router с 3 endpoints
     ↓
Result:
  "Найдены endpoints:
  - GET /api/users
  - GET /api/posts
  - GET /api/auth
  - Местоположение: src/api/routes.ts"
```

#### Шаг 8: Architect Agent → @developer

```
Architect получает результат от @explore
    ↓
Architect вызывает Task tool:
  Task(
    description: "add logging middleware",
    prompt: "создай logging middleware и подключи ко всем endpoints",
    subagent_type: "developer"
  )
```

#### Шаг 9: @developer → Tools

```
@developer executes:
  1. read("src/middleware/logger.ts")
     → Файл существует, содержит базовый logger
     ↓
  2. edit("src/middleware/logger.ts")
     → Добавляет request logging функцию
     ↓
  3. read("src/server.ts")
     → Файл содержит Express app setup
     ↓
  4. edit("src/server.ts")
     → Подключает logging middleware к app
     ↓
Result:
  "Logging middleware создан и подключен:
  - src/middleware/logger.ts: добавлена функция logRequest()
  - src/server.ts: middleware подключен ко всем routes"
```

#### Шаг 10: Architect Agent → @review

```
Architect получает результат от @developer
    ↓
Architect вызывает Task tool:
  Task(
    description: "review logging changes",
    prompt: "проверь добавленный logging код на качество",
    subagent_type: "review"
  )
```

#### Шаг 11: @review → Tools

```
@review executes:
  1. read("src/middleware/logger.ts")
     → Код выглядит корректно
     ↓
  2. grep("console.log", "src/")
     → Не найдено (хорошо, нет временного logging)
     ↓
  3. bash("npm run lint")
     → No linting errors
     ↓
Result:
  "✅ Code review пройден:
  - Logging middleware реализован корректно
  - Нет временного console.log
  - Linting passed (0 errors)"
```

#### Шаг 12: Architect Agent → Bash tool (Git)

```
Architect получает результат от @review
    ↓
Architect вызывает Bash tool:
  Bash(
    command: "git status"
  )
    ↓
Result: "modified: src/middleware/logger.ts, src/server.ts"
    ↓
Architect вызывает Bash tool:
  Bash(
    command: "git add . && git commit -m 'feat: add request logging for all API endpoints'"
  )
    ↓
Result: "[master abc1234] feat: add request logging for all API endpoints"
    ↓
Architect вызывает Bash tool:
  Bash(
    command: "git push"
  )
    ↓
Result: "To github.com:user/repo.git
   abc1234..def5678  master -> master"
```

#### Шаг 13: Architect Agent → Opencode Server → OpencodeToolProvider

```
Architect формирует summary:
  "✅ Логирование API запросов успешно добавлено!

  Изменения:
  - Создана функция logRequest() в src/middleware/logger.ts
  - Middleware подключен в src/server.ts
  - Все 3 endpoints теперь логируются

  Git:
  - Commit: abc1234 feat: add request logging for all API endpoints
  - Push: успешно в master"

Opencode Server возвращает:
  HTTP 200 OK
  Body: {
    "info": {...},
    "parts": [{
      "type": "text",
      "text": "✅ Логирование API запросов успешно добавлено!..."
    }]
  }
```

#### Шаг 14: Результат → LLM Agent → User

```
OpencodeToolProvider возвращает:
  Ok("✅ Логирование API запросов успешно добавлено!...")
    ↓
ToolRegistry отправляет прогресс:
  AgentEvent::ToolResult {
    name: "opencode",
    output: "✅ Логирование API запросов успешно добавлено!..."
  }
    ↓
LLM Agent получает результат
    ↓
LLM форматирует ответ:
  "✅ Задача выполнена!

  Логирование API запросов добавлено:

  📝 Создана функция logRequest() в src/middleware/logger.ts
  🔌 Middleware подключен в src/server.ts
  📊 Все 3 endpoints теперь логируются

  Git:
  ✅ Commit: abc1234 - feat: add request logging
  ✅ Push: успешно в master"
    ↓
User получает сообщение
```

---

## Многошаговые потоки

### Сценарий: Скачать видео, извлечь аудио, добавить API для аудио

**Запрос:** "скачай видео, извлечи аудио и добавь поддержку аудио в наш API"

#### Фаза 1: Sandbox (Download video)

```
LLM Decision: "скачать видео → нужен yt-dlp → Sandbox"

ToolRegistry.execute(
  "execute_command",
  r#"{"command": "yt-dlp -f best https://youtube.com/watch?v=xxx -o video.mp4"}"#
)
    ↓
Sandbox downloads video
    ↓
Result: "Video downloaded: video.mp4"
```

#### Фаза 2: Sandbox (Extract audio)

```
LLM Decision: "извлечь аудио → нужен ffmpeg → Sandbox"

ToolRegistry.execute(
  "execute_command",
  r#"{"command": "ffmpeg -i video.mp4 -vn -acodec libmp3lame -q:a 2 audio.mp3"}"#
)
    ↓
Sandbox extracts audio
    ↓
Result: "Audio extracted: audio.mp3"
```

#### Фаза 3: Opencode (Add API endpoint)

```
LLM Decision: "добавить API endpoint → нужен разработчик → Opencode"

ToolRegistry.execute(
  "opencode",
  r#"{"task": "add audio upload API endpoint with validation"}"#
)
    ↓
Opencode Server:
  - @explore: находит существующие upload endpoints
  - @developer: создает /api/upload/audio endpoint
  - @review: проверяет код
  - Bash tool: git commit + push
    ↓
Result: "Audio upload API endpoint added:
  - POST /api/upload/audio
  - Validates MP3 files
  - Saves to /uploads/audio"
```

#### Финальный результат

```
LLM форматирует все результаты:
  "✅ Задача выполнена!

  1. 📥 Video downloaded: video.mp4 (50MiB)
  2. 🎵 Audio extracted: audio.mp3 (5MiB)
  3. 🔌 API endpoint added:
     - POST /api/upload/audio
     - Validates MP3 files
     - Saves to /uploads/audio

  Git:
  ✅ Commit: xyz7890 - feat: add audio upload endpoint
  ✅ Push: успешно в master"
```

---

## Обработка ошибок

### Ошибка 1: Opencode сервер недоступен

```
ToolRegistry.execute("opencode", ...)
    ↓
OpencodeToolProvider.create_session()
    ↓
HTTP POST http://localhost:4096/session
    ↓
ConnectionRefused
    ↓
Error: "Failed to connect to Opencode server: Connection refused"
    ↓
LLM Agent получает ошибку
    ↓
LLM формулирует ответ:
  "❌ Не удалось подключиться к Opencode серверу.

   Пожалуйста, убедитесь, что сервер запущен:
   opencode serve --hostname=127.0.0.1 --port=4096"
```

### Ошибка 2: Команда не найдена в Sandbox

```
ToolRegistry.execute("execute_command", "yt-dlp")
    ↓
SandboxProvider.exec_command("yt-dlp")
    ↓
Docker container executes: sh -c "yt-dlp"
    ↓
stdout: ""
stderr: "sh: yt-dlp: command not found"
exit_code: 127
    ↓
Error: "Command failed (exit code 127): sh: yt-dlp: command not found"
    ↓
LLM Agent получает ошибку
    ↓
LLM формулирует ответ:
  "❌ Команда yt-dlp не найдена в контейнере.

   Пожалуйста, убедитесь, что yt-dlp установлен:
   - Проверьте Dockerfile.sandbox
   - Пересоберите образ Docker"
```

### Ошибка 3: Git не сконфигурирован

```
Architect Agent → Bash tool: git commit
    ↓
Output: "*** Please tell me who you are.
   Run:
     git config --global user.email 'you@example.com'
     git config --global user.name 'Your Name'"
    ↓
Architect получает ошибку
    ↓
Architect формулирует ответ:
  "❌ Git не сконфигурирован.

   Пожалуйста, выполните:
   git config --global user.name 'Your Name'
   git config --global user.email 'your@email.com'

   Затем повторите задачу."
```

---

## Потоки в сравнении

| Тип задачи          | Компоненты                                                  | Количество шагов | Время выполнения |
| ------------------- | ----------------------------------------------------------- | ---------------- | ---------------- |
| **Sandbox only**    | LLM → ToolRegistry → Sandbox → Docker                       | ~5               | ~10-60 сек       |
| **Opencode only**   | LLM → ToolRegistry → Opencode → Architect → Subagents → Git | ~15              | ~2-10 мин        |
| **Комбинированный** | Sandbox (2x) + Opencode (1x)                                | ~20              | ~5-15 мин        |

---

## Следующие шаги

- [ ] Изучить [components.md](./components.md) - Детальное описание компонентов
- [ ] Перейти к [examples/](../examples/) - Практические примеры
- [ ] Перейти к [testing/](../testing/) - Тестирование

---

**Связанные документы:**

- [architecture/overview.md](./overview.md) - Обзор архитектуры
- [architecture/components.md](./components.md) - Компоненты
- [testing/troubleshooting.md](../testing/troubleshooting.md) - Устранение проблем
