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

Для `cache hit` не нужен "магический промпт"; нужен **стабильный префикс запроса**: одинаковые system/tool/project-инструкции в начале, всё динамическое — строго в конце. Для DeepSeek V4 Flash кэш включён автоматически, а экономия максимальна, когда агент много раз переиспользует один и тот же длинный prefix.

### Главное, что нашёл

DeepSeek V4 Flash реально существует в официальной документации: `deepseek-v4-flash` — быстрый и экономичный вариант V4, с 1M context; старые `deepseek-chat` и `deepseek-reasoner` сейчас совместимы с V4 Flash, но помечены к будущему выводу из эксплуатации после 24 июля 2026. ([DeepSeek API Docs][1]) ([DeepSeek API Docs][2])

У DeepSeek context caching включён автоматически: каждый запрос может создавать disk cache, а последующие запросы получают `cache hit`, если их начало совпадает с уже сохранённым префиксом. Важная деталь V4: из-за Sliding Window Attention кэшированные префиксы хранятся как самостоятельные "prefix units", и новый запрос должен **полностью совпасть** с таким unit, иначе hit может не произойти. ([DeepSeek API Docs][3])

По цене это стоит оптимизировать агрессивно: на официальной странице DeepSeek V4 Flash input cache hit стоит `$0.0028 / 1M`, cache miss — `$0.14 / 1M`, output — `$0.28 / 1M`; у V4 Pro cache hit — `$0.003625 / 1M`, miss — `$0.435 / 1M`, output — `$0.87 / 1M`. ([DeepSeek API Docs][4])

DeepSeek возвращает в usage именно те поля, которые надо логировать: `prompt_cache_hit_tokens`, `prompt_cache_miss_tokens`, `prompt_tokens`, `completion_tokens`, `total_tokens`; `prompt_tokens = prompt_cache_hit_tokens + prompt_cache_miss_tokens`. ([DeepSeek API Docs][5])

### Базовая архитектура промпта для высокого `cache hit`

Твоя цель — сделать так, чтобы первые N токенов запроса были **byte-for-byte одинаковыми** между вызовами. Не "похожими", не семантически одинаковыми, а одинаковыми по порядку сообщений, пробелам, JSON-схемам, tools, examples, markdown-заголовкам и переменным.

Правильная структура:

```text
[STATIC PREFIX — cacheable]
1. System / developer prompt
2. Tool definitions, в одном и том же порядке
3. Инвариантные правила агента
4. Статичный project context / repo policy / coding rules
5. Few-shot examples, если нужны
6. Stable output contract

[DYNAMIC SUFFIX — not cache-friendly]
7. Текущий user request
8. Время, request_id, tenant/user-specific data
9. RAG chunks
10. Tool results
11. Последние сообщения диалога, если они разные
```

Это совпадает с рекомендациями OpenAI: cache hits возможны только для exact prefix matches, поэтому static content надо ставить в начало, а user-specific / variable content — в конец; tools и images тоже должны быть одинаковыми между запросами. ([OpenAI Developers][6])

### Готовый cache-friendly system prompt для DeepSeek V4 Flash

Ниже — промпт-ядро. Его смысл не в том, что он "включает кэш" — кэш включает провайдер. Смысл в том, что он делает поведение агента стабильным и помогает не тащить динамику в начало контекста.

```text
# AGENT_KERNEL_V1.0_DO_NOT_EDIT

You are an autonomous engineering agent optimized for reliable, low-cost API execution.

Core objective:
- Solve the user's task accurately.
- Keep reasoning focused and avoid unnecessary verbosity.
- Prefer simple, auditable steps over speculative complexity.
- Use tools only when they materially improve correctness.

Cache discipline:
- Treat this entire kernel as immutable.
- Do not request changes to the static prefix.
- Do not ask the orchestrator to insert timestamps, request IDs, user metadata, tool outputs, or retrieved documents before this kernel.
- All dynamic task data must appear after the static project context and after tool definitions.
- Never duplicate large tool results back into future prompts unless they are summarized into a stable task memory block.

Tool discipline:
- Use tools only for fresh facts, files, code execution, external state, or verification.
- When a tool result is large, summarize only the durable facts needed for the next step.
- Keep tool-result summaries short, factual, and scoped to the current task.

Output discipline:
- Answer in the user's language unless the task requires another language.
- Start with the conclusion.
- Then provide the reasoning, commands, code, or next actions.
- Be explicit about uncertainty and missing information.
- Do not invent facts, APIs, versions, prices, or undocumented behavior.

Failure handling:
- If a task cannot be completed fully, provide the best partial result.
- State what was verified, what was assumed, and what remains unknown.
```

### Статичный project context block

Этот блок тоже должен быть одинаковым между запросами. Меняй его редко и версионируй. Не вставляй сюда текущую дату, путь текущего файла, branch name, user id, свежие RAG-чанки или результат последнего tool call.

```text
# PROJECT_CONTEXT_V1.0_DO_NOT_EDIT

Agent role:
- DevOps / AgentOps assistant.
- Primary domains: CI/CD, containers, Kubernetes, observability, LLM agents, prompt/runtime optimization, API cost control.

Engineering defaults:
- Prefer reproducible commands.
- Prefer idempotent infrastructure changes.
- Separate diagnosis from mutation.
- Never run destructive operations without explicit user intent.
- For shell commands, include safety checks where useful.
- For code, prefer small composable changes over large rewrites.

Cost-control defaults:
- Minimize output tokens.
- Avoid repeating long context in answers.
- Summarize durable state after tool-heavy steps.
- Keep dynamic data out of the cacheable prefix.
- Prefer stable templates and deterministic serialization.
```

### Dynamic suffix template

Вот сюда уже кладётся текущая задача. Этот блок может меняться; он будет cache miss, и это нормально.

```text
# RUNTIME_REQUEST

request_type: <debug|build|research|code_review|ops_task|other>
user_goal:
"""
{USER_TASK}
"""

runtime_constraints:
- current_time: {CURRENT_TIME_IF_NEEDED_ONLY}
- environment: {ENVIRONMENT_IF_RELEVANT}
- budget_priority: minimize unnecessary input and output tokens
- freshness_required: {yes|no}

available_dynamic_context:
"""
{RAG_CHUNKS_OR_TOOL_RESULTS_OR_EMPTY}
"""

required_response_style:
- Start with TL;DR.
- Be direct.
- No tables unless explicitly requested.
```

### Tool result summarizer prompt

Это полезно для long-horizon agents. Исследование по prompt caching для агентных задач показало, что стратегическое управление cache blocks, вынос динамического контента в конец system prompt и исключение динамических tool results даёт более стабильную экономию, чем наивное кэширование всего контекста; в их тестах prompt caching снижал API cost на 41–80% и TTFT на 13–31%. ([arXiv][7])

Используй такой prompt для промежуточного сжатия tool outputs:

```text
# TOOL_RESULT_COMPACTOR_V1.0

Summarize the tool result for future agent steps.

Rules:
- Keep only facts needed to continue the current task.
- Remove logs, stack traces, duplicate lines, irrelevant metadata, and timestamps unless they explain the failure.
- Preserve exact error messages only when they are diagnostic.
- Preserve filenames, versions, commands, exit codes, URLs, IDs, and config keys when relevant.
- Do not include raw secrets, tokens, credentials, cookies, or private keys.
- Output maximum 12 bullets.
```

### DeepSeek API pattern: stable prefix + dynamic tail

```python
from openai import OpenAI
import os

client = OpenAI(
    api_key=os.environ["DEEPSEEK_API_KEY"],
    base_url="https://api.deepseek.com",
)

STATIC_SYSTEM_PROMPT = """# AGENT_KERNEL_V1.0_DO_NOT_EDIT
You are an autonomous engineering agent optimized for reliable, low-cost API execution.
... keep this string byte-for-byte stable ...
"""

STATIC_PROJECT_CONTEXT = """# PROJECT_CONTEXT_V1.0_DO_NOT_EDIT
Agent role:
- DevOps / AgentOps assistant.
... keep this string byte-for-byte stable ...
"""

def build_dynamic_tail(user_task: str, dynamic_context: str = "") -> str:
    return f"""# RUNTIME_REQUEST

user_goal:
\"\"\"
{user_task}
\"\"\"

available_dynamic_context:
\"\"\"
{dynamic_context}
\"\"\"

required_response_style:
- Start with TL;DR.
- Be direct.
- No tables unless explicitly requested.
"""

response = client.chat.completions.create(
    model="deepseek-v4-flash",
    messages=[
        {"role": "system", "content": STATIC_SYSTEM_PROMPT},
        {"role": "user", "content": STATIC_PROJECT_CONTEXT},
        {"role": "user", "content": build_dynamic_tail("Diagnose failed Kubernetes rollout")},
    ],
    stream=False,
    extra_body={"thinking": {"type": "disabled"}},
)

usage = response.usage
hit = getattr(usage, "prompt_cache_hit_tokens", 0)
miss = getattr(usage, "prompt_cache_miss_tokens", 0)
prompt = getattr(usage, "prompt_tokens", hit + miss)

hit_rate = hit / prompt if prompt else 0

print({
    "prompt_tokens": prompt,
    "cache_hit_tokens": hit,
    "cache_miss_tokens": miss,
    "cache_hit_rate": round(hit_rate, 4),
})
```

В DeepSeek usage fields официально включают `prompt_cache_hit_tokens` и `prompt_cache_miss_tokens`, так что это надо логировать на каждый request и выводить в метрики вроде Prometheus/Grafana. ([DeepSeek API Docs][5])

### Что чаще всего ломает cache hit

Самые дорогие ошибки:

1. Вставлять `current_time`, `request_id`, username, branch, path, RAG chunks или tool output в system prompt.
2. Генерировать tools array в разном порядке.
3. Сериализовать JSON без стабильной сортировки ключей.
4. Добавлять/убирать tools между шагами агента.
5. Менять wording system prompt при каждом деплое без версии.
6. Вставлять длинную историю диалога перед статичным контекстом.
7. Добавлять "память пользователя" в начало префикса.
8. Использовать разные SDK/adapters, которые по-разному форматируют messages.
9. Подмешивать screenshots/images с разными параметрами detail.
10. Класть tool results внутрь cacheable prefix.

Anthropic отдельно предупреждает, что timestamps, per-request context и incoming message перед cache breakpoint приводят к miss; также изменение `tool_choice`, наличие/отсутствие images и нестабильный порядок ключей в tool-use blocks могут ломать кэш. ([Claude API Docs][8])

### Provider-specific рекомендации

Для **DeepSeek V4 Flash**: кэш автоматический, explicit `cache_control` не нужен. Делай одинаковый начальный prefix и логируй hit/miss tokens. Учитывай нюанс DeepSeek: первый `A+B`, второй `A+C` может не дать hit по `A`, но система может сохранить общий prefix `A`, и третий `A+D` уже сможет hit'нуть его. ([DeepSeek API Docs][3])

Для **OpenAI models**: prompt caching включается автоматически для prompt от 1024 tokens, exact prefix match обязателен, а `prompt_cache_key` может помочь маршрутизировать похожие запросы к одному cache pool. Но не делай ключ слишком широким при высоком QPS: OpenAI указывает, что при частоте выше примерно 15 requests/min для одной prefix/key комбинации часть запросов может уходить на другие машины, снижая cache effectiveness. ([OpenAI Developers][6])

Для **Anthropic Claude**: используй `cache_control`. Лучший паттерн — явный breakpoint на последнем блоке, который остаётся идентичным между запросами. Доступно до 4 cache breakpoints; automatic caching занимает один слот, если совмещается с block-level caching. Для долгих агентов можно использовать TTL `1h`, но он дороже на запись; стандартный TTL — 5 минут. ([Claude API Docs][8]) ([Claude API Docs][8])

Для **Gemini**: есть implicit caching на Gemini 2.5+ и explicit caching. Google прямо рекомендует ставить large/common content в начало prompt и отправлять похожие prefix-запросы в коротком временном окне; explicit caching полезен, когда нужны гарантированные savings и есть готовность управлять cached content/TTL. ([Google AI for Developers][9])

Для **OpenRouter**: если вызываешь DeepSeek/OpenAI/Gemini/Claude через router, используй `session_id` для sticky routing. OpenRouter пишет, что sticky routing помогает держать provider cache warm, а `session_id` особенно полезен для multi-turn agentic workflows. ([OpenRouter][10])

### Практический production-чеклист

Сделай `static_prefix_id`, например:

```text
agent-kernel:v1.0|project-context:v1.0|tools:v3.2|output-contract:v1.1
```

Логируй на каждый запрос:

```text
model
static_prefix_id
prompt_tokens
prompt_cache_hit_tokens
prompt_cache_miss_tokens
completion_tokens
reasoning_tokens
cache_hit_rate
latency_ms
cost_estimate_usd
```

Формула для DeepSeek:

```text
cache_hit_rate = prompt_cache_hit_tokens / prompt_tokens

input_cost =
  prompt_cache_hit_tokens  / 1_000_000 * cache_hit_price
+ prompt_cache_miss_tokens / 1_000_000 * cache_miss_price

total_cost =
  input_cost
+ completion_tokens / 1_000_000 * output_price
```

Цель для агентного runtime: после первых нескольких запросов держать `cache_hit_rate` хотя бы 70–90% для длинных статичных prompts. Если падает ниже — почти всегда изменился prefix, tools array, формат сериализации или динамические данные попали слишком рано.

### Мой рекомендуемый вариант для твоего агента на DeepSeek V4 Flash

Сделай три отдельных блока: `AGENT_KERNEL`, `PROJECT_CONTEXT`, `RUNTIME_REQUEST`. Первые два не меняй вообще; третий меняй сколько нужно. Это даст лучший шанс `cache hit` и одновременно упростит дебаг стоимости.

Самый быстрый next step: внедрить логирование `prompt_cache_hit_tokens / prompt_cache_miss_tokens`, зафиксировать порядок `messages` и `tools`, а затем прогнать 20–50 одинаково структурированных агентных задач и посмотреть, где именно начинает падать hit rate.

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
