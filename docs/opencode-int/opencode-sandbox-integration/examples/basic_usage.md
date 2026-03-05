# Basic Usage

> **Базовые примеры использования интеграции**
>
> 📁 **Раздел:** Examples
> 🎯 **Цель:** Быстрое начало работы

---

## 📋 Оглавление

- [Пример 1: Простая Opencode задача](#пример-1-простая-opencode-задача)
- [Пример 2: Простой Sandbox вызов](#пример-2-простой-sandbox-вызов)
- [Пример 3: Комбинированная задача](#пример-3-комбинированная-задача)

---

## Пример 1: Простая Opencode задача

### Сценарий: Список файлов в проекте

**Запрос пользователя:** "покажи мне структуру проекта"

### Шаг 1: LLM принимает решение

```
LLM Thinking:
  - Запрос: показать структуру проекта
  - Требуется: анализ кодовой базы
  - Инструмент: opencode

Decision:
  Использовать Opencode tool
```

### Шаг 2: Формирование вызова

```rust
// В LLM Agent
let tool_call = serde_json::json!({
    "tool": "opencode",
    "task": "list files in current directory"
});

let arguments = tool_call["task"].as_str().unwrap();
```

### Шаг 3: Выполнение

```rust
// ToolRegistry.execute()
let result = registry.execute(
    "opencode",
    r#"{"task": "list files in current directory"}"#,
    &progress_tx,
    None
).await?;

// Result:
// "📁 Структура проекта:
//  src/
//    ├── api/
//    │   └── routes.ts
//    ├── middleware/
//    │   └── logger.ts
//    ├── server.ts
//    └── index.ts
//  tests/
//    ├── api.test.ts
//    └── utils.test.ts
//  package.json
//  tsconfig.json
//  README.md"
```

### Шаг 4: Результат пользователю

```
LLM Response:
  "📁 Структура вашего проекта:

   src/
   ├── api/
   │   └── routes.ts
   ├── middleware/
   │   └── logger.ts
   ├── server.ts
   └── index.ts
   tests/
   ├── api.test.ts
   └── utils.test.ts
   package.json
   tsconfig.json
   README.md

   Всего: 8 TypeScript файлов, 3 тестовых файла"
```

---

## Пример 2: Простой Sandbox вызов

### Сценарий: Скачивание файла

**Запрос пользователя:** "скачай этот файл https://example.com/data.json"

### Шаг 1: LLM принимает решение

```
LLM Thinking:
  - Запрос: скачать файл с URL
  - Требуется: скачивание (yt-dlp или curl)
  - Инструмент: execute_command (sandbox)

Decision:
  Использовать Sandbox tool
```

### Шаг 2: Формирование вызова

```rust
// В LLM Agent
let tool_call = serde_json::json!({
    "tool": "execute_command",
    "command": "curl -O https://example.com/data.json"
});

let command = tool_call["command"].as_str().unwrap();
```

### Шаг 3: Выполнение

```rust
// ToolRegistry.execute()
let result = registry.execute(
    "execute_command",
    r#"{"command": "curl -O https://example.com/data.json"}"#,
    &progress_tx,
    None
).await?;

// Result:
// "  % Total    % Received % Xferd  Average Speed   Time    Dload  Upload   Total   Spent    Left  Speed
// 100  1024  100  1024    0     0   1234      0 --:--:-- --:--:--  1234
// Downloaded: data.json"
```

### Шаг 4: Результат пользователю

```
LLM Response:
  "✅ Файл успешно скачан!

   Файл: data.json
   Размер: 1024 bytes
   Местоположение: /workspace/data.json"
```

---

## Пример 3: Комбинированная задача

### Сценарий: Скачать и проанализировать данные

**Запрос пользователя:** "скачай данные с этого API и проанализируй их"

### Шаг 1: LLM принимает решение

```
LLM Thinking:
  - Запрос: скачать + проанализировать данные
  - Требуется:
    1. Скачивание (sandbox)
    2. Анализ (Python в sandbox)
  - Инструменты: execute_command (sandbox)

Decision:
  Использовать Sandbox tool (2 вызова)
```

### Шаг 2: Первый вызов - скачивание

```rust
// Шаг 1: Скачивание
let result1 = registry.execute(
    "execute_command",
    r#"{"command": "curl -O https://api.example.com/data.json"}"#,
    &progress_tx,
    None
).await?;

// Result:
// "Downloaded: data.json"
```

### Шаг 3: Второй вызов - анализ

```rust
// Шаг 2: Анализ Python скриптом
let result2 = registry.execute(
    "execute_command",
    r#"{"command": "python3 -c \"import json; data=json.load(open('data.json')); print(f'Items: {len(data)}'); print(f'First item: {data[0]}')\""}"#,
    &progress_tx,
    None
).await?;

// Result:
// "Items: 42
// First item: {\"id\": 1, \"name\": \"Item 1\", \"value\": 100}"
```

### Шаг 4: Результат пользователю

```
LLM Response:
  "✅ Данные скачаны и проанализированы!

   📊 Анализ:
   - Файл: data.json
   - Количество элементов: 42
   - Первый элемент:
     * ID: 1
     * Name: Item 1
     * Value: 100

   Файл доступен в песочнице: /workspace/data.json"
```

---

## Общие паттерны

### Паттерн 1: Проверка результата

```rust
// Всегда проверяйте результат выполнения
match result {
    Ok(output) => {
        // Успех
        println!("✅ Выполнено успешно:\n{}", output);
    }
    Err(e) => {
        // Ошибка
        eprintln!("❌ Ошибка: {}", e);
        // Попробуйте альтернативный подход
    }
}
```

### Паттерн 2: Мониторинг прогресса

```rust
// Создайте канал для событий прогресса
let (progress_tx, mut progress_rx) = mpsc::channel(100);

// Запустите монитор в фоне
let monitor = tokio::spawn(async move {
    while let Some(event) = progress_rx.recv().await {
        match event {
            AgentEvent::ToolCall { name, .. } => {
                println!("🔧 Выполняется: {}", name);
            }
            AgentEvent::ToolResult { name, .. } => {
                println!("✅ Завершено: {}", name);
            }
            _ => {}
        }
    }
});

// Выполните задачу
let result = registry.execute(..., &progress_tx, None).await;

// Дождитесь завершения монитора
monitor.await.ok();
```

### Паттерн 3: Обработка ошибок

```rust
// Обрабатывайте ошибки соответствующим образом
let result = registry.execute("opencode", args, &progress_tx, None).await;

match result {
    Ok(output) => {
        // Успех
        Ok(output)
    }
    Err(e) => {
        // Ошибка - покажите пользователю
        let user_message = match e {
            e if e.contains("connection refused") => {
                "❌ Opencode сервер недоступен. Пожалуйста, запустите: opencode serve".to_string()
            }
            e if e.contains("timeout") => {
                "❌ Время ожидания истекло. Попробуйте снова.".to_string()
            }
            _ => {
                format!("❌ Ошибка: {}", e)
            }
        };

        Err(user_message)
    }
}
```

---

## Следующие шаги

- [ ] Изучить [advanced_workflow.md](./advanced_workflow.md) - Сложные сценарии
- [ ] Изучить [integration_examples.rs](./integration_examples.rs) - Код примеры
- [ ] Перейти к [testing/](../testing/) - Тестирование

---

**Связанные документы:**

- [examples/advanced_workflow.md](./advanced_workflow.md) - Сложные сценарии
- [examples/integration_examples.rs](./integration_examples.rs) - Код примеры
- [architecture/flow.md](../architecture/flow.md) - Потоки выполнения
