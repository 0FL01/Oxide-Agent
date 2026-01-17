# Hook Registry

Реестр для регистрации и выполнения хуков.

## Hook Trait

Трейт для реализации хуков.

```rust
// src/agent/hooks/registry.rs:10-18
pub trait Hook: Send + Sync {
    /// Имя хука для логирования и отладки
    fn name(&self) -> &'static str;

    /// Обработать событие хука и вернуть результат
    ///
    /// Хуки должны возвращать `HookResult::Continue` если не нужно
    /// модифицировать поведение. Любой другой результат повлияет на выполнение.
    fn handle(&self, event: &HookEvent, context: &HookContext) -> HookResult;
}
```

## HookRegistry

Класс для управления несколькими хуками.

```rust
// src/agent/hooks/registry.rs:22-29
pub struct HookRegistry {
    hooks: Vec<Box<dyn Hook>>,
}
```

### Методы

#### new()
```rust
// src/agent/hooks/registry.rs:28-31
pub const fn new() -> Self {
    Self { hooks: Vec::new() }
}
```

Создаёт пустой реестр хуков.

#### register()
```rust
// src/agent/hooks/registry.rs:34-37
pub fn register(&mut self, hook: Box<dyn Hook>) {
    info!(hook = hook.name(), "Registered hook");
    self.hooks.push(hook);
}
```

Регистрирует новый хук. Хуки выполняются в порядке регистрации.

#### execute()
```rust
// src/agent/hooks/registry.rs:39-87
pub fn execute(&self, event: &HookEvent, context: &HookContext) -> HookResult {
    for hook in &self.hooks {
        let result = hook.handle(event, context);

        match &result {
            HookResult::Continue => {
                debug!(hook = hook.name(), "Hook returned Continue");
            }
            HookResult::InjectContext(ctx) => {
                debug!(
                    hook = hook.name(),
                    context_len = ctx.len(),
                    "Hook injecting context"
                );
                return result;
            }
            HookResult::ForceIteration { reason, .. } => {
                info!(
                    hook = hook.name(),
                    reason = %reason,
                    "Hook forcing iteration"
                );
                return result;
            }
            HookResult::Block { reason } => {
                info!(
                    hook = hook.name(),
                    reason = %reason,
                    "Hook blocking action"
                );
                return result;
            }
            HookResult::Finish(report) => {
                info!(
                    hook = hook.name(),
                    report_len = report.len(),
                    "Hook requested finish"
                );
                return result;
            }
        }
    }

    HookResult::Continue
}
```

Выполняет все хуки для события. Хуки выполняются в порядке регистрации. Первый не-`Continue` результат останавливает цепочку.

#### is_empty()
```rust
// src/agent/hooks/registry.rs:90-93
pub fn is_empty(&self) -> bool {
    self.hooks.is_empty()
}
```

Проверяет, есть ли зарегистрированные хуки.

#### len()
```rust
// src/agent/hooks/registry.rs:96-99
pub fn len(&self) -> usize {
    self.hooks.len()
}
```

Возвращает количество зарегистрированных хуков.

## Порядок выполнения хуков

Хуки выполняются в порядке регистрации. Цепочка останавливается при первом не-`Continue` результате.

```
Hook 1 → Continue
    ↓
Hook 2 → ForceIteration { reason: "..." }
    ↓
[Цепочка останавливается, Hook 3 не выполняется]
```

## Пример регистрации хуков

### В AgentRunner

```rust
// src/agent/executor.rs:52-57
let mut runner = AgentRunner::new(llm_client.clone());
runner.register_hook(Box::new(CompletionCheckHook::new()));
runner.register_hook(Box::new(WorkloadDistributorHook::new()));
runner.register_hook(Box::new(DelegationGuardHook::new()));
runner.register_hook(Box::new(SearchBudgetHook::new(get_agent_search_limit())));
runner.register_hook(Box::new(TimeoutReportHook::new()));
```

## Интеграция в runner

Методы `AgentRunner` в `src/agent/runner/hooks.rs` используют `HookRegistry.execute()`:

```rust
// src/agent/runner/hooks.rs:35-42
let result = self.hook_registry.execute(
    &HookEvent::BeforeAgent {
        prompt: ctx.task.to_string(),
    },
    &hook_context,
);
```

## Логирование

Каждый результат хука логируется с соответствующим уровнем:

| Результат | Уровень | Пример сообщения |
|-----------|----------|------------------|
| `Continue` | `debug` | "Hook returned Continue" |
| `InjectContext` | `debug` | "Hook injecting context" |
| `ForceIteration` | `info` | "Hook forcing iteration" |
| `Block` | `info` | "Hook blocking action" |
| `Finish` | `info` | "Hook requested finish" |

## Пример реализации хука

```rust
// src/agent/hooks/completion.rs:31-35
impl Hook for CompletionCheckHook {
    fn name(&self) -> &'static str {
        "completion_check"
    }

    fn handle(&self, event: &HookEvent, context: &HookContext) -> HookResult {
        // Логика хука
        HookResult::Continue
    }
}
```
