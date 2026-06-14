# SearchBudgetHook

Лимитирует количество поисковых запросов за сессию агента.

**Событие:** `BeforeTool`

**Конфигурация:**
- `AGENT_SEARCH_LIMIT` = 10

**Регистрация:**
- ✅ Main Agent
- ✅ Sub-Agent

## Назначение

Предотвращает перерасход токенов на поисковые запросы. После достижения лимита агент должен синтезировать результаты из уже полученных данных вместо выполнения новых поисков.

## Поисковые инструменты

Следующие инструменты считаются поисковыми и учитываются в лимите:
- `web_search`
- `web_extract`
- `web_markdown`
- `web_crawler`

## Логика работы

```
BeforeTool событие
    ↓
1. Проверка инструмента
    ├─ Не поисковый? → Continue
    └─ Поисковый? → продолжить
         ↓
2. Инкремент счётчика (atomic operation)
    ↓
3. Проверка лимита
    ├─ Текущее > лимит? → Block
    └─ Текущее ≤ лимит? → Continue
```

## Реализация

```rust
// src/agent/hooks/search_budget.rs:9-55
pub struct SearchBudgetHook {
    limit: usize,
    count: AtomicUsize,
}

impl SearchBudgetHook {
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            count: AtomicUsize::new(0),
        }
    }

    fn is_search_tool(&self, tool_name: &str) -> bool {
        matches!(
            tool_name,
            "web_search"
                | "web_extract"
                | "web_markdown"
                | "web_crawler"
        )
    }
}

impl Hook for SearchBudgetHook {
    fn name(&self) -> &'static str {
        "search_budget"
    }

    fn handle(&self, event: &HookEvent, _context: &HookContext) -> HookResult {
        if let HookEvent::BeforeTool { tool_name, .. } = event {
            if self.is_search_tool(tool_name) {
                let current = self.count.fetch_add(1, Ordering::SeqCst) + 1;
                if current > self.limit {
                    return HookResult::Block {
                        reason: format!(
                            "Search budget exceeded ({}/{}). Please synthesize findings from existing data instead of searching more.",
                            current, self.limit
                        ),
                    };
                }
            }
        }

        HookResult::Continue
    }
}
```

## Примеры сценариев

### Сценарий 1: Поисковые запросы в пределах лимита
```
Вызовы:
1. web_search → count=1 (≤10) → Continue
2. web_extract → count=2 (≤10) → Continue
3. searxng_search → count=3 (≤10) → Continue
...
10. web_crawler → count=10 (≤10) → Continue
```

### Сценарий 2: Превышение лимита
```
Вызовы:
1-10. Поисковые запросы → count=1...10 → Continue
11. web_search → count=11 (>10) → Block {
    reason: "Search budget exceeded (11/10). Please synthesize findings from existing data..."
}
```

### Сценарий 3: Непоисковый инструмент игнорируется
```
Вызовы:
1. execute_command → не поисковый → Continue
2. write_todos → не поисковый → Continue
3. web_search → поисковый → count=1 → Continue
```

## Конструктор

```rust
// src/agent/hooks/search_budget.rs:15-23
impl SearchBudgetHook {
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            count: AtomicUsize::new(0),
        }
    }
}
```

## Регистрация

### В Main Agent

```rust
// src/agent/executor.rs:56
runner.register_hook(Box::new(SearchBudgetHook::new(get_agent_search_limit())));
```

### В Sub-Agent

```rust
// src/agent/providers/delegation.rs:170
runner.register_hook(Box::new(SearchBudgetHook::new(get_agent_search_limit())));
```

## Логирование

Блокировка логируется через `info` в `HookRegistry.execute()`:

```
[INFO] Hook blocking action: "Search budget exceeded (11/10). Please synthesize findings from existing data..."
```

## Конфигурация

```rust
// src/config.rs:680
pub const AGENT_SEARCH_LIMIT: usize = 10;
```

## Лимит на сессию

Счётчик `count` создаётся при создании хука и сохраняется на протяжении всей сессии агента (main или sub).

## Атомарность

```rust
let current = self.count.fetch_add(1, Ordering::SeqCst) + 1;
```

Используется `AtomicUsize` с `Ordering::SeqCst` для потокобезопасного инкремента в многопоточной среде.

## Рекомендации

### ✅ Правильное поведение при достижении лимита
```
1. Выполнить до 10 поисков
2. Синтезировать результаты из полученных данных
3. Не пытаться выполнить дополнительные поиски
```

### ❌ Неправильное поведение при достижении лимита
```
1. Выполнить 10 поисков
2. Попытаться выполнить 11-й поисковый запрос
3. Получить блокировку от SearchBudgetHook
```

### 🔄 Оптимальная стратегия
```
1. Начать с 1-2 целевых поисковых запросов
2. Проверить результаты на достаточность
3. Синтезировать ответ
4. Использовать оставшиеся попытки только при необходимости
```
