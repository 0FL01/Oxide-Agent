## Ключевые выводы по текущему репо

Текущая архитектура хорошо подходит под Brave без большой переделки:

`SearxngProvider` уже оформлен как отдельный provider в `crates/oxide-agent-core/src/agent/providers/searxng/`, подключается через `SearxngToolModule`, feature `tool-searxng`, и регистрируется в `AgentExecutor::register_tool_runtime_modules`.

`DuckDuckGoProvider` лучше брать как основной шаблон для Brave, потому что он возвращает `structured_payload` и различает `success/failure`. Это важно для fallback-логики: SearXNG сейчас возвращает просто markdown/error string, а DuckDuckGo уже даёт нормальную машинную сигнализацию для hook’ов.

`SearchBudgetHook` сейчас считает `web_search`, `web_extract`, `duckduckgo_search`, `duckduckgo_news`, `searxng_search`; Brave туда надо добавить как `brave_search`. Там же уже есть паттерн “если DuckDuckGo заблокирован/rate-limited — не ретраить, использовать `searxng_search`”.

Главная зона риска для cache hit — не HTTP-клиент Brave, а изменение tool set/tool order. В `AGENTS.md` прямо зафиксирована архитектура static prefix + dynamic suffix, а `docs/tips/cache-hit.md` отдельно подчёркивает, что tool definitions должны идти в стабильном порядке. Поэтому добавление нового инструмента неизбежно даст один cold-cache период после rollout, но дальше cache-hit сохранится, если не менять имя, schema, описание и порядок регистрации.

---

## Внешние ограничения Brave + Crawl4AI

Brave Web Search API использует endpoint `https://api.search.brave.com/res/v1/web/search` и header `X-Subscription-Token`; в примерах Brave также передаёт `Accept: application/json` и `Accept-Encoding: gzip`. ([Brave][1])

Для MVP достаточно Web Search endpoint, а не LLM Context: Brave сам рекомендует LLM Context для chatbot/agent use cases, но твоя архитектурная цель другая — Brave только как index search, а открытие страниц через Crawl4AI ради экономии квоты Brave. ([Brave][2])

Brave поддерживает freshness filters `pd`, `pw`, `pm`, `py` и custom date range, country/search language targeting, extra snippets до 5 дополнительных excerpts, pagination через `count` max 20 и `offset` max 9, safe search `off/moderate/strict`. ([Brave][2]) ([Brave][2]) ([Brave][2])

По квотам: Brave пишет, что Search plan стоит `$5 per 1,000 requests`, включает `$5` monthly credits и имеет capacity `50 queries per second`; rate limiting работает через 1-second sliding window, при превышении возвращается HTTP 429, а response headers дают лимиты/remaining/reset. ([Brave][1]) ([Brave][3])

Crawl4AI уже логически подходит под роль opener: он даёт headless browser crawl, Markdown conversion, JS/dynamic page support, `BrowserConfig` и `CrawlerRunConfig`; self-host вариант запускается контейнером на `11235`, что у тебя уже отражено в `docker-compose.web.yml`. ([docs.crawl4ai.com][4]) ([docs.crawl4ai.com][5]) ([docs.crawl4ai.com][6])

---

## Архитектурное решение

Делать так:

`brave_search` → возвращает список URL/snippets/metadata → агент выбирает 1–3 URL → `crawl4ai_markdown` открывает выбранные URL → если Brave недоступен, `SearchBudgetHook` блокирует повторный Brave и направляет агент на `searxng_search`.

Не делать так:

Не строить общий `SearchProvider` trait/router поверх Brave/SearXNG/Crawl4AI. Это overengineering.

Не делать скрытый fallback внутри `BraveSearchClient` на SearXNG в MVP. Это ухудшит наблюдаемость, усложнит тесты и размажет ответственность. Пусть fallback будет явным: Brave failure payload → hook summary → следующий tool call `searxng_search`.

Не включать Brave engine внутри SearXNG как fallback. Это может незаметно жечь Brave quota через SearXNG.

Не открывать все результаты Brave через Crawl4AI автоматически. Только выбранные URL.

Не использовать Brave LLM Context в первой версии. Он дублирует роль Crawl4AI и противоречит твоей цели “Brave ищет индекс, Crawl4AI открывает”.

---

# План по чанкам для LLM-кодера

## Chunk 0 — Baseline и guardrails

Цель: зафиксировать текущее состояние до правок.

Команды:

```bash
cargo fmt --check
cargo test -p oxide-agent-core --features tool-searxng
cargo test -p oxide-agent-core --features tool-duckduckgo
cargo check -p oxide-agent-core --features profile-web-embedded-opencode-local
```

Что проверить в коде до изменений:

`crates/oxide-agent-core/src/agent/executor/registry.rs` — текущий порядок регистрации tools. Brave нужно вставить один раз и больше не двигать.

`crates/oxide-agent-core/src/agent/hooks/search_budget.rs` — текущая логика DuckDuckGo unavailable должна быть скопирована по смыслу для Brave.

`crates/oxide-agent-core/src/agent/tool_failure_summary.rs` — добавить Brave как provider-level dead-end.

`crates/oxide-agent-core/src/capabilities/compiled.rs` — добавить capability module.

Acceptance:

Код до изменений зелёный или список текущих failing tests зафиксирован отдельно. Без этого LLM-кодер начнёт чинить несуществующие проблемы.

---

## Chunk 1 — Feature flag, config, capability plumbing

Добавить feature:

В `crates/oxide-agent-core/Cargo.toml`:

```toml
tool-brave-search = ["dep:reqwest"]
```

Добавить feature в профили, где нужен web search:

`profile-full`

`profile-web-embedded-opencode-local`

`profile-search-only`

возможно `profile-embedded-opencode-local` и `profile-host-bwrap`, если эти профили реально используются для search workloads.

В `AgentSettings` добавить минимальный набор полей:

```rust
pub brave_search_api_key: Option<String>,
pub brave_search_enabled: Option<bool>,
pub brave_search_timeout_secs: Option<u64>,
pub brave_search_country: Option<String>,
pub brave_search_lang: Option<String>,
pub brave_search_ui_lang: Option<String>,
pub brave_search_safesearch: Option<String>,
pub brave_search_max_concurrent: Option<usize>,
pub brave_search_min_delay_ms: Option<u64>,
```

Минимальные env:

```env
BRAVE_SEARCH_API_KEY=
BRAVE_SEARCH_ENABLED=
BRAVE_SEARCH_TIMEOUT_SECS=10
BRAVE_SEARCH_COUNTRY=US
BRAVE_SEARCH_LANG=en
BRAVE_SEARCH_UI_LANG=en-US
BRAVE_SEARCH_SAFESEARCH=moderate
BRAVE_SEARCH_MAX_CONCURRENT=1
BRAVE_SEARCH_MIN_DELAY_MS=1000
```

Логика:

`is_brave_search_enabled()` должна возвращать `false`, если нет API key, кроме случая явного override. Практично: если `BRAVE_SEARCH_ENABLED=false`, не регистрировать tool вообще; если `true`, требовать key и логировать warn при отсутствии.

Добавить capability:

В `compiled.rs`:

```rust
push_module!(
    modules,
    "tool-brave-search",
    "tool/brave-search",
    Search,
    ["tool/brave-search"]
);
```

Acceptance:

`cargo check -p oxide-agent-core --features tool-brave-search` проходит.

При feature on, но без `BRAVE_SEARCH_API_KEY`, provider не регистрируется.

При `BRAVE_SEARCH_ENABLED=false`, provider не регистрируется даже с key.

---

## Chunk 2 — Provider skeleton

Создать:

```text
crates/oxide-agent-core/src/agent/providers/brave_search/
  mod.rs
  types.rs
  error.rs
  client.rs
  format.rs
  provider.rs
```

Подключить в `agent/providers/mod.rs`:

```rust
#[cfg(feature = "tool-brave-search")]
pub mod brave_search;

#[cfg(feature = "tool-brave-search")]
pub use brave_search::BraveSearchProvider;
```

В `types.rs`:

```rust
pub const TOOL_NAME: &str = "brave_search";
pub const DEFAULT_MAX_RESULTS: u8 = 5;
pub const MAX_RESULTS_LIMIT: u8 = 10;
pub const DEFAULT_PAGE: u8 = 1;
```

Tool args:

```rust
pub struct BraveSearchArgs {
    pub query: String,
    #[serde(default = "default_max_results")]
    pub max_results: u8,
    pub country: Option<String>,
    pub search_lang: Option<String>,
    pub ui_lang: Option<String>,
    pub freshness: Option<String>,
    pub safesearch: Option<String>,
    #[serde(default)]
    pub extra_snippets: bool,
    #[serde(default = "default_page")]
    pub page: u8,
}
```

Нормализация:

`max_results`: clamp `1..=10`, даже если Brave поддерживает 20. Это экономит tokens и снижает noise.

`page`: входной `1..=10`, в Brave отправлять как `offset = page - 1`.

`freshness`: разрешить `pd`, `pw`, `pm`, `py` и custom range string без сложного парсера. Не надо городить date parser.

`safesearch`: только `off`, `moderate`, `strict`, default из config.

Response structs делать tolerant:

```rust
#[derive(Debug, Deserialize)]
pub struct BraveSearchResponse {
    #[serde(default)]
    pub web: Option<BraveWebResults>,
    #[serde(default)]
    pub query: Option<BraveQuery>,
}

#[derive(Debug, Deserialize)]
pub struct BraveWebResults {
    #[serde(default)]
    pub results: Vec<BraveWebResult>,
}

#[derive(Debug, Deserialize)]
pub struct BraveWebResult {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub age: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub family_friendly: Option<bool>,
    #[serde(default)]
    pub extra_snippets: Vec<String>,
}
```

Не пытаться покрыть `news`, `videos`, `locations`, `infobox`, `rich` в первой версии. Парсить только `web.results`, остальные поля игнорировать.

Acceptance:

Unit tests на normalization.

Unit test на deserialize sample response с `web.results`.

Unit test: пустой/пробельный query → `EmptyQuery`.

---

## Chunk 3 — Brave HTTP client

В `client.rs` реализовать `BraveSearchClient`.

Endpoint hardcoded:

```text
https://api.search.brave.com/res/v1/web/search
```

Request:

GET.

Headers:

```text
Accept: application/json
Accept-Encoding: gzip
X-Subscription-Token: <key>
```

Query params:

```text
q
count
offset
country
search_lang
ui_lang
freshness
safesearch
extra_snippets
```

Дополнительно можно отправлять `text_decorations=false`, если подтверждено API reference в кодереской проверке. Если сомнения — не добавлять.

Rate limit для MVP:

`Semaphore` на `BRAVE_SEARCH_MAX_CONCURRENT`, default `1`.

`BRAVE_SEARCH_MIN_DELAY_MS`, default `1000`.

Не делать adaptive limiter по `X-RateLimit-*` в MVP. Достаточно обработать 429 как `rate_limited` и дать fallback на SearXNG. Brave headers можно залогировать debug-level, но не строить scheduler.

Retry policy:

Не ретраить 401/403/400.

429 — не ретраить в MVP; сразу structured failure и fallback. Это бережёт quota и не создаёт tool loops.

Timeout/connect/5xx — максимум один retry с коротким jitter backoff.

Acceptance:

Non-2xx мапится:

401/403 → `auth`

429 → `rate_limited`

500/502/503/504 → `server`

timeout/connect → `network` или `timeout`

`provider_unavailable=true` для `rate_limited`, `server`, `network`, `timeout`, `auth`, `missing_api_key`.

---

## Chunk 4 — Formatter + structured payload

`format.rs` должен возвращать `(markdown, serde_json::Value)`.

Markdown формат:

```md
## Brave Search results for: <query>

1. **<title>**
   URL: <url>
   Snippet: <description>
   Age: <age>
```

Structured payload:

```json
{
  "provider": "brave_search",
  "kind": "search",
  "query": "...",
  "country": "US",
  "search_lang": "en",
  "freshness": "pw",
  "results": [
    {
      "title": "...",
      "url": "...",
      "description": "...",
      "age": "...",
      "language": "en",
      "extra_snippets": []
    }
  ]
}
```

Failure payload:

```json
{
  "provider": "brave_search",
  "kind": "search",
  "query": "...",
  "error_kind": "rate_limited",
  "error": "...",
  "provider_unavailable": true,
  "retryable": false,
  "fallback": "searxng_search",
  "results": []
}
```

Почему это важно: `SearchBudgetHook` и `tool_failure_summary` смогут понять, что Brave умер в рамках задачи, и направить агента к `searxng_search`, а не к повторному Brave.

Acceptance:

Success tool output имеет `structured_payload`.

Failure tool output имеет `structured_payload.provider = "brave_search"`.

Invalid JSON args → `ToolRuntimeError::InvalidArguments`, как в DuckDuckGo.

---

## Chunk 5 — Provider executor и tool definition

В `provider.rs` копировать стиль `DuckDuckGoProvider`, а не SearXNG.

Tool definition:

```rust
ToolDefinition {
    name: "brave_search".to_string(),
    description: concat!(
        "Search the public web using Brave Search API. Use this to discover URLs and snippets. ",
        "Open only selected result URLs with crawl4ai_markdown; do not crawl every result. ",
        "If Brave is unavailable, use searxng_search as fallback."
    ).to_string(),
    parameters: json!({ ... }),
}
```

Параметры:

`query` required.

`max_results`, `country`, `search_lang`, `ui_lang`, `freshness`, `safesearch`, `extra_snippets`, `page`.

Важно для cache hit: описание и schema должны быть статичными. Не вставлять туда env/config/current quotas/current date.

Acceptance:

`BraveSearchProvider::new_from_config()` или `new(api_key, config)` создаёт provider.

`tool_runtime_executors()` возвращает один executor.

Tool name строго `brave_search`.

---

## Chunk 6 — Registry и ToolModule

В `agent/tool_runtime/modules.rs` добавить:

```rust
#[cfg(feature = "tool-brave-search")]
pub struct BraveSearchToolModule;
```

`module_id`:

```rust
ModuleId::new("tool/brave-search")
```

`provider()`:

Проверяет `is_brave_search_enabled()`.

Берёт key через config/env helper.

Создаёт `BraveSearchProvider`.

В `agent/executor/registry.rs` добавить import и регистрацию.

Рекомендованный порядок:

```rust
DuckDuckGoToolModule
BraveSearchToolModule
SearxngToolModule
```

Почему так: Brave становится preferred alternative перед SearXNG, но SearXNG остаётся рядом как fallback. После merge этот порядок не трогать.

Особое замечание: в `registry.rs` сейчас в одном `#[cfg(any(...))]` для `register_tool_runtime_module` виден риск — `tool-searxng` присутствует не во всех cfg-группах. Кодеру надо grep’нуть все `cfg(any(feature = "tool-..."))` в `registry.rs` и добавить `tool-brave-search` во все связанные списки, иначе feature-only сборка может сломаться.

Acceptance:

`cargo check -p oxide-agent-core --features tool-brave-search` проходит.

`cargo check -p oxide-agent-core --features "tool-brave-search tool-searxng tool-crawl4ai-markdown"` проходит.

`current_tool_definitions()` при включённом key содержит `brave_search`.

---

## Chunk 7 — Fallback через SearchBudgetHook

В `search_budget.rs`:

Добавить поле:

```rust
brave_search_unavailable: AtomicBool,
```

Добавить tool:

```rust
"brave_search"
```

Добавить helper:

```rust
fn is_brave_search_tool(tool_name: &str) -> bool {
    tool_name == "brave_search"
}
```

Добавить detector:

```rust
fn result_marks_brave_search_unavailable(result: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(result) else {
        return false;
    };
    let Some(payload) = value.get("structured_payload") else {
        return false;
    };
    if payload.get("provider").and_then(|v| v.as_str()) != Some("brave_search") {
        return false;
    }

    payload.get("provider_unavailable").and_then(|v| v.as_bool()) == Some(true)
        || matches!(
            payload.get("error_kind").and_then(|v| v.as_str()),
            Some("rate_limited" | "auth" | "missing_api_key" | "server" | "network" | "timeout")
        )
}
```

BeforeTool block:

```text
Brave Search is unavailable in this task. Do not retry brave_search with rewritten queries; use searxng_search fallback.
```

AfterTool:

Если `brave_search` вернул unavailable → set AtomicBool true.

Acceptance tests:

`brave_search` считается в search budget.

После payload `rate_limited` повторный `brave_search` блокируется.

`searxng_search` после Brave failure не блокируется.

---

## Chunk 8 — Failure summary

В `tool_failure_summary.rs` добавить Brave branch по аналогии с DuckDuckGo.

Helper:

```rust
fn is_brave_search(provider: Option<&str>, tool_name: &str) -> bool {
    provider == Some("brave_search") || tool_name == "brave_search"
}
```

Structured failure:

Если provider `brave_search` и `error_kind` в `rate_limited/auth/missing_api_key/server/network/timeout`, summary должен сказать:

```text
Brave Search <error_kind> query: <query>
```

Guidance:

```text
Do not retry brave_search in this task; use searxng_search or synthesize from existing results.
```

Acceptance:

Raw Brave failure payload с `rate_limited` сжимается в compact JSON summary.

Summary содержит `dead_end_scope = "provider"` и `target = "brave_search"`.

---

## Chunk 9 — Crawl4AI guidance, без новой интеграции

`crawl4ai_markdown` уже есть, отдельный Rust provider менять почти не надо.

Нужна только лёгкая правка tool description:

```text
Use after selecting specific URLs from brave_search or searxng_search. Do not crawl every search result.
```

Не менять API Crawl4AI provider.

Не добавлять “Brave + Crawl4AI combined tool”.

Не делать batch crawl на все результаты.

Acceptance:

Tool schema `crawl4ai_markdown` меняется один раз, потом замораживается.

Agent behavior: search first, crawl selected URLs second.

---

## Chunk 10 — Docs, env, docker

`.env.example`:

Добавить Brave env section рядом с SearXNG/Crawl4AI.

`README.md`:

Обновить список web tools:

```text
Brave Search — primary indexed web discovery when BRAVE_SEARCH_API_KEY is configured.
SearXNG — fallback/self-hosted aggregator.
Crawl4AI — browser-rendered opener for selected URLs.
```

`docker-compose.web.yml`:

Добавить env passthrough для приложения:

```yaml
- BRAVE_SEARCH_API_KEY=${BRAVE_SEARCH_API_KEY:-}
- BRAVE_SEARCH_ENABLED=${BRAVE_SEARCH_ENABLED:-}
```

Не добавлять новый service.

SearXNG оставить включённым.

Проверить `docker/searxng/settings.yml`: не включать Brave engine как часть SearXNG fallback, чтобы не было скрытого расхода Brave quota.

---

## Chunk 11 — Tests

Минимальные Rust tests:

`tests/brave_search_provider.rs`:

Tool runtime registers only `brave_search`.

Tool spec name is `brave_search`.

Spec requires `query`.

Invalid args returns `InvalidArguments`.

`types.rs` tests:

`max_results` clamp.

`page` to offset.

`safesearch` validation/default.

`freshness` passthrough for `pd/pw/pm/py`.

`client.rs` tests:

Не добавлять новые heavy test crates. Если нужен HTTP mock — использовать `tokio::net::TcpListener`, как уже сделано в `searxng_provider.rs`.

Проверить:

request path `/res/v1/web/search`

header `X-Subscription-Token`

query params `q`, `count`, `offset`

429 → `rate_limited`

401/403 → `auth`

`search_budget.rs` tests:

Brave counts against budget.

Brave rate limit blocks repeated Brave.

SearXNG fallback remains allowed.

`tool_failure_summary.rs` tests:

Brave rate limit summary.

Brave auth/missing key summary.

---

## Chunk 12 — Build matrix

Команды после реализации:

```bash
cargo fmt --check

cargo test -p oxide-agent-core --features tool-brave-search

cargo test -p oxide-agent-core --features "tool-brave-search tool-searxng tool-crawl4ai-markdown"

cargo check -p oxide-agent-core --features profile-search-only

cargo check -p oxide-agent-core --features profile-web-embedded-opencode-local
```

Если snapshot tests есть для modular registry, обновлять snapshot осознанно, потому что добавление Brave capability/tool — ожидаемое изменение.

---

## Rollout без убийства cache hit

Важно сказать прямо: добавление нового tool definition физически меняет tool set. Один cold-cache период при включении Brave неизбежен. Но это не “сломанный cache hit”, если после rollout tool schema/order стабильны.

Правила rollout:

Сначала merge feature и код, но `BRAVE_SEARCH_ENABLED=false` в production.

Проверить сборки и staging.

Включить `BRAVE_SEARCH_API_KEY` и `BRAVE_SEARCH_ENABLED=true` одним релизным окном.

После включения не менять:

имя `brave_search`

description

JSON schema

порядок регистрации tools

module id `tool/brave-search`

Не добавлять динамические данные в tool description или system prompt: quota, current date, key status, base URL, provider health.

Следить за `TokenUsage.cache_hit_rate()` после прогрева. Ожидаемо будет просадка сразу после изменения tool set, затем возврат к нормальному уровню.

---

## Финальный рекомендуемый порядок реализации

1. Config + feature + capability.
2. Brave provider skeleton/types.
3. HTTP client.
4. Formatter + structured payload.
5. ToolModule + registry.
6. SearchBudgetHook fallback.
7. Failure summary.
8. Docs/env/docker.
9. Tests/build matrix.
10. One-shot rollout и freeze schema.

Главное: не прятать SearXNG fallback внутри Brave-клиента. В этой кодовой базе cleanest path — сделать Brave нормальным native provider с хорошим failure payload, а orchestration оставить агенту и hook’ам. Это проще, надёжнее и меньше рискует cache-hit.

[1]: https://brave.com/search/api/ "Brave Search API | Brave"
[2]: https://api-dashboard.search.brave.com/app/documentation/web-search/get-started "Brave Search - API"
[3]: https://api-dashboard.search.brave.com/documentation/guides/rate-limiting "Brave Search - API"
[4]: https://docs.crawl4ai.com/core/quickstart/ "Quick Start - Crawl4AI Documentation (v0.8.x)"
[5]: https://docs.crawl4ai.com/core/browser-crawler-config/ "Browser, Crawler & LLM Config - Crawl4AI Documentation (v0.8.x)"
[6]: https://docs.crawl4ai.com/core/self-hosting/ "Self-Hosting Guide - Crawl4AI Documentation (v0.8.x)"
