# Документация Oxide Agent

Главная навигация по документации проекта Oxide Agent.

## Разделы

### Хуки (Hooks)
- [**Система хуков**](./hooks/index.md) - система перехвата событий жизненного цикла агента
  - [Обзор архитектуры](./hooks/overview.md)
  - [Типы системы](./hooks/types.md)
  - [Registry](./hooks/registry.md)
  - [CompletionCheckHook](./hooks/completion-check.md)
  - [DelegationGuardHook](./hooks/delegation-guard.md)
  - [WorkloadDistributorHook](./hooks/workload-distributor.md)
  - [SubAgentSafetyHook](./hooks/sub-agent-safety.md)
  - [SearchBudgetHook](./hooks/search-budget.md)
  - [TimeoutReportHook](./hooks/timeout-report.md)
  - [Практические примеры](./hooks/examples.md)

### Саб-агенты (Sub-Agents)
- [**Обзор саб-агентов**](./hooks/sub-agents/index.md)
  - [Жизненный цикл](./hooks/sub-agents/lifecycle.md)
  - [Механизм делегирования](./hooks/sub-agents/delegation.md)
  - [EphemeralSession](./hooks/sub-agents/ephemeral-session.md)

## Быстрый старт

### Основной агент (Main Agent)
```
Хуки:
  ✅ CompletionCheckHook
  ✅ WorkloadDistributorHook
  ✅ DelegationGuardHook
  ✅ SearchBudgetHook
  ✅ TimeoutReportHook

Лимиты:
  - Макс. итераций: 1000
  - Макс. токены: 200,000
  - Тайм-аут: 600 сек (10 мин)
  - Лимит поисков: 10 запросов
  - Лимит продолжений: 10
```

### Саб-агент (Sub-Agent)
```
Хуки:
  ✅ CompletionCheckHook
  ✅ SubAgentSafetyHook
  ✅ SearchBudgetHook
  ✅ TimeoutReportHook

Лимиты:
  - Макс. итераций: 60
  - Макс. токены: 64,000
  - Тайм-аут: настроенный
  - Лимит поисков: 10 запросов

Заблокированные инструменты:
  - delegate_to_sub_agent
  - send_file_to_user
```

## Поток выполнения агента

```
User Request
    ↓
[BeforeAgent] → WorkloadDistributorHook (inject context если сложный)
    ↓
LLM Call + Tool Calls
    ↓
[BeforeTool] → DelegationGuardHook (если delegate_to_sub_agent)
               WorkloadDistributorHook (блокирует тяжёлые команды)
               SubAgentSafetyHook (если sub-agent)
               SearchBudgetHook (проверяет лимит поисков)
    ↓
Tool Execution (или delegate_to_sub_agent)
    ↓
[AfterTool] → логирование/метрики
    ↓
[AfterAgent] → CompletionCheckHook: проверка todos
    ↓
Если не завершено → ForceIteration → возврат к LLM
Если завершено → Finish
```

## Конфигурация

Основные константы из `src/config.rs`:

| Константа | Значение | Описание |
|-----------|----------|----------|
| `AGENT_CONTINUATION_LIMIT` | 10 | Макс. принудительных продолжений |
| `AGENT_SEARCH_LIMIT` | 10 | Лимит поисковых запросов |
| `AGENT_MAX_TOKENS` | 200,000 | Макс. токенов в памяти (main agent) |
| `AGENT_MAX_ITERATIONS` | 1000 | Макс. итераций (main agent) |
| `AGENT_TIMEOUT_SECS` | 600 | Тайм-аут агента (10 минут) |
| `SUB_AGENT_MAX_ITERATIONS` | 60 | Макс. итераций (sub-agent) |
| `SUB_AGENT_MAX_TOKENS` | 64,000 | Макс. токенов (sub-agent) |

## Структура проекта

```
src/
├── agent/
│   ├── hooks/           # Система хуков
│   │   ├── types.rs    # HookEvent, HookResult, HookContext
│   │   ├── registry.rs # Hook trait, HookRegistry
│   │   ├── completion.rs
│   │   ├── delegation_guard.rs
│   │   ├── workload.rs
│   │   ├── sub_agent_safety.rs
│   │   ├── search_budget.rs
│   │   └── timeout_report.rs
│   ├── runner/
│   │   └── hooks.rs    # Интеграция хуков в runner
│   ├── providers/
│   │   └── delegation.rs # delegate_to_sub_agent инструмент
│   └── context.rs        # EphemeralSession
└── config.rs             # Константы конфигурации
```

## Архитектурные паттерны

### Цепочка ответственности
Хуки выполняются последовательно в порядке регистрации. Первый не-`Continue` результат останавливает цепочку.

### Шаблон наблюдатель
Хуки подписываются на события жизненного цикла агента и реагируют на них.

### Стратегия
Разные хуки реализуют разные стратегии обработки одного и того же события.

### Декоратор
Хуки "оборачивают" базовую логику агента, добавляя проверки и инъекции контекста.
