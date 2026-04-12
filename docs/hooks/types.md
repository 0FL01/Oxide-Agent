# Типы системы хуков

## HookEvent

События жизненного цикла агента, на которые подписываются хуки.

```rust
// src/agent/hooks/types.rs:9-45
pub enum HookEvent {
    /// Перед обработкой пользовательского промпта
    BeforeAgent {
        prompt: String,
    },

    /// Перед началом новой итерации
    BeforeIteration {
        iteration: usize,
    },

    /// После ответа агента (когда нет tool calls)
    AfterAgent {
        response: String,
    },

    /// Перед выполнением инструмента
    BeforeTool {
        tool_name: String,
        arguments: String,
    },

    /// После выполнения инструмента
    AfterTool {
        tool_name: String,
        result: String,
    },

    /// Достигнут мягкий тайм-аут
    Timeout,
}
```

### Использование по хукам

| Событие | Хуки |
|---------|-------|
| `BeforeAgent` | `WorkloadDistributorHook` |
| `BeforeIteration` | `SubAgentSafetyHook` |
| `AfterAgent` | `CompletionCheckHook` |
| `BeforeTool` | `DelegationGuardHook`, `WorkloadDistributorHook`, `SubAgentSafetyHook`, `SearchBudgetHook` |
| `AfterTool` | (логирование/метрики) |
| `Timeout` | `TimeoutReportHook` |

---

## HookResult

Результат выполнения хука, влияющий на дальнейшее выполнение.

```rust
// src/agent/hooks/types.rs:48-72
pub enum HookResult {
    /// Продолжить нормальное выполнение
    #[default]
    Continue,

    /// Инъектировать контекст в следующий LLM запрос
    InjectContext(String),

    /// Принудить агента к следующей итерации
    ForceIteration {
        reason: String,
        context: Option<String>,
    },

    /// Заблокировать действие (для BeforeTool)
    Block {
        reason: String,
    },

    /// Завершить выполнение с результатом
    Finish(String),
}
```

### Варианты использования

#### Continue
```rust
// src/agent/hooks/completion.rs:38-40
let HookEvent::AfterAgent { response: _ } = event else {
    return HookResult::Continue;
};
```

#### InjectContext
```rust
// src/agent/hooks/workload.rs:119-130
if self.is_complex_prompt(prompt) {
    return HookResult::InjectContext(
        "[SYSTEM NOTICE: High Complexity Detected]\n\
        You must SPLIT your workflow to handle this request efficiently...".to_string(),
    );
}
```

#### ForceIteration
```rust
// src/agent/hooks/completion.rs:91-94
HookResult::ForceIteration {
    reason: format!("Not all tasks are completed ({}/{} done, {} remaining)..."),
    context: Some(todo_context),
}
```

#### Block
```rust
// src/agent/hooks/delegation_guard.rs:80-87
return HookResult::Block {
    reason: format!(
        "⛔ Delegation Blocked: The task contains an analytical keyword ('{}'). \
         Sub-agents are restricted to raw data retrieval...",
        keyword
    ),
};
```

#### Finish
```rust
// src/agent/hooks/timeout_report.rs:84-85
if matches!(event, HookEvent::Timeout) {
    return HookResult::Finish(self.build_report(context));
}
```

---

## HookContext

Контекст, предоставляемый хукам во время выполнения.

```rust
// src/agent/hooks/types.rs:74-92
pub struct HookContext<'a> {
    /// Текущий список todo
    pub todos: &'a TodoList,

    /// Память агента
    pub memory: &'a crate::agent::memory::AgentMemory,

    /// Номер текущей итерации
    pub iteration: usize,

    /// Количество принудительных продолжений
    pub continuation_count: usize,

    /// Максимальное разрешённых продолжений
    pub max_continuations: usize,

    /// Текущее количество токенов в памяти
    pub token_count: usize,

    /// Максимальное разрешённых токенов
    pub max_tokens: usize,

    /// Это саб-агент?
    pub is_sub_agent: bool,
}
```

### Методы построения

```rust
// src/agent/hooks/types.rs:97-129
pub const fn new(
    todos: &'a TodoList,
    memory: &'a crate::agent::memory::AgentMemory,
    iteration: usize,
    continuation_count: usize,
    max_continuations: usize,
) -> Self

pub const fn with_sub_agent(mut self, is_sub_agent: bool) -> Self

pub const fn with_tokens(mut self, token_count: usize, max_tokens: usize) -> Self

pub const fn at_continuation_limit(&self) -> bool
```

### Пример создания контекста

```rust
// src/agent/runner/hooks.rs:22-33
let hook_context = HookContext::new(
    &ctx.agent.memory().todos,
    ctx.agent.memory(),
    0,
    0,
    ctx.config.continuation_limit,
)
.with_sub_agent(ctx.config.is_sub_agent)
.with_tokens(
    ctx.agent.memory().token_count(),
    ctx.agent.memory().max_tokens(),
);
```

### Использование полей в хуках

#### Проверка лимита продолжений
```rust
// src/agent/hooks/completion.rs:43-49
if context.at_continuation_limit() {
    info!(
        continuation_count = context.continuation_count,
        max = context.max_continuations,
        "Continuation limit reached, allowing completion"
    );
    return HookResult::Continue;
}
```

#### Проверка типа агента
```rust
// src/agent/hooks/workload.rs:140-142
if context.is_sub_agent {
    return HookResult::Continue;
}
```

#### Доступ к todo-задачам
```rust
// src/agent/hooks/completion.rs:74-76
let pending = context.todos.pending_count();
let total = context.todos.items.len();
let completed = context.todos.completed_count();
```
