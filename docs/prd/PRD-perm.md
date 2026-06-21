# PRD: `Permanent Life Mode` для Oxide Agent

Статус: `design / ready for implementation after live contract verification`

Дата: `2026-06-20`

Основание:

- локальный снимок репозитория из архива `Oxide-Agent-dev(33).zip`
- текущие upstream-источники по `ly-wang19/engram`: `README.md`, `API.md`, `engram/server/app.py`, `engram/service.py`, `engram/memory.py`, `engram/types.py`, paper `arXiv:2606.09900`, лицензия

Ограничение источников:

- в интернете есть несколько разных проектов с именем `Engram`; в этом документе `Engram` всегда означает только `ly-wang19/engram`
- документ не опирается на старую обучающую память; только на текущий локальный код и текущие внешние источники

---

## 1. Решение

Нужен не `еще один чат`, а отдельный bounded context:

- **текущий chat / topic / session mode остается как есть**
- **`permanent life mode` делается отдельно**
- **источник истины** для life mode = `Postgres`, не `Engram`
- **`Engram` = производный long-term memory engine / retrieval index**, rebuildable из данных Oxide
- **один life principal** должен быть доступен и из web, и из Telegram
- **никакой интеграции Engram в обычные web-session / telegram-topic session**

Корневой тезис:

> `Permanent life mode` нельзя честно построить на текущих transport-scoped chat sessions. Его надо строить как отдельный продуктовый режим со стабильной identity-моделью, собственной БД-моделью, собственным runtime-контрактом и отдельной memory-пайплайной: `Postgres hot/default context + Engram long-term memory`.

---

## 2. Что подтверждено в текущем Oxide Agent

### 2.1. Hot/context memory уже живет в Postgres и имеет стабильный scope

Подтверждено:

- `AgentMemoryScope` уже существует и состоит из `user_id`, `context_key`, `flow_id` — `crates/oxide-agent-core/src/agent/session.rs:77-86`
- `agent_memory_snapshots` уже есть и ключуется по `(user_id, context_key, flow_id)` — `migrations/0003_core_storage.sql:36-48`
- `agent_flows`, `user_contexts`, `topic_contexts`, `topic_agents_md` уже существуют — `migrations/0003_core_storage.sql:12-143`

Следствие:

- life mode **может и должен** использовать существующую durable hot-context persistence в Postgres
- отдельный `context_key = "life"`, `flow_id = "main"` уже ложится в существующий контракт без насилия над текущим storage

### 2.2. Текущий web mode — намеренно session-scoped, не permanent

Подтверждено:

- web при создании сессии генерирует `context_key = format!("web-session-{session_id}")` — `crates/oxide-agent-transport-web/src/server/session_routes.rs:408-445`
- web runtime создает `AgentSession::new_with_scopes(...)`, гидратит `AgentMemory`, ставит checkpoint и только потом создает `AgentExecutor` — `crates/oxide-agent-transport-web/src/session.rs:633-745`
- web API хранит список `context_keys` и поддерживает session history / branching semantics — `crates/oxide-agent-web-contracts/src/sessions.rs:188-199`

Следствие:

- web-session модель — это **правильная** модель для обычных чатов
- это **не** модель для одной непрерывной жизни человека между web и Telegram

### 2.3. Текущий Telegram mode — тоже transport/thread scoped

Подтверждено:

- Telegram вычисляет `context_key` из `chat_id + thread_spec` — `crates/oxide-agent-transport-telegram/src/bot/context.rs:23-33`
- `ensure_session()` создает `AgentSession` c `AgentMemoryScope(user_id, context_key, flow_id)`, грузит flow memory, инжектит topic `AGENTS.md`, создает `AgentExecutor` — `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/session.rs:275-309`

Следствие:

- Telegram chat/topic mode тоже **не является permanent identity runtime**
- life mode нельзя лепить поверх forum/topic/session ключей

### 2.4. Первая системная мина: web и Telegram живут в разных identity-моделях

Подтверждено:

- web auth выделяет **случайный** положительный `i64 user_id` — `crates/oxide-agent-transport-web/src/auth.rs:318-356`
- Telegram использует внешний Telegram user id как `i64 user_id` — `crates/oxide-agent-transport-telegram/src/runner.rs:191-205`
- таблица `users` — просто `user_id BIGINT PRIMARY KEY`, без namespace транспорта — `migrations/0002_web_persistence.sql:3-7`
- helper просто обеспечивает наличие `users(user_id)` — `crates/oxide-agent-core/src/storage/sqlx/helpers.rs:27-42`

Следствие:

- сегодня web-user и Telegram-user **не являются одной и той же сущностью**
- без отдельного principal/linking слоя никакой честной общей `life memory` между web и Telegram быть не может
- если оставить как есть, то `life mode` будет либо раздвоенным, либо сломанным по идентичности

### 2.5. Вторая системная мина: runtime registries process-local

Подтверждено:

- Telegram держит глобальный статический `SESSION_REGISTRY` — `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/session.rs:22`
- `SessionRegistry` — это просто in-memory `HashMap` над `AgentExecutor` и `RuntimeContextInbox` — `crates/oxide-agent-runtime/src/session_registry.rs:16-21`, `crates/oxide-agent-runtime/src/session_registry.rs:29-37`
- web создает собственный `SessionRegistry` в своем бинаре — `crates/oxide-agent-transport-web/src/bin/oxide-agent-web-console.rs:259-272`

Следствие:

- текущая модель сессий не может быть источником истины для одного life-agent, если web и Telegram — разные процессы
- `permanent life mode` обязан опираться на **DB-backed queue/state**, а не на memory of process

### 2.6. Runtime continuation primitive уже есть и его надо переиспользовать

Подтверждено:

- `RuntimeContextInbox` уже существует — `crates/oxide-agent-core/src/agent/session.rs:236-271`
- `AgentSession` уже умеет держать runtime context inbox — `crates/oxide-agent-core/src/agent/session.rs:318-327`, `crates/oxide-agent-core/src/agent/session.rs:371-389`
- runner уже умеет забирать pending runtime context и превращать его в continuation — `crates/oxide-agent-core/src/agent/runner/execution.rs:207-235`
- Telegram уже использует `enqueue_runtime_context` для follow-up во время активного run — `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/task_runner.rs:857-862`

Следствие:

- для life mode не нужно изобретать continuation semantics с нуля
- нужно сделать **DB-backed эквивалент inbox**, чтобы continuation работал между web и Telegram, а не только внутри одного процесса

### 2.7. Текущая durable memory = wiki memory и она hardwired в executor/tool runtime

Подтверждено:

- `AgentExecutor` держит `wiki_memory_store` — `crates/oxide-agent-core/src/agent/executor.rs:163-182`
- builder/config тоже завязан на `with_wiki_memory_store()` — `crates/oxide-agent-core/src/agent/executor/config.rs:88-103`
- prompt assembly вставляет wiki context перед system prompt — `crates/oxide-agent-core/src/agent/executor/execution.rs:464-497`
- `prompt composer` вставляет отдельный wiki block — `crates/oxide-agent-core/src/agent/prompt/composer.rs:579-591`
- tool runtime context несет `wiki_memory_store`, и есть отдельный module `tool/wiki-memory` — `crates/oxide-agent-core/src/agent/tool_runtime/modules.rs:208-223`, `crates/oxide-agent-core/src/agent/tool_runtime/modules.rs:607-634`
- background write path в wiki идет fire-and-forget через `tokio::spawn` после successful run — `crates/oxide-agent-core/src/agent/executor/execution.rs:628-756`
- текущие memory hooks используют keyword heuristics для “memory advisor” и episodic extraction — `crates/oxide-agent-core/src/agent/hooks/memory.rs:24-120`, `crates/oxide-agent-core/src/agent/hooks/memory.rs:123-168`

Следствие:

- **нельзя** честно подменить wiki на engram только “на проводах” внутри текущего chat mode
- для life mode нужен **новый bounded context**, а в core — правильный abstraction seam для dynamic context provider

### 2.8. В проекте уже есть место для секретов, и это надо уважать

Подтверждено:

- существует таблица `private_secrets` — `migrations/0003_core_storage.sql:144-150`

Следствие:

- life mode не должен складывать секреты в Engram
- если пользователь просит “запомнить API key / пароль / токен”, это должно идти в secret storage или быть отвергнуто как неподходящий тип memory

### 2.9. Reminder schema transport-biased

Подтверждено:

- `reminder_jobs.chat_id` обязателен (`NOT NULL`) — `migrations/0004_reminders_audit.sql:3-27`
- в доменной модели это прямо описано как `Destination Telegram chat identifier` — `crates/oxide-agent-core/src/storage/reminder.rs:143-148`

Следствие:

- существующий reminder contract не является transport-agnostic
- full cross-transport life reminders требуют отдельного пересмотра destination-контракта

---

## 3. Что подтверждено в `ly-wang19/engram`

Ниже только факты, подтвержденные текущими upstream-источниками.

### 3.1. Что Engram реально дает

Подтверждено:

- upstream описывает Engram как durable/queryable memory across sessions, которая хранит episodes, извлекает atomic facts, ведет bi-temporal историю, умеет contradiction handling и hybrid retrieval — `README.md`
- `Episode` = append-only raw event/turn; `Fact` = atomic claim с valid time + transaction time + provenance + supersession chain; `WorkingMemory` = ephemeral tier, которая не должна загрязнять long-term store — `engram/types.py`
- `Memory` facade прямо собирает ingest, consolidation и hybrid read path за API `add()/consolidate()/search()/as_of()/history()/profile()` — `engram/memory.py`
- paper `arXiv:2606.09900` утверждает dual-process architecture: fast write path для raw episodes + async consolidation + hybrid read path с facts + chunks — `paper`

### 3.2. Текущий HTTP contract Engram

Подтверждено:

- `API.md` и `server/app.py` документируют `/v1/remember`, `/v1/recall`, `/v1/profile`, `/v1/profile/structured`, `/v1/memories`, `/v1/facts`, `/v1/conflicts`, `/v1/export`, `/v1/forget`, `/v1/chat/completions`, `/v1/import`
- `scope` у `/v1/remember` = `auto | long | working`
- `scope=auto` должен маршрутизировать ephemeral state в working memory, не загрязняя durable profile, но dated episode все равно сохраняется
- manual facts через `/v1/facts` помечаются как authoritative и не должны silently override-иться автоэкстракцией

### 3.3. Ключевые ограничения текущей surface area Engram

Подтверждено:

- auth model в server сейчас = `ENGRAM_API_KEYS=user:key,...` или `ENGRAM_OPEN=1`; один Bearer key = один isolated namespace — `engram/server/app.py`, `API.md`
- server прямо говорит, что он single-node + file-backed by default — `engram/server/app.py`, `engram/service.py`
- HTTP `/v1/recall` сегодня hardwired на `answer=True` в route, хотя service layer умеет `answer=False` — `engram/server/app.py`, `engram/service.py`
- `/v1/remember` принимает flat `content: str`, а не структурированный episode payload

Следствие:

- текущий upstream surface **подходит как reference implementation**, но **не является идеальным production contract** для Oxide life mode
- это не причина отказываться от Engram; это причина **взять его как engine и исправить контракт**

### 3.4. Лицензия

Подтверждено:

- в репозитории есть dual-license: `AGPL-3.0` или commercial license — `COMMERCIAL-LICENSE.md`
- текст прямо говорит: если вы запускаете modified version как network service и не хотите выполнять AGPL obligations, нужна commercial license — `COMMERCIAL-LICENSE.md`

Следствие:

- до shipping life mode как networked service нужно сознательно выбрать legal path:
  - либо соблюдать AGPL/open-source obligations для модифицированной версии
  - либо получить commercial license
  - либо переписать backend с нуля, не используя код Engram

---

## 4. Product definition

### 4.1. Что такое `Permanent Life Mode`

`Permanent Life Mode` — это не список независимых чатов.

Это:

- **один long-lived principal**
- **один canonical transcript/event stream**
- **один stable memory scope**
- **один default context state**
- **один long-term memory namespace**
- **два интерфейса доступа**: web и Telegram

У life mode нет концепции `web-session-<uuid>` и нет привязки к Telegram thread как к identity.

### 4.2. Что life mode обязан уметь

1. Пользователь пишет из web или Telegram — попадает в одну и ту же жизнь.
2. Контекст между устройствами и транспортами не теряется.
3. Последние активные вещи доступны сразу из Postgres hot/default context.
4. Долговременные факты, процедуры, проекты, предпочтения, биографические вещи и прошлые решения доступны через Engram recall.
5. Пользователь может смотреть, редактировать, удалять память.
6. Секреты не утекают в Engram.
7. Life mode не ломает и не загрязняет обычный chat mode.

### 4.3. Что life mode не делает

- не заменяет текущий normal chat / topic / session mode
- не включает Engram в текущие обычные чаты
- не запоминает ambient group/forum chat как “личную жизнь” пользователя
- не использует OpenAI-compatible `/v1/chat/completions` Engram как agent runtime
- не делает Engram источником истины

---

## 5. Главные мины и как их разминировать

### Мина 1. Нельзя строить universal life поверх текущих `user_id`

Проблема:

- web и Telegram сегодня дают разные типы identity
- одна и та же “жизнь” не может стабильно адресоваться из обоих транспортов

Решение:

- ввести **life principal layer**
- не использовать Telegram external id как primary key life identity
- использовать внутренний `principal_user_id: i64` как единственную life identity
- добавить transport link table `provider + provider_subject -> principal_user_id`

### Мина 2. Нельзя хранить canonical life state в process memory

Проблема:

- web и Telegram — разные процессы
- текущий registry process-local

Решение:

- Postgres = source of truth для life inputs / turns / runs / events / checkpoints
- serialization = per-principal advisory lock
- execution = DB-backed queue + worker/gateway runner

### Мина 3. Нельзя считать Engram source of truth

Проблема:

- upstream сейчас single-node + file-backed by default
- вы хотите иметь свободу переписать backend на Rust без потери данных

Решение:

- source of truth = Oxide Postgres transcript + structured profile + snapshots
- Engram = derived long-term memory index / recall engine
- rebuild должен быть возможен из Postgres

### Мина 4. Нельзя слать в Engram сырую product semantics без структуры

Проблема:

- flat `/v1/remember {content: string}` не несет нормального source/provenance contract
- assistant output может случайно стать durable fact

Решение:

- structured episode ingest
- role-aware payload
- promotion policy:
  - `user explicit memory assertions` — promotable
  - `confirmed structured profile updates` — authoritative
  - `tool outputs` — promotable only if tool is trusted
  - `assistant free-form text` — episodic only by default

### Мина 5. Нельзя смешивать temporary override и durable personality

Проблема:

- “сегодня отвечай подробно” и “по умолчанию отвечай кратко” — разные уровни памяти

Решение:

- `life_context_overrides` в Postgres с TTL
- `life_profile_state` в Postgres для deterministic defaults
- Engram только для long-term semantic/episodic/procedural memory

### Мина 6. Нельзя складывать секреты в memory engine

Проблема:

- personal life mode неизбежно встретит токены, пароли, приватные реквизиты

Решение:

- секреты идут в `private_secrets` или не принимаются как memory
- перед outbox->Engram обязателен sensitivity/redaction gate
- Engram не получает raw secrets ни в facts, ни в episodes

### Мина 7. Нельзя строить life mode как новый special-case в prompt composer

Проблема:

- сегодня в core жестко зашит `wiki_context`

Решение:

- вынести generic `DynamicPromptContextProvider`
- chat mode продолжит использовать wiki provider
- life mode получит свой provider: `PG deterministic state + hot handoff + Engram recall`

### Мина 8. Нельзя считать current reminder contract transport-agnostic

Проблема:

- reminders завязаны на Telegram destination contract

Решение:

- v1 life mode не должен зависеть от cross-transport reminder delivery
- если reminders нужны в life mode как first-class feature — менять destination contract на transport-agnostic

---

## 6. Контрактный тест

### 6.1. Boundary: `Transport -> LifeGateway`

Передающая сторона знает надежно:

- `provider` (`web` / `telegram`)
- `provider_subject` (web user id / telegram user id)
- текст входа
- attachment refs
- correlation id
- transport metadata

Передающая сторона **не знает**:

- life principal id
- состояние текущего life run
- можно ли влить сообщение в активный run или надо поставить в очередь
- какие memory facts уже есть в Engram

Контракт должен быть:

```text
submit_life_input(provider, provider_subject, content, attachments, metadata)
-> resolved principal + queued/started run id
```

Не должен быть:

```text
open_or_resume_session_with_context_key
reuse current web-session id
caller picks flow id / fact ids / memory ids
```

### 6.2. Boundary: `LifeGateway -> LifeRuntime`

Gateway знает надежно:

- `principal_user_id`
- `input_id`
- payload входа

Gateway не знает:

- как собирать prompt context
- как сериализовать concurrent inputs
- что писать в long-term memory

Контракт:

```text
process_principal_input(principal_user_id, input_id)
```

### 6.3. Boundary: `LifeRuntime -> Engram`

LifeRuntime знает надежно:

- principal id / tenant id
- turn ids / run ids / transport provenance
- roles сообщений
- trusted tool outputs
- explicitness user request
- sensitivity classification outcome

LifeRuntime не должен требовать от LLM:

- `fact_id`
- `episode_id` чужой системы
- точные ids конфликтов
- ручной выбор namespace key string

Контракт:

```text
append_episode(tenant, structured_episode)
assert_fact(tenant, assertion)
recall_context(tenant, query, budget, filters)
forget(tenant, descriptor_or_internal_id)
```

### 6.4. Boundary: `LLM -> memory tools`

LLM знает только intent и descriptors.

Контракт tools:

```text
life_memory_search(query, scope?, limit?, as_of?)
life_memory_remember(content, permanence, category?)
life_memory_forget(descriptor, reason)
life_profile_set(patch)  // only for explicit user defaults/preferences
```

LLM **не** оперирует raw Engram IDs.

Если нужен выбор из нескольких кандидатов, tool runtime возвращает local handles, созданные принимающей стороной, scoped to current run.

### 6.5. Precedence contract

Ответ на текущий turn определяется так:

1. system/developer rules
2. текущий explicit user request этого turn
3. `life_context_overrides` (TTL)
4. `life_profile_state` (authoritative defaults)
5. hot handoff / checkpoint context
6. Engram recall (evidence, not instruction source)

Это обязательный контракт. Иначе life memory начнет спорить с текущей задачей или с системными правилами.

---

## 7. Целевая архитектура

```text
┌─────────────┐         ┌─────────────┐
│   Web /life │         │ Telegram DM │
└──────┬──────┘         └──────┬──────┘
       │                       │
       │ (web user id)         │ (tg user id)
       ▼                       ▼
       └───────────┬───────────┘
                   ▼
           ┌───────────────┐
           │  LifeGateway  │
           └───────┬───────┘
                   │
         resolve (provider, subject)
                   │
                   ▼
         principal_user_id = 100500
                   │
    ┌──────────────┼──────────────────────────┐
    │              │                           │
    ▼              ▼                           ▼
┌────────┐  ┌───────────┐              ┌──────────────┐
│life_   │  │life_inputs│              │ Postgres     │
│turns   │  │ (queue)   │              │ source of    │
│(log)   │  └─────┬─────┘              │ truth        │
└────────┘        │                    └──────────────┘
                  │ queued
                  ▼
          ┌───────────────┐
          │  LifeWorker   │ ◄── per-principal lock
          └───────┬───────┘
                  │
                  ▼
          ┌───────────────┐
          │ AgentExecutor │ ◄── ephemeral, per run
          │ (existing)    │
          └───────┬───────┘
                  │
          ┌───────┴───────┐
          ▼               ▼
    READ from PG    READ from Engram
    (always)        (on demand)
          │               │
          └───────┬───────┘
                  │ assembled prompt
                  ▼
              LLM answer
                  │
          run finished
                  │
        ┌─────────┴──────────────┐
        ▼                        ▼
  ┌───────────┐          ┌───────────────┐
  │ write to  │          │ post-run      │
  │ PG: turn  │          │ curator (LLM) │
  │ +snapshot │          └───────┬───────┘
  └───────────┘                  │
                         structured payload
                         (durable/ephemeral/
                          profile/secret)
                                  │
                                  ▼
                         ┌───────────────┐
                         │sensitivity gate│
                         └───────┬───────┘
                          clean │ redacted │ secret
                                 │
                                 ▼
                         ┌───────────────┐
                         │life_engram_   │
                         │outbox (PG)    │
                         │pending        │
                         └───────┬───────┘
                                 │ async
                                 ▼
                         ┌───────────────┐
                         │ outbox worker │ ◄── Rust task, не LLM
                         └───────┬───────┘
                                 │
                                 ▼
                         ┌───────────────┐
                         │    Engram     │ ◄── derived index
                         │ (dumb engine) │     rebuildable
                         └───────────────┘
```

Postgres tables (source of truth):

- `life_principals`
- `life_identity_links`
- `life_turns`
- `life_inputs`
- `life_runs`
- `life_events`
- `life_context_overrides`
- `life_engram_outbox`
- `agent_memory_snapshots` (existing, reused)

LifeWorker / Orchestrator components:

- `AgentExecutor` (existing core, stable life scope)
- `DynamicPromptContextProvider` — PG deterministic state + Hot handoff + Engram recall
- `Post-run memory curator` (LLM via existing client) — durable vs ephemeral classification + promotion policy + sensitivity flags + structured outbox payload
- `EngramMemoryBackend` — structured episode ingest + context-only recall + fact assertion / forget / conflicts

### 7.1. Базовый принцип

Life mode не хранит canonical состояние в `AgentExecutor` между transport requests.

Correct model:

- **canonical state lives in Postgres**
- executor — ephemeral compute object, который на каждый run гидратится из PG и в PG же коммитится
- Engram — производная память для recall, которую можно rebuild-ить из PG

### 7.2. Stable life scope

Для одного principal:

```text
user_id    = principal_user_id
context_key = "life"
flow_id     = "main"
```

Это позволит без нового storage-формата использовать:

- `agent_memory_snapshots`
- `agent_flows`
- current memory checkpoint machinery

### 7.3. Почему life mode не должен опираться на текущий `SessionRegistry`

Потому что registry:

- process-local
- transport-local
- не является shared truth

`SessionRegistry` остается полезным для обычного chat mode.

Для life mode он может использоваться только как локальная оптимизация внутри worker-а, но не как архитектурная опора.

---

## 8. Identity model

### 8.1. Решение

Использовать **внутренний** `principal_user_id: i64` как canonical life principal.

Не вводить life identity через Telegram raw id.

### 8.2. Почему не надо переписывать всю codebase на новый UUID principal сейчас

Это увеличит blast radius на весь проект, хотя задача ограничена отдельным bounded context.

Правильный шаг:

- для life mode сделать отдельный principal layer поверх уже существующего `users.user_id`
- текущие обычные чаты не трогать

### 8.3. Таблицы

#### `life_principals`

```sql
CREATE TABLE life_principals (
    principal_user_id BIGINT PRIMARY KEY REFERENCES users(user_id) ON DELETE CASCADE,
    profile_state JSONB NOT NULL,
    settings JSONB NOT NULL,
    schema_version INTEGER NOT NULL DEFAULT 1,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);
```

Смысл:

- authoritative structured state
- life feature flag / settings

#### `life_identity_links`

```sql
CREATE TABLE life_identity_links (
    provider TEXT NOT NULL,
    provider_subject TEXT NOT NULL,
    principal_user_id BIGINT NOT NULL REFERENCES life_principals(principal_user_id) ON DELETE CASCADE,
    verified_at BIGINT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (provider, provider_subject)
);
```

Примеры:

- `("web", "<web-user-id>") -> principal_user_id`
- `("telegram", "<telegram-user-id>") -> principal_user_id`

#### `life_link_tokens`

```sql
CREATE TABLE life_link_tokens (
    token_hash TEXT PRIMARY KEY,
    principal_user_id BIGINT NOT NULL REFERENCES life_principals(principal_user_id) ON DELETE CASCADE,
    target_provider TEXT NOT NULL,
    expires_at BIGINT NOT NULL,
    consumed_at BIGINT,
    created_at BIGINT NOT NULL
);
```

### 8.4. Linking flow

#### Web-first

1. Пользователь залогинен в web.
2. Открывает `Life -> Link Telegram`.
3. Сервер создает one-time token.
4. Пользователь отправляет `/link <token>` боту в Telegram DM.
5. Telegram handler валидирует token и создает `life_identity_links(provider='telegram', provider_subject=tg_user_id)`.
6. После этого Telegram life mode работает на том же `principal_user_id`.

#### Telegram-first

Нужен shared allocator внутреннего `user_id`, потому что сегодня allocator живет только в web auth (`crates/oxide-agent-transport-web/src/auth.rs:344-356`).

Правильное решение:

- вынести allocator в shared storage service
- Telegram-first onboarding создает `users` row + `life_principals`
- later web account links to same principal

---

## 9. Storage model

### 9.1. `life_turns` — canonical transcript / event log

```sql
CREATE TABLE life_turns (
    turn_id UUID PRIMARY KEY,
    principal_user_id BIGINT NOT NULL REFERENCES life_principals(principal_user_id) ON DELETE CASCADE,
    run_id UUID,
    role TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'system', 'tool')),
    source_transport TEXT NOT NULL CHECK (source_transport IN ('web', 'telegram', 'internal')),
    source_ref TEXT,
    content TEXT NOT NULL,
    attachments JSONB NOT NULL DEFAULT '[]'::jsonb,
    redaction_state TEXT NOT NULL DEFAULT 'clean' CHECK (redaction_state IN ('clean', 'redacted', 'secret-blocked')),
    created_at BIGINT NOT NULL
);
```

Это — первичный append-only журнал жизни.

### 9.2. `life_inputs` — DB-backed continuation queue

```sql
CREATE TABLE life_inputs (
    input_id UUID PRIMARY KEY,
    principal_user_id BIGINT NOT NULL REFERENCES life_principals(principal_user_id) ON DELETE CASCADE,
    turn_id UUID NOT NULL REFERENCES life_turns(turn_id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK (status IN ('queued', 'claimed', 'consumed', 'dead')),
    claimed_by TEXT,
    claimed_at BIGINT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);
```

Назначение:

- если life agent уже работает, новые user inputs не теряются
- worker может забирать их на safe iteration boundaries и превращать в continuation

### 9.3. `life_runs`

```sql
CREATE TABLE life_runs (
    run_id UUID PRIMARY KEY,
    principal_user_id BIGINT NOT NULL REFERENCES life_principals(principal_user_id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK (status IN ('queued', 'running', 'completed', 'failed', 'cancelled')),
    started_at BIGINT,
    finished_at BIGINT,
    last_checkpoint_at BIGINT,
    error_text TEXT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);
```

### 9.4. `life_events` — transport-neutral progress/event stream

```sql
CREATE TABLE life_events (
    event_id UUID PRIMARY KEY,
    run_id UUID NOT NULL REFERENCES life_runs(run_id) ON DELETE CASCADE,
    seq BIGINT NOT NULL,
    kind TEXT NOT NULL,
    payload JSONB NOT NULL,
    created_at BIGINT NOT NULL,
    UNIQUE(run_id, seq)
);
```

Назначение:

- web SSE может читать progress из БД
- Telegram handler может обновлять status message/typing state без прямой связи с worker process
- event stream остается после рестартов

### 9.5. `life_context_overrides`

```sql
CREATE TABLE life_context_overrides (
    override_id UUID PRIMARY KEY,
    principal_user_id BIGINT NOT NULL REFERENCES life_principals(principal_user_id) ON DELETE CASCADE,
    key TEXT NOT NULL,
    value JSONB NOT NULL,
    reason TEXT,
    expires_at BIGINT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);
```

Примеры:

- `answer_verbosity = "detailed"` до конца дня
- `current_focus = "prepare PRD"` на 2 часа

### 9.6. `life_engram_outbox`

```sql
CREATE TABLE life_engram_outbox (
    outbox_id UUID PRIMARY KEY,
    principal_user_id BIGINT NOT NULL REFERENCES life_principals(principal_user_id) ON DELETE CASCADE,
    idempotency_key TEXT NOT NULL UNIQUE,
    payload JSONB NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'flushing', 'flushed', 'dead')),
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at BIGINT NOT NULL,
    last_error TEXT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);
```

### 9.7. Используем существующий `agent_memory_snapshots`

Life mode не изобретает новый hot-memory storage.

Он использует уже существующую таблицу:

- `migrations/0003_core_storage.sql:36-48`

со stable scope:

```text
(user_id = principal_user_id, context_key = "life", flow_id = "main")
```

---

## 10. Memory model

### 10.1. Слои памяти

#### Layer A. `life_profile_state` в Postgres

Это authoritative deterministic state.

Содержит только то, что должно быть надежно доступно **каждый run**, без риска retrieval miss.

Примеры:

- display name
- timezone
- default language
- default answer style
- confirmed durable user preferences
- pinned constraints / standing instructions

#### Layer B. `AgentMemory` snapshot в Postgres

Это hot runtime state:

- последние сообщения
- compaction summary / handoff
- текущий working state
- pending user input state

Это то, что нужно для continuity между runs и after restart.

#### Layer C. `life_turns`

Это canonical append-only transcript.

Назначение:

- audit/debug/rebuild
- fallback source
- feed для Engram outbox

#### Layer D. `Engram`

Это long-term memory engine / retrieval index:

- episodic memory
- semantic memory
- procedural memory
- contradiction history / provenance / supersession

**Не source of truth.**

#### ASCII-схема слоёв

```text
┌──────────────────────────────────────────────────────────────┐
│ Layer A. life_profile_state (Postgres)                       │
│                                                               │
│  ВСЕГДА injected, БЕЗ поиска, БЕЗ retrieval miss              │
│  имя, timezone, язык, стиль, standing constraints             │
│  ↑ только из explicit user action / confirmed instruction     │
│                                                               │
│  Curator формирует КАНДИДАТЫ → применяются из user turn       │
│  Engram НЕ пишет сюда (§13.6 — запрещено)                     │
├──────────────────────────────────────────────────────────────┤
│ Layer B. agent_memory_snapshots (Postgres, existing table)    │
│                                                               │
│  scope = (principal_user_id, "life", "main")                  │
│  последние сообщения, compaction summary, working state       │
│  синхронный final checkpoint в конце run                      │
├──────────────────────────────────────────────────────────────┤
│ Layer C. life_turns (Postgres, append-only)                   │
│                                                               │
│  полный протокол жизни: role, transport, content, redaction   │
│  audit / debug / rebuild Engram из transcript                 │
├──────────────────────────────────────────────────────────────┤
│ Layer D. Engram (derived, rebuildable)                        │
│                                                               │
│  episodic / semantic / procedural / contradictions            │
│  recall → EVIDENCE в prompt, НЕ instruction                   │
│  если Engram умер — жизнь продолжается на Layers A-C          │
└──────────────────────────────────────────────────────────────┘
```

### 10.2. Почему default context memory должен жить в Postgres

Потому что есть класс вещей, которые retrieval не имеет права “иногда не вспомнить”:

- язык ответа по умолчанию
- базовый стиль общения
- timezone
- пользовательское имя
- явные standing constraints

Эти данные должны быть injected deterministically, а не доставаться только через semantic retrieval.

### 10.3. Promotion policy

#### В `life_profile_state` попадает только:

- явное действие пользователя в UI
- explicit user instruction вида “по умолчанию …”, “запомни как настройку …”
- подтвержденное изменение профиля

#### В Engram manual/assertive memory попадает:

- явное `remember this permanently`
- подтвержденные долгоживущие предпочтения/биография/проекты/процедуры

#### В Engram episodic memory попадает:

- turns, события, outcomes, решения, tool observations

#### В durable memory **не** попадает:

- временное состояние дня
- текущая одноразовая задача
- мимолетная style override
- raw secrets

### 10.4. Secret/sensitivity gate

Перед записью в `life_engram_outbox` обязательно:

1. sensitivity classification
2. redaction / deny
3. route to `private_secrets`, если это действительно secret-type datum

Иначе personal life mode очень быстро станет хранилищем токенов и паролей, что архитектурно недопустимо.

### 10.5. Post-run memory curator

Static promotion rules (§10.3) надёжно работают для explicit cases: "запомни навсегда", "по умолчанию отвечай X". Но есть класс borderline cases где deterministic rules недостаточны:

- "Я последние полгода работаю над Oxide Agent, это мой основной проект" — durable fact (биография/проект), но нет маркера "запомни"
- "Раньше я предпочитал подробные ответы, но сейчас мне нужна краткость" — profile update с supersession, не явное "по умолчанию"
- borderline sensitivity — не явный секрет, но содержит персональные данные

Для этих случаев life mode использует **post-run memory curator** — single LLM-вызов после завершённого run, перед outbox.

#### Что curator делает

Curator получает transcript завершённого run и для каждого candidate memory item определяет:

- `durable_fact` — долговременный факт → Engram (authoritative или episodic по promotion policy)
- `profile_update` — confirmed default/preference → кандидат в `life_profile_state` (применяется только из confirmed user turn)
- `ephemeral` — мимолётное, не в durable memory
- `secret_candidate` — флаг для sensitivity gate
- `skip` — не worth remembering

#### Что curator НЕ делает

- НЕ является continuously-running агентом — fire-and-forget после run
- НЕ имеет tools — single structured-output LLM call
- НЕ пишет в Engram напрямую — формирует payload для outbox
- НЕ пишет в `life_profile_state` напрямую — формирует кандидаты, применяется из user turn
- НЕ инициирует обратный поток из Engram в Postgres (см. §13.6)

#### Runtime — переиспользование существующего LLM client

Curator использует существующую LLM client infrastructure (`llm/client.rs`, providers в `llm/providers/`). Никакого нового LLM client, нового provider, новой HTTP инфраструктуры, нового абстракционного слоя.

Конфигурация через env:

```text
LIFE_CURATOR_PROVIDER=<существующий провайдер: openrouter, mistral, anthropic, ...>
LIFE_CURATOR_MODEL=<модель>
```

Temperature не задаётся — используется default провайдера. Что даст провайдер — то и используется. Single structured-output call, не multi-turn agent.

#### Why not deterministic-only

Current wiki planner (`wiki_memory/planner.rs`) — deterministic, keyword heuristics (`contains("запомни")`). Достаточно для explicit cases. Но для personal life mode класс borderline cases шире. Deterministic rules либо пропустят durable facts без маркера, либо запишут мимолётное как durable. Curator закрывает этот класс через LLM understanding, не regex.

#### Relationship to existing wiki planner

Wiki planner остаётся для chat mode (behavior change = 0). Curator — только для life mode. Разные bounded contexts, разные sinks (wiki pages vs engram outbox), общий паттерн (post-run analysis).

---

## 11. Prompt context assembly

### 11.1. Root refactor в core

Текущая hardcoded граница:

- `crates/oxide-agent-core/src/agent/executor/execution.rs:464-497`
- `crates/oxide-agent-core/src/agent/prompt/composer.rs:579-591`

Нужно вынести generic abstraction:

```rust
pub trait DynamicPromptContextProvider: Send + Sync {
    async fn build_blocks(&self, request: PromptContextRequest) -> Result<Vec<PromptContextBlock>>;
}

pub struct PromptContextBlock {
    pub name: String,
    pub body: String,
    pub semantics: PromptContextSemantics,
}
```

Где `PromptContextSemantics` как минимум различает:

- `DeterministicRuleLike`
- `AuthoritativeUserDefault`
- `EvidenceOnly`

### 11.2. Chat mode и life mode после refactor

#### Chat mode

- продолжает использовать текущий wiki provider
- behavior change = `0`

#### Life mode

Использует свой provider:

1. `Life deterministic state`
2. `Life active overrides`
3. `Life hot handoff`
4. `Life long-term memory recall`

### 11.3. Как должен выглядеть life memory block

Пример:

```text
## Life Defaults
- Default answer language: Russian
- Default style: concise technical markdown
- Timezone: UTC+3
- Standing preference: be direct; no fluff

## Active Temporary Overrides
- Today only: answer in extra detail

## Long-Term Memory (evidence)
- [2026-06-18, explicit] User prefers architecture-first decisions over local patches.
- [2026-06-19, project:oxide-agent] User intends to remove LLM Wiki and replace it with a real memory layer.
- [2026-06-20, procedure] For design work, first verify contracts and blast radius.
```

Требование:

- `Long-Term Memory` должен быть framed как **evidence**, а не как instruction source

---

## 12. Runtime model

### 12.1. Submit path

```text
web/telegram input
  -> resolve principal
  -> write life_turns(user)
  -> write life_inputs(queued)
  -> try start/attach run
```

### 12.2. Serialization

Использовать per-principal advisory lock.

В проекте уже есть helper вокруг `pg_advisory_xact_lock`:

- `crates/oxide-agent-core/src/storage/sqlx/helpers.rs:50-66`

Нужно использовать lock key вида:

```text
life:<principal_user_id>
```

### 12.3. Worker loop

```text
claim lock
load principal state
hydrate AgentSession from agent_memory_snapshots(scope=life/main)
build dynamic prompt context
run AgentExecutor
at safe boundaries: drain life_inputs -> enqueue_runtime_context
stream AgentEvents -> life_events
on finish:
  - write assistant turn
  - persist final checkpoint synchronously
  - run post-run memory curator (LLM via existing client, §10.5)
      → structured payload: durable/ephemeral/profile/secret classification
  - sensitivity gate (rules + curator flags, §10.4)
  - write engram outbox row(s) with curated payload
  - mark run completed
release lock
```

### 12.4. Continuation during active run

Если пока worker думает приходит новый message из другого транспорта:

- он не пытается открыть еще один executor
- он попадает в `life_inputs`
- worker на safe boundary подхватывает input и превращает его в `RuntimeContextInjection`

Это правильное продолжение уже существующего runtime контракта, но вынесенное из process-local inbox в DB-backed queue.

### 12.5. Final checkpoint commit

Текущее checkpoint persistence может быть async/coalesced.

Для life mode этого недостаточно как единственного контракта.

Требование:

- в конце run обязателен **synchronous final snapshot commit**
- он должен коммититься вместе с final assistant turn и Engram outbox row

Так life mode не теряет последнюю консистентную точку после ответа пользователю.

### 12.6. Пошаговый поток одного сообщения

```text
Шаг 1. Submit — пользователь пишет
═══════════════════════════════════
    Web /life  или  Telegram DM
       │
       ▼  submit_life_input(provider, subject, text, ...)
    LifeGateway
       │
       ├─ resolve: life_identity_links(provider, subject) → principal
       ├─ INSERT life_turns (role=user, transport, content)
       ├─ INSERT life_inputs (status=queued)
       └─ return {principal, run_id, queued: true}


Шаг 2. Worker — сборка prompt
═════════════════════════════
    LifeWorker
       │
       ├─ pg_advisory_xact_lock("life:<principal>")  ◄── один lock
       ├─ load profile_state из Postgres (Layer A, мгновенно, ВСЕГДА)
       ├─ load hot snapshot из Postgres (Layer B, мгновенно, ВСЕГДА)
       ├─ recall из Engram (Layer D, только если нужно)
       │
       │  Собранный prompt:
       │  ┌─────────────────────────────────────────┐
       │  │ ## Life Defaults          (Layer A)     │
       │  │ ## Active Overrides       (TTL)         │
       │  │ ## Hot Handoff            (Layer B)     │
       │  │ ## Long-Term Memory       (Layer D)     │
       │  │   (evidence, НЕ instruction)            │
       │  │ [system rules]                          │
       │  │ [user message]                          │
       │  └─────────────────────────────────────────┘
       │
       ▼
    AgentExecutor → LLM → answer


Шаг 3. Final commit — синхронно в Postgres (одна транзакция)
════════════════════════════════════════════════════════════
    ├─ INSERT life_turns (role=assistant, content=response)
    ├─ UPDATE agent_memory_snapshots (final checkpoint)
    └─ UPDATE life_runs SET status=completed

    Если процесс упал здесь → ответ и snapshot сохранены.


Шаг 4. Post-run curator — что worth remembering
═══════════════════════════════════════════════
    Curator (LLM, single call, fire-and-forget, existing llm/client.rs)
    получает transcript завершённого run → классифицирует:
    │
    ├─ durable_fact    → Engram (authoritative или episodic)
    ├─ profile_update  → кандидат в life_profile_state (из user turn)
    ├─ ephemeral       → не в durable memory
    ├─ secret_candidate→ флаг для sensitivity gate
    └─ skip            → не worth remembering


Шаг 5. Sensitivity gate
═══════════════════════
    payload от curator → rules + curator flags
       │
       ├─ clean     → outbox как есть
       ├─ redacted  → секрет заменён, outbox обезличенным
       └─ secret    → DENY, маршрут в private_secrets


Шаг 6. Outbox — почтовый ящик для Engram
═════════════════════════════════════════
    INSERT life_engram_outbox (pending, idempotency_key)
    Postgres = source of truth. Engram может быть недоступен — ничего не теряется.


Шаг 7. Outbox worker → Engram (асинхронно)
═══════════════════════════════════════════
    Outbox worker (Rust task, НЕ LLM)
       ├─ SELECT FROM life_engram_outbox WHERE status=pending
       ├─ POST /v1/internal/episodes → Engram
       ├─ успех → status=flushed
       └─ неудача → retry с backoff, idempotency_key защищает от дублей
```

---

## 13. Engram integration contract

### 13.1. Что делать нельзя

Нельзя:

- использовать `Engram /v1/chat/completions` как основной runtime path
- использовать MCP как mandatory memory path
- требовать от LLM Engram `fact_id`
- подстраивать Oxide под текущий flat `/v1/remember` как будто это финальный контракт

### 13.2. Что делать правильно

Использовать Engram как internal long-term memory engine behind Oxide adapter.

Целевой trait:

```rust
#[async_trait]
pub trait LifeLongTermMemoryBackend: Send + Sync {
    async fn recall_context(&self, req: LifeMemoryRecallRequest) -> Result<LifeMemoryRecallResult>;
    async fn append_episode(&self, req: LifeEpisodeAppendRequest) -> Result<LifeMemoryWriteReceipt>;
    async fn assert_fact(&self, req: LifeFactAssertionRequest) -> Result<LifeMemoryWriteReceipt>;
    async fn forget(&self, req: LifeForgetRequest) -> Result<LifeForgetReceipt>;
    async fn list_conflicts(&self, principal_user_id: i64) -> Result<Vec<LifeMemoryConflict>>;
}
```

### 13.3. Почему текущий upstream HTTP surface надо расширить

Текущий upstream:

- auth = key->namespace env map
- `/v1/remember` = flat string
- `/v1/recall` = hardwired answering HTTP route

Для Oxide life mode нужен контракт уровня продукта:

#### `POST /v1/internal/context`

```json
{
  "tenant_id": "life:123",
  "query": "какие у меня принципы по архитектуре памяти",
  "max_tokens": 3000,
  "include": ["semantic", "episodic", "procedural"],
  "redact_sensitive": true,
  "answer": false
}
```

#### `POST /v1/internal/episodes`

```json
{
  "tenant_id": "life:123",
  "external_id": "turn:4e9c...",
  "run_id": "8b5b...",
  "observed_at": "2026-06-20T10:15:00Z",
  "messages": [
    {"role": "user", "text": "Запомни: для Oxide Agent приоритет — фундаментальные решения."},
    {"role": "assistant", "text": "Принял. Сохраню как долговременный принцип."}
  ],
  "trusted_tool_outputs": [],
  "promotion_policy": {
    "promote_user_assertions": true,
    "promote_assistant_output": false,
    "promote_tool_outputs": true
  },
  "metadata": {
    "transport": "web",
    "principal_user_id": 123,
    "source_system": "oxide-agent"
  }
}
```

#### `POST /v1/internal/assertions`

```json
{
  "tenant_id": "life:123",
  "external_id": "assert:0d2f...",
  "kind": "preference",
  "text": "User prefers architecture-first fixes over local patches.",
  "authoritative": true,
  "source": {
    "transport": "web",
    "turn_id": "4e9c..."
  }
}
```

### 13.4. Почему Oxide не должен хранить per-user API keys к Engram

Текущий upstream auth-contract (`ENGRAM_API_KEYS` / open mode) годится для manual demo и small lab use, но плох как product boundary.

Для life mode нужен internal contract:

- один backend service credential между Oxide и Engram
- tenant/user выбирается в payload/header, а не отдельным env-key на каждого пользователя

### 13.5. Что остается source of truth even with Engram

Всегда:

- `life_turns`
- `life_profile_state`
- `agent_memory_snapshots`
- `life_context_overrides`

Если Engram умер / сломан / переписывается на Rust:

- replay outbox
- reindex from transcript
- rebuild semantic memory

### 13.6. Запрещённый обратный поток

Engram → Postgres source of truth — архитектурно запрещён.

Запрещено:

- Engram (или curator, или любой компонент над Engram) пишет в `life_profile_state`
- Engram recall инициирует update durable state без explicit user confirmation
- Engram выступает источником для profile defaults

Разрешено:

- Engram recall → evidence в prompt → agent response → user confirms/corrects → profile updated from user turn

Почему:

- Engram = derived, rebuildable (§13.5). Если derived пишет в source of truth, source отравлен derived индексом
- Engram consolidation может отставать → stale fact → profile update → устаревший state как "истина"
- Profile state writable только из explicit user actions (§10.3 promotion policy)

#### ASCII-схема запрещённого и разрешённого потока

```text
ЗАПРЕЩЕНО:
                                              ┌──────────────┐
  Engram ───────────────────────────────────► │  Postgres    │
  (derived)                                   │  source of    │
  пишет в source of truth                     │  truth        │
                                              └──────────────┘
  ↑ нарушает инвариант: derived не отравляет source

  Curator ─────► profile_state напрямую
  ↑ profile writable только из confirmed user turns

  Engram ─────► /v1/chat/completions как runtime
  ↑ Engram не агент, не runtime, dumb engine

  LLM ─────► raw Engram fact_id
  ↑ LLM не оперирует чужими id (контрактный тест П0)


РАЗРЕШЕНО:
                                              ┌──────────────┐
  User turn ──► life_turns ──► Postgres ─────► │  Postgres    │
                                              │  source of    │
  Curator ───► outbox payload (pending) ─────► │  truth        │
                                              └──────┬───────┘
                                                     │
  Outbox worker ────────────────────────────► Engram (derived)
                                                     │
  Recall ◄──────────────────────────────────── Engram
     │
     ▼
  prompt (evidence) → agent → user confirms → profile update
```

### 13.7. Rerank — отложенный вопрос

Engram hybrid retrieval уже включает ranking. Cross-encoder rerank в adapter или LLM-based rerank не добавляются в v1.

Решение о rerank принимается только после измерения:

- recall quality на реальных данных life mode
- доля шума в top-N results
- влияние шума на agent response quality

Если измерения покажут что rerank нужен:

1. Cross-encoder rerank в `EngramMemoryBackend` adapter (не LLM, дёшево)
2. LLM-based rerank — только как last resort с доказанной необходимостью

Это не компонент v1. Это отложенный вопрос с измерением, не premature complexity.

---

## 14. Web UX

### 14.1. Отдельная область `/life`

Нельзя использовать существующий web session list как life UI.

Нужно:

- отдельный route `/life`
- отдельный transcript UI
- без списка независимых sessions
- без branching semantics обычного web chat mode

### 14.2. Web API

Минимум:

```text
GET    /api/life/state
GET    /api/life/turns?cursor=...
POST   /api/life/messages
GET    /api/life/runs/{run_id}/events   // SSE
GET    /api/life/profile
PATCH  /api/life/profile
GET    /api/life/memory/search?q=...
GET    /api/life/memory/conflicts
POST   /api/life/memory/forget
POST   /api/life/link/telegram
```

### 14.3. Что должно быть видно пользователю в web

- единая life transcript лента
- live progress текущего run
- memory inspector:
  - profile defaults
  - temporary overrides
  - recalled memory
  - conflict queue
  - delete/forget actions
- account linking status для Telegram

---

## 15. Telegram UX

### 15.1. Только private DM в v1

Life mode в Telegram должен жить только в private chat с ботом.

Не в группах.
Не в forum topics.
Не через ambient listening.

Иначе личная память загрязнится групповым шумом.

### 15.2. Команды

Минимум:

```text
/life          // открыть/объяснить режим
/link <token>  // привязать Telegram к web life principal
/unlink        // отвязать
/memory        // показать memory controls/help
```

### 15.3. Поведение

- если Telegram account не привязан — life mode не стартует silently; предлагает link/onboarding
- после linking DM пишет в тот же principal, что и web `/life`
- пока идет run, новые Telegram messages встают в `life_inputs` и подхватываются worker-ом как continuation

---

## 16. Examples of actual work

### 16.1. Example A — onboarding в web, продолжение в Telegram

#### Шаг 1. Web

Пользователь в `/life` пишет:

```text
Меня зовут Алекс. По умолчанию отвечай по-русски, коротко и технично.
```

Система делает:

1. `life_turns` вставляет user turn
2. `life_runs` создает run
3. `life_profile_state` обновляет:

```json
{
  "identity": {"display_name": "Алекс", "timezone": "UTC+3", "language": "ru"},
  "communication": {"default_style": "concise_technical_markdown"}
}
```

4. `agent_memory_snapshots(user_id=principal, context_key='life', flow_id='main')` обновляется после run
5. `life_engram_outbox` получает structured episode
6. worker пушит episode/assertions в Engram namespace `life:<principal>`

#### Шаг 2. Telegram

Через час в Telegram DM:

```text
Как ты должен мне отвечать по умолчанию?
```

Система:

1. резолвит тот же `principal_user_id` через `life_identity_links`
2. гидратит тот же `AgentMemoryScope(principal, "life", "main")`
3. injects deterministic defaults из PG
4. при необходимости добирает long-term evidence из Engram
5. отвечает:

```text
По умолчанию — по-русски, коротко и технично.
```

Это и есть корректная cross-transport continuity.

### 16.2. Example B — временная override не становится постоянной памятью

Пользователь пишет:

```text
Сегодня отвечай очень подробно.
```

Система должна:

- создать `life_context_overrides(key='answer_verbosity', value='detailed', expires_at=end_of_day)`
- **не** обновлять `life_profile_state.default_style`
- **не** писать это как durable preference в Engram

На следующий день, если override истек, assistant возвращается к дефолтному краткому стилю.

### 16.3. Example C — explicit permanent memory about project principle

Пользователь пишет:

```text
Запомни навсегда: для Oxide Agent приоритет — фундаментальные решения, а не локальные патчи.
```

Система должна:

1. записать user turn в `life_turns`
2. создать `life_engram_outbox` с `authoritative=true`
3. отправить в Engram manual assertion / structured episode
4. на следующем вопросе:

```text
Какой у меня принцип по архитектурным исправлениям?
```

assistant должен достать это как durable memory и ответить по существу.

### 16.4. Example D — follow-up во время активного run из другого транспорта

Сценарий:

1. В web пользователь запускает длинную задачу.
2. Пока worker работает, пользователь пишет в Telegram:

```text
И еще учти: это только для life mode, обычные чаты не трогай.
```

Правильное поведение:

- Telegram не пытается открыть второй life executor
- сообщение попадает в `life_inputs(status='queued')`
- worker на safe boundary забирает input
- превращает его в `RuntimeContextInjection`
- текущий run корректируется на лету

### 16.5. Example E — секрет не уходит в Engram

Пользователь пишет:

```text
Запомни мой API key: sk-...
```

Правильное поведение:

- life mode **не** пишет это в Engram
- либо предлагает сохранить в secret storage
- либо отказывается хранить как memory
- transcript может быть redacted
- `private_secrets` остается единственным местом для такого типа данных

---

## 17. Verification matrix before coding

Ниже каркас П0.5, который надо прогнать перед implementation.

### 17.1. Engram live contract verification

#### Проверка 1. Self-host / auth / health

```bash
B=http://localhost:8456
K=oxide-test
curl -sS "$B/health" -H "Authorization: Bearer $K"
```

Зафиксировать raw response.

#### Проверка 2. `/v1/remember`

```bash
curl -sS -X POST "$B/v1/remember" \
  -H "Authorization: Bearer $K" \
  -H "Content-Type: application/json" \
  -d '{"content":"User prefers concise technical Russian replies.","session_id":"life-test","scope":"long"}'
```

Зафиксировать:

- exact response schema
- есть ли `episode_id`
- есть ли idempotency support
- можно ли прикрепить metadata

#### Проверка 3. `/v1/recall`

```bash
curl -sS -X POST "$B/v1/recall" \
  -H "Authorization: Bearer $K" \
  -H "Content-Type: application/json" \
  -d '{"query":"How should the user be answered?","lean":true,"n_chunks":4,"session_id":"life-test"}'
```

Зафиксировать:

- exact response schema
- действительно ли route всегда возвращает `answer`
- можно ли получить context-only response без patch

#### Проверка 4. manual facts / delete / conflicts / export

```bash
curl -sS -X POST "$B/v1/facts" ...
curl -sS "$B/v1/memories" ...
curl -sS "$B/v1/conflicts" ...
curl -sS "$B/v1/export?include_sensitive=false" ...
```

Зафиксировать raw payloads и ограничения.

### 17.2. Oxide DB/runtime verification

#### Проверка 5. Stable life scope в existing snapshot table

SQL:

```sql
SELECT *
FROM agent_memory_snapshots
WHERE user_id = $1 AND context_key = 'life' AND flow_id = 'main';
```

Подтвердить, что existing checkpoint store честно работает на этом scope.

#### Проверка 6. Advisory lock serialization

Параллельно из двух процессов:

- web submit
- telegram submit

Проверить, что only one process владеет `pg_advisory_xact_lock(hash('life:<principal>'))`.

#### Проверка 7. Continuation safety

Во время длинного run добавить запись в `life_inputs` и подтвердить, что она подхватывается only at safe boundary, а не ломает history.

#### Проверка 8. Secret gate

Проверить сценарий с токеном/API key:

- не появляется в Engram outbox
- transcript redaction policy работает
- secret storage path корректен

### 17.3. Failure/recovery verification

#### Проверка 9. Crash between answer and Engram flush

Сценарий:

1. assistant ответил
2. final turn записан в `life_turns`
3. outbox row создан
4. процесс убит до flush

Ожидание:

- ответ не потерян
- hot snapshot не потерян
- outbox pending и будет допушен после рестарта

#### Проверка 10. Engram unavailable

Ожидание:

- life mode продолжает работать на `PG deterministic state + hot snapshot + transcript`
- outbox копится
- degraded mode прозрачно наблюдаем

#### Проверка 11. Curator classification quality

Сценарий:

- run с borderline durable fact (без явного "запомни")
- run с мимолётным упоминанием
- run с borderline sensitivity
- run с assistant free-form text который мог бы стать "фактом"

Ожидание:

- curator классифицирует durable vs ephemeral правильно
- curator не продвигает assistant free-form как authoritative
- curator флагирует borderline sensitivity для gate
- curator не пишет в `life_profile_state` напрямую
- curator использует существующий LLM client (нет нового provider)

---

## 18. Implementation plan

### Phase 0. Правильные abstraction seams в core

1. Вынести `DynamicPromptContextProvider`
2. Оставить chat mode с wiki provider как есть
3. Добавить life-mode provider interface без включения его в обычный chat mode

### Phase 1. Life principal/storage foundation

1. Миграции:
   - `life_principals`
   - `life_identity_links`
   - `life_link_tokens`
   - `life_turns`
   - `life_inputs`
   - `life_runs`
   - `life_events`
   - `life_context_overrides`
   - `life_engram_outbox`
2. Shared `user_id` allocator
3. Principal resolution service

### Phase 2. Life gateway + DB-backed runtime

1. Новый crate `crates/oxide-agent-life`
2. `LifeGateway`
3. `LifeWorker/Orchestrator`
4. Per-principal advisory lock
5. DB event sink

### Phase 3. Postgres default/hot context

1. `life_profile_state`
2. stable scope `("life","main")`
3. final synchronous checkpoint commit
4. temporary overrides with TTL

### Phase 4. Engram adapter

1. `LifeLongTermMemoryBackend`
2. patched/forked Engram internal API or local wrapper
3. structured episode ingest
4. context-only recall
5. outbox worker + retries + idempotency
6. post-run memory curator (LLM via existing `llm/client.rs`, env-configured model, §10.5)

### Phase 5. Web/Telegram UX

1. Web `/life`
2. Telegram `/life` DM router + linking
3. SSE/updates from `life_events`
4. memory inspector/editor/conflict review

### Phase 6. Hardening

1. secret gate
2. rebuild-from-transcript tooling
3. degraded-mode observability
4. concurrency tests
5. migration/import tools if later понадобятся

---

## 19. Blast radius

### Тронуть придется

#### Core

- `crates/oxide-agent-core/src/agent/executor/execution.rs:464-497`
- `crates/oxide-agent-core/src/agent/prompt/composer.rs:579-591`
- вероятно новый generic dynamic context provider seam в `oxide-agent-core`

#### New bounded context

- новый crate: `crates/oxide-agent-life`
- новый life storage/service/orchestrator layer
- post-run memory curator (LLM call via existing `llm/client.rs`, без нового provider)

#### Web

- новые `/api/life/*` routes
- новый `/life` UI path
- без изменения semantics обычных session routes (`crates/oxide-agent-transport-web/src/server/session_routes.rs:408-445` остаются chat-mode path)

#### Telegram

- отдельный router для life mode в private DM
- не использовать обычный topic routing как life identity path (`crates/oxide-agent-transport-telegram/src/bot/context.rs:23-33` остается chat/topic path)

#### Storage / migrations

- новые migrations для life tables
- existing `agent_memory_snapshots` reused

### Не трогать по смыслу

- обычный web session/chat mode
- обычный Telegram topic/chat mode
- wiki memory runtime текущего chat mode

Это принципиально: life mode и ordinary chat mode должны быть разделены архитектурно, а не флагом в одном и том же path.

---

## 20. Acceptance criteria

1. Обычный chat mode не делает Engram calls.
2. Life mode использует stable scope `(principal_user_id, 'life', 'main')`.
3. Один и тот же linked user видит одну и ту же жизнь в web и Telegram.
4. После рестарта life mode продолжает работу из Postgres snapshot + transcript.
5. Concurrent inputs из web/Telegram сериализуются per principal.
6. Temporary overrides истекают и не загрязняют durable profile.
7. Explicit permanent memory доступна к recall из обоих транспортов.
8. Assistant free-form output не становится authoritative memory по умолчанию.
9. Secrets не уходят в Engram.
10. Пользователь может inspect/edit/forget memory.
11. Engram может быть заменен на fork / Rust rewrite без изменения life-mode product contract.
12. Curator не пишет в source of truth напрямую; profile updates только из confirmed user turns.
13. Curator использует существующий LLM client infrastructure, без нового provider/client.
14. Обратный поток Engram → Postgres source of truth отсутствует.

---

## 21. Рекомендуемая организационная граница в коде

Лучший вариант для минимального мусора в текущем проекте:

```text
oxide-agent-core        // execution engine, generic prompt context seam
oxide-agent-runtime     // existing runtime primitives
oxide-agent-life        // principal model, DB queue, worker, Engram adapter
transport-web           // /life UI + API integration
transport-telegram      // life DM router + linking
```

Именно так life mode остается отдельным bounded context, а не тонет в текущей логике `web-session-*` и Telegram topic routing.

### 21.1. Компоненты и их runtime

```text
┌──────────────┬───────────────────────────────────────────────┐
│ Компонент    │ Что это, какой runtime                         │
├──────────────┼───────────────────────────────────────────────┤
│ LifeGateway  │ Rust код, Postgres queries                     │
│              │ резолвит principal, пишет turns + inputs       │
├──────────────┼───────────────────────────────────────────────┤
│ LifeWorker   │ Rust код, per-principal advisory lock          │
│              │ гидрирует AgentSession, запускает executor     │
├──────────────┼───────────────────────────────────────────────┤
│ AgentExecutor│ existing core, ephemeral per run               │
│              │ собирает prompt → LLM → ответ                  │
├──────────────┼───────────────────────────────────────────────┤
│ Curator      │ LLM single call (existing llm/client.rs)       │
│              │ env: LIFE_CURATOR_PROVIDER + LIFE_CURATOR_MODEL│
│              │ temperature = provider default                 │
│              │ fire-and-forget после run, без tools           │
├──────────────┼───────────────────────────────────────────────┤
│ Sensitivity  │ rules-based gate + curator flags               │
│ gate         │ clean / redacted / secret                      │
├──────────────┼───────────────────────────────────────────────┤
│ Outbox       │ Rust background task, НЕ LLM                   │
│ worker       │ retries + idempotency → Engram HTTP            │
├──────────────┼───────────────────────────────────────────────┤
│ Engram       │ external dumb engine                           │
│              │ episodes + facts + contradictions              │
│              │ rebuildable из life_turns                      │
├──────────────┼───────────────────────────────────────────────┤
│ EngramMemory │ Rust adapter, trait LifeLongTermMemoryBackend  │
│ Backend      │ recall_context / append_episode / assert_fact  │
└──────────────┴───────────────────────────────────────────────┘
```

---

## 22. Финальная рекомендация

Делать `permanent life mode` как отдельный продуктовый режим с тремя слоями памяти:

1. **Postgres authoritative defaults/profile state**
2. **Postgres hot context + transcript + queue + checkpoint**
3. **Engram as derived long-term memory engine**

Не интегрировать Engram в обычный chat mode.

Не считать Engram source of truth.

Не использовать current transport sessions как identity life mode.

Не хранить secrets в memory engine.

Сначала выстроить principal model + DB-backed runtime + generic prompt context seam, и только потом подключать Engram через правильный internal contract.

---

## Appendix A. Локальные code refs

- `crates/oxide-agent-core/src/agent/session.rs:77-86`
- `crates/oxide-agent-core/src/agent/session.rs:236-271`
- `crates/oxide-agent-core/src/agent/session.rs:294-389`
- `crates/oxide-agent-core/src/agent/runner/execution.rs:207-235`
- `crates/oxide-agent-core/src/agent/executor.rs:163-182`
- `crates/oxide-agent-core/src/agent/executor/config.rs:88-103`
- `crates/oxide-agent-core/src/agent/executor/execution.rs:464-497`
- `crates/oxide-agent-core/src/agent/executor/execution.rs:628-756`
- `crates/oxide-agent-core/src/agent/prompt/composer.rs:579-591`
- `crates/oxide-agent-core/src/agent/hooks/memory.rs:24-120`
- `crates/oxide-agent-core/src/agent/hooks/memory.rs:123-168`
- `crates/oxide-agent-core/src/agent/tool_runtime/modules.rs:208-223`
- `crates/oxide-agent-core/src/agent/tool_runtime/modules.rs:607-634`
- `crates/oxide-agent-core/src/storage/sqlx/helpers.rs:27-66`
- `crates/oxide-agent-core/src/storage/reminder.rs:136-149`
- `crates/oxide-agent-runtime/src/session_registry.rs:16-21`
- `crates/oxide-agent-runtime/src/session_registry.rs:29-37`
- `crates/oxide-agent-transport-web/src/auth.rs:318-356`
- `crates/oxide-agent-transport-web/src/server/session_routes.rs:408-445`
- `crates/oxide-agent-transport-web/src/session.rs:633-745`
- `crates/oxide-agent-transport-web/src/bin/oxide-agent-web-console.rs:259-272`
- `crates/oxide-agent-web-contracts/src/sessions.rs:188-199`
- `crates/oxide-agent-transport-telegram/src/bot/context.rs:23-33`
- `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/session.rs:22`
- `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/session.rs:275-309`
- `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/task_runner.rs:857-862`
- `crates/oxide-agent-transport-telegram/src/runner.rs:191-205`
- `migrations/0002_web_persistence.sql:3-39`
- `migrations/0003_core_storage.sql:12-150`
- `migrations/0004_reminders_audit.sql:3-27`
- `migrations/0005_wiki_memory.sql:3-32`

## Appendix B. Upstream Engram refs

- `https://github.com/ly-wang19/engram`
- `https://raw.githubusercontent.com/ly-wang19/engram/main/README.md`
- `https://raw.githubusercontent.com/ly-wang19/engram/main/API.md`
- `https://raw.githubusercontent.com/ly-wang19/engram/main/engram/server/app.py`
- `https://raw.githubusercontent.com/ly-wang19/engram/main/engram/service.py`
- `https://raw.githubusercontent.com/ly-wang19/engram/main/engram/memory.py`
- `https://raw.githubusercontent.com/ly-wang19/engram/main/engram/types.py`
- `https://raw.githubusercontent.com/ly-wang19/engram/main/COMMERCIAL-LICENSE.md`
- `https://arxiv.org/abs/2606.09900`

