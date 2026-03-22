# Жизненный цикл саб-агента

Полный цикл создания, выполнения и завершения саб-агента.

## Пошаговый процесс

```
1. Main Agent вызывает delegate_to_sub_agent
   ├─ task: "Задача для саб-агента"
   ├─ tools: ["execute_command", "cat", ...]
   └─ context: "Дополнительный контекст (опционально)"
    ↓
2. DelegationGuardHook проверяет задачу
   ├─ Аналитические ключевые слова? → Block
   └─ Retrieval глаголы? → Continue
    ↓
3. Создание EphemeralSession
   ├─ AgentMemory с ограничением SUB_AGENT_MAX_TOKENS (64,000)
   ├─ parent.child_token() для отмены
   └─ started_at для elapsed_secs()
    ↓
4. Создание провайдеров саб-агента
   ├─ TodosProvider (изолированные todos)
   ├─ SandboxProvider (изолированная песочница)
   ├─ FileHosterProvider (общий хостинг)
   ├─ YtdlpProvider (изолированная песочница)
   ├─ TavilyProvider (если включён)
   └─ Crawl4aiProvider (если включён)
    ↓
5. Фильтрация инструментов
   ├─ Удаление BLOCKED_SUB_AGENT_TOOLS
   ├─ Пересечение с requested_tools
   └─ Проверка доступности в providers
    ↓
6. Создание SubAgentRunner с хуками
   ├─ CompletionCheckHook
   ├─ SubAgentSafetyHook (ограничения)
   ├─ SearchBudgetHook
   └─ TimeoutReportHook
    ↓
7. Создание AgentRunnerContext
   ├─ task: задача саб-агента
   ├─ system_prompt: системный промпт саб-агента
   ├─ tools: доступные инструменты
   ├─ registry: реестр инструментов
   ├─ config: конфигурация с лимитами
   └─ agent: EphemeralSession
    ↓
8. Запуск с тайм-аутом
   ├─ timeout_duration = sub_agent_timeout_secs + 30
   ├─ runner.run(&mut ctx)
   └─ timeout(timeout_duration, ...)
    ↓
9. Выполнение саб-агента
   ├─ apply_before_agent_hooks()
   ├─ run_loop() с хуками
   │   ├─ apply_timeout_hook() при достижении времени
   │   ├─ apply_before_iteration_hooks()
   │   ├─ apply_before_tool_hooks()
   │   └── after_agent_hook_result()
   └─ или TimeoutReportHook при тайм-ауте
    ↓
10. Возврат результата
    ├─ Success → JSON с результатом
    ├─ Error → JSON с ошибкой
    └─ Timeout → JSON с тайм-аутом и отчётом
```

## Пример выполнения

### Шаг 1: Вызов из Main Agent
```json
{
  "task": "Найти все .rs файлы в src/agent/",
  "tools": ["execute_command", "cat"],
  "context": null
}
```

### Шаг 2: Создание EphemeralSession
```rust
// src/agent/providers/delegation.rs:245-250
let mut sub_session = match cancellation_token {
    Some(parent_token) => {
        EphemeralSession::with_parent_token(SUB_AGENT_MAX_TOKENS, parent_token)
    }
    None => EphemeralSession::new(SUB_AGENT_MAX_TOKENS),
};
```

### Шаг 3: Фильтрация инструментов
```rust
// src/agent/providers/delegation.rs:136-160
fn filter_allowed_tools(
    &self,
    requested_tools: Vec<String>,
    available_tools: &HashSet<String>,
) -> Result<HashSet<String>> {
    let blocked = Self::blocked_tool_set();
    let requested: HashSet<String> = requested_tools.into_iter().collect();

    let allowed: HashSet<String> = requested
        .iter()
        .filter(|name| !blocked.contains(*name))
        .filter(|name| available_tools.contains(*name))
        .cloned()
        .collect();

    Ok(allowed)
}
```

### Шаг 4: Создание runner
```rust
// src/agent/providers/delegation.rs:273
let mut runner = self.create_sub_agent_runner(Self::blocked_tool_set());
```

### Шаг 5: Запуск с тайм-аутом
```rust
// src/agent/providers/delegation.rs:300-328
let timeout_secs = self.settings.get_sub_agent_timeout_secs();
let timeout_duration = Duration::from_secs(timeout_secs + 30);
match timeout(timeout_duration, runner.run(&mut ctx)).await {
    Ok(Ok(result)) => Ok(result),
    Ok(Err(err)) => Ok(build_sub_agent_report(...)),
    Err(_) => Ok(build_sub_agent_report(...)),
}
```

## Отмена саб-агента

При отмене родительского агента:
```rust
// src/agent/context.rs:55-62
pub fn with_parent_token(max_tokens: usize, parent: &CancellationToken) -> Self {
    Self {
        memory: AgentMemory::new(max_tokens),
        cancellation_token: parent.child_token(),  // При отмене родителя отменяется и саб-агент
        loaded_skills: HashSet::new(),
        skill_token_count: 0,
        started_at: std::time::Instant::now(),
    }
}
```

## Отчёт саб-агента

### Успешное завершение
```json
{
  "status": "success",
  "result": "Найдено 15 .rs файлов в src/agent/",
  "tokens": 12345
}
```

### Ошибка выполнения
```rust
// src/agent/providers/delegation.rs:390-402
enum SubAgentReportStatus {
    Timeout,
    Error,
}

fn build_sub_agent_report(ctx: SubAgentReportContext<'_>) -> String {
    let report = json!({
        "status": ctx.status.as_str(),
        "task_id": ctx.task_id,
        "error": ctx.error,
        "note": "Sub-agent did not finish the task. Use partial results below.",
        "timeout_secs": ctx.timeout_secs,
        "tokens": ctx.memory.token_count(),
        "todos": &ctx.memory.todos,
        "recent_messages": summarize_recent_messages(ctx.memory),
    });
    serde_json::to_string_pretty(&report).unwrap_or_else(|_| ...)
}
```

## Ограничения саб-агента

| Параметр | Значение | Описание |
|-----------|----------|----------|
| `max_iterations` | 60 | Лимит итераций |
| `max_tokens` | 64,000 | Лимит токенов |
| `timeout` | настроенный + 30 сек | Жёсткий тайм-аут |
| `blocked_tools` | delegate_to_sub_agent, send_file_to_user | Заблокированные инструменты |

## Логирование

```
[INFO] Running sub-agent delegation (task_id=sub-uuid)
[WARN] Sub-agent failed (task_id=sub-uuid, error=...)
[WARN] Sub-agent hard timed out (task_id=sub-uuid)
```
