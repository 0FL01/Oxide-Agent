# Механизм делегирования

Асинхронные инструменты `spawn_sub_agents`, `wait_sub_agents` и `cancel_sub_agents` запускают задачи в изолированных саб-агентах.

## Инструмент spawn_sub_agents

### Определение

```rust
// src/agent/providers/delegation.rs:182-209
ToolDefinition {
    name: "spawn_sub_agents".to_string(),
    description: "Spawn up to five lightweight sub-agents and return their ids immediately. \
    Use wait_sub_agents only when results are needed."
        .to_string(),
    parameters: json!({
        "type": "object",
        "properties": {
            "tasks": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "task": {"type": "string"},
                        "tools": {"type": "array", "items": {"type": "string"}},
                        "context": {"type": "string"}
                    },
                    "required": ["task", "tools"]
                }
            }
        },
        "required": ["tasks"]
    }),
}
```

### Параметры

| Параметр | Тип | Обязательный | Описание |
|----------|------|--------------|----------|
| `tasks` | array[object] | ✅ Да | До пяти задач для саб-агентов |
| `tasks[].task` | string | ✅ Да | Задача для саб-агента |
| `tasks[].tools` | array[string] | ✅ Да | Whitelist разрешённых инструментов |
| `tasks[].context` | string | ❌ Нет | Дополнительный контекст |

## Примеры вызова

### Пример 1: Поиск файлов
```json
{
  "tasks": [
    {
      "task": "Найди все .rs файлы в src/agent/",
      "tools": ["execute_command", "cat"]
    }
  ]
}
```

### Пример 2: Клонирование и поиск
```json
{
  "tasks": [
    {
      "task": "Клонируй репозиторий и найди все вызовы async fn",
      "tools": ["execute_command", "grep"],
      "context": "Ищем в src/ директории"
    }
  ]
}
```

### Пример 3: Веб-поиск
```json
{
  "tasks": [
    {
      "task": "Найди последние новости о Rust",
      "tools": ["web_search", "web_extract"]
    }
  ]
}
```

## Реализация

```rust
// src/agent/providers/delegation.rs
async fn execute(
    &self,
    tool_name: &str,
    arguments: &str,
    progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    cancellation_token: Option<&tokio_util::sync::CancellationToken>,
) -> Result<String> {
    match tool_name {
        "spawn_sub_agents" => {
            // Validate up to five tasks, reserve active slots, prepare isolated
            // EphemeralSession contexts, then tokio::spawn background jobs.
            // The JSON response contains started job ids immediately.
        }
        "wait_sub_agents" => {
            // Poll or wait for selected job ids. Terminal results include
            // success/error/timeout/cancelled reports from the job store.
        }
        "cancel_sub_agents" => {
            // Cancel selected jobs or all run-scoped jobs.
        }
        _ => return Err(anyhow!("Unknown delegation tool: {tool_name}")),
    }
}
```

## Фильтрация инструментов

```rust
// src/agent/providers/delegation.rs:131-160
fn filter_allowed_tools(
    &self,
    requested_tools: Vec<String>,
    available_tools: &HashSet<String>,
    task_id: &str,
) -> Result<HashSet<String>> {
    let blocked = Self::blocked_tool_set();
    let requested: HashSet<String> = requested_tools.into_iter().collect();

    let allowed: HashSet<String> = requested
        .iter()
        .filter(|name| !blocked.contains(*name))
        .filter(|name| available_tools.contains(*name))
        .cloned()
        .collect();

    if allowed.is_empty() {
        warn!(
            task_id = %task_id,
            requested = ?requested,
            available = ?available_tools,
            "No allowed tools left after filtering"
        );
        return Err(anyhow!(
            "No allowed tools left after filtering (blocked or unavailable). Requested: {:?}, Available: {:?}",
            requested,
            available_tools
        ));
    }
    Ok(allowed)
}
```

## RestrictedToolProvider

Обёртка над провайдерами, которая фильтрует инструменты:

```rust
// src/agent/providers/delegation.rs:340-388
struct RestrictedToolProvider {
    inner: Box<dyn ToolProvider>,
    allowed_tools: Arc<HashSet<String>>,
}

#[async_trait]
impl ToolProvider for RestrictedToolProvider {
    fn tools(&self) -> Vec<ToolDefinition> {
        self.inner
            .tools()
            .into_iter()
            .filter(|tool| self.allowed_tools.contains(&tool.name))
            .collect()
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        self.allowed_tools.contains(tool_name) && self.inner.can_handle(tool_name)
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        if !self.allowed_tools.contains(tool_name) {
            warn!(tool_name = %tool_name, "Tool blocked by delegation whitelist");
            return Err(anyhow!("Tool '{tool_name}' is not allowed for sub-agent"));
        }

        self.inner
            .execute(tool_name, arguments, progress_tx, cancellation_token)
            .await
    }
}
```

## Провайдеры саб-агента

```rust
// src/agent/providers/delegation.rs:72-113
fn build_sub_agent_providers(
    &self,
    todos_arc: Arc<Mutex<TodoList>>,
    progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
) -> Vec<Box<dyn ToolProvider>> {
    let sandbox_provider = if let Some(tx) = progress_tx {
        SandboxProvider::new(self.user_id).with_progress_tx(tx.clone())
    } else {
        SandboxProvider::new(self.user_id)
    };
    let ytdlp_provider = if let Some(tx) = progress_tx {
        YtdlpProvider::new(self.user_id).with_progress_tx(tx.clone())
    } else {
        YtdlpProvider::new(self.user_id)
    };

    let mut providers: Vec<Box<dyn ToolProvider>> = vec![
        Box::new(TodosProvider::new(todos_arc)),
        Box::new(sandbox_provider),
        Box::new(FileHosterProvider::new(self.user_id)),
        Box::new(ytdlp_provider),
    ];

    #[cfg(feature = "tavily")]
    if let Ok(tavily_key) = std::env::var("TAVILY_API_KEY") {
        if !tavily_key.is_empty() {
            if let Ok(provider) = TavilyProvider::new(&tavily_key) {
                providers.push(Box::new(provider));
            }
        }
    }

    providers.push(Box::new(WebFetchMdProvider::new()));

    providers
}
```

## Ожидание результата

### Успешное завершение
```json
{
  "results": [
    {
      "id": "sub-uuid",
      "status": "completed",
      "output": "Результат выполнения задачи саб-агента"
    }
  ]
}
```

### Ошибка или тайм-аут
```json
{
  "status": "error" | "timeout",
  "task_id": "sub-uuid",
  "error": "сообщение об ошибке",
  "note": "Sub-agent did not finish the task. Use partial results below.",
  "timeout_secs": 120,
  "tokens": 12345,
  "todos": {
    "items": [...],
    "updated_at": "..."
  },
  "recent_messages": [...]
}
```

## Проверка политики sub-agent

Перед выполнением tool call у sub-agent остается `SubAgentSafetyHook`: он блокирует рекурсивное делегирование и инструменты, которые не должны выполняться из ephemeral worker-сессии.
