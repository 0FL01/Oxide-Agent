# Саб-агенты

Документация по механизму делегирования и изолированным сессиям саб-агентов.

## Структура

- [**Жизненный цикл саб-агента**](lifecycle.md) - создание, выполнение, завершение
- [**Механизм делегирования**](delegation.md) - `delegate_to_sub_agent` инструмент
- [**EphemeralSession**](ephemeral-session.md) - изолированная сессия саб-агента

## Отличия от Main Agent

| Характеристика | Main Agent | Sub-Agent |
|---------------|-------------|-----------|
| Роль | Оркестратор (анализ, принятие решений) | Рабочий (выполнение задач) |
| CompletionCheckHook | ✅ Да | ✅ Да |
| WorkloadDistributorHook | ✅ Да | ❌ Нет |
| DelegationGuardHook | ✅ Да | ❌ Нет |
| SubAgentSafetyHook | ❌ Нет | ✅ Да |
| SearchBudgetHook | ✅ Да | ✅ Да |
| TimeoutReportHook | ✅ Да | ✅ Да |
| Может делегировать | ✅ Да | ❌ Нет |
| Макс. итераций | 1000 | 60 |
| Макс. токены | 200,000 | 64,000 |
| Тайм-аут | 600 сек (10 мин) | Конфигурируется |

## Хуки саб-агента

```
Саб-агент:
├── CompletionCheckHook     - проверка завершения todos
├── SubAgentSafetyHook    - ограничения (итерации, токены, инструменты)
├── SearchBudgetHook      - лимит поисковых запросов
└── TimeoutReportHook    - отчёт при тайм-ауте
```

## Заблокированные инструменты для саб-агентов

```rust
// src/agent/providers/delegation.rs:39
const BLOCKED_SUB_AGENT_TOOLS: &[&str] = &[
    "delegate_to_sub_agent",  // Запрещено рекурсивное делегирование
    "send_file_to_user",     // Запрещена отправка файлов пользователю
];
```

## Создание саб-агента

```rust
// src/agent/providers/delegation.rs:162-173
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

## Поток делегирования

```
Main Agent вызывает delegate_to_sub_agent
    ↓
DelegationGuardHook проверяет задачу
    ↓
Создаётся EphemeralSession с родительским токеном отмены
    ↓
Фильтруются инструменты (удаление blocked + проверка whitelist)
    ↓
Создаётся SubAgentRunner с хуками
    ↓
Запускается runner.run() с тайм-аутом
    ↓
Результат возвращается как JSON-отчёт
```

## Конфигурация саб-агентов

| Константа | Значение | Описание |
|-----------|----------|----------|
| `SUB_AGENT_MAX_ITERATIONS` | 60 | Макс. итераций саб-агента |
| `SUB_AGENT_MAX_TOKENS` | 64,000 | Макс. токенов саб-агента |

## Безопасность

### Изоляция
- `EphemeralSession` имеет отдельную `AgentMemory`
- Лимит токенов предотвращает переполнение контекста

### Отмена
- При отмене родителя через `child_token()`, отменяется и саб-агент

### Ограничения
- `SubAgentSafetyHook` проверяет итерации, токены и инструменты
- Нет возможности делегировать (`delegate_to_sub_agent` заблокирован)
- Нет возможности отправлять файлы (`send_file_to_user` заблокирован)
