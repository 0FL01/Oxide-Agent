# CompletionCheckHook

Гарантирует завершение всех todo-задач перед тем как агент может закончить работу.

**Событие:** `AfterAgent`

**Конфигурация:**
- `AGENT_CONTINUATION_LIMIT` = 10 (макс. принудительных продолжений)

**Регистрация:**
- ✅ Main Agent
- ✅ Sub-Agent

## Назначение

LLM модели по своей природе "ленивы" и часто пытаются завершить задачу раньше времени для экономии токенов. Этот хук **обязателен** для гарантии выполнения работы.

> **КРИТИЧЕСКИ КОММЕНТАРИЙ в коде** (строки 57-62):
> ```
> // CRITICAL: LLMs are inherently "lazy" and will often try to finish early
> // to save tokens or effort, even if tasks remain.
> // This deterministic check is MANDATORY to guarantee work completion.
> ```

## Логика работы

```
AfterAgent событие
    ↓
1. Проверка лимита продолжений
    ├─ Достигнут лимит? → Continue (разрешить завершение)
    └─ Не достигнут? → продолжить проверку
         ↓
2. Проверка пустоты списка todos
    ├─ Список пуст? → Continue (разрешить завершение)
    └─ Список не пуст? → продолжить проверку
         ↓
3. Проверка завершённости всех todos
    ├─ Все завершены? → Continue (разрешить завершение)
    └─ Не все завершены? → ForceIteration (принудить к следующей итерации)
```

## Реализация

```rust
// src/agent/hooks/completion.rs:31-95
impl Hook for CompletionCheckHook {
    fn name(&self) -> &'static str {
        "completion_check"
    }

    fn handle(&self, event: &HookEvent, context: &HookContext) -> HookResult {
        // Только для AfterAgent событий
        let HookEvent::AfterAgent { response: _ } = event else {
            return HookResult::Continue;
        };

        // Проверка лимита продолжений
        if context.at_continuation_limit() {
            info!(
                continuation_count = context.continuation_count,
                max = context.max_continuations,
                "Continuation limit reached, allowing completion"
            );
            return HookResult::Continue;
        }

        // Если нет todos, разрешить завершение
        if context.todos.items.is_empty() {
            return HookResult::Continue;
        }

        // CRITICAL: LLMs are inherently "lazy"...
        // Проверка завершённости todos
        if context.todos.is_complete() {
            info!(
                completed = context.todos.completed_count(),
                total = context.todos.items.len(),
                "All todos completed"
            );
            return HookResult::Continue;
        }

        // Todos не завершены - принудительная итерация
        let pending = context.todos.pending_count();
        let total = context.todos.items.len();
        let completed = context.todos.completed_count();

        let reason = format!(
            "Not all tasks are completed ({completed}/{total} done, {pending} remaining). Continue working on remaining tasks."
        );

        let todo_context = context.todos.to_context_string();

        info!(
            pending = pending,
            completed = completed,
            total = total,
            "Forcing continuation due to incomplete todos"
        );

        HookResult::ForceIteration {
            reason,
            context: Some(todo_context),
        }
    }
}
```

## Примеры сценариев

### Сценарий 1: Все задачи завершены
```
TodoList:
  ✅ Task 1 - Completed
  ✅ Task 2 - Completed

Результат: HookResult::Continue (агент завершает работу)
```

### Сценарий 2: Есть незавершённые задачи
```
TodoList:
  ✅ Task 1 - Completed
  ⏳ Task 2 - Pending

Результат: HookResult::ForceIteration {
    reason: "Not all tasks are completed (1/2 done, 1 remaining)...",
    context: Some("Tasks:\n1. ✅ Task 1\n2. ⏳ Task 2")
}
```

### Сценарий 3: Достигнут лимит продолжений
```
continuation_count = 10
max_continuations = 10

TodoList:
  ✅ Task 1 - Completed
  ⏳ Task 2 - Pending

Результат: HookResult::Continue (разрешить завершение несмотря на незавершённую задачу)
```

## Конструктор

```rust
// src/agent/hooks/completion.rs:17-22
pub struct CompletionCheckHook;

impl CompletionCheckHook {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}
```

## Логирование

| Ситуация | Уровень | Сообщение |
|----------|---------|-----------|
| Достигнут лимит продолжений | `info` | "Continuation limit reached, allowing completion" |
| Все todos завершены | `info` | "All todos completed" |
| Принудительная итерация | `info` | "Forcing continuation due to incomplete todos" |
