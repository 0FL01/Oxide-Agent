# Механизм делегирования

`delegate_to_sub_agent` инструмент для выполнения задач в изолированном саб-агенте.

## Инструмент delegate_to_sub_agent

### Определение

```rust
// src/agent/providers/delegation.rs:182-209
ToolDefinition {
    name: "delegate_to_sub_agent".to_string(),
    description: "Delegate rough work to lightweight sub-agent. \
    Pass a short, clear task and a list of allowed tools. \
    You can add additional context (e.g., a quote from a skill). \
    If the sub-agent doesn't finish, a partial report will be returned."
        .to_string(),
    parameters: json!({
        "type": "object",
        "properties": {
            "task": {
                "type": "string",
                "description": "Task for sub-agent"
            },
            "tools": {
                "type": "array",
                "description": "Whitelist of allowed tools",
                "items": {"type": "string"}
            },
            "context": {
                "type": "string",
                "description": "Additional context (optional)"
            }
        },
        "required": ["task", "tools"]
    }),
}
```

### Параметры

| Параметр | Тип | Обязательный | Описание |
|----------|------|--------------|----------|
| `task` | string | ✅ Да | Задача для саб-агента |
| `tools` | array[string] | ✅ Да | Whitelist разрешённых инструментов |
| `context` | string | ❌ Нет | Дополнительный контекст (опционально) |

## Примеры вызова

### Пример 1: Поиск файлов
```json
{
  "task": "Найди все .rs файлы в src/agent/",
  "tools": ["execute_command", "cat"]
}
```

### Пример 2: Клонирование и поиск
```json
{
  "task": "Клонируй репозиторий и найди все вызовы async fn",
  "tools": ["execute_command", "grep"],
  "context": "Ищем в src/ директории"
}
```

### Пример 3: Веб-поиск
```json
{
  "task": "Найди последние новости о Rust",
  "tools": ["web_search", "web_extract"]
}
```

## Реализация

```rust
// src/agent/providers/delegation.rs:216-329
async fn execute(
    &self,
    tool_name: &str,
    arguments: &str,
    progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    cancellation_token: Option<&tokio_util::sync::CancellationToken>,
) -> Result<String> {
    if tool_name != "delegate_to_sub_agent" {
        return Err(anyhow!("Unknown delegation tool: {tool_name}"));
    }

    let args: DelegateToSubAgentArgs = serde_json::from_str(arguments)?;
    if args.task.trim().is_empty() {
        return Err(anyhow!("Sub-agent task cannot be empty"));
    }
    if args.tools.is_empty() {
        return Err(anyhow!("Sub-agent tools whitelist cannot be empty"));
    }

    let task_id = format!("sub-{}", Uuid::new_v4());

    // Создание sub-session с родительским токеном отмены
    let mut sub_session = match cancellation_token {
        Some(parent_token) => {
            EphemeralSession::with_parent_token(SUB_AGENT_MAX_TOKENS, parent_token)
        }
        None => EphemeralSession::new(SUB_AGENT_MAX_TOKENS),
    };
    sub_session
        .memory_mut()
        .add_message(AgentMessage::user(task.as_str()));

    let todos_arc = Arc::new(Mutex::new(sub_session.memory().todos.clone()));
    let providers = self.build_sub_agent_providers(Arc::clone(&todos_arc), progress_tx);
    let available_tools: HashSet<String> = providers
        .iter()
        .flat_map(|provider| provider.tools())
        .map(|tool| tool.name)
        .collect();

    let allowed = self.filter_allowed_tools(requested_tools, &available_tools, &task_id)?;
    let registry = self.build_registry(&allowed, providers);
    let tools = registry.all_tools();

    let mut messages =
        AgentRunner::convert_memory_to_messages(sub_session.memory().get_messages());

    let system_prompt =
        create_sub_agent_system_prompt(task.as_str(), &tools, context.as_deref());

    let mut runner = self.create_sub_agent_runner(Self::blocked_tool_set());

    let mut ctx = AgentRunnerContext {
        task: task.as_str(),
        system_prompt: &system_prompt,
        tools: &tools,
        registry: &registry,
        progress_tx,
        todos_arc: &todos_arc,
        task_id: &task_id,
        messages: &mut messages,
        agent: &mut sub_session,
        skill_registry: None,
        config: {
            let (model_id, _, _) = self.settings.get_configured_sub_agent_model();
            AgentRunnerConfig::new(
                model_id,
                SUB_AGENT_MAX_ITERATIONS,
                AGENT_CONTINUATION_LIMIT,
                self.settings.get_sub_agent_timeout_secs(),
            )
            .with_sub_agent(true)
        },
    };

    info!(task_id = %task_id, "Running sub-agent delegation");

    let timeout_secs = self.settings.get_sub_agent_timeout_secs();
    let timeout_duration = Duration::from_secs(timeout_secs + 30);
    match timeout(timeout_duration, runner.run(&mut ctx)).await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(err)) => {
            warn!(task_id = %task_id, error = %err, "Sub-agent failed");
            Ok(build_sub_agent_report(SubAgentReportContext {
                task_id: &task_id,
                status: SubAgentReportStatus::Error,
                error: Some(err.to_string()),
                memory: sub_session.memory(),
                timeout_secs: self.settings.get_sub_agent_timeout_secs(),
            }))
        }
        Err(_) => {
            warn!(task_id = %task_id, "Sub-agent hard timed out");
            Ok(build_sub_agent_report(SubAgentReportContext {
                task_id: &task_id,
                status: SubAgentReportStatus::Timeout,
                error: Some(format!(
                    "Sub-agent hard timed out after {} seconds",
                    limit + 30
                )),
                memory: sub_session.memory(),
                timeout_secs: limit,
            }))
        }
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

    #[cfg(feature = "crawl4ai")]
    if let Ok(url) = std::env::var("CRAWL4AI_URL") {
        if !url.is_empty() {
            providers.push(Box::new(Crawl4aiProvider::new(&url)));
        }
    }

    providers
}
```

## Возврат результата

### Успешное завершение
```
"Результат выполнения задачи саб-агента"
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

## Проверка DelegationGuardHook

Перед выполнением делегирования срабатывает `DelegationGuardHook`:

```rust
// src/agent/hooks/delegation_guard.rs:56-91
impl Hook for DelegationGuardHook {
    fn handle(&self, event: &HookEvent, _context: &HookContext) -> HookResult {
        let HookEvent::BeforeTool {
            tool_name,
            arguments,
        } = event
        else {
            return HookResult::Continue;
        };

        if tool_name != "delegate_to_sub_agent" {
            return HookResult::Continue;
        }

        let task = match serde_json::from_str::<Value>(arguments) {
            Ok(json) => json.get("task").and_then(|v| v.as_str()).unwrap_or(""),
            Err(_) => return HookResult::Continue,
        };

        if let Some(keyword) = self.check_task(&task) {
            return HookResult::Block {
                reason: format!(
                    "⛔ Delegation Blocked: The task contains an analytical keyword ('{}'). \
                     Sub-agents are restricted to raw data retrieval...",
                    keyword
                ),
            };
        }

        HookResult::Continue
    }
}
```
