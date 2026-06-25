# Система хуков Oxide Agent

Документация по системе хуков агента и саб-агентов.

## Структура

### Основные концепции
- [**Обзор архитектуры**](overview.md) - архитектура, паттерны проектирования, поток выполнения
- [**Типы системы**](types.md) - `HookEvent`, `HookResult`, `HookContext`
- [**Registry**](registry.md) - `Hook` trait и `HookRegistry`

### Хуки основного агента
- [**CompletionCheckHook**](completion-check.md) - проверка завершения todo-задач
- [**SearchBudgetHook**](search-budget.md) - лимит поисковых запросов
- [**TimeoutReportHook**](timeout-report.md) - отчёт при достижении тайм-аута

### Хуки саб-агентов
- [**SubAgentSafetyHook**](sub-agent-safety.md) - ограничения и блокировка инструментов

### Примеры
- [**Практические примеры**](examples.md) - кастомные хуки, последовательность выполнения, отладка

## Конфигурация

Основные константы из `config.rs`:

| Константа | Значение | Описание |
|-----------|----------|----------|
| `AGENT_CONTINUATION_LIMIT` | 10 | Макс. принудительных продолжений |
| `AGENT_SEARCH_LIMIT` | 10 | Лимит поисковых запросов |
| `AGENT_MAX_TOKENS` | 200,000 | Макс. токенов в памяти (main agent) |
| `AGENT_MAX_ITERATIONS` | 200 | Макс. итераций (main agent, env override) |
| `AGENT_TIMEOUT_SECS` | 1800 | Тайм-аут агента (30 минут) |
| `SUB_AGENT_MAX_ITERATIONS` | 2000 | Макс. итераций (sub-agent, env override) |
| sub-agent context budget | inherited | Наследует budget основного агента, если не задан explicit override |

## Карта хуков по агентам

### Main Agent (оркестратор)
```
✅ CompletionCheckHook
✅ HotContextHealthHook
✅ RetrievalAdvisorHook (gated by HookAccessPolicy)
✅ EpisodicExtractHook (gated by HookAccessPolicy)
✅ SearchBudgetHook (policy-controlled)
✅ ToolAccessPolicyHook
✅ TimeoutReportHook (policy-controlled)
```

### Sub-Agent (рабочий)
```
✅ CompletionCheckHook
✅ HotContextHealthHook
✅ SubAgentSafetyHook
✅ SearchBudgetHook
✅ TimeoutReportHook
```

## Поток выполнения через хуки

```
User Request
    ↓
LLM Call + Tool Calls
    ↓
[BeforeTool] → policy/safety/search hooks
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
