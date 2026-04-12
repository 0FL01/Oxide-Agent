# WorkloadDistributorHook

Обеспечивает разделение обязанностей между Main Agent (оркестратор) и Sub-Agents (рабочие).

**События:** `BeforeAgent`, `BeforeTool`

**Конфигурация:**
- `min_word_count` = 60 (порог сложности промпта)

**Регистрация:**
- ✅ Main Agent
- ❌ Sub-Agent (саб-агенты сами выполняют работу)

## Назначение

Две основные функции:

### 1. Hard Blocking (жёсткая блокировка)
Блокирует выполнение тяжёлых операций Main Agent'ом, принуждая к делегированию:
- `git clone`
- `git fetch`
- `grep -r` / `grep -R`
- `find` с `-exec` или `-name`
- Прямые вызовы `deep_crawl`, `web_markdown`, `web_pdf`

### 2. Context Injection (инъекция контекста)
Для сложных промптов (>60 слов или ключевые слова) инъектирует системные инструкции о разделении workflow.

## Логика работы

### Context Injection

```
BeforeAgent событие
    ↓
1. Проверка сложности промпта
    ├─ Сложный? → InjectContext с инструкциями
    └─ Не сложный? → Continue
```

### Hard Blocking

```
BeforeTool событие
    ↓
1. Проверка типа агента
    ├─ Sub-agent? → Continue (разрешено всё)
    └─ Main agent? → продолжить проверку
         ↓
2. Проверка инструмента
    ├─ Crawl4AI tool? → Block
    ├─ execute_command с тяжёлой командой? → Block
    └─ Другой? → Continue
```

## Реализация

```rust
// src/agent/hooks/workload.rs:109-183
impl Hook for WorkloadDistributorHook {
    fn name(&self) -> &'static str {
        "workload_distributor"
    }

    fn handle(&self, event: &HookEvent, context: &HookContext) -> HookResult {
        match event {
            // 1. Context Injection для сложных промптов
            HookEvent::BeforeAgent { prompt } => {
                if self.is_complex_prompt(prompt) {
                    return HookResult::InjectContext(
                        "[SYSTEM NOTICE: High Complexity Detected]\n\
                        You must SPLIT your workflow to handle this request efficiently:\n\
                        1. 🟢 DELEGATE retrieval tasks (git clone, grep, find, cat, deep_crawl, web_markdown) to `delegate_to_sub_agent`.\n\
                           - Goal: Get raw data/files/web content.\n\
                           - Forbidden for sub-agent: analysis, reasoning, explaining \"why\".\n\
                        2. 🧠 RETAIN analysis tasks for yourself.\n\
                           - Goal: Read files/content returned by sub-agent and perform high-level reasoning.\n\
                        Example of GOOD delegation: \"Use deep_crawl to find news about X\".\n\
                        Example of BAD delegation: \"Analyze why project X is failing\"."
                            .to_string(),
                    );
                }
            }

            // 2. Hard Blocking тяжёлых команд
            HookEvent::BeforeTool {
                tool_name,
                arguments,
            } => {
                // Sub-agents могут выполнять всё
                if context.is_sub_agent {
                    return HookResult::Continue;
                }

                // Блокировка прямых Crawl4AI вызовов для Main Agent
                if self.is_crawl4ai_tool(tool_name) {
                    return HookResult::Block {
                        reason: format!(
                            "⛔ DIRECT SEARCH BLOCKED: You are trying to use '{}' directly. \
                            For efficiency and context saving, you MUST delegate web crawling/extraction to a sub-agent.\n\
                            ACTION REQUIRED: Use `delegate_to_sub_agent` with tool '{}' in the whitelist.",
                            tool_name, tool_name
                        ),
                    };
                }

                if tool_name == "execute_command" {
                    let command = match serde_json::from_str::<Value>(arguments) {
                        Ok(json) => json
                            .get("command")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        Err(_) => return HookResult::Continue,
                    };

                    if let Some(op) = self.is_heavy_command(&command) {
                        return HookResult::Block {
                            reason: format!(
                                "⛔ MANUAL LABOR DETECTED: You are trying to run a heavy operation ('{}') yourself. \
                                This wastes your context window.\n\
                                ACTION REQUIRED: Use `delegate_to_sub_agent` to run this command and summarize results.",
                                op
                            ),
                        };
                    }
                }
            }
            _ => {}
        }

        HookResult::Continue
    }
}
```

## Определение тяжёлых команд

```rust
// src/agent/hooks/workload.rs:27-49
fn is_heavy_command(&self, command: &str) -> Option<&'static str> {
    let normalized = command.trim();

    // Git операции для получения данных
    if normalized.starts_with("git clone") {
        return Some("git clone");
    }
    if normalized.starts_with("git fetch") {
        return Some("git fetch");
    }

    // Тяжёлые поисковые операции
    if normalized.contains("grep -r") || normalized.contains("grep -R") {
        return Some("recursive grep");
    }
    if normalized.starts_with("find")
        && (normalized.contains("-exec") || normalized.contains("-name"))
    {
        return Some("find search");
    }

    None
}
```

## Определение сложности промпта

```rust
// src/agent/hooks/workload.rs:55-100
fn is_complex_prompt(&self, prompt: &str) -> bool {
    let normalized = prompt.to_lowercase();
    let word_count = normalized.split_whitespace().count();
    if word_count >= self.min_word_count {  // 60 слов
        return true;
    }

    let keywords = [
        // Русские
        "исслед", "сравн", "обзор", "анализ", "отчет",
        "подбор", "репозитор", "код", "файлы", "сканир", "изучи",
        // Английские
        "compare", "research", "analysis", "overview", "report",
        "benchmark", "repo", "codebase", "scan", "investigate",
    ];

    if keywords.iter().any(|keyword| normalized.contains(keyword)) {
        return true;
    }

    // Детекция многосоставного сложного запроса (3+ предложений)
    let sentence_markers = ["?", "!", "."];
    let sentence_hits: usize = sentence_markers
        .iter()
        .map(|marker| normalized.matches(marker).count())
        .sum();

    sentence_hits >= 3
}
```

## Примеры сценариев

### Сценарий 1: Сложный промпт с контекстной инъекцией
```
Prompt (80 слов, содержит "исследуй и сравни"):
"Исследуй несколько репозиториев и сравни их архитектуры,
выполнение, тестовое покрытие..."

Результат: HookResult::InjectContext("[SYSTEM NOTICE: High Complexity Detected]...")
```

### Сценарий 2: Попытка выполнить git clone напрямую
```
Tool: execute_command
Arguments: {"command": "git clone https://github.com/repo"}

Результат: HookResult::Block {
    reason: "⛔ MANUAL LABOR DETECTED: You are trying to run a heavy operation ('git clone')..."
}
```

### Сценарий 3: Прямой вызов deep_crawl
```
Tool: deep_crawl

Результат: HookResult::Block {
    reason: "⛔ DIRECT SEARCH BLOCKED: You are trying to use 'deep_crawl' directly..."
}
```

### Сценарий 4: Саб-агент пытается выполнить тяжёлую команду
```
Agent type: Sub-agent
Tool: execute_command
Arguments: {"command": "git clone ..."}

Результат: HookResult::Continue (саб-агентам разрешено)
```

## Конструктор

```rust
// src/agent/hooks/workload.rs:20-25
pub struct WorkloadDistributorHook {
    min_word_count: usize,
}

impl WorkloadDistributorHook {
    #[must_use]
    pub const fn new() -> Self {
        Self { min_word_count: 60 }
    }
}
```

## Логирование

Блокировки логируются через `info` в `HookRegistry.execute()`:

```
[INFO] Hook injecting context
[INFO] Hook blocking action: "⛔ DIRECT SEARCH BLOCKED: ..."
[INFO] Hook blocking action: "⛔ MANUAL LABOR DETECTED: ..."
```

## Рекомендации

### ✅ Правильное делегирование для Main Agent
```
1. Сложный запрос: "Исследуй код и составь отчёт"
   → Получены инструкции через InjectContext

2. Delegate: "git clone repo и найди все .rs файлы"
3. Delegate: "grep -r 'async fn' в src/"
4. Analze: Прочитать результаты и создать отчёт
```

### ❌ Неправильное поведение Main Agent
```
1. Прямое выполнение: git clone repo (BLOCKED)
2. Прямое выполнение: grep -r pattern (BLOCKED)
3. Прямое выполнение: deep_crawl url (BLOCKED)
```
