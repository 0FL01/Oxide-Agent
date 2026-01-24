# TimeoutReportHook

Генерирует структурированный отчёт при достижении мягкого тайм-аута агента.

**Событие:** `Timeout`

**Конфигурация:**
- Нет

**Регистрация:**
- ✅ Main Agent
- ✅ Sub-Agent

## Назначение

Обеспечивает graceful degradation при превышении времени выполнения. Вместо неудачного завершения генерирует JSON-отчёт с:
- Статусом и причиной завершения
- Статистикой выполнения
- Состоянием todo-задач
- Последними сообщениями агента

## Структура отчёта

```rust
// src/agent/hooks/timeout_report.rs:26-43
fn build_report(&self, context: &HookContext) -> String {
    let report = json!({
        "status": "timeout",
        "termination_reason": "Soft timeout reached",
        "note": "Agent did not finish the task within the time limit. Partial results included.",
        "stats": {
            "iterations": context.iteration,
            "continuation_count": context.continuation_count,
            "tokens_used": context.token_count,
            "max_tokens": context.max_tokens,
        },
        "todos": &context.todos,
        "recent_messages": summarize_recent_messages(context.memory),
    });

    serde_json::to_string_pretty(&report)
        .unwrap_or_else(|_| "{\"status\": \"timeout\"}".to_string())
}
```

### Поля отчёта

| Поле | Тип | Описание |
|------|------|----------|
| `status` | `"timeout"` | Статус завершения |
| `termination_reason` | `"Soft timeout reached"` | Причина завершения |
| `note` | string | Пояснение с частичными результатами |
| `stats.iterations` | number | Количество выполненных итераций |
| `stats.continuation_count` | number | Количество принудительных продолжений |
| `stats.tokens_used` | number | Использовано токенов |
| `stats.max_tokens` | number | Лимит токенов |
| `todos` | TodoList | Состояние todo-задач |
| `recent_messages` | array | Последние сообщения агента |

## Реализация

```rust
// src/agent/hooks/timeout_report.rs:78-89
impl Hook for TimeoutReportHook {
    fn name(&self) -> &'static str {
        "TimeoutReportHook"
    }

    fn handle(&self, event: &HookEvent, context: &HookContext) -> HookResult {
        if matches!(event, HookEvent::Timeout) {
            return HookResult::Finish(self.build_report(context));
        }
        HookResult::Continue
    }
}
```

## Суммирование сообщений

```rust
// src/agent/hooks/timeout_report.rs:46-67
const MAX_REPORT_MESSAGES: usize = 5;
const MAX_REPORT_CHARS: usize = 500;

fn summarize_recent_messages(memory: &AgentMemory) -> Vec<serde_json::Value> {
    let mut items = Vec::new();
    for message in memory.get_messages().iter().rev().take(MAX_REPORT_MESSAGES) {
        let content = crate::utils::truncate_str(&message.content, MAX_REPORT_CHARS);
        let reasoning = message
            .reasoning
            .as_ref()
            .map(|text| crate::utils::truncate_str(text, MAX_REPORT_CHARS));

        items.push(json!({
            "role": role_label(&message.role),
            "content": content,
            "reasoning": reasoning,
            "tool_name": message.tool_name.as_deref(),
        }));
    }
    items.reverse();
    items
}
```

## Константы отчёта

```rust
// src/agent/hooks/timeout_report.rs:46-47
const MAX_REPORT_MESSAGES: usize = 5;   // Макс. сообщений в отчёте
const MAX_REPORT_CHARS: usize = 500;     // Макс. символов на сообщение
```

## Пример отчёта

```json
{
  "status": "timeout",
  "termination_reason": "Soft timeout reached",
  "note": "Agent did not finish the task within the time limit. Partial results included.",
  "stats": {
    "iterations": 10,
    "continuation_count": 3,
    "tokens_used": 25000,
    "max_tokens": 200000
  },
  "todos": {
    "items": [
      {
        "description": "Analyze codebase",
        "status": "InProgress"
      },
      {
        "description": "Generate report",
        "status": "Pending"
      }
    ],
    "updated_at": "2025-01-17T10:30:00Z"
  },
  "recent_messages": [
    {
      "role": "user",
      "content": "Analyze the codebase",
      "reasoning": null,
      "tool_name": null
    },
    {
      "role": "assistant",
      "content": "I'll start by exploring the structure...",
      "reasoning": "The user wants me to analyze...",
      "tool_name": null
    },
    {
      "role": "tool",
      "content": "Found 15 files...",
      "reasoning": null,
      "tool_name": "execute_command"
    }
  ]
}
```

## Конструктор

```rust
// src/agent/hooks/timeout_report.rs:11-22
pub struct TimeoutReportHook;

impl TimeoutReportHook {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}
```

## Логирование

Финализация логируется через `info` в `HookRegistry.execute()`:

```
[INFO] Hook requested finish (report_len: 1234)
```

## Тайм-ауты

### Мягкий тайм-аут
Генерируется внутри агента при превышении `AGENT_TIMEOUT_SECS` (600 сек / 10 мин).

### Жёсткий тайм-аут
В `DelegationProvider` используется `timeout()` с дополнительным буфером:

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

## Конфигурация

```rust
// src/config.rs:685
pub const AGENT_TIMEOUT_SECS: u64 = 600;  // 10 минут
```

## Интеграция

```rust
// src/agent/runner/hooks.rs:178-202
pub(super) fn apply_timeout_hook(
    &mut self,
    ctx: &mut AgentRunnerContext<'_>,
    state: &RunState,
) -> anyhow::Result<Option<String>> {
    let hook_context = HookContext::new(
        &ctx.agent.memory().todos,
        ctx.agent.memory(),
        state.iteration,
        state.continuation_count,
        ctx.config.continuation_limit,
    )
    .with_sub_agent(ctx.config.is_sub_agent)
    .with_tokens(
        ctx.agent.memory().token_count(),
        ctx.agent.memory().max_tokens(),
    );

    let result = self
        .hook_registry
        .execute(&HookEvent::Timeout, &hook_context);

    self.apply_hook_result(result, ctx)
}
```

## Graceful Degradation

При достижении тайм-аута:
1. **Прерывается выполнение агента**
2. **Генерируется структурированный отчёт**
3. **Возвращаются частичные результаты** через `recent_messages`
4. **Сохраняется состояние** через `todos` и `stats`

Пользователь получает JSON с полной информацией о том, что было сделано и где остановлено выполнение.
