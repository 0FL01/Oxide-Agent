# DelegationGuardHook

Предотвращает делегирование высокоуровневых когнитивных задач (анализ, рассуждение) саб-агентам.

**Событие:** `BeforeTool`

**Конфигурация:**
- Нет

**Регистрация:**
- ✅ Main Agent
- ❌ Sub-Agent (саб-агентам запрещено делегировать)

## Назначение

Обеспечивает разделение обязанностей:
- **Main Agent (оркестратор)** - анализ, принятие решений
- **Sub-Agents (рабочие)** - получение сырых данных

Главная задача: предотвратить делегирование аналитических задач типа "почему", "анализируй", "объясни".

## Логика работы

```
BeforeTool событие (tool_name == "delegate_to_sub_agent")
    ↓
1. Парсинг аргументов для получения 'task'
    ↓
2. Whitelist: Проверка retrieval глаголов
    ├─ Начинается с find/search/grep/...? → Continue (безопасный путь)
    └─ Не начинается? → продолжить к blocklist
         ↓
3. Blocklist: Проверка аналитических ключевых слов
    ├─ Обнаружен why/analyze/explain/...? → Block
    └─ Не обнаружен? → Continue
```

## Регулярные выражения

### Whitelist (безопасный путь)

```rust
// src/agent/hooks/delegation_guard.rs:25-27
static RE_RETRIEVAL_INTENT: lazy_regex::Lazy<regex::Regex> = lazy_regex!(
    r"(?iu)^\s*(?:please\s+|kindly\s+)?(?:find|search|grep|locate|list|ls|cat|read|get|fetch|download|clone|найти|найди|поиск|искать|перечисли|список|покажи|скачай|загрузи|прочитай|выведи)\b"
);
```

**Допустимые глаголы:**
- `find`, `search`, `grep`, `locate`, `list`, `ls`, `cat`, `read`, `get`, `fetch`, `download`, `clone`
- `найти`, `найди`, `поиск`, `искать`, `перечисли`, `список`, `покажи`, `скачай`, `загрузи`, `прочитай`, `выведи`

### Blocklist (защитный путь)

```rust
// src/agent/hooks/delegation_guard.rs:35-37
static RE_ANALYTICAL_INTENT: lazy_regex::Lazy<regex::Regex> = lazy_regex!(
    r"(?iu)\b(why|analyz\w*|explain\w*|review\w*|opinion\w*|reason\w*|evaluate\w*|compare\w*|почему|анализ\w*|объясн\w*|обзор\w*|мнени\w*|оцени\w*|сравни\w*|выясни\w*|эффективн\w*)\b"
);
```

**Блокируемые ключевые слова:**
- `why`, `analyz*`, `explain*`, `review*`, `opinion*`, `reason*`, `evaluate*`, `compare*`
- `почему`, `анализ*`, `объясн*`, `обзор*`, `мнени*`, `оцени*`, `сравни*`, `выясни*`, `эффективн*`

## Реализация

```rust
// src/agent/hooks/delegation_guard.rs:51-92
impl Hook for DelegationGuardHook {
    fn name(&self) -> &'static str {
        "delegation_guard"
    }

    fn handle(&self, event: &HookEvent, _context: &HookContext) -> HookResult {
        let HookEvent::BeforeTool {
            tool_name,
            arguments,
        } = event
        else {
            return HookResult::Continue;
        };

        if tool_name != "delegate_to_sub_agent" {
            return HookResult::Continue;
        }

        // Парсинг аргументов для получения 'task'
        let task = match serde_json::from_str::<Value>(arguments) {
            Ok(json) => json
                .get("task")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            Err(_) => return HookResult::Continue,
        };

        if let Some(keyword) = self.check_task(&task) {
            return HookResult::Block {
                reason: format!(
                    "⛔ Delegation Blocked: The task contains an analytical keyword ('{}'). \
                     Sub-agents are restricted to raw data retrieval (cloning, grep, list files). \
                     Please split the task: delegate retrieval, but perform analysis yourself.",
                    keyword
                ),
            };
        }

        HookResult::Continue
    }
}
```

## Примеры сценариев

### Сценарий 1: Допустимая задача (whitelist)
```json
{
  "task": "Find files about architecture",
  "tools": ["execute_command", "cat"]
}
```

```
Результат: HookResult::Continue
```

### Сценарий 2: Задача с аналитическим ключевым словом (blocklist)
```json
{
  "task": "Analyze why the project is failing",
  "tools": ["execute_command"]
}
```

```
Результат: HookResult::Block {
    reason: "⛔ Delegation Blocked: The task contains an analytical keyword ('analyze'). \
             Sub-agents are restricted to raw data retrieval (cloning, grep, list files). \
             Please split the task: delegate retrieval, but perform analysis yourself."
}
```

### Сценарий 3: Смешанная задача
```json
{
  "task": "Find and analyze the logs",
  "tools": ["execute_command"]
}
```

```
Результат: HookResult::Block
```

**Правильный подход:**
```
1. Delegate: "Find all log files" (whitelist - find)
2. Analyze: Read the files returned by sub-agent
```

## Конструктор

```rust
// src/agent/hooks/delegation_guard.rs:14-19
pub struct DelegationGuardHook;

impl DelegationGuardHook {
    #[must_use]
    pub fn new() -> Self {
        Self {}
    }
}
```

## Логирование

Блокировка логируется через `info` в `HookRegistry.execute()`:

```
[INFO] Hook blocking action: "⛔ Delegation Blocked: ..."
```

## Рекомендации

### ✅ ХОРОШО для делегирования
```
"Find files matching pattern"
"Search for occurrences in codebase"
"List files in directory"
"Download the repository"
"Clone the git repo"
"Get the content of file X"
```

### ❌ ПЛОХО для делегирования
```
"Analyze why X fails"
"Explain how Y works"
"Compare A and B"
"Give your opinion on Z"
"Reason about the architecture"
"Review the implementation"
```

### 🔄 ПРАВИЛЬНЫЙ подход для сложных задач
```
1. Delegate retrieval: "Find all occurrences of function X"
2. Analyze yourself: Read the results and explain why it fails
```
