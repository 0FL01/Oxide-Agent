# Advanced Workflow

> **Сложные рабочие процессы и сценарии**
>
> 📁 **Раздел:** Examples
> 🎯 **Цель:** Понять сложные сценарии

---

## 📋 Оглавление

- [Сценарий 1: YouTube видео → API endpoint](#сценарий-1-youtube-видео--api-endpoint)
- [Сценарий 2: Анализ данных с Pandas](#сценарий-2-анализ-данных-с-pandas)
- [Сценарий 3: Рефакторинг модуля](#сценарий-3-рефакторинг-модуля)
- [Сценарий 4: Обработка ошибки](#сценарий-4-обработка-ошибки)
- [Сценарий 5: Параллельное выполнение](#сценарий-5-параллельное-выполнение)

---

## Сценарий 1: YouTube видео → API endpoint

### Описание

Скачать видео с YouTube, извлечь аудио, создать API endpoint для загрузки аудио, и добавить его в систему.

### Пошаговый процесс

#### Шаг 1: Скачать видео

**LLM Decision:** Скачать видео → нужен yt-dlp → Sandbox

```rust
let result = registry.execute(
    "execute_command",
    r#"{"command": "yt-dlp -f best https://youtube.com/watch?v=xxx -o video.mp4"}"#,
    &progress_tx,
    None
).await?;

// Result:
// "✅ Видео скачано: video.mp4 (50MiB)"
```

#### Шаг 2: Извлечь аудио

**LLM Decision:** Извлечь аудио → нужен ffmpeg → Sandbox

```rust
let result = registry.execute(
    "execute_command",
    r#"{"command": "ffmpeg -i video.mp4 -vn -acodec libmp3lame -q:a 2 audio.mp3"}"#,
    &progress_tx,
    None
).await?;

// Result:
// "✅ Аудио извлечено: audio.mp3 (5MiB)"
```

#### Шаг 3: Создать API endpoint

**LLM Decision:** Добавить API endpoint → нужен Opencode

```rust
let result = registry.execute(
    "opencode",
    r#"{"task": "add audio upload API endpoint with validation and storage"}"#,
    &progress_tx,
    None
).await?;

// Result:
// "✅ Audio upload API endpoint добавлен:
//  - POST /api/upload/audio
//  - Валидация MP3 файлов
//  - Сохранение в /uploads/audio
//  - Аутентификация JWT
//  - Лимит размера файла: 10MB"
```

#### Шаг 4: Тестировать endpoint

**LLM Decision:** Тестировать → нужен bash → Sandbox

```rust
let result = registry.execute(
    "execute_command",
    r#"{"command": "curl -X POST http://localhost:3000/api/upload/audio -F 'audio=@audio.mp3'"}"#,
    &progress_tx,
    None
).await?;

// Result:
// "✅ Upload успешен!
//  Response: {\"id\":\"audio-123\",\"url\":\"/uploads/audio/audio-123.mp3\"}"
```

#### Итоговый результат

```
LLM Response:
  "✅ Полный workflow завершен!

  1. 📥 Video downloaded: video.mp4 (50MiB)
  2. 🎵 Audio extracted: audio.mp3 (5MiB)
  3. 🔌 API endpoint added:
     - POST /api/upload/audio
     - Validates MP3 files
     - JWT authentication required
     - File size limit: 10MB
  4. ✅ Upload tested successfully

  Git:
  ✅ Commit: abc1234 - feat: add audio upload endpoint
  ✅ Push: успешно в master"
```

---

## Сценарий 2: Анализ данных с Pandas

### Описание

Скачать CSV данные с URL, проанализировать их с помощью Python и Pandas, создать API endpoint для доступа к анализу.

### Пошаговый процесс

#### Шаг 1: Скачать данные

```rust
let result = registry.execute(
    "execute_command",
    r#"{"command": "curl -O https://example.com/data.csv"}"#,
    &progress_tx,
    None
).await?;
```

#### Шаг 2: Проанализировать с Python

```rust
let python_script = r#"
import pandas as pd
import json

# Read CSV
df = pd.read_csv('data.csv')

# Basic statistics
stats = {
    'rows': len(df),
    'columns': len(df.columns),
    'column_names': df.columns.tolist(),
    'dtypes': df.dtypes.astype(str).to_dict(),
    'null_counts': df.isnull().sum().to_dict(),
}

# Calculate correlations for numeric columns
numeric_cols = df.select_dtypes(include=['int64', 'float64']).columns
if len(numeric_cols) > 0:
    stats['correlations'] = df[numeric_cols].corr().to_dict()

print(json.dumps(stats, indent=2))
"#;

let result = registry.execute(
    "execute_command",
    &format!(r#"{{"command": "python3 -c \"{}\"}}"#, python_script.replace('\n', " ")),
    &progress_tx,
    None
).await?;
```

#### Шаг 3: Создать API endpoint

```rust
let result = registry.execute(
    "opencode",
    r#"{"task": "create data analysis API endpoint with statistics"}"#,
    &progress_tx,
    None
).await?;
```

---

## Сценарий 3: Рефакторинг модуля

### Описание

Рефакторить аутентификационный модуль: вынести общую логику в utils, добавить JWT токены, обновить все точки использования.

### Пошаговый процесс

#### Шаг 1: Анализировать текущий код

**LLM Decision:** Понять структуру → @explore → Opencode

```rust
let result = registry.execute(
    "opencode",
    r#"{"task": "analyze authentication module and identify refactor opportunities"}"#,
    &progress_tx,
    None
).await?;

// Architect calls @explore:
// - glob("src/auth/**/*.ts")
// - grep("authenticate\|login", "src/")
// - read("src/auth/index.ts")
// - read("src/auth/utils.ts")

// Result:
// "📊 Анализ auth модуля:
//  - Файлы: 8 TypeScript файлов
//  - Использование: authenticate() в 15 местах
//  - Дублирование: JWT creation в 3 местах
//  - Возможности для рефакторинга:
//    * Вынести JWT utils
//    * Объединить auth providers
//    * Унифицировать error handling"
```

#### Шаг 2: Создать utils модуль

**LLM Decision:** Создать utils → @developer → Opencode

```rust
let result = registry.execute(
    "opencode",
    r#"{"task": "create auth utils module with JWT token generation and validation"}"#,
    &progress_tx,
    None
).await?;

// Architect calls @developer:
// - write("src/auth/utils/jwt.ts")
// - add functions: generateToken(), validateToken(), decodeToken()

// Result:
// "✅ Auth utils модуль создан:
//  - src/auth/utils/jwt.ts
//  - generateToken()
//  - validateToken()
//  - decodeToken()"
```

#### Шаг 3: Рефакторить использование

**LLM Decision:** Обновить код → @developer → Opencode

```rust
let result = registry.execute(
    "opencode",
    r#"{"task": "refactor all authentication code to use new auth utils"}"#,
    &progress_tx,
    None
).await?;

// Architect calls @developer:
// - read(src/auth/index.ts)
// - edit(src/auth/index.ts) - replace JWT creation with utils.generateToken()
// - read(src/auth/providers/local.ts)
// - edit(src/auth/providers/local.ts) - replace JWT validation
// - ... (11 more files)

// Result:
// "✅ Рефакторинг завершен:
//  - Обновлено 15 файлов
//  - Все JWT создание заменено на utils.generateToken()
//  - Все JWT валидация заменено на utils.validateToken()
//  - Удалено 150 строк дублирования"
```

#### Шаг 4: Тесты и review

**LLM Decision:** Протестировать → @review → Opencode

```rust
let result = registry.execute(
    "opencode",
    r#"{"task": "run tests and code review on refactored auth module"}"#,
    &progress_tx,
    None
).await?;

// Architect calls @review:
// - bash("npm test")
// - bash("npm run lint")
// - grep("TODO", "src/auth/")

// Result:
// "✅ Тесты и review завершены:
//  - Tests: passed (45/45)
//  - Lint: passed (0 errors)
//  - No TODOs left
//  - Code quality: improved (cyclomatic complexity reduced from 15 to 8)"
```

#### Шаг 5: Git операции

**LLM Decision:** Commit и push → bash → Opencode

```rust
// Architect calls bash:
// git status → modified: 15 files
// git add .
// git commit -m "refactor(auth): extract JWT utils and unify auth handling"
// git push
```

---

## Сценарий 4: Обработка ошибки

### Описание

Обработка ошибки при скачивании файла и попытка альтернативного метода.

### Пошаговый процесс

#### Шаг 1: Попытка скачать с yt-dlp

```rust
let result = registry.execute(
    "execute_command",
    r#"{"command": "yt-dlp https://youtube.com/watch?v=xxx -o video.mp4"}"#,
    &progress_tx,
    None
).await;

match result {
    Ok(output) => {
        // Успех
        return Ok(output);
    }
    Err(e) if e.contains("HTTP Error 429") => {
        // Too many requests - использовать альтернативный метод
    }
    Err(e) => {
        // Другая ошибка
        return Err(e);
    }
}
```

#### Шаг 2: Альтернативный метод (если yt-dlp не работает)

```rust
// Если yt-dlp не работает из-за rate limiting
// Использовать yt-dlp с proxy
let result = registry.execute(
    "execute_command",
    r#"{"command": "yt-dlp --proxy socks5://127.0.0.1:1080 https://youtube.com/watch?v=xxx -o video.mp4"}"#,
    &progress_tx,
    None
).await?;
```

---

## Сценарий 5: Параллельное выполнение

### Описание

Параллельное выполнение нескольких независимых задач.

### Реализация

```rust
use tokio::spawn;

async fn parallel_workflow() {
    let ctx = setup().await;

    // Task 1: Скачивать видео
    let task1 = tokio::spawn(async move {
        registry.execute(
            "execute_command",
            r#"{"command": "yt-dlp url1 -o video1.mp4"}"#,
            &progress_tx,
            None
        ).await
    });

    // Task 2: Скачивать другое видео
    let task2 = tokio::spawn(async move {
        registry.execute(
            "execute_command",
            r#"{"command": "yt-dlp url2 -o video2.mp4"}"#,
            &progress_tx,
            None
        ).await
    });

    // Task 3: Анализировать данные
    let task3 = tokio::spawn(async move {
        registry.execute(
            "execute_command",
            r#"{"command": "python3 analyze.py"}"#,
            &progress_tx,
            None
        ).await
    });

    // Дождаться всех задач
    let (result1, result2, result3) = tokio::join!(
        task1,
        task2,
        task3
    );

    assert!(result1.is_ok());
    assert!(result2.is_ok());
    assert!(result3.is_ok());

    teardown(ctx).await;
}
```

---

## Лучшие практики

### 1. Разбивка сложных задач

Вместо одной большой задачи:

```
Плохо: "создай полную систему для работы с видео"
Хорошо:
  1. "создай endpoint для загрузки видео"
  2. "добавь обработку видео с ffmpeg"
  3. "создай endpoint для скачивания видео"
```

### 2. Предоставление контекста

```
Плохо: "исправь логин"
Хорошо: "исправь баг с 500 ошибкой при логине пользователя через /api/auth/login. Текущий код использует устаревший bcrypt"
```

### 3. Обработка ошибок

Всегда обрабатывайте ошибки соответствующим образом:

```rust
match result {
    Ok(output) => {
        // Успех
        println!("✅ {}", output);
    }
    Err(e) => {
        // Ошибка - покажите пользователю что делать
        let user_message = match e {
            e if e.contains("timeout") => "⏱️ Время истекло. Попробуйте снова.",
            e if e.contains("connection refused") => "🔗 Сервер недоступен. Проверьте соединение.",
            _ => "❌ Произошла ошибка. Попробуйте позже."
        };
        println!("{}", user_message);
    }
}
```

### 4. Прогресс индикация

Всегда отправляйте прогресс события:

```rust
// Перед выполнением
progress_tx.send(AgentEvent::ToolCall {
    name: tool_name,
    input: arguments,
    command_preview: Some("Short description"),
}).await;

// После выполнения
progress_tx.send(AgentEvent::ToolResult {
    name: tool_name,
    output: result,
}).await;
```

---

## Следующие шаги

- [ ] Изучить [integration_examples.rs](./integration_examples.rs) - Код примеры
- [ ] Изучить [testing/](../testing/) - Тестирование
- [ ] Перейти к [configuration/](../configuration/) - Конфигурация

---

**Связанные документы:**

- [examples/basic_usage.md](./basic_usage.md) - Базовые примеры
- [examples/integration_examples.rs](./integration_examples.rs) - Код примеры
- [architecture/flow.md](../architecture/flow.md) - Потоки выполнения
