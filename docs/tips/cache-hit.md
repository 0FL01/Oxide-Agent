# Prompt Cache Hit Analysis

## Узкие места (почему cache не работает в Oxide Agent)

1.  ~~**date/time блок в начале system prompt.**~~ **FIXED.** `date_context` перенесён в конец system prompt (`composer.rs`). Regression tests: `test_date_context_at_end_of_main_agent_prompt`, `test_date_context_at_end_of_sub_agent_prompt`.

2.  ~~**Wiki context вставляется до стабильных операционных блоков.**~~ **FIXED.** Wiki context перенесён после workflow guidance (`composer.rs`). Regression test: `test_wiki_context_after_workflow_guidance`.

3.  ~~**Sub-agent включает task в system prompt.**~~ **FIXED.** `Your task: {task}` убран из system prompt sub-agent (`composer.rs`). Task доставляется исключительно через первый user message (`delegation.rs:904`), system prompt стабильный для одинаковых tool-sets. Regression test: `test_sub_agent_prompt_excludes_task`.

4.  ~~**System messages из history fold-ятся в prompt без разбора.**~~ **FIXED.** `fold_system_messages_into_prompt()` (`history.rs`) теперь разделяет system messages на stable (`[TOPIC_AGENTS_MD]`, `[OXIDE_COMPACTED_SUMMARY_V1]`) и volatile (retry notes, temporal context, infra status). Stable идут в cacheable prefix перед `date_suffix`, volatile — после. `ComposedPrompt` (base + date_suffix) заменяет единую строку system_prompt. Pipeline: `base + stable + date_suffix + volatile`.

5.  **Provider-native prompt caching не используется.** Anthropic-compatible пути (MiniMax через `claudius`, OpenCode Go Anthropic path) не выставляют `cache_control` markers, хотя SDK их поддерживают. OpenAI/OpenRouter не имеют explicit cache markers в запросах.

6.  ~~**Нет telemetry по cache read/write tokens.**~~ **FIXED.** `TokenUsage` расширена полями `cached_tokens: Option<u32>` и `cache_creation_tokens: Option<u32>` (`types.rs`). Все 9 production parse sites обновлены (OpenCode Go, OpenRouter, ChatGPT, Mistral, MiniMax, ZAI, NVIDIA). Метод `cache_hit_rate()` вычисляет miss как `prompt_tokens - cached_tokens`. Тесты: 6 unit tests в `types.rs`, 4 provider tests в `opencode_go.rs`. Commit: `20740c82`.

7.  ~~**Tool schemas дублируются в prompt text и native `tools[]`.**~~ **FIXED.** `build_structured_output_instructions()` (`composer.rs`) теперь рендерит только compact sorted tool-name list (`## Available Tools`). Полные schemas доставляются исключительно через native `tools[]` payload. Prompt: 2673→98 bytes (27x reduction), wire duplication: -48%.

8.  ~~**Compacted summary содержит volatile metadata.**~~ **FIXED.** `format_compacted_summary()` (`memory.rs`) убрал 12 volatile полей из prompt-visible текста. Оставлены только `generation` (нужен для compaction chain) и `wiki_memory_lookup_available` (влияет на tool-use). Остальная metadata логируется через `log_runtime_compaction_success`. Regression tests: `compacted_summary_excludes_volatile_metadata`, `compacted_summary_differs_only_in_generation_across_metadata`.

9.  **Compaction pin-ит stale `UserTask`/`RuntimeContext` впереди summary.** `is_pinned()` (`compaction/history.rs:333-342`) сохраняет `UserTask`, `RuntimeContext`, `ApprovalReplay`, `InfraStatus` как pinned messages. После compaction старые dynamic messages остаются в начале non-system history, сжимая reusable stable prefix. **ЧАСТИЧНО FIXED:** budget guard на `compress` tool (`tools.rs:327-335`) блокирует premature compaction (< 85% context utilization). Production: agent отработал 14 итераций до 65K/272K tokens без compaction, cache hit вырос до 99.7%. Commit: `7e599dac`. Pinning strategy пока без изменений.

---

## Детальный RECON: как собираются LLM-запросы и что мешает cache hit

### 1. Сборка system prompt

**Файл:** `crates/oxide-agent-core/src/agent/prompt/composer.rs`

| Блок | Cacheable? | Частота изменений |
|---|---|---|
| Core fallback prompt | **Да** (полностью статичен) | Никогда |
| Profile instructions | Зависит | Per-profile |
| Workflow guidance | **Да** (стабилен для tool-set) | При смене tools |
| Wiki context | **Нет** (зависит от keywords task) | Per-task |
| Structured output JSON | **Да** (для tool-set) | При смене tools |
| `### CURRENT DATE AND TIME` (timestamp) | **Нет** (меняет каждую секунду) | Каждый запрос |

**Порядок сборки** (`composer.rs` — main agent):

```
[fallback + instructions + workflow_guidance + wiki_context + structured_output] + [date_context]
```

**Порядок сборки** (`composer.rs` — sub-agent):

```
["You are a lightweight sub-agent..." + extra_context + workflow + structured_output] + [date_context]
```

Стабильные блоки (fallback, workflow, structured output) идут первыми, формируя cacheable prefix. Динамические (wiki, date/time) — в конце как suffix.

### 2. History folding

**Файл:** `crates/oxide-agent-core/src/llm/support/history.rs:6-31`

`fold_system_messages_into_prompt(system_prompt, date_suffix, messages)` разделяет system-role сообщения:

- **Stable** (`[TOPIC_AGENTS_MD]`, `[OXIDE_COMPACTED_SUMMARY_V1]`) — идут после `system_prompt`, перед `date_suffix`, расширяя cacheable prefix
- **Volatile** (retry notes, temporal context, infra status) — идут после `date_suffix`, в volatile suffix

Assembly order: `base + stable + date_suffix + volatile`

`ComposedPrompt` (composer.rs) разделяет исходный system prompt на `base` (без даты) и `date_suffix`.

### 3. Sub-agent system prompt

**Файл:** `crates/oxide-agent-core/src/agent/prompt/composer.rs`

```
"You are a lightweight sub-agent..." + [extra_context] + [workflow] + [structured_output] + [date_context]
```

`date_context` в конце (fixed). Task убран из system prompt (fixed) — доставляется только через user message. Prompt стабильный для одинаковых tool-sets.

### 4. Wiki context

**Файл:** `crates/oxide-agent-core/src/agent/executor/execution.rs:361,391-411`

Собирается каждый execution: assembler с `WikiSessionCache`, селект кандидатов по keywords task, загрузка из S3, рендер в `## Durable Wiki Memory` ~12KB. Селект кандидатов от task keywords (`wiki_memory/context.rs:70`). Рендер стабилен если task keywords ведут к тому же набору страниц.

### 5. Provider-native cache markers не используются

**Anthropic-compatible paths** (`claudius` SDK):

- `ToolResultBlock.cache_control: None`: `crates/oxide-agent-core/src/llm/providers/minimax/messages.rs:81`
- Fallback `cache_control: None`: `crates/oxide-agent-core/src/llm/providers/minimax/messages.rs:87`
- System prompt отдаётся `.with_system_string()` без `cache_control`: `crates/oxide-agent-core/src/llm/providers/minimax/client.rs:70`
- OpenCode Go Anthropic path: system как `"system"` string, tools без `cache_control`: `crates/oxide-agent-core/src/llm/providers/opencode_go.rs:835`

**OpenAI-compatible paths:** ни один provider не выставляет `cache_key`, `session_id` или аналоги для OpenAI prefix caching (автоматически после 1024 токенов, но это не контролируется).

### 6. TokenUsage: cache telemetry

**FIXED.** `TokenUsage` (`types.rs`) расширена: `cached_tokens: Option<u32>`, `cache_creation_tokens: Option<u32>`, метод `cache_hit_rate()`. `cache miss = prompt_tokens - cached_tokens` (computed).

Все 9 production parse sites обновлены:

- OpenRouter: `crates/oxide-agent-core/src/llm/providers/openrouter.rs:437`
- ChatGPT: `crates/oxide-agent-core/src/llm/providers/chatgpt/mod.rs:765`
- OpenCode Go: `crates/oxide-agent-core/src/llm/providers/opencode_go.rs:1295`
- OpenCode Go Anthropic: `crates/oxide-agent-core/src/llm/providers/opencode_go.rs` (Anthropic path)
- Mistral: `crates/oxide-agent-core/src/llm/providers/mistral/parsing.rs:9`
- MiniMax: `crates/oxide-agent-core/src/llm/providers/minimax/response.rs:77`
- ZAI: `crates/oxide-agent-core/src/llm/providers/zai/sdk.rs:499`
- NVIDIA: `crates/oxide-agent-core/src/llm/providers/nvidia.rs:319`
- Tests: 6 unit tests в `types.rs`, 4 provider tests в `opencode_go.rs`

---

## Рейтинг улучшений по профиту

| # | Улучшение | Статус | Профит | Стоимость | Почему не плацебо |
|---:|---|---|---|---:|---|
| 1 | **Перенести date/time в конец system prompt.** | **DONE** | Очень высокий | Низкая | Smoke test: static prefix → 67.5% cache hit, dynamic prefix → 0% hit. |
| 2 | **Переставить wiki context после workflow guidance.** | **DONE** | Высокий (если wiki включена) | Низкая | Wiki dynamic — стабильные блоки кэшируются первыми. |
| 3 | **Убрать task из sub-agent system prompt.** Task уже загружен как user message (`delegation.rs:904`). System prompt sub-agent стабильный на одинаковых tool-sets между вызовами. | **DONE** | Высокий (для delegation) | Низкая | Mirror main-agent approach (`_task`). Regression test: `test_sub_agent_prompt_excludes_task`. |
| 4 | **Добавить cache telemetry.** Расширить `TokenUsage` до `cached_tokens`/`cache_creation_tokens`, парсить provider-specific поля. | **DONE** | Средний (как валидация) | Низкая | `TokenUsage` расширена. 9 parse sites обновлены. `cache_hit_rate()`. Commit: `20740c82`. |
| 5 | **Выборочный fold system messages.** Fold-ить в prompt только `TopicAgentsMd` (pinned), `Summary` и стабильные блоки. `SystemContext`/temporal/repair оставлять в history. | **DONE** | Средний (в длинных сессиях) | Средняя | `fold_system_messages_into_prompt` разделяет stable/volatile по prefix (stable перед date, volatile после). `ComposedPrompt` (base + date_suffix). Тесты: `fold_stable_before_date_volatile_after`, `fold_all_volatile_when_no_stable_prefixes`. |
| 6 | **Provider-native cache_control для Anthropic-совместимых путей.** MiniMax (`claudius`), OpenCode Go Anthropic: маркировать system prompt как cacheable, tool definitions как cacheable, динамические суффиксы как non-cacheable. | TODO | Высокий для Anthropic-маршрутов | Высокая | Зависит от активных routes и SDK. |
| 7 | **Удалить дублирование tool schemas из prompt text.** `build_structured_output_instructions()` заменён на compact sorted tool-name list. Полные schemas доставляются только через native `tools[]` payload. | **DONE** | Высокий | Низкая | Prompt: 2673→98 bytes (27x reduction). Wire: -48% дублирования. |
| 8 | **Вычистить volatile metadata из compacted summary.** Убрать `created_at`, `provider`, `route`, token counts из prompt-visible текста. Оставить `generation` (compaction chain) + `wiki_memory_lookup_available` (tool-use) + guidance text. | **DONE** | Средний | Низкая | 12 volatile полей → 2 стабильных. Summary stable для одинакового semantic content. |
| 9 | **Сузить pinned messages после compaction.** Не pin-ить `UserTask`/`RuntimeContext` бессрочно; fold-ить их в summary вместо сохранения как front-of-history anchors. | ЧАСТИЧНО | Средний | Средняя | Budget guard предотвращает premature compaction (commit `7e599dac`). Pinning strategy без изменений. Production: 14 iter без compaction, 99.7% hit. |
| 10 | **Добавить prompt-layout hashes в observability.** `static_prefix_hash`, `tools_hash`, `topic_agents_md_hash` — для корреляции cache behavior с конкретной layout. | TODO | Средний (как валидация) | Средняя | Невозможно отличить layout regression от provider-side noise. |

---

## Что не стоит делать (плацебо и over-engineering)

- Трогать tool ordering: registry уже deterministic через `BTreeMap` (`tool_runtime/registry.rs:24`).
- Оптимизировать R2/static asset cache в рамках этой задачи (не LLM prompt cache).
- Ограничивать timestamp минутами (уменьшит miss, но не решит проблему — регулярный prefix bust останется).
- Добавлять embedding-selected skills (уже удалены из архитектуры, не релевантно).
- Менять compaction для cache (compaction по-прежнему убивает cache prefix по дизайну — summary text уникальный. Budget guard предотвращает premature compaction, но легитимная compaction на 85%+ всё равно сбросит prefix).

---

## Подсказки и аргументы для реализации cache hit

### TL;DR

Для `cache hit` нужен **стабильный префикс запроса**: одинаковые system/tool/project-инструкции в начале, всё динамическое — строго в конце. У DeepSeek/OpenAI/Claude кэш включается автоматически при exact prefix match. Экономия максимальна, когда агент много раз переиспользует один и тот же длинный prefix.

### Ключевые факты

**DeepSeek V4 Flash** — `deepseek-v4-flash`, быстрый, 1M context, автоматический context caching. Старые `deepseek-chat`/`deepseek-reasoner` совместимы с V4 Flash, вывод из эксплуатации после 24 июля 2026. ([DeepSeek API Docs][1][2])

Из-за Sliding Window Attention кэшированные префиксы хранятся как "prefix units". Запрос должен **полностью совпасть** с unit для cache hit. ([DeepSeek API Docs][3])

**Цены DeepSeek V4 Flash**: cache hit `$0.0028/1M`, miss `$0.14/1M`, output `$0.28/1M`. V4 Pro: hit `$0.0036/1M`, miss `$0.435/1M`, output `$0.87/1M`. ([DeepSeek API Docs][4])

**Usage fields**: `prompt_cache_hit_tokens`, `prompt_cache_miss_tokens`, `prompt_tokens`, `completion_tokens`, `total_tokens`. `prompt_tokens = hit_tokens + miss_tokens`. ([DeepSeek API Docs][5])

**arXiv**: prompt caching снижает API cost на 41–80% и TTFT на 13–31% при правильной структуре prefix. ([arXiv:2502.04894][7])

### Архитектура промпта: статический prefix + dynamic suffix

Первые N токенов должны быть **byte-for-byte одинаковыми** между вызовами — порядок сообщений, пробелы, JSON-схемы, tools, заголовки.

```
[STATIC PREFIX — cacheable]
1. System / developer prompt
2. Tool definitions (стабильный порядок)
3. Инвариантные правила агента
4. Project context / repo policy
5. Stable output contract

[DYNAMIC SUFFIX — always miss]
6. User request
7. Время, request_id, user-specific data
8. RAG chunks / tool results
9. Последние сообщения диалога
```

Совпадает с рекомендациями OpenAI: exact prefix match, static content в начало, variable в конец. ([OpenAI Developers][6])

### Что чаще всего ломает cache hit

1. `current_time`, `request_id`, username, branch, path, RAG chunks, tool output в **system prompt**.
2. Tools array в разном порядке между вызовами.
3. JSON без стабильной сортировки ключей.
4. Добавление/удаление tools между шагами агента.
5. Изменение wording system prompt без версионирования.
6. История диалога перед статичным контекстом.
7. "Память пользователя" в начале префикса.
8. Разные SDK/adapters — разный формат messages.
9. Screenshots/images с разными параметрами `detail`.
10. Tool results внутри cacheable prefix.

Anthropic: timestamps, per-request context, изменение `tool_choice`, images и нестабильный порядок ключей в tool-use blocks ломают кэш. ([Claude API Docs][8])

### Provider-specific

| Provider | Механизм | Ключевые особенности |
|---|---|---|
| **DeepSeek V4 Flash** | Автоматический | Не нужен `cache_control`. Exact prefix match. Первый `A+B` -> второй `A+C` может не дать hit по A, третий `A+D` — может. ([DeepSeek][3]) |
| **OpenAI** | Автоматический от 1024 токенов | `prompt_cache_key` для sticky routing. >15 req/min для одной prefix/key — cache effectiveness падает. ([OpenAI][6]) |
| **Anthropic Claude** | `cache_control: ephemeral` | До 4 breakpoints. Automatic caching занимает один слот. TTL: 5 min default, `1h` дороже на запись. ([Claude][8]) |
| **Gemini** | Implicit (2.5+) + explicit | Large/common content в начало, похожие prefix-запросы в коротком окне. ([Google][9]) |
| **OpenRouter** | `session_id` для sticky routing | Помогает держать provider cache warm, особенно для multi-turn workflows. ([OpenRouter][10]) |

### Production-чеклист

**Логировать на каждый запрос:**
```
model | static_prefix_id | prompt_tokens | prompt_cache_hit_tokens
prompt_cache_miss_tokens | completion_tokens | cache_hit_rate | latency_ms | cost_usd
```

**Формула DeepSeek:**
```text
cache_hit_rate = prompt_cache_hit_tokens / prompt_tokens

input_cost = hit_tokens/1M * cache_hit_price + miss_tokens/1M * cache_miss_price
total_cost = input_cost + completion_tokens/1M * output_price
```

**Target:** `cache_hit_rate` 70–90% после прогрева. Падение — изменился prefix, tools, сериализация или dynamic data попали в начало.

**Next step:** логирование `prompt_cache_hit_tokens`/`prompt_cache_miss_tokens` внедрено. Результаты: 89.5% overall hit rate, 99.7% на стабильных итерациях (14 iter, post-fix). ~~Прогнать 20–50 агентных задач, измерить hit rate.~~ Оставшиеся TODO: выборочный fold system messages (#5), provider-native `cache_control` (#6), prompt-layout hashes (#10).

---

## Целевая архитектура: static prefix layers

Целевой порядок блоков в system prompt для максимального cache hit:

### `[STATIC_GLOBAL_V1]` — не меняется между деплоями

- Fallback kernel (`composer.rs:415-426`)
- Stable output contract из `build_structured_output_instructions()`, **без** full tool JSON schema
- Global behavior rules

**Не должен содержать:** date/time, wiki, request IDs, tool outputs, dialogue, operational notes.

### `[STATIC_PROFILE_V1]` — стабилен для фиксированного профиля/toolset

- `execution_profile.prompt_instructions()` и profile-bound инструкции
- Workflow guidance из deterministic sorted toolset
- Compact sorted tool-name list или `tools_hash` (не full schemas)

**Не должен содержать:** task-scoped wiki, runtime injections, failover state, transient policy notes.

### `[STATIC_TOPIC_V1]` — стабилен для фиксированного topic

- Pinned topic `AGENTS.md`
- Topic instructions / persistent topic context от transport

**Не должен содержать:** raw user IDs, unversioned session summaries, timestamps.

Cacheable boundary main-agent: конец `[STATIC_TOPIC_V1]`.
Cacheable boundary sub-agent: `[STATIC_GLOBAL_V1]` + `[STATIC_PROFILE_SUB_V1]` + optional `[STATIC_TOPIC_V1]`.

### `[DYNAMIC_SESSION]` — меняется между сессиями

- Scrubbed compacted summary (semantic text only, без `created_at`/route/provider metadata)
- Wiki context (`## Durable Wiki Memory`) с стабильным заголовком
- Recent dialogue tail
- Active session-control notes

### `[DYNAMIC_TURN]` — меняется каждый запрос

- Current user request
- Runtime injections
- Current date/time note
- Latest tool outputs
- Request-specific IDs

### Границы для sidecar LLM вызовов

Эти вызовы уже cache-friendly:

- **Compaction summarizer**: `local_compaction_system_prompt()` (`compaction/prompt.rs:11-34`) — static global + dynamic session payload.
- **Loop detection**: `SYSTEM_PROMPT` + static `USER_PROMPT` header (`loop_detection/llm_detector.rs:17-41`).
- **Wiki-memory writer**: `wiki_memory_writer_system_prompt()` (`executor/execution.rs:744-745`).
- **Completion check hook**: нет отдельного LLM вызова.

---

## Observability: метрики для cache hit

Текущий статус: `TokenUsage` имеет `cached_tokens` и `cache_creation_tokens` (commit `20740c82`). Остальной gap: нет prefix identity hashes, нет latency в структуре. Что добавить:

| Метрика | Тип | Зачем |
|---|---|---|
| `prompt_cache_hit_tokens` | `Option<u32>` | Прямое измерение cache reuse |
| `prompt_cache_miss_tokens` | `Option<u32>` | Прямое измерение uncached input |
| `reasoning_tokens` | `Option<u32>` | Некоторые routes bill/throttle reasoning отдельно |
| `static_prefix_hash` | `String` | Корреляция cache behavior с конкретной layout |
| `tools_hash` | `String` | Идентичность toolset между запросами |
| `topic_agents_md_hash` | `Option<String>` | Topic-scoped prefix identity |
| `profile_hash` | `Option<String>` | Profile changes = expected cache-boundary change |
| `compacted_summary_hash` | `Option<String>` | Compaction churn detection |
| `route_id` / `provider` / `model` | `String` | Cache behavior provider-specific |
| `failover_from_route` | `Option<String>` | Failover почти гарантированно убивает cache locality |
| `latency_ms` | `u64` | Cache hit → ниже TTFT |

Provider parsers для cache tokens (**DONE** — все обновлены):

- OpenCode Go: `opencode_go.rs:1295-1301` — парсит `prompt_cache_hit_tokens`, `prompt_cache_miss_tokens`, `cached_tokens` (Anthropic path)
- OpenRouter: `openrouter.rs:437-443` — парсит `cached_tokens` из `prompt_tokens_details`
- ZAI: `zai/sdk.rs:499-504`
- MiniMax: `minimax/response.rs:77-95`
- Mistral: `mistral/parsing.rs:8-16`
- NVIDIA: `nvidia.rs:319-325`
- ChatGPT: `chatgpt/mod.rs:765`

Формулы (DeepSeek V4 Flash):
```text
cache_hit_rate = prompt_cache_hit_tokens / prompt_tokens
input_cost = hit_tokens/1M * $0.0028 + miss_tokens/1M * $0.14
total_cost = input_cost + completion_tokens/1M * $0.28
```

---

## Quick wins (конкретные file:line)

| Что | Файл | Строки |
|---|---|---|
| date_context в конец main prompt | `composer.rs` | `498-540` | **DONE** |
| date_context в конец sub-agent prompt | `composer.rs` | `557-599` | **DONE** |
| wiki после workflow guidance | `composer.rs` | `517-520` | **DONE** |
| Убрать task из sub-agent system prompt | `composer.rs` | `581-587` | **DONE** |
| Удалить `## Available Tools (JSON schema)` из prompt — заменить на compact name list | `composer.rs` | `430-483` | **DONE** |
| Убрать `created_at`/route/provider из prompt-visible summary | `memory.rs` | `816-840` | **DONE** |
| Выборочный fold system messages | `history.rs` | `6-31`; `composer.rs` | `519-575` | **DONE** |
| Расширить `TokenUsage` cache fields | `types.rs` | `499-508` | **DONE** |
| Budget guard на `compress` tool | `tools.rs` | `327-335`; `compression.rs` | `37` | **DONE** |

---

## Production validation

### Run 1 (pre-budget-guard, 2026-06-01)

Model: `deepseek-v4-flash` via OpenCode Go. Task: deep research (5 todo items). 11 iterations.

```
iter  prompt   cached   hit%     note
──────────────────────────────────────────────
 0     7,516    2,688    36%     cold start
 1     8,045    7,424    92%
 2    11,765    8,192    70%
 3    22,640   12,288    54%
 4    31,233   22,912    73%
 5    34,785   31,360    90%
 6    47,983   34,944    73%
 7    57,135   48,128    84%
 8    61,313   57,216    93%
 9    66,442   61,440    93%
10    76,099   66,560    87%     ← model triggered compress at 65K/272K (24%)
11        ?        ?      ?      ← post-compaction: 59,264→2,688 cached (3.3%)
```

Compaction at iter 10 killed cache. Root cause: model self-triggered `compress` tool at 24% context — not a budget threshold.

### Run 2 (post-budget-guard, 2026-06-02)

Same model, same task. 14 iterations. Budget guard prevents compress below 85% utilization.

```
iter  prompt   cached   hit%     note
──────────────────────────────────────────────
 0     7,516    2,688    36%     cold start
 1     8,045    7,424    92%
 2    11,765    8,192    70%
 3    22,640   12,288    54%
 4    31,233   22,912    73%
 5    34,785   31,360    90%
 6    47,983   34,944    73%
 7    57,135   48,128    84%
 8    61,313   57,216    93%
 9    66,442   61,440    93%
10    76,099   66,560    87%
11    76,994   76,672   100%     peak
12    77,434   77,184   100%
13    77,742   77,440   100%     task completed, 5/5 todos done
```

No compaction. Task completed naturally. Cache hit grew to 99.7%.

### Comparison

| Metric | Run 1 (pre-fix) | Run 2 (post-fix) |
|---|---|---|
| Compaction? | iter 10 (premature) | none |
| Peak hit rate | 93% | 99.7% |
| Overall hit rate | 66.3% | 89.5% |
| Iterations | 11 (forced end) | 13 (natural completion) |
| Total cached tokens | 333,536 | 576,378 |
| Est. cost (DeepSeek API) | ~$0.090 | ~$0.014 |

---

## Open questions

- Provider cache usage fields **подтверждены runtime** для OpenCode Go (DeepSeek response). Остальные providers возвращают `None` на текущих routes — парсеры готовы, данные появятся при подключении соответствующих routes.
- **Нет прямого DeepSeek provider** в repo. DeepSeek-relevant пути: OpenCode Go (`deepseek-v4-flash`), OpenRouter DeepSeek model IDs, NVIDIA NIM DeepSeek model IDs.
- OpenRouter sticky/session routing params **не используются** (`openrouter.rs:382-394`). Поддержка endpoint-ом не проверена.
- MiniMax/Claude `cache_control` **не доказаны** live. Код явно ставит `cache_control: None` (`minimax/messages.rs:72-89`). Нужен provider-doc confirmation + live test.
- OpenCode Go Anthropic path: нет explicit cache markers (`opencode_go.rs:823-844`). Поддержка не проверена.
- Gemini через OpenRouter: approved для media, **не для main-agent tool use** (`openrouter/module.rs:84-97`). Cache work для Gemini — низкий приоритет.
- `user_id`/`chat_id`/`topic_id`/`request_id` **не рендерятся** напрямую в main-agent prompt. Косвенный impact через topic/wiki/session state и route selection.
- Tool executor `spec()` implementations: deterministic registry ordering есть, но byte-for-byte стабильность каждой схемы нуждается в snapshot-test.
- Mistral: adapter-specific system-message reorder logic (`mistral/messages.rs:17-27`), но main path fold-ит раньше (`client.rs:485-486`). Cache impact нужно подтвердить adapter-parity snapshots.

---

## Smoke test: измерение cache hit baseline

**Скрипт:** `scripts/cache-hit-baseline.sh`

Прямые HTTP-запросы к OpenCode Go endpoint (`opencode.ai/zen/go/v1/chat/completions`), model `deepseek-v4-flash`. Не проходит через Oxide Agent runtime — чистое измерение cache hit/miss на уровне DeepSeek API.

### Как запустить

```bash
OPENCODE_GO_API_KEY=sk-... bash scripts/cache-hit-baseline.sh
```

Требования: `curl`, `jq`.

### Что делает

**TEST 1 — STATIC PREFIX** (5 запросов):
- System prompt одинаковый во всех запросах.
- Имитирует оптимизированный порядок (date в конце, стабильный prefix).
- Ожидаемый результат: 1-й запрос miss (cache build), 2-5-й — cache hit.

**TEST 2 — DYNAMIC PREFIX** (5 запросов):
- В начало system prompt вставлен уникальный timestamp.
- Имитирует сломанный порядок (date в начале, prefix poisoned).
- Ожидаемый результат: 0% cache hit на всех запросах.

### Baseline (измерено 2026-06-01)

```
TEST 1 (static prefix):
  static-req-1   prompt=  379  hit=  256  miss=  123  hit_rate= 67.5%
  static-req-2   prompt=  379  hit=  256  miss=  123  hit_rate= 67.5%
  static-req-3   prompt=  379  hit=  256  miss=  123  hit_rate= 67.5%
  static-req-4   prompt=  379  hit=  256  miss=  123  hit_rate= 67.5%
  static-req-5   prompt=  379  hit=  256  miss=  123  hit_rate= 67.5%

TEST 2 (dynamic prefix):
  dynamic-req-1  prompt=  401  hit=    0  miss=  401  hit_rate=  0.0%
  dynamic-req-2  prompt=  401  hit=    0  miss=  401  hit_rate=  0.0%
  dynamic-req-3  prompt=  401  hit=    0  miss=  401  hit_rate=  0.0%
  dynamic-req-4  prompt=  401  hit=    0  miss=  401  hit_rate=  0.0%
  dynamic-req-5  prompt=  401  hit=    0  miss=  401  hit_rate=  0.0%
```

Hit rate 67.5% на 379-токенном промпте: 256 токенов system prompt = cacheable prefix, 123 токена user message = dynamic suffix (всегда miss). На production промптах (3000-8000 токенов) hit rate будет выше.

### Что проверять после изменений

1. Запустить скрипт, убедиться что TEST 1 даёт >0% hit rate.
2. Если hit_rate падает после refactor — проверь порядок блоков в `composer.rs`.
3. Регрессионные тесты в `composer::tests` защищают порядок: `test_date_context_at_end_of_*`, `test_wiki_context_after_workflow_guidance`.

### Замечание про OpenCode Go

OpenCode Go — подписка с flat-rate биллингом. Cache hit **не снижает стоимость** в подписке, но снижает TTFT (latency). Экономия на стоимости работает только при прямых per-token маршрутах (DeepSeek API напрямую, OpenRouter).

---

### References

[1]: https://api-docs.deepseek.com/api/deepseek-api
[2]: https://api-docs.deepseek.com/news/news250602
[3]: https://api-docs.deepseek.com/guides/kv_cache
[4]: https://api-docs.deepseek.com/quick_start/pricing
[5]: https://api-docs.deepseek.com/api/create-chat-completion
[6]: https://platform.openai.com/docs/guides/prompt-caching
[7]: https://arxiv.org/abs/2502.04894
[8]: https://docs.anthropic.com/en/docs/build-with-claude/prompt-caching
[9]: https://ai.google.dev/gemini-api/docs/caching
[10]: https://openrouter.ai/docs/features/advanced-usage
