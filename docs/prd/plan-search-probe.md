# Search Probe v2: agentic research sidecar before main runtime

## TL;DR

Старый план про deterministic `SearchProbe` нужно считать obsolete и заменить полностью.

Новая цель: перед основным агентом в web transport запускать короткий agentic research sidecar из 1-3 свежих probe-сессий. Каждая probe-сессия использует тот же core runtime path, наследует выбранную модель/route, работает с ограниченным набором web-research tools (`searxng_search`, `crawl4ai_markdown`, fallback `web_markdown`), сама решает что искать, отдаёт пользователю короткий промежуточный TL;DR и возвращает компактный handoff. После этого main agent стартует с чистым attention и получает только original user prompt + `SearchProbeDossier`, без transcript шума от probe.

MVP строго web-only.

```text
User prompt
  ↓
Search Probe generation 1
  ↓
Search Probe generation 2
  ↓
optional generation 3
  ↓
SearchProbeDossier
  ↓
clean main agent runtime
```

Токены можно тратить свободно. Но main runtime cache-hit нужно сохранить настолько, насколько это возможно: probe data не должна попадать в stable system prompt prefix основного агента.

---

## 1. Что именно меняется относительно старого плана

Старый план был про дешёвый deterministic preflight:

```text
should_probe()
  ↓
extract protected entities
  ↓
build exact-first search queries
  ↓
score exact/near-miss
  ↓
inject ProbeReport
```

Это больше не целевая архитектура.

В новом плане нет research-эвристик в Rust:

```text
нет should_probe()
нет deterministic entity extraction
нет exact/near-miss scorer как центральной логики
нет Rust query planner
нет max_results/query-template конфигов
нет markdown parsing как "интеллекта"
```

Rust делает только orchestration protocol:

```text
enabled/disabled
generation lifecycle
timeouts/cancellation
tool allowlist
probe final contract parsing
dossier rendering
main input injection
```

Что искать, какие источники читать и когда остановиться — решает probe model.

---

## 2. Product behavior

Пользователь отправляет обычный task в web UI.

Если `OXIDE_SEARCH_PROBE_ENABLED=true`, перед основным runtime запускается Search Probe pipeline:

```text
Generation 1:
  - fresh AgentSession / AgentExecutor
  - same selected model route
  - probe instructions
  - allowed tools: searxng/crawl4ai/web_markdown
  - input: original user prompt
  - output: public_update + handoff + continue/stop

Generation 2:
  - fresh AgentSession / AgentExecutor
  - input: original user prompt + handoff from generation 1
  - output: public_update + cumulative handoff + continue/stop

Generation 3:
  - optional, only if previous generation says continue and max_generations allows it

Main runtime:
  - original user task session
  - clean attention, no probe transcript
  - input: SearchProbeDossier + original user prompt
```

Example UX:

```text
User:
Поясни как реанимировать работу картинок? ...

Search probe generation 1:
TL;DR: похоже, проблема не в Vision модели, а в цепочке capabilities client/proxy/model route.

Search probe generation 2:
Нашёл важную деталь: для custom provider/model в OpenCode нужно явно объявить image input modality.

Main agent:
TL;DR: добавь modalities.input: ["text", "image"] в описание моделей и проверь, что new-api прокидывает image parts.
```

---

## 3. Web-only MVP boundary

MVP реализуется только в web transport:

```text
crates/oxide-agent-transport-web/src/server/search_probe.rs
```

Подключение:

```rust
// crates/oxide-agent-transport-web/src/server/mod.rs
mod search_probe;
```

Почему web-only для MVP:

```text
- пользователь сейчас хочет web UX;
- меньше surface area;
- можно проверить поведение без изменений Telegram transport;
- core runtime не должен разрастаться до подтверждения, что схема работает.
```

Но реализация probe должна использовать core `AgentExecutor`, а не отдельный HTTP search client. Это важно, чтобы поведение было максимально близко к обычному agent runtime.

---

## 4. Точка интеграции в web task executor

Основной вызов агента сейчас живёт в web task executor:

```text
crates/oxide-agent-transport-web/src/server/task_executor.rs:399
```

Parent executor write-lock начинается внутри `spawn_executor_task`:

```text
crates/oxide-agent-transport-web/src/server/task_executor.rs:415
```

Main executor call:

```text
crates/oxide-agent-transport-web/src/server/task_executor.rs:455
crates/oxide-agent-transport-web/src/server/task_executor.rs:472
```

Probe нужно запускать до parent executor lock, иначе long-running search sidecar будет держать lock основного session executor.

Целевой порядок:

```rust
fn spawn_executor_task(ctx: ExecutorTaskCtx) {
    tokio::spawn(async move {
        let ExecutorTaskCtx { ... } = ctx;

        let run_request = search_probe::maybe_run_search_probe(
            SearchProbeCtx {
                session_manager: session_manager.clone(),
                session_id: session_id.clone(),
                task_id: task_id.clone(),
                progress_tx: tx.clone(),
                queued_at,
                agent_started_at,
            },
            run_request,
        )
        .await;

        let result = {
            let mut executor = executor_arc.write().await;
            match run_request { ... }
        };

        // existing completion/error handling
    });
}
```

Event collector уже стартует до `spawn_executor_task`, поэтому probe events можно отправлять в тот же stream:

```text
crates/oxide-agent-transport-web/src/server/task_executor.rs:267
```

---

## 5. Когда запускать probe

MVP rule:

```text
if OXIDE_SEARCH_PROBE_ENABLED=true:
  TaskRunRequest::Execute -> run probe
  TaskRunRequest::ResumeUserInput -> skip probe
```

Без content heuristics.

Создание обычного task запускает `Execute`:

```text
crates/oxide-agent-transport-web/src/server/task_routes.rs:693
```

Создание task version тоже запускает `Execute`:

```text
crates/oxide-agent-transport-web/src/server/task_routes.rs:1056
```

Resume user input запускает `ResumeUserInput`:

```text
crates/oxide-agent-transport-web/src/server/task_routes.rs:1177
```

Почему resume без probe: resume часто является продолжением уже идущего runtime, ответом на уточнение, approval/confirmation flow или передачей данных. Автоматический research sidecar там может сломать семантику.

---

## 6. Probe generation final contract

Каждая generation должна завершаться machine-readable contract.

Prompt просит модель вернуть:

```xml
<search_probe_public_update>
Короткий TL;DR для пользователя. 1-3 предложения.
</search_probe_public_update>

<search_probe_handoff>
Компактный handoff для следующей generation или main agent:
- what was searched
- what was found
- source URLs
- confidence
- unresolved questions
- likely answer direction
- what main agent should verify
- what main agent must not assume
</search_probe_handoff>

<search_probe_decision>
continue | stop
</search_probe_decision>
```

Rust parser не делает research decisions. Он только извлекает секции.

Fallback parsing:

```text
если public_update отсутствует:
  public_update = markdown_preview(final_response)

если handoff отсутствует:
  handoff = whole final_response

если decision отсутствует:
  decision = continue, кроме последней allowed generation
```

---

## 7. Prompt для probe generation

Probe prompt должен быть стабильным и скучным.

Он не должен содержать dynamic timestamps, random ids или volatile transport fields, кроме generation index и handoff content в user message.

System/profile instructions для probe:

```text
You are Search Probe, a short-lived research sidecar for the main agent.

Goal:
- Investigate the user's task before the main runtime starts.
- Use available web research tools when useful.
- Prefer primary sources, docs, repos, changelogs, issues, and source-backed facts.
- Produce a compact handoff, not a final user answer.
- Do not ask the user questions.
- Do not mutate external state.
- Do not use tools outside web research.

Output contract:
- Always return search_probe_public_update.
- Always return search_probe_handoff.
- Return search_probe_decision as continue or stop.
```

Generation user input:

```text
### Original user task

{original_user_prompt}

### Previous Search Probe handoffs

{handoffs_from_previous_generations}

### Your generation

Generation {n} of at most {max_generations}.
Use search/crawl tools as needed. Return the required XML-like contract.
```

Важно: prompt не должен заранее говорить какие query строить. Query planning делает модель.

---

## 8. Probe runtime creation

Нужен web-only helper в `WebSessionManager`, например:

```rust
pub(crate) async fn create_search_probe_executor(
    &self,
    session_id: &str,
    task_id: &str,
    generation_index: usize,
    config: &SearchProbeConfig,
) -> Option<AgentExecutor>
```

Он создаёт fresh ephemeral executor:

```text
- новый AgentSession;
- новый SessionId;
- отдельный ephemeral AgentMemoryScope;
- без hydrate_agent_memory;
- без memory checkpoint;
- без записи probe transcript в durable storage;
- без регистрации в SessionRegistry;
- same Arc<LlmClient>;
- same Arc<AgentSettings>;
- same selected model route override;
- probe-specific execution profile/tool policy.
```

`WebSessionManager` уже содержит нужные зависимости:

```text
crates/oxide-agent-transport-web/src/session.rs:208
crates/oxide-agent-transport-web/src/session.rs:210
```

Доступ к LLM/settings уже есть:

```text
crates/oxide-agent-transport-web/src/session.rs:380
crates/oxide-agent-transport-web/src/session.rs:386
```

Обычная session creation уже применяет selected model route override:

```text
crates/oxide-agent-transport-web/src/session.rs:644
crates/oxide-agent-transport-web/src/session.rs:648
```

Probe helper должен переиспользовать ту же route-selection logic внутри `session.rs`, а не дублировать её в `server/search_probe.rs`.

---

## 9. Model/effort inheritance

Probe наследует выбранную модель/route из web session.

Это важно для UX:

```text
если пользователь выбрал конкретную модель в web UI,
Search Probe должен использовать её же,
а не глобальный default route.
```

Effort policy:

```text
request.effort = Heavy    -> probe Heavy
request.effort = Extended -> probe Extended or Heavy by config
request.effort = Standard -> probe min_effort from config, default Heavy
```

Причина: цель Search Probe v2 — сделать систему кратно умнее. Экономия токенов не является приоритетом.

---

## 10. Probe tool policy

MVP allowlist:

```text
searxng_search
crawl4ai_markdown
web_markdown
```

`web_markdown` нужен только как fallback, потому что `webfetch_md` и `crawl4ai_markdown` mutually exclusive at runtime. Если Crawl4AI включён, основным extraction tool будет `crawl4ai_markdown`; если нет — `web_markdown`.

Не давать probe tools, которые мутируют состояние или расширяют blast radius:

```text
- sandbox exec/fileops/recreate
- ssh
- manager control plane
- reminders
- agents_md mutation
- file delivery
- stack logs
- ytdlp
- delegation
- wiki memory mutation
- todos mutation
```

Tool allowlist уже поддерживается через `ToolAccessPolicy`:

```text
crates/oxide-agent-core/src/agent/profile.rs:108
crates/oxide-agent-core/src/agent/profile.rs:137
crates/oxide-agent-core/src/agent/profile.rs:150
```

Tool registry уже фильтрует executors по policy:

```text
crates/oxide-agent-core/src/agent/executor/registry.rs:299
crates/oxide-agent-core/src/agent/executor/registry.rs:306
```

---

## 11. Search budget policy

Не делать deterministic `max_results` или query limits.

Ограничители MVP:

```text
- max_generations: 1..3
- per_generation_timeout_secs
- total_timeout_secs
- cancellation token
- normal AgentExecutor iteration/timeout limits from effort
```

Если существующий `search_budget` hook мешает probe, для probe execution profile можно отключить или ослабить именно этот hook. Но не нужно строить отдельную deterministic budget model.

Main runtime search budget не уменьшается из-за probe. Search Probe — pre-runtime discovery, а не часть main agent search quota.

---

## 12. Public updates and event stream

MVP не требует новых `AgentEvent` variants.

Используем существующие:

```text
AgentEvent::Milestone
AgentEvent::Reasoning
AgentEvent::ToolCall
AgentEvent::ToolResult
```

`Reasoning` уже есть:

```text
crates/oxide-agent-core/src/agent/progress.rs:183
```

Events:

```text
search_probe_started
search_probe_generation_started
search_probe_generation_completed
search_probe_completed
search_probe_failed
```

После каждой generation отправляем public update:

```text
Reasoning: TL;DR: ...
```

Если `OXIDE_SEARCH_PROBE_FORWARD_TOOL_EVENTS=true`, probe runtime tool events форвардятся в тот же web task stream. Пользователь видит, что probe реально ищет и читает страницы.

Для MVP это допустимо: task timeline становится timeline всего пользовательского task, включая pre-runtime sidecar и main runtime.

---

## 13. SearchProbeDossier

Main agent не получает probe transcript.

Main agent получает только original user prompt plus compact dossier. Original prompt stays first; dossier is appended as auxiliary context so user intent remains primary.

```text
{original_user_prompt}

<search_probe_dossier>
Generated before the main agent runtime by web Search Probe.
Treat this as research grounding and leads, not as final truth.
Verify important claims before final answer.

<generation index="1">
<public_update>
...
</public_update>
<handoff>
...
</handoff>
</generation>

<generation index="2">
<public_update>
...
</public_update>
<handoff>
...
</handoff>
</generation>

<final_synthesis>
...
</final_synthesis>

<decision>
stop
</decision>

<truncated>
false
</truncated>

<instructions_for_main_runtime>
- Use this as starting context, not as proof.
- Verify source-backed claims before final answer.
- Do not repeat unsupported assumptions.
- If sources are insufficient, say so explicitly.
</instructions_for_main_runtime>
</search_probe_dossier>
```

Dossier content rules:

```text
1. Include compact generation handoffs, public updates only if useful, final synthesis, and final decision.
2. Do not include full probe transcript, raw tool outputs, internal reasoning, or internal event stream.
3. If there are no handoffs, do not inject a dossier; run the main runtime with unchanged input.
4. If `dossier_max_chars` is exceeded, preserve the newest handoffs first and set `<truncated>true</truncated>`.
5. Renderer is deterministic formatting only. It does not interpret research content.
```

Failure dossier uses the same XML-like envelope:

```text
{original_user_prompt}

<search_probe_dossier>
<partial_failure>true</partial_failure>
<failure_summary>
Search probe was attempted but failed or returned partial output.
</failure_summary>
<generation index="1">
<handoff>
partial notes if available
</handoff>
</generation>
<instructions_for_main_runtime>
Main runtime should perform its own verification.
</instructions_for_main_runtime>
</search_probe_dossier>
```

Renderer is deterministic formatting only. It does not interpret research content.

---

## 14. Injection into main runtime

Only modify `AgentUserInput.content`.

Do not drop attachments.

`AgentUserInput` shape:

```text
crates/oxide-agent-core/src/agent/executor.rs:97
crates/oxide-agent-core/src/agent/executor.rs:100
crates/oxide-agent-core/src/agent/executor.rs:121
```

Helper shape:

```rust
fn inject_search_probe_dossier(
    mut input: AgentUserInput,
    dossier: &SearchProbeDossier,
) -> AgentUserInput {
    if dossier.is_empty() {
        return input;
    }

    let original = input.content;
    input.content = format!(
        "{}\n\n{}",
        original,
        dossier.render_for_main_runtime(),
    );
    input
}
```

Attachments remain unchanged.

---

## 15. Cache-hit policy

Goal: не ломать main runtime prompt cache больше необходимого.

Rules:

```text
1. Не менять core prompt composer для MVP.
2. Не добавлять SearchProbeDossier в system prompt.
3. Не добавлять probe results в stable prefix.
4. Inject dossier only into user input after the original user prompt.
5. Main executor keeps the same system prompt/tool schema path.
6. Probe instructions should be stable across requests.
7. Dynamic probe data stays in generation user input and final dossier.
8. Do not add timestamps to prompt-visible probe text unless needed by the user task.
```

Probe runtimes сами могут иметь отдельный prompt/tool allowlist и хуже cache-hit. Это приемлемо. Важнее, чтобы main runtime стартовал с тем же cacheable prefix, что и раньше.

---

## 16. Config MVP

```text
OXIDE_SEARCH_PROBE_ENABLED=false

OXIDE_SEARCH_PROBE_MAX_GENERATIONS=2
# clamp 1..3

OXIDE_SEARCH_PROBE_PER_GENERATION_TIMEOUT_SECS=180
OXIDE_SEARCH_PROBE_TOTAL_TIMEOUT_SECS=480

OXIDE_SEARCH_PROBE_MIN_EFFORT=heavy
# standard | extended | heavy

OXIDE_SEARCH_PROBE_PUBLIC_UPDATES=true
OXIDE_SEARCH_PROBE_FORWARD_TOOL_EVENTS=true

OXIDE_SEARCH_PROBE_TOOL_ALLOWLIST=searxng_search,crawl4ai_markdown,web_markdown

OXIDE_SEARCH_PROBE_DOSSIER_MAX_CHARS=80000
```

`DOSSIER_MAX_CHARS` не для экономии токенов, а как safety guard против случайного raw transcript dump.

Не добавлять в MVP:

```text
- OXIDE_SEARCH_PROBE_MAX_RESULTS
- exact/near-miss flags
- query templates
- entity extraction config
- should_probe marker list
```

---

## 17. Failure behavior

Search Probe best-effort.

If probe fails:

```text
- searxng unavailable
- crawl4ai timeout
- model error
- generation timeout
- invalid final contract
```

Main runtime still starts.

Dossier, if there is at least one partial handoff, uses the approved XML-like envelope after the original prompt:

```text
{original_user_prompt}

<search_probe_dossier>
<partial_failure>true</partial_failure>
<failure_summary>
Search probe was attempted but failed or returned partial output.
</failure_summary>

<generation index="1">
<handoff>
partial notes if available
</handoff>
</generation>

<instructions_for_main_runtime>
Main runtime should perform its own verification.
</instructions_for_main_runtime>
</search_probe_dossier>
```

If probe produces no handoffs at all, do not inject a dossier; start main runtime with unchanged input.

Exception: if user cancels task while probe is running, main runtime must not start.

---

## 18. Cancellation

Probe must respect the same task cancellation semantics as main runtime.

Implementation requirement:

```text
- if task cancellation is requested during probe, stop current generation;
- emit search_probe_cancelled;
- do not inject dossier;
- do not start main executor;
- persist task as cancelled using existing web task cancellation flow.
```

Do not create a separate cancellation universe for probe.

---

## 19. Data structures

Suggested module-local types:

```rust
pub(crate) struct SearchProbeConfig {
    pub enabled: bool,
    pub max_generations: usize,
    pub per_generation_timeout_secs: u64,
    pub total_timeout_secs: u64,
    pub min_effort: SearchProbeEffort,
    pub public_updates: bool,
    pub forward_tool_events: bool,
    pub tool_allowlist: Vec<String>,
    pub dossier_max_chars: usize,
}

pub(crate) enum SearchProbeDecision {
    Continue,
    Stop,
}

pub(crate) struct SearchProbeGenerationResult {
    pub generation_index: usize,
    pub public_update: String,
    pub handoff: String,
    pub decision: SearchProbeDecision,
    pub final_response_raw: String,
    pub completed: bool,
    pub error: Option<String>,
}

pub(crate) struct SearchProbeDossier {
    pub original_task_preview: String,
    pub generations: Vec<SearchProbeGenerationResult>,
    pub partial_failure: bool,
    pub truncated: bool,
}
```

These types live in web transport for MVP.

---

## 20. Implementation path

### Step 1: replace docs

Replace old plan with this v2 plan.

### Step 2: add web module

Add:

```text
crates/oxide-agent-transport-web/src/server/search_probe.rs
```

Wire:

```text
crates/oxide-agent-transport-web/src/server/mod.rs
```

### Step 3: add orchestrator API

```rust
pub(crate) async fn maybe_run_search_probe(
    ctx: SearchProbeCtx,
    run_request: TaskRunRequest,
) -> TaskRunRequest
```

Behavior:

```text
Execute + enabled -> run generations -> inject dossier -> Execute
Execute + disabled -> Execute unchanged
ResumeUserInput -> unchanged
probe failure -> Execute with failure dossier or unchanged input, depending on failure point
task cancellation -> do not start main runtime
```

### Step 4: integrate before executor lock

Insert `maybe_run_search_probe` in `spawn_executor_task` before `executor_arc.write().await`.

### Step 5: add ephemeral executor helper

Add helper in `WebSessionManager` / `session.rs` so it can reuse private model selection logic.

Do not duplicate model route selection in `server/search_probe.rs`.

### Step 6: add probe execution profile

Create `AgentExecutionProfile` with:

```text
- agent_id = search_probe
- prompt_instructions = stable Search Probe instructions
- tool_policy = allowlist(searxng_search, crawl4ai_markdown, web_markdown)
- hook policy = default, except optional search_budget relaxation if needed
```

`AgentExecutionProfile` supports prompt/tool policy:

```text
crates/oxide-agent-core/src/agent/profile.rs:350
crates/oxide-agent-core/src/agent/profile.rs:357
crates/oxide-agent-core/src/agent/profile.rs:379
crates/oxide-agent-core/src/agent/profile.rs:385
```

### Step 7: generation runner

For each generation:

```text
- create fresh probe executor;
- build generation input from original prompt + previous handoffs;
- run execute_user_input_with_options;
- collect final response;
- parse public_update/handoff/decision;
- emit public update;
- stop if decision=stop;
- continue until max_generations.
```

The normal executor input path is attachment-aware:

```text
crates/oxide-agent-core/src/agent/executor/execution.rs:916
```

For probe generations, do not pass user attachments by default. The main runtime still receives original attachments. If later we want image-aware probe, make it explicit.

### Step 8: event forwarding

If enabled, pass a proxy `mpsc::Sender<AgentEvent>` to probe executor and forward probe tool events to the existing web task stream.

### Step 9: dossier render and inject

Render compact dossier and inject into main input.

### Step 10: tests

Add focused tests around orchestration and no deterministic research behavior.

---

## 21. Tests

Minimum test set:

```text
1. Execute runs probe when enabled.
2. Execute skips probe when disabled.
3. ResumeUserInput never runs probe in MVP.
4. Probe runs before parent executor write-lock.
5. Probe failure still starts main runtime.
6. Cancellation during probe prevents main runtime start.
7. Dossier injection preserves AgentUserInput.attachments.
8. Probe executor inherits selected model route.
9. Probe tool policy exposes only searxng/crawl4ai/web_markdown.
10. Invalid probe final contract falls back to raw final response handoff.
11. decision=stop prevents unnecessary later generations.
12. main runtime system prompt path is not modified by search_probe module.
```

Explicitly do not add tests for deterministic query construction or exact/near-miss scoring, because that logic is not part of v2.

---

## 22. What not to build in MVP

Do not build:

```text
- core-level SearchProbe abstraction;
- Telegram transport integration;
- deterministic query planner;
- entity scorer;
- custom search HTTP clients;
- new search result storage tables;
- new AgentEvent variants;
- long-term probe memory;
- probe transcript persistence;
- separate observability stack;
- new crates.
```

Keep MVP boring:

```text
web orchestrator + ephemeral AgentExecutor + allowlisted tools + final contract parser + dossier injection
```

---

## 23. Acceptance criteria

The feature is acceptable when:

```text
- enabling env var causes web Execute tasks to run 1-3 probe generations before main runtime;
- user sees short probe TL;DR updates before main answer;
- probe uses selected model route;
- probe can call searxng_search and crawl4ai_markdown/web_markdown;
- main runtime receives only SearchProbeDossier + original prompt;
- main runtime starts with clean attention, no probe transcript in memory;
- attachments are preserved for main runtime;
- probe failures do not fail the task;
- cancellation during probe stops the task;
- no deterministic query/scoring logic is introduced;
- main system prompt/cacheable prefix is not changed by the feature.
```

---

## 24. Final formulation

Search Probe v2 is a web-only pre-runtime agentic research sidecar.

Before the main agent starts, the web task executor runs 1-3 fresh probe runtimes on the same selected model. Each probe generation uses only web research tools, publishes a short user-visible TL;DR, and returns a compact handoff. The handoffs are rendered into `SearchProbeDossier`. The main agent then starts clean and receives only the dossier plus the original user prompt.

No deterministic research logic. No exact-match scorer. No query templates. The intelligence lives in the probe model and normal tool use.
