# SubAgentSafetyHook

Обеспечивает безопасность саб-агентов через лимиты и блокировку инструментов.

**События:** `BeforeIteration`, `BeforeTool`

**Конфигурация:**
- `max_iterations` = 60
- `max_tokens` = 64,000
- `blocked_tools` - динамический набор из `BLOCKED_SUB_AGENT_TOOLS`

**Регистрация:**
- ❌ Main Agent
- ✅ Sub-Agent

## Назначение

Ограничивает саб-агентов, предотвращая:
- Бесконечное выполнение (лимит итераций)
- Переполнение контекста (лимит токенов)
- Рекурсивное делегирование (blocked tools)

## Конфигурация

```rust
// src/agent/hooks/sub_agent_safety.rs:10-17
pub struct SubAgentSafetyConfig {
    /// Максимальное количество итераций
    pub max_iterations: usize,

    /// Максимальное количество токенов в памяти
    pub max_tokens: usize,

    /// Заблокированные инструменты
    pub blocked_tools: HashSet<String>,
}
```

### Заблокированные инструменты

```rust
// src/agent/providers/delegation.rs:39
const BLOCKED_SUB_AGENT_TOOLS: &[&str] = &[
    "delegate_to_sub_agent",  // Запрещено рекурсивное делегирование
    "send_file_to_user",     // Запрещена отправка файлов пользователю
];
```

## Логика работы

```
BeforeIteration событие
    ↓
1. Проверка лимита итераций
    ├─ Достигнут? → Block
    └─ Не достигнут? → продолжить
         ↓
2. Проверка лимита токенов
    ├─ Достигнут? → Block
    └─ Не достигнут? → Continue

BeforeTool событие
    ↓
1. Проверка tool_name в blocked_tools
    ├─ Заблокирован? → Block
    └─ Разрешён? → Continue
```

## Реализация

```rust
// src/agent/hooks/sub_agent_safety.rs:32-70
impl Hook for SubAgentSafetyHook {
    fn name(&self) -> &'static str {
        "sub_agent_safety"
    }

    fn handle(&self, event: &HookEvent, context: &HookContext) -> HookResult {
        match event {
            HookEvent::BeforeIteration { iteration } => {
                if *iteration >= self.config.max_iterations {
                    return HookResult::Block {
                        reason: format!(
                            "Sub-agent iteration limit reached ({})",
                            self.config.max_iterations
                        ),
                    };
                }

                if context.token_count >= self.config.max_tokens {
                    return HookResult::Block {
                        reason: format!(
                            "Sub-agent token limit reached ({})",
                            self.config.max_tokens
                        ),
                    };
                }
            }
            HookEvent::BeforeTool { tool_name, .. } => {
                if self.config.blocked_tools.contains(tool_name) {
                    return HookResult::Block {
                        reason: format!("Tool '{tool_name}' is blocked for sub-agents"),
                    };
                }
            }
            _ => {}
        }

        HookResult::Continue
    }
}
```

## Конфигурация при создании саб-агента

```rust
// src/agent/providers/delegation.rs:164-171
fn create_sub_agent_runner(&self, blocked: HashSet<String>) -> AgentRunner {
    let mut runner = AgentRunner::new(self.llm_client.clone());
    runner.register_hook(Box::new(CompletionCheckHook::new()));
    runner.register_hook(Box::new(SubAgentSafetyHook::new(SubAgentSafetyConfig {
        max_iterations: SUB_AGENT_MAX_ITERATIONS,
        max_tokens: SUB_AGENT_MAX_TOKENS,
        blocked_tools: blocked,
    })));
    runner.register_hook(Box::new(SearchBudgetHook::new(get_agent_search_limit())));
    runner.register_hook(Box::new(TimeoutReportHook::new()));
    runner
}
```

## Примеры сценариев

### Сценарий 1: Достигнут лимит итераций
```
iteration = 60
max_iterations = 60

Результат: HookResult::Block {
    reason: "Sub-agent iteration limit reached (60)"
}
```

### Сценарий 2: Достигнут лимит токенов
```
token_count = 64,000
max_tokens = 64,000

Результат: HookResult::Block {
    reason: "Sub-agent token limit reached (64000)"
}
```

### Сценарий 3: Попытка делегировать
```
tool_name = "delegate_to_sub_agent"
blocked_tools = ["delegate_to_sub_agent", "send_file_to_user"]

Результат: HookResult::Block {
    reason: "Tool 'delegate_to_sub_agent' is blocked for sub-agents"
}
```

### Сценарий 4: Попытка отправить файл
```
tool_name = "send_file_to_user"
blocked_tools = ["delegate_to_sub_agent", "send_file_to_user"]

Результат: HookResult::Block {
    reason: "Tool 'send_file_to_user' is blocked for sub-agents"
}
```

### Сценарий 5: Разрешённый инструмент
```
tool_name = "execute_command"
blocked_tools = ["delegate_to_sub_agent", "send_file_to_user"]

Результат: HookResult::Continue
```

## Конструктор

```rust
// src/agent/hooks/sub_agent_safety.rs:24-29
impl SubAgentSafetyHook {
    #[must_use]
    pub fn new(config: SubAgentSafetyConfig) -> Self {
        Self { config }
    }
}
```

## Логирование

Блокировки логируются через `info` в `HookRegistry.execute()`:

```
[INFO] Hook blocking action: "Sub-agent iteration limit reached (60)"
[INFO] Hook blocking action: "Tool 'delegate_to_sub_agent' is blocked for sub-agents"
```

## Сравнение с Main Agent

| Параметр | Main Agent | Sub-Agent |
|-----------|-------------|-----------|
| `max_iterations` | 1000 | 60 |
| `max_tokens` | 200,000 | 64,000 |
| `SubAgentSafetyHook` | ❌ Нет | ✅ Да |
| Может делегировать | ✅ Да | ❌ Нет |
| Может отправлять файлы | ✅ Да | ❌ Нет |

## Безопасность

### Защита от бесконечного цикла
- Лимит итераций гарантирует остановку саб-агента

### Защита от перерасхода токенов
- Лимит токенов предотвращает переполнение контекста

### Защита от рекурсивного делегирования
- Блокировка `delegate_to_sub_agent` предотвращает вложенность

### Защита от прямого взаимодействия с пользователем
- Блокировка `send_file_to_user` обеспечивает изоляцию
