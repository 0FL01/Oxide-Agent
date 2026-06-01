# Prompt Cache Hit Analysis

## Текущие узкие места (почему cache не работает в Oxide Agent)

1.  **date/time блок в начале system prompt.** `chrono::Local::now()` с секундами генерирует `### CURRENT DATE AND TIME` первым блоком. Prefix/KV cache у любого LLM провайдера ломается уже с первой строки — cache miss гарантирован для каждого запроса, даже если остальные 99% prompt стабильны.

2.  **Wiki context вставляется до стабильных операционных блоков.** `## Durable Wiki Memory` попадает в середину system prompt, до workflow guidance и structured output. Если wiki меняется между запросами (разные keywords, разные загруженные страницы), cache не доживает до больших стабильных секций с tool JSON.

3.  **Sub-agent включает task в system prompt.** `Your task: {task}` делает каждый sub-agent prompt уникальным, хотя task уже есть как user message в history. Для частых `spawn_sub_agents` полностью убивает cache hit.

4.  **System messages из history fold-ятся в prompt без разбора.** `fold_system_messages_into_prompt()` добавляет system messages (temporal context, compacted summaries, retry notes) в начало system prompt. Нестабильные system messages размывают prefix.

5.  **Provider-native prompt caching не используется.** Anthropic-compatible пути (MiniMax через `claudius`, OpenCode Go Anthropic path) не выставляют `cache_control` markers, хотя SDK их поддерживают. OpenAI/OpenRouter не имеют explicit cache markers в запросах.

6.  **Нет telemetry по cache read/write tokens.** `TokenUsage` хранит только `prompt_tokens`/`completion_tokens`; cache read tokens нигде не парсятся. Невозможно измерить, есть ли hit хоть от какой-то оптимизации.

---

## Детальный RECON: как собираются LLM-запросы и что мешает cache hit

### 1. Сборка system prompt

**Файл:** `crates/oxide-agent-core/src/agent/prompt/composer.rs`

| Блок | Строки | Cacheable? | Частота изменений |
|---|---|---|---|
| `### CURRENT DATE AND TIME` (timestamp) | 11-42 | **Нет** (меняет каждую секунду) | Каждый запрос |
| Core fallback prompt | 417-426 | **Да** (полностью статичен) | Никогда |
| Profile instructions | 510-515 | Зависит | Per-profile |
| Wiki context | 517-521 | **Нет** (зависит от keywords task) | Per-task |
| Workflow guidance | 112-412 | **Да** (стабилен для tool-set) | При смене tools |
| Structured output JSON | 439-482 | **Да** (для tool-set) | При смене tools |

**Порядок сборки** (`composer.rs:498-541`):

```
[date_context] + [fallback + instructions + wiki_context + workflow_guidance + structured_output]
```

**Критическая проблема:** `date_context` открывает prompt — гарантированный cache miss.

### 2. History folding

**Файл:** `crates/oxide-agent-core/src/llm/support/history.rs:6-31`

`fold_system_messages_into_prompt()` добавляет system-role messages из history в конец system prompt. Сюда попадают:

- `[TOPIC_AGENTS_MD]\n...` — стабильно, pinned, ок
- `[SYSTEM: ...]` — retry/repair notes, нестабильно
- `[TEMPORAL_CONTEXT]` — только при паузе >2h, редко
- Compacted summaries — pinned, но разные после каждого compaction

### 3. Sub-agent system prompt

**Файл:** `crates/oxide-agent-core/src/agent/prompt/composer.rs:559-599`

```
[date_context] + "You are a lightweight sub-agent..." + "Your task: {task}" + [extra_context] + [workflow] + [structured_output]
```

Task делается частью system prompt, хотя task уже загружен как `AgentMessage::user_task(task)` в `crates/oxide-agent-core/src/agent/providers/delegation.rs:904`.

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

### 6. TokenUsage не учитывает cache

**Тип:** `crates/oxide-agent-core/src/llm/types.rs:501` — только `prompt_tokens`, `completion_tokens`, `total_tokens`.

Парсеры не смотрят на `cached_tokens` / `cache_read_input_tokens` / `prompt_tokens_details`:

- OpenRouter: `crates/oxide-agent-core/src/llm/providers/openrouter.rs:437`
- ChatGPT: `crates/oxide-agent-core/src/llm/providers/chatgpt/mod.rs:765`
- OpenCode Go: `crates/oxide-agent-core/src/llm/providers/opencode_go.rs:1295`
- Mistral: `crates/oxide-agent-core/src/llm/providers/mistral/parsing.rs:9`
- MiniMax: `crates/oxide-agent-core/src/llm/providers/minimax/response.rs:77`

---

## Рейтинг улучшений по профиту

| # | Улучшение | Профит | Стоимость | Почему не плацебо |
|---:|---|---|---:|---|
| 1 | **Перенести date/time в конец system prompt.** Поменять порядок в `composer.rs:506-540` так, чтобы timestamp был после стабильных блоков. Править reminder guidance в `composer.rs:286`. | Очень высокий | Низкая | Убирает главный cache poison из начала prefix. Без этого все остальные оптимизации бесполезны. |
| 2 | **Переставить wiki context после workflow guidance и structured output.** Wiki вставляется в `composer.rs:517` до workflow `:529` и structured output `:535`. Если wiki разная, cache не доходит до больших стабильных секций. | Высокий (если wiki включена) | Средняя | Wiki dynamic — пусть будет позже, чтобы стабильные блоки успели кешироваться первыми. |
| 3 | **Убрать task из sub-agent system prompt.** Task уже загружен как user message (`delegation.rs:904`). System prompt sub-agent станет стабильным на одинаковых tool-sets между вызовами. | Высокий (для delegation) | Низкая | Сейчас каждый sub-agent prompt уникален по task. |
| 4 | **Добавить cache telemetry.** Расширить `TokenUsage` до `cache_read_tokens`/`cache_write_tokens`, парсить provider-specific поля. | Средний (как валидация) | Низкая | Без этого любые изменения — гадание. Помогает отличить реальный hit от шума. |
| 5 | **Выборочный fold system messages.** Fold-ить в prompt только `TopicAgentsMd` (pinned), `Summary` и стабильные блоки. `SystemContext`/temporal/repair оставлять в history, не размывать prefix. | Средний (в длинных сессиях) | Средняя | Меньше динамики в prefix. |
| 6 | **Provider-native cache_control для Anthropic-совместимых путей.** MiniMax (`claudius`), OpenCode Go Anthropic: маркировать system prompt как cacheable, tool definitions как cacheable, динамические суффиксы как non-cacheable. | Высокий для Anthropic-маршрутов | Высокая | Зависит от активных routes и SDK. Глубокая интеграция — не все SDK expose cache_control на уровне `MessageCreateParams`. |

---

## Что не стоит делать (плацебо и over-engineering)

- Трогать tool ordering: registry уже deterministic через `BTreeMap` (`tool_runtime/registry.rs:24`).
- Оптимизировать R2/static asset cache в рамках этой задачи (не LLM prompt cache).
- Ограничивать timestamp минутами (уменьшит miss, но не решит проблему — регулярный prefix bust останется).
- Добавлять embedding-selected skills (уже удалены из архитектуры, не релевантно).
- Менять compaction для cache (compaction не влияет на prompt prefix — он меняет history, а prefix формируется из system prompt).

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

**Next step:** внедрить логирование `prompt_cache_hit_tokens`/`prompt_cache_miss_tokens`, зафиксировать порядок messages и tools, прогнать 20–50 агентных задач, измерить hit rate.

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
