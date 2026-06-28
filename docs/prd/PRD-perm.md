# PRD: Permanent Life Mode — Transport-Scalable Chat

Статус: `web permanent chat shipped (C1-C9); Telegram bridge redesign required before implementation`

Дата: `2026-06-28`

## 1. Что это

Permanent Life Mode — отдельный продуктовый режим: один непрерывный чат с агентом, без сброса контекста между сессиями. Агент использует те же инструменты, что и в обычных сессиях, но с стабильной памятью `(principal, "life", "main")` и транскриптом в Postgres.

Источник истины — Postgres. Никакого Engram, curator, memory generations, friction patterns, support protocols — этот слой удалён как мёртвый код (commit `f71ed6ac`, -7765 строк).

Целевая модель — **один Life transcript и одна Life memory на principal, доступные из нескольких транспортов**:

- Web UI (`/life`)
- Telegram DM / future Telegram chat variants
- future Linux/system app
- future Android app
- future local/system integrations

Главный инвариант: **добавление нового транспорта не должно требовать изменения executor-а, life runtime-а, memory scope или canonical transcript model**.

## 2. Что уже сделано (C1-C9)

### 2.1. Backend (oxide-agent-life + oxide-agent-transport-web)

| Компонент | Назначение |
|-----------|-----------|
| `0010_life_mode.sql` | Миграция: 6 таблиц (`life_principals`, `life_identity_links`, `life_turns`, `life_runs`, `life_inputs`, `life_events`) |
| `LifeGateway` | Narrow submit path: `(provider, provider_subject, content, attachments, metadata)` → principal → turn + queued input |
| `LifeRuntimeHandle` | Wake: claim input + start run (advisory lock), link turn to run |
| `LifeWorker` | Execute claimed run, link turns, append events, complete/fail run |
| `LifeAgentExecutor` | Real `LifeRunExecutor` over `AgentExecutor`: stable scope `(principal, "life", "main")`, hydrate from `agent_memory_snapshots`, execute with ordinary tools, persist assistant turn, force final checkpoint |
| AgentEvent → life_events bridge | mpsc channel → `agent_event_to_life_parts` → `life_events` rows with monotonic seq |
| Cursor paging | `list_turns_page` / `list_events_page` / `list_turns_ascending` / `list_events_ascending` |
| SSE stream | `GET /api/v1/life/stream` — Postgres-backed DB-poll (2s), no SessionRegistry dependency |
| REST endpoints | `POST /api/v1/life/inputs`, `GET /api/v1/life/turns`, `GET /api/v1/life/events`, `GET /api/v1/life/state`, `POST /api/v1/life/uploads`, `POST /api/v1/life/large-input` |
| Typed attachments | `ApiLifeSubmitRequest.attachments: Vec<TaskAttachment>` |
| Privacy hard wipe | `DELETE /api/v1/life/state` — cascades to `life_principals` + `agent_memory_snapshots` |

### 2.2. Frontend (oxide-agent-web-ui)

| Компонент | Назначение |
|-----------|-----------|
| `AppRoute::Life` | Route `/life`, auth-guarded |
| SessionSidebar "Permanent" entry | Infinity icon, fixed entry above ordinary sessions |
| `LifeConsole` | Main layout: transcript + composer + activity drawer + SSE streaming |
| `LifeTranscript` | Turn list: user (escaped text), assistant (markdown), "Load older" paging |
| `LifeComposer` | Textarea, attachment handling (drag/drop/paste/file-picker), submit to `/api/v1/life/inputs` |
| `LifeActivityDrawer` | Activity events for active run, reuses shared `ActivityItemCard`, "Load older" paging |
| `LifeSseClient` | SSE subscription: `snapshot`, `turn`, `life_event`, `run_status`, `keepalive`, cursor-based reconnect |
| `life_event_to_persisted` | Convert `ApiLifeEventResponse` → `PersistedTaskEvent` for shared activity rendering |

### 2.3. Мёртвый код удалён

Из оригинального PRD убрано (commit `f71ed6ac`):

- Engram integration (outbox, projector, recall adapter)
- Post-run memory curator (LLM classification, sensitivity gate)
- Memory generations (build/compare/activate/rollback/wipe)
- Canonical memory ledger (`life_memory_items`)
- Task resume packets (`life_task_states`)
- Friction patterns / support protocols
- Context overrides with TTL
- Link tokens (Telegram linking flow)
- AuDHD operating profile / operating contract
- Profile state / settings inspector
- Memory inspector / editor / conflict review

Миграция trimmed: 15 → 6 таблиц. Мёртвых миграций не деплоится.

## 3. Verified current state and mines

Этот раздел фиксирует не желаемое состояние, а проверенные факты по коду/миграции на `2026-06-28`.

### 3.1. Current Postgres model

```
life_principals
life_identity_links(provider CHECK IN ('web','telegram'), provider_subject, principal_user_id)
life_turns(source_transport CHECK IN ('web','telegram','internal'), role, content, attachments, metadata)
life_inputs(status IN ('queued','claimed','consumed','dead'))
life_runs(status IN ('queued','running','completed','failed','cancelled'))
life_events(run_id, seq, kind, payload)
agent_memory_snapshots(user_id, context_key='life', flow_id='main')
```

### 3.2. Mine A — closed transport enums/checks

Сейчас transport/provider закрыты в двух местах:

- SQL CHECK: `provider IN ('web', 'telegram')`
- SQL CHECK: `source_transport IN ('web', 'telegram', 'internal')`
- Rust enums: `LifeIdentityProvider::{Web, Telegram}` и `LifeSourceTransport::{Web, Telegram, Internal}`

Это не масштабируется. Новый transport (`linux`, `android`, `system`) потребует schema migration + Rust enum changes + mapping changes по всему стеку.

**Target:** transport namespace должен быть открытым строковым id/newtype, а не closed enum в ядре.

### 3.3. Mine B — identity смешана с delivery

Старый Telegram bridge plan использовал `LIFE_TELEGRAM_CHAT_ID` одновременно как:

1. identity subject
2. delivery target

Для Telegram private DM это случайно совпадает (`user.id == chat.id`). Для будущих транспортов и даже Telegram group/thread это неверно:

- Telegram group/thread: identity = `telegram_user_id`, delivery = `{chat_id, thread_id}`
- Android: identity = app account/device owner, delivery = push token/device channel
- Linux app: identity = web account/local principal, delivery = local daemon/socket/session
- system integration: identity может быть web user, delivery — отдельный endpoint

**Target:** `life_identity_links` и delivery endpoints должны быть разными таблицами/контрактами.

### 3.4. Mine C — submit path создаёт principal

`LifeGateway::submit_life_input` сейчас, если identity не найдена, вызывает allocator и создаёт новый principal.

Для single-transport web это допустимо. Для bridge — опасно:

- ошибочный Telegram env/link создаст отдельный principal
- Web UI его не увидит, потому что web читает principal из web user id
- memory/transcript разделятся
- баг выглядит как “агент иногда не помнит”

**Target:** обычный submit path только резолвит identity. Если identity не связана — `UnlinkedIdentity`. Создание principal/link — отдельный privileged bootstrap/linking operation.

### 3.5. Mine D — follow-up inputs могут быть silently consumed

`LifeWorker::execute_claimed_run` drain-ит queued follow-up inputs и помечает их `consumed`, но `LifeAgentExecutor` получает только `claimed_run.user_content`.

Это значит: follow-up message может быть связан с run, но не попасть в agent input.

**Target:** выбрать один контракт:

- preferred: one queued input = one run; если run active, новые inputs остаются queued до следующего run
- alternative: batch-run, но тогда executor получает ordered content всех drained turns

Для personal-use и future transports preferred вариант проще и надёжнее.

### 3.6. Mine E — active run crash recovery неполный

Postgres-backed reads survive restart, но active execution не полностью self-healing:

- process может умереть после `life_runs.status='running'`
- after restart run остаётся `running`
- new input получает `AttachedToActive`
- worker уже мёртв
- principal зависает

**Target:** run lease/reaper: `lease_owner`, `lease_expires_at`, `heartbeat_at`; startup/poller marks expired running runs as interrupted/failed and releases queued work.

### 3.7. Mine F — Telegram notifier inside executor is wrong boundary

Старый T3 предлагал `TelegramLifeNotifier` внутри `life_executor.rs` после `persist_assistant_turn`.

Это прибивает executor к Telegram и не масштабируется на Android/Linux.

**Target:** executor только пишет canonical assistant turn. Доставка — durable delivery outbox + per-transport delivery workers.

### 3.8. Mine G — Telegram MarkdownV2 on raw assistant output

Telegram `sendMessage` contract:

- `text`: 1–4096 characters after entity parsing
- `parse_mode=MarkdownV2` требует escaping спецсимволов: underscore, asterisk, square/round brackets, tilde, backtick, greater-than, hash, plus, minus, equals, pipe, braces, dot, bang
- malformed MarkdownV2 returns HTTP 400 and drops the message

**Target:** first Telegram delivery sends plain text chunks <= 4096 chars. Rich rendering can be added later via deterministic renderer, not raw assistant markdown.

## 4. Target architecture

### 4.1. Process model

Web process remains the only executor owner. Other transports are thin submit/delivery adapters.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              Postgres source of truth                       │
│                                                                             │
│ life_principals        life_transport_identities   life_delivery_endpoints   │
│ life_turns             life_inputs                 life_delivery_outbox      │
│ life_runs              life_events                 agent_memory_snapshots    │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │
              ┌────────────────────────┼────────────────────────┐
              │                        │                        │
              ▼                        ▼                        ▼
     ┌─────────────────┐      ┌──────────────────┐      ┌──────────────────┐
     │ Web HTTP/UI     │      │ Web Life Runtime │      │ Transport adapters│
     │ /life           │      │ executor+worker  │      │ Telegram/Linux/   │
     │ SSE/read/write  │      │ poller+reaper    │      │ Android/system    │
     └─────────────────┘      └──────────────────┘      └──────────────────┘
```

### 4.2. Stable life scope

```
AgentMemoryScope(
    user_id     = principal_user_id,
    context_key = "life",
    flow_id     = "main",
)
```

Гидрируется из `agent_memory_snapshots` в начале каждого run, коммитится синхронно в конце. Этот scope не зависит от transport-а.

### 4.3. Open transport identity contract

Canonical identity link:

```text
life_transport_identities
  transport_id       TEXT       -- "web", "telegram", "linux", "android", "system", ...
  provider_subject   TEXT       -- stable subject within that transport namespace
  principal_user_id  BIGINT
  verified_at        BIGINT
  created_at         BIGINT
  updated_at         BIGINT
  PRIMARY KEY (transport_id, provider_subject)
```

Examples:

```text
web       subject="42"                 → principal=42
telegram  subject="telegram_user:4242" → principal=42
android   subject="device_owner:abc"   → principal=42
linux     subject="local_user:stfu"    → principal=42
```

Transport submit contract:

```text
submit_life_input(
  transport_id,
  provider_subject,
  source_ref,
  content,
  attachments,
  transport_metadata,
  sensitivity
)
```

Sender knows only its local subject and message metadata. Receiver (`LifeGateway`) resolves principal. Sender never supplies `principal_user_id`.

### 4.4. Separate delivery endpoints

Delivery endpoint is not identity.

```text
life_delivery_endpoints
  endpoint_id        UUID PRIMARY KEY
  principal_user_id  BIGINT
  transport_id       TEXT
  endpoint_address   JSONB      -- transport-owned address, e.g. chat_id/thread_id/push token/socket id
  enabled            BOOLEAN
  created_at         BIGINT
  updated_at         BIGINT
  UNIQUE(principal_user_id, transport_id, endpoint_address)
```

Examples:

```json
{"transport_id":"telegram","endpoint_address":{"chat_id":424242,"thread_id":null}}
{"transport_id":"android","endpoint_address":{"device_id":"pixel-8","push_token_ref":"storage:fcm/pixel-8"}}
{"transport_id":"linux","endpoint_address":{"channel":"local-daemon","instance_id":"workstation"}}
```

### 4.5. Durable delivery outbox

Assistant turn persistence enqueues delivery work; executor does not call external APIs.

```text
life_delivery_outbox
  delivery_id        UUID PRIMARY KEY
  turn_id            UUID NOT NULL
  principal_user_id  BIGINT NOT NULL
  endpoint_id        UUID NOT NULL
  transport_id       TEXT NOT NULL
  status             TEXT CHECK IN ('queued','claimed','delivered','failed','dead')
  attempt_count      INT
  claimed_by         TEXT
  claimed_at         BIGINT
  next_attempt_at    BIGINT
  last_error         TEXT
  created_at         BIGINT
  updated_at         BIGINT
```

Per-transport delivery worker:

```text
claim queued outbox rows for transport_id
→ load endpoint_address + turn content
→ deliver using transport-specific API
→ mark delivered/failed/dead
```

This supports Telegram today and Android/Linux later without changing executor.

### 4.6. Runtime queue contract

Preferred contract:

```text
one queued life_input = one agent run
```

If a principal has active run:

- new input remains `queued`
- poller wakes it after active run finishes
- no input is marked `consumed` unless its content is actually executed

This removes silent loss of follow-up messages and gives deterministic ordering across transports.

### 4.7. Run lease/reaper contract

Running run must have a lease:

```text
life_runs
  lease_owner TEXT
  lease_expires_at BIGINT
  heartbeat_at BIGINT
```

Rules:

- worker renews lease while executing
- startup/poller marks expired `running` runs as `failed`/`interrupted`
- queued inputs remain processable
- claimed-but-not-executed inputs must have explicit recovery policy

No process-local state may be required to recover Life Mode after restart.

## 5. Replacement implementation plan

The old T1-T5 Telegram-special-case plan is superseded. Do not implement `TelegramLifeNotifier` inside `LifeAgentExecutor`.

### S1: Open transport namespace

- Replace `LifeIdentityProvider` enum with `TransportId` newtype.
- Replace `LifeSourceTransport` enum with open `TransportId`/`TurnSource` model.
- Remove SQL CHECKs that enumerate concrete transports.
- Keep `internal` as reserved source id for system/assistant turns.

### S2: Split identity and delivery

- Rename/model identity links as transport identities.
- Add `life_delivery_endpoints`.
- Add env/bootstrap path that creates both:
  - `web` identity for existing web user
  - `telegram` identity for Telegram user id
  - `telegram` delivery endpoint for chat/thread target

### S3: Narrow submit contract

- Submit requires existing identity.
- Unknown identity returns `UnlinkedIdentity`.
- Principal creation/linking moves to explicit privileged bootstrap/linking operations.

### S4: Fix queue semantics

- Remove “drain queued inputs and mark consumed without executing content”.
- Implement one-input-one-run ordering, or pass all drained content into executor as ordered batch. Preferred: one-input-one-run.
- Add tests proving multiple messages across transports are executed exactly once and in order.

### S5: Add run lease and reaper

- Add run lease fields.
- Worker heartbeats.
- Startup/poller reaps expired running runs.
- Tests: crash after run claimed does not permanently block principal.

### S6: Add delivery outbox

- Assistant turn persistence enqueues outbox rows for enabled endpoints.
- No external HTTP calls from `LifeAgentExecutor`.
- Add delivery claim/retry/dead status transitions.

### S7: Telegram adapter over generic delivery

- Telegram `/life <text>` remains thin submit client.
- Telegram delivery worker claims `transport_id='telegram'` outbox rows.
- `sendMessage` plain text, split into <=4096 char chunks.
- No `parse_mode=MarkdownV2` until deterministic renderer exists.
- Ack: `💭 Обрабатываю...`.

### S8: Future transports

Linux/system app:

- submit via local daemon or HTTP API using `transport_id='linux'`
- read transcript via REST/SSE or local delivery endpoint
- optional local notification delivery worker

Android app:

- submit via authenticated API using `transport_id='android'`
- read transcript via REST/SSE/poll
- optional push endpoint in `life_delivery_endpoints`

No executor/runtime/memory changes should be needed for either.

## 6. Telegram bridge after redesign

### 6.1. Env bootstrap

```env
LIFE_WEB_USER_LOGIN=alice
LIFE_TELEGRAM_USER_ID=424242
LIFE_TELEGRAM_CHAT_ID=424242
LIFE_TELEGRAM_THREAD_ID=
LIFE_TELEGRAM_BOT_TOKEN=123456:ABC...

TELEGRAM_TOKEN=123456:ABC...
```

`LIFE_TELEGRAM_USER_ID` is identity. `LIFE_TELEGRAM_CHAT_ID` is delivery. They may be equal for private DM, but the contract must not depend on equality.

### 6.2. Telegram → Life → all endpoints

```
Telegram DM: "Привет"
  │
  ▼ submit_life_input(transport_id="telegram", subject="telegram_user:424242")
  │  → resolve principal via life_transport_identities
  │  → INSERT life_turns(role=user, source_transport=telegram, source_ref=message_id)
  │  → INSERT life_inputs(status=queued)
  │
  ▼ ack "💭 Обрабатываю..."
  │
  ▼ Web poller claims queued input
  │  → one input = one run
  │
  ▼ LifeAgentExecutor
  │  → hydrate memory
  │  → execute ordinary AgentExecutor
  │  → persist assistant turn(source=internal)
  │  → enqueue delivery outbox rows
  │
  ▼ Delivery workers
     ├─ Web sees turn via SSE/Postgres polling
     └─ Telegram worker sends plain text chunks to chat_id
```

### 6.3. Web → Life → all endpoints

```
Web /life: "Что нового?"
  │
  ▼ submit_life_input(transport_id="web", subject="42")
  │
  ▼ Web runtime claims input immediately
  │
  ▼ LifeAgentExecutor persists assistant turn + outbox
  │
  ├─ Web UI receives SSE turn
  └─ Telegram worker sends to configured Telegram endpoint
```

## 7. Validation

### Design validation

- New transport can be added without SQL enum/CHECK migration.
- New transport can be added without changing `LifeAgentExecutor`.
- Identity subject and delivery endpoint are separate values.
- Submitter never supplies principal id.
- Unknown identity cannot silently create a new principal.
- Queued input cannot be marked consumed unless executed or explicitly dead-lettered.
- Expired running run cannot block principal forever.

### Code gates

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
- If touching web UI: `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`
- If touching web UI: `trunk build --release` from `crates/oxide-agent-web-ui`

## 8. Границы

### В scope

- Web permanent chat: `/life` UI, transcript, composer, activity, paging, SSE
- Transport-scalable identity model
- Transport-scalable delivery outbox
- Telegram bridge as first non-web adapter
- Future Linux/Android/system transports without executor changes
- Postgres-backed runtime: turns, inputs, runs, events, memory snapshots

### Out of scope (намеренно удалено)

- Engram / derived memory index / recall engine
- Post-run memory curator (LLM classification)
- Memory generations (build/compare/activate/rollback)
- Canonical memory ledger (`life_memory_items`)
- Task resume packets (`life_task_states`)
- Friction patterns / support protocols
- Context overrides with TTL
- AuDHD operating profile / operating contract
- Profile state / settings inspector
- Memory inspector / editor / conflict review
- User-facing token linking flow (`/link`) — bootstrap/env link is enough for personal use until multi-user linking is explicitly designed
- Cross-transport reminders — reminder delivery needs separate transport-neutral notifier design

Эти компоненты могут вернуться в будущих goals, но не как часть core transport-scalable Life bridge.

## 9. История коммитов

| Checkpoint | Commit | Описание |
|-----------|--------|----------|
| C1 | `bb9d28ce` | Activate migration + cursor-paged storage |
| C2 | `b74ee7a5` + `e3633f8c` | Runtime wake + run-bound turn linkage |
| C3 | `4f2ed6f0` | Real LifeRunExecutor over AgentExecutor |
| C4 | `57b1666c` | AgentEvent → life_events bridge |
| C5 | `a637d471` | Postgres-backed SSE stream |
| C6 | `c4de3cff` | Typed attachments + life upload/large-input |
| C7 | `26257698` | /life route + sidebar entry |
| C8 | `06da7d9a` | Full life chat UI |
| C9 | `5307e91e` | Restart survival audit for read paths |
| Cleanup | `f71ed6ac` | Dead code removal (-7765 lines, 15→6 tables) |
| PRD rewrite | `75ae9bb9` | Replace old Engram PRD with current Life chat + initial Telegram plan |

Goal doc: `docs/goals/2026-06-28-web-permanent-chat.md` (status: complete for C1-C9)
