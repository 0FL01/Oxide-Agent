# Обзор архитектуры системы хуков

Система хуков реализует гибкий механизм перехвата событий жизненного цикла агента для модификации поведения без изменения основного кода исполнения.

## Архитектурные паттерны

### 1. Цепочка ответственности
Хуки выполняются последовательно в порядке регистрации. Первый не-`Continue` результат останавливает цепочку и возвращается как финальный.

```rust
// src/agent/hooks/registry.rs:39-87
pub fn execute(&self, event: &HookEvent, context: &HookContext) -> HookResult {
    for hook in &self.hooks {
        let result = hook.handle(event, context);
        if !matches!(result, HookResult::Continue) {
            return result;
        }
    }
    HookResult::Continue
}
```

### 2. Шаблон наблюдатель
Хуки подписываются на события жизненного цикла агента (`BeforeAgent`, `AfterAgent`, `BeforeTool`, и т.д.) и реагируют на них.

### 3. Стратегия
Разные хуки реализуют разные стратегии обработки одного и того же события. Например, `BeforeTool` обрабатывается:
- `DelegationGuardHook` - блокирует аналитическое делегирование
- `WorkloadDistributorHook` - блокирует тяжёлые команды
- `SubAgentSafetyHook` - блокирует запрещённые инструменты

### 4. Декоратор
Хуки "оборачивают" базовую логику агента, добавляя проверки и инъекции контекста.

## Структура модулей

```
src/agent/hooks/
├── types.rs               # HookEvent, HookResult, HookContext
├── registry.rs            # Hook trait, HookRegistry
├── completion.rs          # CompletionCheckHook
├── delegation_guard.rs     # DelegationGuardHook
├── workload.rs            # WorkloadDistributorHook
├── sub_agent_safety.rs    # SubAgentSafetyHook
├── search_budget.rs        # SearchBudgetHook
├── timeout_report.rs      # TimeoutReportHook
└── mod.rs                # Публичные экспорты

src/agent/runner/
└── hooks.rs              # Интеграция хуков в runner
```

## Регистрация хуков

### Основной агент (src/agent/executor.rs:52-57)

```rust
let mut runner = AgentRunner::new(llm_client.clone());
runner.register_hook(Box::new(CompletionCheckHook::new()));
runner.register_hook(Box::new(WorkloadDistributorHook::new()));
runner.register_hook(Box::new(DelegationGuardHook::new()));
runner.register_hook(Box::new(SearchBudgetHook::new(get_agent_search_limit())));
runner.register_hook(Box::new(TimeoutReportHook::new()));
```

### Саб-агент (src/agent/providers/delegation.rs:164-171)

```rust
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

## Поток выполнения

```
run()
├── reset_loop_detector()
├── apply_before_agent_hooks()          # HookEvent::BeforeAgent
└── run_loop()
    ├── [для каждой итерации]
    │   ├── [проверка отмены/тайм-аута]
    │   ├── apply_timeout_hook()        # HookEvent::Timeout
    │   ├── apply_before_iteration_hooks()  # HookEvent::BeforeIteration
    │   ├── call_llm_with_tools()
    │   └── handle_llm_response()
    │       ├── [если есть tool_calls]
    │       │   └── execute_tools()
    │       │       ├── [для каждого инструмента]
    │       │       │   ├── apply_before_tool_hooks()      # HookEvent::BeforeTool
    │       │       │   ├── [выполнение инструмента]
    │       │       │   └── apply_after_tool_hooks()       # HookEvent::AfterTool
    │       │
    │       └── [если final answer]
    │           └── handle_final_response()
    │               └── after_agent_hook_result()   # HookEvent::AfterAgent
```

## Интеграция в runner

Методы интеграции в `src/agent/runner/hooks.rs`:

```rust
// Перед запуском агента
apply_before_agent_hooks(&mut ctx) -> Result<()>

// Перед каждой итерацией
apply_before_iteration_hooks(&mut ctx, &RunState) -> Result<()>

// Перед выполнением инструмента
apply_before_tool_hooks(&mut ctx, &RunState, &ToolCall) -> Result<ToolHookDecision>

// После выполнения инструмента
apply_after_tool_hooks(&mut ctx, &RunState, &ToolExecutionResult)

// После финального ответа агента
after_agent_hook_result(&ctx, &RunState, final_response: &str) -> HookResult

// При достижении тайм-аута
apply_timeout_hook(&mut ctx, &RunState) -> Result<Option<String>>
```

## Отличия Main Agent vs Sub-Agent

| Характеристика | Main Agent | Sub-Agent |
|---------------|-------------|-----------|
| Роль | Оркестратор (анализ, принятие решений) | Рабочий (выполнение задач) |
| WorkloadDistributorHook | ✅ Да | ❌ Нет |
| DelegationGuardHook | ✅ Да | ❌ Нет |
| SubAgentSafetyHook | ❌ Нет | ✅ Да |
| Может делегировать | ✅ Да | ❌ Нет |
| Макс. итераций | 1000 | 60 |
| Макс. токены | 200,000 | 64,000 |
| Тип работы | Анализ данных | Получение данных |

## Ключевые особенности

### Безопасность
- `SubAgentSafetyHook` ограничивает итерации, токены и инструменты
- `DelegationGuardHook` предотвращает делегирование аналитических задач

### Надёжность
- `CompletionCheckHook` гарантирует выполнение всех задач через принудительные итерации

### Эффективность
- `WorkloadDistributorHook` распределяет нагрузку между агентами (Main Agent → анализ, Sub-Agents → выполнение)

### Отказоустойчивость
- `TimeoutReportHook` генерирует структурированный отчёт при превышении времени

### Экономия ресурсов
- `SearchBudgetHook` предотвращает перерасход токенов на поисковые запросы
