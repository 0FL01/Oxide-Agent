# Практические примеры

Примеры работы с системой хуков Oxide Agent.

## 1. Создание кастомного хука

```rust
use crate::agent::hooks::{Hook, HookContext, HookEvent, HookResult};

pub struct CustomHook;

impl Hook for CustomHook {
    fn name(&self) -> &'static str {
        "custom_hook"
    }

    fn handle(&self, event: &HookEvent, context: &HookContext) -> HookResult {
        match event {
            HookEvent::BeforeTool { tool_name, .. } => {
                if tool_name == "dangerous_operation" {
                    return HookResult::Block {
                        reason: "Operation blocked by custom hook".to_string(),
                    };
                }
            }
            _ => {}
        }
        HookResult::Continue
    }
}
```

Регистрация:

```rust
runner.register_hook(Box::new(CustomHook));
```

## 2. Последовательность выполнения хуков

```
User Request: "Исследуй и сравни репозитории"

1. BeforeAgent
   └─ CompletionCheckHook: Continue (не AfterAgent)
    ↓
2. LLM Call + Tool Calls
    ↓
3. BeforeTool (execute_command)
   ├─ CompletionCheckHook: Continue
   └─ SubAgentSafetyHook: Continue (не sub-agent)
    ↓
4. Tool Execution
    ↓
5. AfterTool
   └─ логирование/метрики
    ↓
6. LLM Call с результатами
    ↓
7. BeforeTool (spawn_sub_agents)
   ├─ CompletionCheckHook: Continue
   └─ SubAgentSafetyHook: Continue (не sub-agent)
```

## 3. Отладка хуков

### Включение логирования

```bash
RUST_LOG=debug ./target/release/bot
```

### Фильтрация логов хука

```bash
RUST_LOG=agent::hooks=debug ./target/release/bot
```

### Пример лога

```
[INFO] Registered hook: completion_check
[INFO] Registered hook: search_budget
[INFO] Registered hook: timeout_report

[DEBUG] Hook returned Continue (hook=completion_check)
[INFO] Hook forcing iteration (hook=completion_check, reason="Not all tasks are completed...")
```

## 4. Инъекция контекста через хук

```rust
use crate::agent::hooks::{Hook, HookContext, HookEvent, HookResult};

pub struct ContextInjectorHook {
    context: String,
}

impl ContextInjectorHook {
    pub fn new(context: String) -> Self {
        Self { context }
    }
}

impl Hook for ContextInjectorHook {
    fn name(&self) -> &'static str {
        "context_injector"
    }

    fn handle(&self, event: &HookEvent, _context: &HookContext) -> HookResult {
        match event {
            HookEvent::BeforeAgent { .. } => {
                HookResult::InjectContext(self.context.clone())
            }
            _ => HookResult::Continue,
        }
    }
}
```

## 5. Принудительная итерация

```rust
use crate::agent::hooks::{Hook, HookContext, HookEvent, HookResult};

pub struct ForceIterationHook;

impl Hook for ForceIterationHook {
    fn name(&self) -> &'static str {
        "force_iteration"
    }

    fn handle(&self, event: &HookEvent, context: &HookContext) -> HookResult {
        match event {
            HookEvent::AfterAgent { response } => {
                if response.len() < 100 {
                    return HookResult::ForceIteration {
                        reason: "Response too short, continue working".to_string(),
                        context: None,
                    };
                }
            }
            _ => {}
        }
        HookResult::Continue
    }
}
```

## 6. Блокировка инструмента

```rust
use crate::agent::hooks::{Hook, HookContext, HookEvent, HookResult};

pub struct ToolBlockerHook {
    blocked_tools: Vec<String>,
}

impl ToolBlockerHook {
    pub fn new(blocked_tools: Vec<String>) -> Self {
        Self { blocked_tools }
    }
}

impl Hook for ToolBlockerHook {
    fn name(&self) -> &'static str {
        "tool_blocker"
    }

    fn handle(&self, event: &HookEvent, _context: &HookContext) -> HookResult {
        match event {
            HookEvent::BeforeTool { tool_name, .. } => {
                if self.blocked_tools.contains(tool_name) {
                    return HookResult::Block {
                        reason: format!("Tool '{}' is blocked", tool_name),
                    };
                }
            }
            _ => {}
        }
        HookResult::Continue
    }
}
```

## 7. Отслеживание состояния хука

```rust
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct MetricsHook {
    call_count: AtomicUsize,
    block_count: AtomicUsize,
}

impl MetricsHook {
    pub fn new() -> Self {
        Self {
            call_count: AtomicUsize::new(0),
            block_count: AtomicUsize::new(0),
        }
    }

    pub fn stats(&self) -> (usize, usize) {
        (
            self.call_count.load(Ordering::Relaxed),
            self.block_count.load(Ordering::Relaxed),
        )
    }
}

impl Hook for MetricsHook {
    fn name(&self) -> &'static str {
        "metrics"
    }

    fn handle(&self, event: &HookEvent, _context: &HookContext) -> HookResult {
        self.call_count.fetch_add(1, Ordering::Relaxed);

        match event {
            HookEvent::BeforeTool { tool_name, .. } => {
                if tool_name == "blocked_tool" {
                    self.block_count.fetch_add(1, Ordering::Relaxed);
                    return HookResult::Block {
                        reason: "Tool blocked".to_string(),
                    };
                }
            }
            _ => {}
        }
        HookResult::Continue
    }
}
```

## 8. Проверка хуков в тестах

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::hooks::{HookContext, HookEvent, HookResult};
    use crate::agent::memory::AgentMemory;
    use crate::agent::providers::TodoList;

    #[test]
    fn test_custom_hook() {
        let hook = CustomHook;
        let todos = TodoList::new();
        let memory = AgentMemory::new(1000);
        let context = HookContext::new(&todos, &memory, 0, 0, 10);

        let event = HookEvent::BeforeTool {
            tool_name: "safe_operation".to_string(),
            arguments: "{}".to_string(),
        };

        let result = hook.handle(&event, &context);
        assert!(matches!(result, HookResult::Continue));
    }
}
```

## 9. Комбинация хуков

```
Регистрация:
├── CompletionCheckHook     (1-й в цепочке)
├── SearchBudgetHook      (2-й в цепочке)
├── CustomHook           (3-й в цепочке)
└── TimeoutReportHook     (4-й в цепочке)

Выполнение BeforeTool:
1. CompletionCheckHook → Continue
2. SearchBudgetHook → Block
   → [Цепочка останавливается, CustomHook и TimeoutReportHook не выполняются]
```

## 10. Отмена хука по условию

```rust
pub struct ConditionalHook {
    enabled: AtomicBool,
}

impl ConditionalHook {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled: AtomicBool::new(enabled),
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }
}

impl Hook for ConditionalHook {
    fn name(&self) -> &'static str {
        "conditional"
    }

    fn handle(&self, _event: &HookEvent, _context: &HookContext) -> HookResult {
        if !self.enabled.load(Ordering::Relaxed) {
            return HookResult::Continue;
        }

        // Логика хука когда включён
        HookResult::Continue
    }
}
```
