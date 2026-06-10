**TL;DR / Executive summary**

Я прочитал архив и подтверждаю главный вывод: текущая архитектура уже имеет хорошие места для deterministic research layer, но сейчас финальный ответ разрешается по lifecycle-факту “модель закончила”, а не по факту “claims проверены”. Самый важный неожиданный finding: `BeforeTool` событие есть в API, но в реальном typed tool runtime path оно, похоже, вообще не вызывается; поэтому `SearchBudgetHook`, `ToolAccessPolicyHook` и часть sub-agent safety логики тестируются изолированно, но не гарантированно применяются к реальным tool calls.

Acceptance criterion из твоего задания я трактую буквально: система должна детерминированно отличать “модель сказала” от “система проверила”, а финал должен проходить через `ResearchState` / `EvidenceLedger`, а не через уверенность LLM.

## 1. Карта текущего agent lifecycle по коду

### Как сейчас идёт agent lifecycle

Текущий flow выглядит так:

1. `AgentExecutor::prepare_execution` собирает runtime:

   * `todos_arc`;
   * typed `tool_runtime_registry`;
   * список `tools`;
   * system prompt через `create_agent_system_prompt`;
   * `messages` из hot memory;
   * `AgentRunnerConfig` с лимитами iterations / continuation / timeout / search.

2. `PreparedExecution::build_runner_context` создаёт `AgentRunnerContext`:

   * task, prompt, tools, messages, session;
   * `tool_runtime_registry`;
   * memory scope;
   * memory behavior;
   * runner config.

3. `AgentRunner::run`:

   * reset loop detector;
   * `apply_before_agent_hooks`;
   * входит в `run_loop`.

4. На каждой итерации `run_loop`:

   * timeout check;
   * pending runtime context;
   * `apply_before_iteration_hooks`;
   * compaction;
   * loop detection;
   * LLM call;
   * `handle_llm_response`.

5. `handle_llm_response`:

   * если есть native tool calls → `execute_tools_with_runtime`;
   * если structured output → парсит `tool_call`, `final_answer`, `awaiting_user_input`;
   * если финал → `handle_final_response`;
   * если tool call → `execute_tools_with_runtime`.

6. `handle_final_response`:

   * sync todos;
   * вызывает `after_agent_hook_result`;
   * если hook вернул `ForceIteration`, draft не доставляется пользователю, кладётся system retry context, runner продолжает;
   * если не `ForceIteration`, финал сохраняется в memory и возвращается пользователю.

Это значит: `AfterAgent + ForceIteration` уже реально умеет блокировать доставку финала и заставлять агент продолжить. Это правильное место для `FinalAnswerGuardHook`.

### Где вызываются hooks

В `runner/hooks.rs` реально вызываются:

* `BeforeAgent` через `apply_before_agent_hooks`.
* `BeforeIteration` через `apply_before_iteration_hooks`.
* `AfterTool` через `apply_after_tool_hooks`.
* `AfterAgent` через `after_agent_hook_result`.
* `Timeout` через `apply_timeout_hook`.

В `hooks/types.rs` также есть `BeforeTool`, но по `rg` я не нашёл реального вызова `HookEvent::BeforeTool` в runtime path. Оно встречается только в hook implementations и unit tests. Это критично: API выглядит как полноценный lifecycle, но pre-tool policy фактически не встроена в `ToolCallRuntime`.

### Где исполняются tools

Typed tools исполняются здесь:

* `runner/tools.rs::execute_tools_with_runtime`;
* создаётся `ToolCallRuntime`;
* `ToolCallRuntime::execute_batch`;
* внутри `runtime.rs::run_one_tool`;
* далее `ToolRegistry::execute_or_normalize`;
* затем конкретный provider executor.

`ToolCallRuntime::execute_batch`:

* сначала пишет assistant tool calls в buffered history;
* параллельно запускает tool tasks;
* сортирует outputs по `batch_index`;
* проверяет matching;
* пишет `ToolOutput` в history writer.

### Где tool output попадает в memory/history

`BufferedRuntimeHistory::record_tool_output` получает полный `ToolOutput`, но сразу кодирует его в model-facing JSON через `ToolOutput::encode_model_content()`.

Затем `runner/tools.rs::apply_runtime_tool_output`:

1. получает полный `ToolOutput`;
2. emits `AgentEvent::ToolResult`;
3. вызывает `apply_after_tool_hooks(ctx, state, &tool_name, &content)`;
4. потом добавляет `Message::tool_with_correlation` в `ctx.messages`;
5. потом добавляет `AgentMessage::tool_with_correlation` или pruned failure summary в hot memory.

Слабое место: `AfterTool` hook получает только `content: String`, а не `ToolOutput`. Полный `ToolOutput` на этом месте ещё есть, но в hook API он не передаётся.

### Где можно хранить shared research state

Лучшее минимально-инвазивное место:

* добавить `Option<ResearchRuntime>` в `AgentRunnerContext`;
* добавить `research_runtime: Option<ResearchRuntime>` в `PreparedExecution`;
* прокинуть borrow в `HookContext`;
* для sub-agents создать child runtime или shared parent runtime с пометкой `origin = sub_agent`.

Почему не только в `HookContext`: `runner/tools.rs` должен записывать full typed `ToolOutput` напрямую, а `HookContext` сейчас строится заново для каждого hook event и не владеет состоянием.

### Где правильно вставить final answer guard

Правильное место:

* `runner/responses.rs::handle_final_response`;
* через `AfterAgent` hook;
* рядом с `CompletionCheckHook`.

Но есть нюанс: `handle_final_response` сейчас специально обрабатывает только `HookResult::ForceIteration`. Если `AfterAgent` hook вернёт `Finish` или `Block`, этот result не будет применён как финальный override; код просто продолжит и сохранит исходный финал. Поэтому `FinalAnswerGuardHook` на первом этапе должен возвращать именно `ForceIteration`. Если нужен emergency rewrite на continuation limit, надо расширить `handle_final_response` для `Finish(String)` / `Block`.

### Где стандартизировать structured tool outputs

Стандартизировать надо в providers, не в prompt:

* `searxng/provider.rs` + `searxng/format.rs`;
* `crawl4ai_markdown/executor.rs` + `crawl4ai_markdown/crawl.rs`;
* `webfetch_md/mod.rs` + `webfetch_md/fetch.rs`;
* `tavily.rs`;
* Brave / DuckDuckGo уже частично делают structured payload, но shape надо привести к общему canonical shape.

`ToolOutput` уже имеет `structured_payload: Option<Value>`, и `model_content_value()` уже включает это поле. Проблема не в runtime type, а в providers, которые success payload часто кладут в stdout text.

Research provider priority decision: основной production path для research — связка `searxng_search` → `crawl4ai_markdown`. Остальные search/fetch tools (`web_search`, `web_extract`, `web_markdown`, `brave_search`, `duckduckgo_search`, `duckduckgo_news`) остаются optional/fallback providers и не должны диктовать core architecture. Поэтому first-class payload/ledger work в первую очередь оптимизируется под SearXNG search leads и Crawl4AI fetched-source evidence.

### Минимальные совместимые изменения

Минимальный vertical slice:

* добавить `agent/research`;
* добавить `ResearchRuntime` в `AgentRunnerContext`;
* прокинуть его в `HookContext`;
* в `apply_runtime_tool_output` вызвать `research.record_tool_output(&tool_name, &output)` до string-only hooks;
* добавить success `structured_payload` для SearXNG, Crawl4AI, WebMarkdown;
* добавить `FinalAnswerGuardHook`, который на `AfterAgent` делает deterministic high-impact claim scan и возвращает `ForceIteration`;
* не ломать старый hook API.

### Что требует refactor

Refactor нужен для:

* настоящего `BeforeTool` enforcement внутри typed runtime;
* mode-aware provider-aware search/fetch budget;
* sub-agent evidence contracts;
* автоматического fetch planner / query planner, если он должен сам инициировать tool calls, а не только давать guidance;
* `AfterAgent` handling для `Finish` / `Block`;
* full evidence extraction из fetched markdown, если не ограничиваться эвристическим ledger.

## 2. Что реально найдено в коде

Главные подтверждения:

* `HookEvent` действительно имеет `BeforeAgent`, `BeforeIteration`, `AfterAgent`, `BeforeTool`, `AfterTool`, `Timeout`.
* `AfterAgent` реально может force continuation через `ForceIteration`; runner сохраняет undelivered final draft и продолжает.
* `ToolOutput` уже содержит `structured_payload`.
* `ToolOutput::model_content_value()` уже сериализует `structured_payload`, так что модель тоже может видеть structured payload, если provider его поставит.
* `runner/tools.rs` действительно видит полный `ToolOutput`, но `AfterTool` hook получает только string `content`.
* `SearXNG` парсит raw response в typed structs, но success превращает всё в Markdown и кладёт в stdout.
* `Crawl4AI` success делает JSON string в stdout, но не кладёт этот JSON в `structured_payload`; failure payload уже structured.
* `WebMarkdown` success отдаёт Markdown string; failure payload уже structured.
* `Tavily` success отдаёт Markdown/string; errors тоже string; structured payload отсутствует.
* Brave Search и DuckDuckGo уже имеют structured payload на success, но shape не полностью canonical.
* `SearchBudgetHook` сейчас считает только search tools: `web_search`, `web_extract`, `duckduckgo_search`, `duckduckgo_news`, `brave_search`, `searxng_search`.
* `SearchBudgetHook` не считает `crawl4ai_markdown` и `web_markdown` как fetch budget.
* `SearchBudgetHook` provider-aware только частично: DuckDuckGo/Brave unavailable flags и web_markdown anti-bot host quarantine.
* `AGENT_SEARCH_LIMIT` default в `config.rs` равен `10`, но `.env.example` уже ставит `AGENT_SEARCH_LIMIT=1000`.
* `AgentExecutionEffort::Extended` ставит минимум search limit `30`, `Heavy` — `80`. Это лучше default, но всё ещё не mode-aware и не provider-aware.
* `prompt/composer.rs` содержит hardcoded guidance: “Do not fetch every search result automatically; fetch only selected URLs.” Оно добавляется, если есть DuckDuckGo search/news. Это должно стать mode-aware.
* `crawl4ai_markdown` tool description тоже содержит “Do not crawl every search result.” Это тоже должно стать mode-aware.
* `BeforeTool` — главный gap: оно есть, но реального вызова в runner/runtime я не нашёл. Это надо исправить раньше или одновременно с budget redesign.

## 3. Главные архитектурные проблемы

Первая проблема: **нет deterministic boundary между “tool evidence” и “model prose”**. Сейчас tool results, sub-agent reports и финальный ответ смешиваются в memory как текст. Даже если модель пишет уверенно, система не знает, какие claims подтверждены.

Вторая: **rich tool output теряется на hook boundary**. Полный `ToolOutput` существует, но `AfterTool` hook получает string. Это ломает typed ledger.

Третья: **search/fetch providers не имеют единого payload contract**. Где-то success payload есть, где-то JSON лежит в stdout, где-то только Markdown. Research layer не должен парсить Markdown как источник истины, если provider уже знает typed fields.

Четвёртая: **pre-tool policy выглядит активной, но фактически не встроена**. Это касается budget, duplicate query guard, duplicate URL guard, tool access policy, sub-agent safety tool blocking.

Пятая: **финальный ответ не gated by evidence**. `CompletionCheckHook` проверяет todos, но не проверяет claims. Поэтому high-impact claims могут попасть в финал без source proof.

Шестая: **search budget ограничивает “количество поиска”, а не “бесполезные петли”**. Для self-hosted SearXNG/Crawl4AI/WebMarkdown это неправильная ось контроля. Ограничивать надо дубли, anti-bot, failed host retries, near-duplicates, отсутствие progress.

Седьмая: **prompt guidance выполняет роль policy**. В текущем коде есть правильные подсказки, но acceptance criterion требует code-level enforcement, а не “модель должна сама помнить”.

## 4. Предложенная универсальная архитектура

Новая архитектура должна быть не “web prompt layer”, а deterministic runtime layer:

1. `TaskClassifier`

   * классифицирует задачу;
   * выбирает `ResearchMode`;
   * не включает web для стабильных conceptual learning задач;
   * включает strict mode для current, legal, pricing, technical current, politics, exhaustive discovery, verification.

2. `SearchQueryPlanner`

   * создаёт query intents;
   * dedupe queries;
   * tracks coverage;
   * для paranoid/research добавляет counter-evidence queries.

3. `SearchResultNormalizer`

   * canonical URL;
   * host;
   * result rank;
   * source kind guess;
   * duplicate group;
   * score + score reasons.

4. `FetchPlanner`

   * выбирает URLs для fetch;
   * в deep/paranoid может fetch many canonical candidates;
   * не повторяет canonical URLs;
   * учитывает provider cost class.

5. `SourceClassifier`

   * source-of-truth / primary / strong secondary / secondary / weak / untrusted / snippet-only;
   * source kind: official docs, government/regulator, package registry, academic paper, reputable news, forum, social, mirror, unknown, etc.

6. `EvidenceLedger`

   * центральная typed memory проверенных facts;
   * search snippets могут быть leads, но не high-impact evidence;
   * fetched sources дают evidence only after source classification and extraction.

7. `ClaimExtractor`

   * deterministic high-impact claim heuristic;
   * затем можно добавить LLM-assisted extractor, но deterministic layer остаётся обязательным.

8. `ClaimVerifier`

   * проверяет claim ↔ evidence;
   * freshness;
   * source priority;
   * negative claims;
   * all/only coverage;
   * conflicts.

9. `FinalAnswerGuardHook`

   * блокирует unsupported high-impact claims;
   * force-continues with specific next actions;
   * на continuation limit допускает только caveated / downgraded wording или требует refactor to `Finish`.

10. `ResearchAudit`

* JSON artifact для машин;
* Markdown artifact для человека;
* debug trace for observability.

Это соответствует универсальным режимам из задания: conceptual learning не должен насильно гонять web, но current/legal/pricing/politics/comparative/exhaustive/verification должны включать strict research behavior.

## 5. Конкретные файлы, которые менять

Минимальный набор:

* `crates/oxide-agent-core/src/agent/mod.rs`
* `crates/oxide-agent-core/src/agent/hooks/mod.rs`
* `crates/oxide-agent-core/src/agent/hooks/types.rs`
* `crates/oxide-agent-core/src/agent/hooks/search_budget.rs`
* `crates/oxide-agent-core/src/agent/runner/types.rs`
* `crates/oxide-agent-core/src/agent/runner/hooks.rs`
* `crates/oxide-agent-core/src/agent/runner/tools.rs`
* `crates/oxide-agent-core/src/agent/runner/responses.rs`
* `crates/oxide-agent-core/src/agent/executor/types.rs`
* `crates/oxide-agent-core/src/agent/executor/config.rs`
* `crates/oxide-agent-core/src/agent/executor/execution.rs`
* `crates/oxide-agent-core/src/agent/executor/registry.rs`
* `crates/oxide-agent-core/src/agent/providers/searxng/provider.rs`
* `crates/oxide-agent-core/src/agent/providers/searxng/format.rs`
* `crates/oxide-agent-core/src/agent/providers/searxng/types.rs`
* `crates/oxide-agent-core/src/agent/providers/crawl4ai_markdown/executor.rs`
* `crates/oxide-agent-core/src/agent/providers/crawl4ai_markdown/crawl.rs`
* `crates/oxide-agent-core/src/agent/providers/webfetch_md/mod.rs`
* `crates/oxide-agent-core/src/agent/providers/webfetch_md/fetch.rs`
* `crates/oxide-agent-core/src/agent/providers/tavily.rs`
* `crates/oxide-agent-core/src/agent/prompt/composer.rs`
* `crates/oxide-agent-core/src/config.rs`
* `.env.example`
* `profiles/web-embedded-opencode-local.toml`

Новые файлы:

* `crates/oxide-agent-core/src/agent/research/mod.rs`
* `crates/oxide-agent-core/src/agent/research/types.rs`
* `crates/oxide-agent-core/src/agent/research/runtime.rs`
* `crates/oxide-agent-core/src/agent/research/classification.rs`
* `crates/oxide-agent-core/src/agent/research/query_planner.rs`
* `crates/oxide-agent-core/src/agent/research/normalizer.rs`
* `crates/oxide-agent-core/src/agent/research/source_classifier.rs`
* `crates/oxide-agent-core/src/agent/research/extract.rs`
* `crates/oxide-agent-core/src/agent/research/ledger.rs`
* `crates/oxide-agent-core/src/agent/research/gates.rs`
* `crates/oxide-agent-core/src/agent/research/audit.rs`

## 6. Новые structs / enums / modules

Минимальный core:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchMode {
    Standard,
    Research,
    DeepResearch,
    Paranoid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchTaskKind {
    ConceptualLearning,
    FactualLearning,
    CurrentPolitics,
    CurrentNews,
    LegalPolicy,
    TechnicalCurrent,
    ProductPricing,
    ComparativeResearch,
    ExhaustiveDiscovery,
    Verification,
    General,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourcePriority {
    SourceOfTruth,
    Primary,
    StrongSecondary,
    Secondary,
    Weak,
    Untrusted,
    SnippetOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    OfficialWebsite,
    OfficialDocs,
    OfficialApi,
    OfficialRepository,
    PackageRegistry,
    ModelRegistry,
    DataRegistry,
    Government,
    Regulator,
    StandardsBody,
    AcademicPaper,
    OfficialChangelog,
    OfficialPricing,
    StatusPage,
    BenchmarkLeaderboard,
    IndependentBenchmark,
    ReputableNews,
    Blog,
    Forum,
    Social,
    SearchSnippet,
    Mirror,
    Cache,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimType {
    Existence,
    Identity,
    Ownership,
    Availability,
    CurrentStatus,
    Price,
    LicenseOrTerms,
    PolicyOrLaw,
    Capability,
    Compatibility,
    SupportStatus,
    Version,
    ReleaseDate,
    Metric,
    Benchmark,
    Definition,
    Quote,
    Ranking,
    Comparison,
    Recommendation,
    NegativeClaim,
    CausalClaim,
    Prediction,
    SafetyOrSecurity,
    Freshness,
}
```

`ResearchState`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchState {
    pub task_id: String,
    pub task_kind: ResearchTaskKind,
    pub mode: ResearchMode,
    pub started_at: DateTime<Utc>,
    pub checked_at: DateTime<Utc>,

    pub queries: Vec<QueryRecord>,
    pub search_results: Vec<SearchResultRecord>,
    pub fetched_sources: Vec<FetchedSourceRecord>,
    pub source_classifications: Vec<SourceClassification>,

    pub claims: Vec<ClaimRecord>,
    pub evidence: Vec<EvidenceItem>,
    pub conflicts: Vec<ConflictRecord>,
    pub coverage_gaps: Vec<CoverageGap>,
    pub unsupported_high_impact_claims: Vec<ClaimRecord>,

    pub audit_events: Vec<ResearchAuditEvent>,

    pub query_dedup: BTreeSet<String>,
    pub url_dedup: BTreeSet<String>,
    pub failed_fetches_by_host: BTreeMap<String, usize>,
    pub anti_bot_hosts: BTreeSet<String>,
}
```

`ResearchRuntime`:

```rust
#[derive(Clone)]
pub struct ResearchRuntime {
    inner: Arc<Mutex<ResearchState>>,
    config: ResearchRuntimeConfig,
}

impl ResearchRuntime {
    pub fn new(task_id: impl Into<String>, task: &str, config: ResearchRuntimeConfig) -> Self;

    pub fn record_tool_output(&self, tool_name: &str, output: &ToolOutput);

    pub fn evaluate_final_answer(
        &self,
        response: &str,
        at_continuation_limit: bool,
    ) -> FinalAnswerDecision;

    pub fn snapshot(&self) -> ResearchState;
}
```

`EvidenceItem` should be exactly the center object:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceItem {
    pub evidence_id: String,
    pub claim_id: Option<String>,
    pub entity: Option<String>,
    pub claim_type: ClaimType,

    pub source_url: String,
    pub canonical_url: String,
    pub source_title: Option<String>,
    pub source_kind: SourceKind,
    pub source_priority: SourcePriority,

    pub fetched_at: DateTime<Utc>,
    pub published_at: Option<DateTime<Utc>>,
    pub last_modified: Option<DateTime<Utc>>,

    pub evidence_quote: String,
    pub evidence_location: Option<String>,

    pub confidence: f32,
    pub verification_status: VerificationStatus,

    pub is_primary_source: bool,
    pub is_snippet_only: bool,
    pub is_self_reported: Option<bool>,
    pub notes: Option<String>,
}
```

## 7. Новые hooks и lifecycle placement

### `ResearchInitHook`

Placement: `BeforeAgent`.

Responsibilities:

* classify task;
* set mode if auto mode enabled;
* add audit event;
* optionally inject compact mode guidance.

Но лучше classification делать раньше, в `prepare_execution`, чтобы prompt composer тоже мог получить mode.

### `ResearchToolObserver`

Placement: `runner/tools.rs::apply_runtime_tool_output`, directly before old `AfterTool`.

Responsibilities:

* see full `ToolOutput`;
* parse `structured_payload`;
* record search/fetch/failure/anti-bot/truncation;
* update ledger leads.

Это не должен быть обычный hook, потому что обычный hook сейчас string-only. Это минимально-инвазивный путь.

### `FinalAnswerGuardHook`

Placement: `AfterAgent`, registered next to `CompletionCheckHook`.

Responsibilities:

* extract high-impact claims from draft final answer;
* verify against `EvidenceLedger`;
* enforce freshness, negative claim guard, coverage proof;
* return `ForceIteration` with concrete next actions.

### `ResearchAuditHook`

Placement: `AfterAgent`, `Timeout`, maybe after guard decision.

Responsibilities:

* write JSON/Markdown audit artifacts when enabled;
* do not affect model behavior unless debug trace is enabled.

### Важная правка для `BeforeTool`

Надо добавить real pre-tool invocation. Сейчас это не просто enhancement, а correctness gap.

Лучший refactor:

* добавить `ToolExecutionObserver` / `ToolExecutionPolicy` в `ToolCallRuntime`;
* после parse normalized args, до `registry.execute_or_normalize`, вызвать `before_tool`;
* если policy blocks, вернуть pairable `ToolOutput` со status `Failure` и structured payload `{ provider: "oxide_policy", kind: "tool_block" }`.

Это позволит реально enforce:

* search budget;
* duplicate query guard;
* duplicate URL guard;
* blocked tools;
* sub-agent blocked tools;
* anti-bot quarantine.

## 8. Provider structured payload changes

Provider priority: для deterministic research по умолчанию считаем primary stack `searxng_search` + `crawl4ai_markdown`. SearXNG отвечает за discovery/search leads, Crawl4AI — за fetched source evidence. Остальные providers нормализуются для совместимости, fallback и graceful degradation, но не являются обязательной осью initial implementation.

### SearXNG

Сейчас raw structs есть, но success output — Markdown only. Нужно возвращать `(markdown, payload)`.

Canonical success payload:

```json
{
  "provider": "searxng",
  "kind": "search",
  "query": "...",
  "page": 1,
  "max_results": 10,
  "results": [
    {
      "rank": 1,
      "title": "...",
      "url": "...",
      "snippet": "...",
      "engine": "...",
      "published_at": null
    }
  ],
  "answers": [],
  "suggestions": [],
  "corrections": [],
  "unresponsive_engines": [],
  "fetched_at": "2026-06-09T..."
}
```

Also: SearXNG search failure currently becomes `Ok(error.agent_message())`, which normalizer marks as success. That should become `ToolOutputStatus::Failure` with structured failure payload. Keep model-facing message, but do not mark provider failure as successful evidence.

### Crawl4AI

Current success payload is already JSON, but it is serialized into stdout. Refactor `success_payload` to produce `Value`, then pretty-print as stdout and set `structured_payload`.

Add:

* `kind: "fetch"`;
* `fetched_at`;
* `final_url`;
* `status_code`;
* `markdown`;
* `truncated`;
* `chars`;
* `raw_chars`;
* `fresh`;
* `source_kind`.

### WebMarkdown

Current success is Markdown string. Return a typed success struct:

```rust
pub(super) struct WebMarkdownSuccess {
    pub stdout: String,
    pub payload: Value,
}
```

Payload:

```json
{
  "provider": "web_markdown",
  "kind": "fetch",
  "url": "...",
  "final_url": "...",
  "content_type": "...",
  "fetched_bytes": 12345,
  "markdown": "...",
  "truncated": false,
  "fetched_at": "..."
}
```

### Tavily

Current `web_search` / `web_extract` return strings. Add canonical payload:

* `kind: "search"` for `web_search`;
* `kind: "fetch"` for `web_extract`;
* `provider: "tavily"`;
* `results` with rank/title/url/snippet for search;
* `sources` or `documents` with url/raw_content/markdown for extract.

### Brave / DuckDuckGo

They already have structured payloads. Required changes are mostly normalization:

* ensure all search result entries have `rank`, `title`, `url`, `snippet`, `published_at`;
* add `fetched_at`;
* map provider-specific fields like Brave `age` / DuckDuckGo news `date` into canonical freshness fields while preserving raw provider fields.

## 9. SearchBudget changes for unlimited self-hosted search

Current `SearchBudgetHook` should be split conceptually into:

* `ProviderBudgetPolicy`;
* `ResearchLoopGuard`;
* `DuplicateQueryGuard`;
* `DuplicateUrlGuard`;
* `HostFailureQuarantine`;
* `AntiBotQuarantine`.

Provider cost classes:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderCostClass {
    SelfHostedLocal,
    ExternalPaid,
    BrittleFree,
    Unknown,
}

pub fn provider_cost_class(tool_name: &str) -> ProviderCostClass {
    match tool_name {
        "searxng_search" | "crawl4ai_markdown" | "web_markdown" => ProviderCostClass::SelfHostedLocal,
        "web_search" | "web_extract" | "brave_search" => ProviderCostClass::ExternalPaid,
        "duckduckgo_search" | "duckduckgo_news" => ProviderCostClass::BrittleFree,
        _ => ProviderCostClass::Unknown,
    }
}
```

For self-hosted/local in `research`, `deep_research`, `paranoid`:

* hard search cap can be `None`;
* hard fetch cap can be high or `None`;
* duplicate query is blocked;
* duplicate canonical URL fetch is blocked;
* repeated failed host is quarantined;
* anti-bot host is quarantined for `web_markdown`;
* near-duplicate results are collapsed;
* progress is measured by new canonical URLs / new evidence / new source classes.

For external paid/brittle providers:

* keep hard caps;
* use fallback guidance when provider unavailable;
* avoid repeated rewritten queries after rate limit.

Do not represent unlimited internally as `usize::MAX` if avoidable. Better:

```rust
pub enum BudgetLimit {
    Unlimited,
    Limited(usize),
}
```

Env can still parse `0` or `-1` into `Unlimited`.

## 10. FinalAnswerGuard design

The guard should not ask “does the answer sound good?” It should ask:

* Did the final answer contain high-impact claims?
* Did those claims require evidence for this task kind?
* Does `EvidenceLedger` contain non-snippet evidence?
* Is source priority sufficient?
* Is evidence fresh enough?
* Are negative claims backed by coverage?
* Are conflicts resolved or disclosed?
* Are “all/only N” claims backed by coverage proof?
* Are recommendations tied to an explicit ranking basis?

Decision type:

```rust
pub enum FinalAnswerDecision {
    Allow {
        confidence: f32,
        rationale: String,
    },
    Block {
        reason: String,
        unsupported_claims: Vec<ClaimRecord>,
        next_actions: Vec<ResearchNextAction>,
    },
    AllowWithCaveats {
        caveats: Vec<String>,
        downgraded_claims: Vec<ClaimRecord>,
    },
}
```

Hook behavior:

```rust
impl Hook for FinalAnswerGuardHook {
    fn name(&self) -> &'static str {
        "final_answer_guard"
    }

    fn handle(&self, event: &HookEvent, context: &HookContext) -> HookResult {
        let HookEvent::AfterAgent { response } = event else {
            return HookResult::Continue;
        };

        let Some(research) = context.research else {
            return HookResult::Continue;
        };

        let decision = research.evaluate_final_answer(
            response,
            context.at_continuation_limit(),
        );

        match decision {
            FinalAnswerDecision::Allow { .. } => HookResult::Continue,
            FinalAnswerDecision::AllowWithCaveats { .. } if context.at_continuation_limit() => {
                HookResult::Continue
            }
            FinalAnswerDecision::Block { reason, next_actions, .. } => {
                HookResult::ForceIteration {
                    reason,
                    context: Some(render_next_actions(next_actions)),
                }
            }
            FinalAnswerDecision::AllowWithCaveats { caveats, .. } => {
                HookResult::ForceIteration {
                    reason: "Final answer needs caveats/downgraded wording before delivery.".to_string(),
                    context: Some(render_caveat_instructions(caveats)),
                }
            }
        }
    }
}
```

Important: for conceptual learning, guard should allow stable general claims. But if the answer includes volatile facts, prices, versions, legal claims, current political claims, current dates, “best/latest/top”, or concrete product claims, it becomes evidence-sensitive.

## 11. Audit artifact design

Feature flags:

* `RESEARCH_AUDIT_ENABLED`
* `RESEARCH_AUDIT_DIR`
* `RESEARCH_AUDIT_MAX_CHARS`
* `RESEARCH_DEBUG_TRACE`

JSON artifact:

```json
{
  "task_id": "...",
  "task_kind": "technical_current",
  "mode": "research",
  "started_at": "...",
  "checked_at": "...",
  "queries": [],
  "providers_used": [],
  "search_result_count": 0,
  "deduped_url_count": 0,
  "fetched_sources": [],
  "source_classifications": [],
  "evidence": [],
  "claims": [],
  "unsupported_high_impact_claims": [],
  "conflicts": [],
  "freshness_notes": [],
  "coverage_gaps": [],
  "final_guard": {
    "allowed": false,
    "reason": "...",
    "confidence": 0.0
  }
}
```

Markdown artifact:

* task summary;
* mode;
* checked_at;
* query list;
* fetched URLs;
* evidence ledger;
* unsupported claims;
* conflicts;
* final guard decision;
* confidence rationale.

Observability logs/metrics:

* `research.mode`;
* `research.search_calls_by_provider`;
* `research.fetch_calls_by_provider`;
* `research.query_dedup_count`;
* `research.url_dedup_count`;
* `research.primary_source_coverage`;
* `research.unsupported_high_impact_claims`;
* `research.final_guard_blocked`;
* `research.conflicts`;
* `research.freshness_warnings`;
* `research.negative_claims`;
* `research.snippet_only_claims`;
* `research.truncation_count`;
* `research.anti_bot_count`.

## 12. Tests and fixtures

Unit tests:

* task classifier:

  * conceptual learning no web;
  * current politics strict;
  * pricing strict;
  * legal source-of-truth;
  * exhaustive discovery coverage.

* URL canonicalization:

  * http/https;
  * trailing slash;
  * UTM params;
  * fragments;
  * mirrors/cache.

* search result dedup:

  * identical canonical URL;
  * same title + same host;
  * mirrored pages.

* source classifier:

  * official docs;
  * government/regulator;
  * package registry;
  * reputable news;
  * forum/social;
  * snippet-only.

* claim extractor:

  * versions;
  * prices;
  * dates;
  * current/latest;
  * “only/all”;
  * “not found / not supported”;
  * ranking/recommendation.

* final guard:

  * unsupported high-impact claim blocks;
  * evidence allows;
  * snippet-only evidence blocks;
  * negative claim without coverage blocks;
  * conceptual answer passes.

Provider tests:

* SearXNG success has `structured_payload.kind == "search"`.
* Crawl4AI success has `structured_payload.kind == "fetch"`.
* WebMarkdown success has `structured_payload.kind == "fetch"`.
* Tavily search/extract have canonical payload.
* Truncated fetch marks evidence uncertain.
* Search snippet cannot support high-impact claim.

Runner/hook tests:

* final unsupported price claim triggers `ForceIteration`.
* current political claim without fresh evidence triggers `ForceIteration`.
* with fresh primary evidence, final allowed.
* repeated identical query is blocked by loop guard.
* self-hosted deep research is not blocked while producing new canonical URLs.
* sub-agent text output is treated as lead, not evidence.

Golden tests should follow your ten cases: conceptual learning, factual learning with sources, political current event, legal/policy, product pricing, technical library version, comparative recommendation, exhaustive discovery, negative claim, current news.

## 13. Patch / pseudo-diff

Это не полный компилируемый patch; это максимально близкий implementation sketch, который показывает exact placement and contracts.

### Add research module

```diff
diff --git a/crates/oxide-agent-core/src/agent/mod.rs b/crates/oxide-agent-core/src/agent/mod.rs
@@
 pub mod recovery;
+/// Deterministic web research / fact-checking runtime.
+pub mod research;
 pub mod runner;
```

```rust
// crates/oxide-agent-core/src/agent/research/mod.rs
pub mod audit;
pub mod classification;
pub mod extract;
pub mod gates;
pub mod ledger;
pub mod normalizer;
pub mod query_planner;
pub mod runtime;
pub mod source_classifier;
pub mod types;

pub use runtime::{ResearchRuntime, ResearchRuntimeConfig};
pub use types::*;
```

### Add `ResearchRuntime` to runner context

```diff
diff --git a/crates/oxide-agent-core/src/agent/runner/types.rs b/crates/oxide-agent-core/src/agent/runner/types.rs
@@
 use crate::agent::tool_runtime::ToolRegistry as RuntimeToolRegistry;
+use crate::agent::research::ResearchRuntime;
@@
 pub struct AgentRunnerContext<'a> {
@@
     pub tool_runtime_registry: Option<Arc<RuntimeToolRegistry>>,
+    /// Shared deterministic research state for this execution.
+    pub research_runtime: Option<ResearchRuntime>,
@@
 }
@@
             tool_runtime_registry: None,
+            research_runtime: None,
```

### Add research to hook context

```diff
diff --git a/crates/oxide-agent-core/src/agent/hooks/types.rs b/crates/oxide-agent-core/src/agent/hooks/types.rs
@@
 use crate::llm::ToolDefinition;
+use crate::agent::research::ResearchRuntime;
@@
 pub struct HookContext<'a> {
@@
     pub search_limit: Option<usize>,
+    pub research: Option<&'a ResearchRuntime>,
 }
@@
             search_limit: None,
+            research: None,
         }
     }
+
+    #[must_use]
+    pub const fn with_research_runtime(
+        mut self,
+        research: Option<&'a ResearchRuntime>,
+    ) -> Self {
+        self.research = research;
+        self
+    }
```

### Pass research runtime to hook builders

```diff
diff --git a/crates/oxide-agent-core/src/agent/runner/hooks.rs b/crates/oxide-agent-core/src/agent/runner/hooks.rs
@@
         .with_memory_behavior(ctx.memory_behavior.as_deref())
+        .with_research_runtime(ctx.research_runtime.as_ref())
         .with_search_limit(ctx.config.search_limit)
```

Apply this to every `HookContext::new(...).with_*` chain in `runner/hooks.rs`.

### Record full typed tool output

```diff
diff --git a/crates/oxide-agent-core/src/agent/runner/tools.rs b/crates/oxide-agent-core/src/agent/runner/tools.rs
@@
         let tool_name = output.tool_name.as_str().to_string();
@@
-        self.apply_after_tool_hooks(ctx, state, &tool_name, &content);
+        if let Some(research) = ctx.research_runtime.as_ref() {
+            research.record_tool_output(&tool_name, &output);
+        }
+
+        self.apply_after_tool_hooks(ctx, state, &tool_name, &content);
```

This is the least invasive fix for typed evidence recording.

### Register final guard

```diff
diff --git a/crates/oxide-agent-core/src/agent/hooks/mod.rs b/crates/oxide-agent-core/src/agent/hooks/mod.rs
@@
 pub mod completion;
+pub mod final_answer_guard;
@@
 pub use completion::CompletionCheckHook;
+pub use final_answer_guard::FinalAnswerGuardHook;
```

```diff
diff --git a/crates/oxide-agent-core/src/agent/executor/config.rs b/crates/oxide-agent-core/src/agent/executor/config.rs
@@
-    CompletionCheckHook, EpisodicExtractHook, HotContextHealthHook, RetrievalAdvisorHook,
+    CompletionCheckHook, EpisodicExtractHook, FinalAnswerGuardHook, HotContextHealthHook,
+    RetrievalAdvisorHook,
@@
         let mut runner = AgentRunner::new(Arc::clone(&llm_client));
         runner.register_hook(Box::new(CompletionCheckHook::new()));
+        if crate::config::get_research_guard_enabled() {
+            runner.register_hook(Box::new(FinalAnswerGuardHook::new()));
+        }
```

### Initialize runtime in prepared execution

```diff
diff --git a/crates/oxide-agent-core/src/agent/executor/types.rs b/crates/oxide-agent-core/src/agent/executor/types.rs
@@
 use crate::agent::tool_runtime::ToolRegistry as RuntimeToolRegistry;
+use crate::agent::research::ResearchRuntime;
@@
 pub(super) struct PreparedExecution {
@@
     pub(super) runner_config: AgentRunnerConfig,
+    pub(super) research_runtime: Option<ResearchRuntime>,
 }
@@
         ctx.tool_runtime_registry = Some(Arc::clone(&self.tool_runtime_registry));
+        ctx.research_runtime = self.research_runtime.clone();
```

```diff
diff --git a/crates/oxide-agent-core/src/agent/executor/execution.rs b/crates/oxide-agent-core/src/agent/executor/execution.rs
@@
+        let research_runtime = crate::config::get_research_runtime_enabled().then(|| {
+            ResearchRuntime::new(
+                task_id,
+                task,
+                ResearchRuntimeConfig::from_env_and_options(options),
+            )
+        });
+
         PreparedExecution {
@@
             runner_config: AgentRunnerConfig::new(...)
                 .with_reasoning_effort(options.reasoning_effort()),
+            research_runtime,
         }
```

### SearXNG structured payload

```diff
diff --git a/crates/oxide-agent-core/src/agent/providers/searxng/format.rs b/crates/oxide-agent-core/src/agent/providers/searxng/format.rs
@@
-use std::fmt::Write;
+use serde_json::{Value, json};
+use chrono::Utc;
+use std::fmt::Write;
@@
 pub fn format_search_results(...) -> String { ... }
+
+pub fn format_search_results_with_payload(
+    args: &SearxngSearchArgs,
+    response: &SearxngSearchResponse,
+) -> (String, Value) {
+    let max_results = args.normalized_max_results();
+    let markdown = format_search_results(&args.query, response, max_results);
+    let results = response.results.iter().take(max_results).enumerate().map(|(idx, r)| {
+        json!({
+            "rank": idx + 1,
+            "title": r.title,
+            "url": r.url,
+            "snippet": r.content,
+            "engine": r.engine,
+            "published_at": null
+        })
+    }).collect::<Vec<_>>();
+
+    let payload = json!({
+        "provider": "searxng",
+        "kind": "search",
+        "query": args.query.trim(),
+        "page": args.normalized_page(),
+        "max_results": max_results,
+        "results": results,
+        "answers": response.answers,
+        "suggestions": response.suggestions,
+        "corrections": response.corrections,
+        "unresponsive_engines": response.unresponsive_engines,
+        "fetched_at": Utc::now(),
+    });
+
+    (markdown, payload)
+}
```

```diff
diff --git a/crates/oxide-agent-core/src/agent/providers/searxng/provider.rs b/crates/oxide-agent-core/src/agent/providers/searxng/provider.rs
@@
- .map(|output| normalizer.success(&invocation, &output, ""))
+ .map(|result| {
+     let mut output = normalizer.success(&invocation, &result.markdown, "");
+     output.structured_payload = Some(result.payload);
+     output
+ })
```

### Crawl4AI structured payload

```diff
diff --git a/crates/oxide-agent-core/src/agent/providers/crawl4ai_markdown/crawl.rs b/crates/oxide-agent-core/src/agent/providers/crawl4ai_markdown/crawl.rs
@@
-    ) -> Result<String> {
+    ) -> Result<Value> {
@@
         let payload = json!({
             "provider": TOOL_CRAWL4AI_MARKDOWN,
+            "kind": "fetch",
             "url": target_url.as_str(),
@@
             "fresh": args.fresh
+            "fetched_at": Utc::now(),
         });

-        serde_json::to_string_pretty(&payload).context("serialize crawl4ai markdown output")
+        Ok(payload)
     }
```

```diff
diff --git a/crates/oxide-agent-core/src/agent/providers/crawl4ai_markdown/executor.rs b/crates/oxide-agent-core/src/agent/providers/crawl4ai_markdown/executor.rs
@@
-            Ok(output) => Ok(normalizer.success(&invocation, &output, "")),
+            Ok(payload) => {
+                let stdout = serde_json::to_string_pretty(&payload)
+                    .unwrap_or_else(|_| payload.to_string());
+                let mut output = normalizer.success(&invocation, &stdout, "");
+                output.structured_payload = Some(payload);
+                Ok(output)
+            }
```

### WebMarkdown structured payload

```diff
diff --git a/crates/oxide-agent-core/src/agent/providers/webfetch_md/fetch.rs b/crates/oxide-agent-core/src/agent/providers/webfetch_md/fetch.rs
@@
+pub(super) struct WebMarkdownSuccess {
+    pub stdout: String,
+    pub payload: serde_json::Value,
+}
@@
-    ) -> Result<String> {
+    ) -> Result<WebMarkdownSuccess> {
@@
-        Ok(format!(
+        let stdout = format!(
             "## Web Markdown\n\nURL: {}\nContent-Type: {}\nFetched-Bytes: {}\nTruncated: {}\n\n{}",
             fetched.final_url,
             display_content_type(&fetched.content_type),
             fetched.bytes_read,
             truncated_label,
             truncated.text
-        ))
+        );
+        let payload = json!({
+            "provider": "web_markdown",
+            "kind": "fetch",
+            "url": args.url,
+            "final_url": fetched.final_url.as_str(),
+            "content_type": display_content_type(&fetched.content_type),
+            "fetched_bytes": fetched.bytes_read,
+            "markdown": truncated.text,
+            "truncated": truncated.was_truncated,
+            "fetched_at": Utc::now(),
+        });
+        Ok(WebMarkdownSuccess { stdout, payload })
     }
```

```diff
diff --git a/crates/oxide-agent-core/src/agent/providers/webfetch_md/mod.rs b/crates/oxide-agent-core/src/agent/providers/webfetch_md/mod.rs
@@
-            Ok(output) => Ok(normalizer.success(&invocation, &output, "")),
+            Ok(result) => {
+                let mut output = normalizer.success(&invocation, &result.stdout, "");
+                output.structured_payload = Some(result.payload);
+                Ok(output)
+            }
```

### Prompt guidance mode-aware

```diff
diff --git a/crates/oxide-agent-core/src/agent/prompt/composer.rs b/crates/oxide-agent-core/src/agent/prompt/composer.rs
@@
-        if has_tool(&tool_names, "duckduckgo_search") || has_tool(&tool_names, "duckduckgo_news") {
-            lines.push(
-                "Do not fetch every search result automatically; fetch only selected URLs."
-                    .to_string(),
-            );
-        }
+        lines.push(concat!(
+            "Fetch policy is research-mode aware: in standard mode, fetch selected high-value URLs; ",
+            "in research/deep_research/paranoid modes, fetch many canonical candidates when needed ",
+            "for coverage, while avoiding duplicate URLs, repeated failed hosts, anti-bot loops, and low-value mirrors."
+        ).to_string());
```

## 14. Migration plan

Phase 0: no behavior change by default.

* Add config flags.
* Default `RESEARCH_RUNTIME_ENABLED=false` initially.
* Add providers’ structured payloads safely; this is backward-compatible because stdout stays.

Phase 1: passive observe mode.

* Enable `RESEARCH_RUNTIME_ENABLED=true`.
* `RESEARCH_GUARD_ENABLED=false`.
* Record tool outputs and audit artifacts.
* Compare ledger with final answers in logs only.

Phase 2: soft guard.

* Enable `RESEARCH_GUARD_ENABLED=true` for `research/deep_research/paranoid`.
* Allow standard conceptual learning through.
* Block only obvious unsupported high-impact claims.

Phase 3: strict guard.

* Enable negative claim guard.
* Enable freshness strictness.
* Enable current politics strict source diversity.
* Enable coverage proof for exhaustive discovery.

Phase 4: sub-agent contracts.

* Treat sub-agent text as leads.
* Only structured evidence payloads from extractor/verifier sub-agents can enter ledger.
* Add parent/child research runtime linkage.

## 15. Risks / trade-offs

Main risk: false positives in claim extraction. Deterministic heuristics may over-block harmless statements. Mitigation: task classifier must exempt conceptual learning and only escalate high-impact claim types.

Second risk: final answer loops. Guard can keep forcing iterations if the model refuses to fetch or downgrade. Mitigation: next-actions must be concrete; continuation-limit behavior must require caveats or allow a guarded final with unsupported claims removed.

Third risk: evidence extraction from markdown is noisy. Mitigation: initially ledger fetched source metadata and source priority; later add extraction. Do not pretend snippet or raw sub-agent text is verified evidence.

Fourth risk: provider payload bloat. Crawl4AI/WebMarkdown markdown inside structured payload can be large. Mitigation: cap payload markdown or store artifact refs for full markdown; ledger stores quotes and locations, not necessarily whole pages.

Fifth risk: changing SearXNG failures from success-string to failure status can affect model behavior. Mitigation: preserve stdout failure message but set `status=failure`; add tests for fallback behavior.

Sixth risk: `BeforeTool` fix can affect many hooks. Mitigation: add policy observer with pairable blocked `ToolOutput` instead of aborting the whole batch.

## 16. 1-day / 3-day / 1-week implementation plan

### День 1: useful vertical slice

1. Add `agent/research` module with `ResearchState`, `ResearchRuntime`, basic config.
2. Add `ResearchRuntime` to `PreparedExecution`, `AgentRunnerContext`, `HookContext`.
3. Record full `ToolOutput` in `runner/tools.rs`.
4. Add SearXNG success structured payload.
5. Add Crawl4AI success structured payload.
6. Add WebMarkdown success structured payload as optional/fallback compatibility, not as the primary research path.
7. Add deterministic high-impact claim extractor.
8. Add `FinalAnswerGuardHook` using `AfterAgent -> ForceIteration`.
9. Add unit tests:

   * conceptual answer not blocked;
   * price/version/current claim blocked without evidence;
   * fetched non-snippet evidence allows;
   * snippet-only evidence does not allow high-impact claim.

### 3 дня: robust research mode

1. Fix real `BeforeTool` integration through runtime policy observer.
2. Replace global `SearchBudgetHook` behavior with provider-aware/mode-aware policy.
3. Add URL canonicalization and search result dedup.
4. Add source classifier.
5. Add negative claim detector and guard.
6. Add freshness classifier.
7. Add JSON audit artifact.
8. Add provider tests for SearXNG/Crawl4AI/WebMarkdown/Tavily payloads.
9. Add golden tests for conceptual, political current, technical current, legal, negative claim.

### 1 неделя: full deterministic harness

1. Full `EvidenceLedger` with claim/evidence linking.
2. Conflict detector.
3. Fetch planner with source diversity and coverage proof.
4. Query planner with intents and multilingual query expansion.
5. Sub-agent contracts:

   * Discoverer;
   * Extractor;
   * Verifier;
   * Adversarial verifier;
   * Synthesizer.
6. Human-readable audit artifact.
7. Observability metrics.
8. Strict `deep_research` / `paranoid` modes.
9. Backward-compatible rollout in `.env.example` and `profiles/web-embedded-opencode-local.toml`.

Bottom line: the right first patch is not “better prompt”. It is typed state + full `ToolOutput` observer + provider payload normalization + `AfterAgent` final guard. The primary research stack is SearXNG for discovery plus Crawl4AI for fetched evidence. After that, budget/search policy should move away from “count searches” and toward deterministic loop/dedup/freshness/source-quality controls.

## 17. Fixed RECON decisions / Q&A

These decisions are fixed after code RECON and should guide implementation unless later code evidence contradicts them.

1. Нужно ли делать `BeforeTool` fix до `ResearchRuntime`?

   Recommended answer: **да**.

   Rationale: без реального pre-tool dispatch `SearchBudgetHook`, `ToolAccessPolicyHook` и tool-blocking часть `SubAgentSafetyHook` выглядят активными, но не enforce на typed runtime path. Это correctness gap, а не enhancement.

2. Нужно ли сразу строить полный `EvidenceLedger`?

   Recommended answer: **нет**.

   Rationale: начать с passive ledger: queries, search leads, fetched sources, source priority, snippet-only flag, failures, truncation, anti-bot signals. Claim/evidence linking можно расширять после появления typed evidence boundary.

3. Guard должен быть включён по умолчанию?

   Recommended answer: **нет**.

   Rationale: сначала observe-only (`RESEARCH_RUNTIME_ENABLED=true`, `RESEARCH_GUARD_ENABLED=false`) или полностью disabled by default. Затем soft guard только для `research` / `deep_research` / `paranoid` modes.

4. Нужно ли парсить Markdown как главный источник истины?

   Recommended answer: **нет**.

   Rationale: сначала использовать `structured_payload` и source metadata. Markdown extraction можно добавить позже; extracted evidence must carry confidence and source location. Snippets and sub-agent prose are leads, not verified evidence.

5. Нужно ли сразу делать query planner / fetch planner?

   Recommended answer: **нет**.

   Rationale: first slice is observer + ledger + provider payload normalization + final guard. Planner becomes useful after runtime already has typed evidence and URL/query dedup state.

6. Что делать с SearXNG/Tavily errors-as-success?

   Recommended answer: **исправить**.

   Rationale: provider failure must be `ToolOutputStatus::Failure` with structured failure payload. Keep human-readable stdout/model message, but do not let failures enter evidence as successful observations.

7. Нужно ли считать `web_markdown` / `crawl4ai_markdown` в hard search budget?

   Recommended answer: **нет как простой counter**.

   Rationale: fetch tools need duplicate URL guard, failed-host quarantine, anti-bot quarantine, truncation handling and progress guard. A flat global search counter is the wrong control axis for self-hosted/local research.

8. Нужен ли `SUB_AGENT_SEARCH_LIMIT` сейчас?

   Recommended answer: **позже**.

   Rationale: пока `BeforeTool` не dispatch-ится реально, новый лимит будет mostly decorative. После policy integration можно добавить separate sub-agent budget if actual usage shows need.

9. Нужно ли менять prompt guidance прямо сейчас?

   Recommended answer: **минимально**.

   Rationale: prompt should not be policy. Достаточно сделать guidance research-mode-aware после появления `ResearchMode`; enforcement должен быть в runtime.

10. Что является first implementation slice?

    Recommended answer:

    1. real `BeforeTool` dispatch / policy output;
    2. `AfterAgent` `Finish` / `Block` handling;
    3. passive `ResearchRuntime` state;
    4. full `ToolOutput` observer;
    5. SearXNG + Crawl4AI structured success/failure payloads as the primary path;
    6. WebMarkdown/Tavily/Brave/DuckDuckGo normalization as optional/fallback compatibility;
    7. soft `FinalAnswerGuardHook` behind config flag.
