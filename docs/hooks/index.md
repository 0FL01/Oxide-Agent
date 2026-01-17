# Система хуков Oxide Agent

Документация по системе хуков агента и саб-агентов.

## Структура

### Основные концепции
- [**Обзор архитектуры**](overview.md) - архитектура, паттерны проектирования, поток выполнения
- [**Типы системы**](types.md) - `HookEvent`, `HookResult`, `HookContext`
- [**Registry**](registry.md) - `Hook` trait и `HookRegistry`

### Хуки основного агента
- [**CompletionCheckHook**](completion-check.md) - проверка завершения todo-задач
- [**DelegationGuardHook**](delegation-guard.md) - защита от делегирования аналитических задач
- [**WorkloadDistributorHook**](workload-distributor.md) - распределение нагрузки между агентами
- [**SearchBudgetHook**](search-budget.md) - лимит поисковых запросов
- [**TimeoutReportHook**](timeout-report.md) - отчёт при достижении тайм-аута

### Хуки саб-агентов
- [**SubAgentSafetyHook**](sub-agent-safety.md) - ограничения и блокировка инструментов

### Саб-агенты
- [**Обзор саб-агентов**](sub-agents/index.md) - жизненный цикл и отличия от main agent
- [**Механизм делегирования**](sub-agents/delegation.md) - `delegate_to_sub_agent` инструмент
- [**EphemeralSession**](sub-agents/ephemeral-session.md) - изолированная сессия саб-агента

### Примеры
- [**Практические примеры**](examples.md) - кастомные хуки, последовательность выполнения, отладка

## Конфигурация

Основные константы из `config.rs`:

| Константа | Значение | Описание |
|-----------|----------|----------|
| `AGENT_CONTINUATION_LIMIT` | 10 | Макс. принудительных продолжений |
| `AGENT_SEARCH_LIMIT` | 10 | Лимит поисковых запросов |
| `AGENT_MAX_TOKENS` | 200,000 | Макс. токенов в памяти (main agent) |
| `AGENT_MAX_ITERATIONS` | 1000 | Макс. итераций (main agent) |
| `AGENT_TIMEOUT_SECS` | 600 | Тайм-аут агента (10 минут) |
| `SUB_AGENT_MAX_ITERATIONS` | 60 | Макс. итераций (sub-agent) |
| `SUB_AGENT_MAX_TOKENS` | 64,000 | Макс. токенов (sub-agent) |

## Карта хуков по агентам

### Main Agent (оркестратор)
```
✅ CompletionCheckHook
✅ WorkloadDistributorHook
✅ DelegationGuardHook
✅ SearchBudgetHook
✅ TimeoutReportHook
```

### Sub-Agent (рабочий)
```
✅ CompletionCheckHook
✅ SubAgentSafetyHook
✅ SearchBudgetHook
✅ TimeoutReportHook

❌ WorkloadDistributorHook - саб-агенты сами выполняют работу
❌ DelegationGuardHook - саб-агентам запрещено делегировать
```

## Поток выполнения через хуки

```
User Request
    ↓
[BeforeAgent] → WorkloadDistributorHook: inject context (если сложный промпт)
    ↓
LLM Call + Tool Calls
    ↓
[BeforeTool] → DelegationGuardHook (если delegate_to_sub_agent)
               WorkloadDistributorHook (блокирует тяжёлые команды)
    ↓
Tool Execution
    ↓
[AfterTool] → логирование/метрики
    ↓
[AfterAgent] → CompletionCheckHook: проверка todos
    ↓
Если не завершено → ForceIteration → возврат к LLM
Если завершено → Finish
```
