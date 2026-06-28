# PRD: Permanent Life Mode — Solo Bridge Chat

Статус: `web permanent chat shipped (C1-C9); bridge-first redesign in planning`

Дата: `2026-06-28`

## 1. Что это

Permanent Life Mode на ближайший этап — **обычный чат с агентом, синхронизированный между интерфейсами**.

Первый фокус не memory UX, не curator/Engram/инспектор памяти. Первый фокус — сделать универсальный мост:

```text
Web UI  ↔
Telegram dedicated Life bot  ↔   один общий чат агента
future Linux app             ↔   один общий transcript
future Android app           ↔   один executor
```

То есть пользователь может написать в Telegram, увидеть это в Web, продолжить в Web, получить ответ в Telegram и не думать о том, через какой интерфейс был начат диалог.

Проект solo/personal-use. Архитектурно проектируем под одного владельца, а не под SaaS/multi-user linking:

- один владелец/principal;
- транспорты подключаются через явные env/config bindings;
- Postgres — источник истины для transcript/queue/events;
- web process остаётся владельцем executor-а;
- transport processes/adapters — тонкие клиенты ввода/доставки.

Memory-обвязка может быть добавлена отдельным будущим goal. Текущий bridge milestone не должен зависеть от memory tooling и не должен продаваться как memory UX.

## 2. Что уже сделано (C1-C9)

### 2.1. Backend (oxide-agent-life + oxide-agent-transport-web)

| Компонент | Назначение |
|-----------|-----------|
| `0010_life_mode.sql` | Миграция: 6 таблиц (`life_principals`, `life_identity_links`, `life_turns`, `life_runs`, `life_inputs`, `life_events`) |
| `LifeGateway` | Submit path: `(provider, provider_subject, content, attachments, metadata)` → principal → turn + queued input |
| `LifeRuntimeHandle` | Wake: claim input + start run (advisory lock), link turn to run |
| `LifeWorker` | Execute claimed run, link turns, append events, complete/fail run |
| `LifeAgentExecutor` | Real `LifeRunExecutor` over ordinary `AgentExecutor`: execute user input, persist assistant turn, persist checkpoint |
| AgentEvent → life_events bridge | mpsc channel → `agent_event_to_life_parts` → `life_events` rows with monotonic seq |
| Cursor paging | `list_turns_page` / `list_events_page` / `list_turns_ascending` / `list_events_ascending` |
| SSE stream | `GET /api/v1/life/stream` — Postgres-backed DB-poll (2s), no SessionRegistry dependency |
| REST endpoints | `POST /api/v1/life/inputs`, `GET /api/v1/life/turns`, `GET /api/v1/life/events`, `GET /api/v1/life/state`, `POST /api/v1/life/uploads`, `POST /api/v1/life/large-input` |
| Typed attachments | `ApiLifeSubmitRequest.attachments: Vec<TaskAttachment>` |
| Privacy hard wipe | `DELETE /api/v1/life/state` — cascades to `life_principals` + current life checkpoint rows |

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

Миграция trimmed: 15 → 6 таблиц. Мёртвые memory features не возвращаются в bridge milestone.

## 3. UX target

### 3.1. Web Permanent Chat

Web `/life` — полноценная рабочая консоль.

Пользователь видит общий transcript из всех интерфейсов:

```text
[Telegram]  Скинь текущий статус по задаче
[Assistant] Сейчас задача в стадии...

[Web]       Ок, продолжи и проверь PRD
[Assistant] Проверил PRD, нашёл...
```

Web должен давать:

- полный transcript;
- composer;
- attachments;
- source labels (`web`, `telegram`, later `linux`, `android`);
- activity/events для текущего run;
- paging старой истории;
- SSE/poll sync новых turns/events из Postgres.

### 3.2. Dedicated Telegram Life bot

Telegram — отдельный bot только для Life Perm Mode. Поэтому **`/life` не нужен**.

Правило:

```text
Every non-command private DM message to the dedicated Life bot = Life chat input.
```

Пример:

```text
User:   Проверь потом PRD и скажи, что осталось
Bot:    💭 Обрабатываю...
Bot:    Проверил. Осталось...
```

Минимальные команды adapter-а:

- `/start` — коротко объяснить, что это dedicated Life bot;
- `/help` — как пользоваться;
- `/status` — active run / queued inputs / last delivery state.

Обычные текстовые сообщения не требуют команд и не роутятся через общий Telegram bot mode.

## 4. Solo-user binding model

### 4.1. Главный принцип

На ближайший этап нет multi-user linking, token exchange, account discovery, “подтверждения устройств”.

Есть один владелец, и транспорты подключаются явно через env/config:

```env
LIFE_OWNER_WEB_LOGIN=alice

LIFE_TELEGRAM_BOT_TOKEN=123456:ABC...
LIFE_TELEGRAM_CHAT_ID=424242
```

Для будущих транспортов аналогично:

```env
LIFE_LINUX_INSTANCE_ID=workstation
LIFE_ANDROID_DEVICE_ID=pixel-8
```

### 4.2. Web binding

Web owner задаётся login-ом:

```text
LIFE_OWNER_WEB_LOGIN=alice
  → normalize_login("alice")
  → web_store.load_login_index("alice")
  → user_id
  → principal_user_id = user_id
```

Web `/life` продолжает работать как сейчас: authenticated web user читает/пишет свой principal.

### 4.3. Telegram binding

Для dedicated Telegram Life bot в solo mode достаточно:

```text
LIFE_TELEGRAM_BOT_TOKEN = token dedicated bot-а
LIFE_TELEGRAM_CHAT_ID   = private DM chat id владельца
```

Telegram adapter принимает input только из configured `LIFE_TELEGRAM_CHAT_ID`. Всё остальное — ignored/denied.

Для Telegram private DM этот же `chat_id` является и inbound matcher, и delivery address. Это допустимо как Telegram-adapter contract, но core bridge не должен зашивать это как универсальное правило для всех транспортов.

### 4.4. Target binding table

Чтобы не плодить отдельные identity/delivery абстракции для solo проекта, целевая durable модель — одна таблица bindings:

```text
life_transport_bindings
  binding_id         UUID PRIMARY KEY
  principal_user_id  BIGINT NOT NULL
  transport_id       TEXT NOT NULL       -- "web", "telegram", "linux", "android", ...
  inbound_address    JSONB NOT NULL      -- how adapter recognizes owner input
  delivery_address   JSONB NOT NULL      -- where adapter sends assistant output
  enabled            BOOLEAN NOT NULL
  created_at         BIGINT NOT NULL
  updated_at         BIGINT NOT NULL
```

Examples:

```json
{"transport_id":"web","inbound_address":{"login":"alice"},"delivery_address":{"mode":"sse"}}
{"transport_id":"telegram","inbound_address":{"chat_id":424242},"delivery_address":{"chat_id":424242}}
{"transport_id":"linux","inbound_address":{"instance_id":"workstation"},"delivery_address":{"instance_id":"workstation"}}
{"transport_id":"android","inbound_address":{"device_id":"pixel-8"},"delivery_address":{"device_id":"pixel-8"}}
```

This keeps solo-user setup simple while leaving an open transport namespace.

## 5. Verified current mines

### 5.1. Closed transport enums/checks

Current implementation is closed over `web` and `telegram`:

- SQL CHECK: `provider IN ('web', 'telegram')`
- SQL CHECK: `source_transport IN ('web', 'telegram', 'internal')`
- Rust enums: `LifeIdentityProvider::{Web, Telegram}` and `LifeSourceTransport::{Web, Telegram, Internal}`

This blocks future `linux`/`android` transports without migrations and enum edits.

**Fix:** replace closed provider/source enums in the life core with open `TransportId`/reserved `internal` source.

### 5.2. Current submit path can create accidental principals

`LifeGateway::submit_life_input` currently allocates a new principal if identity is missing.

For bridge mode this is unsafe: wrong Telegram config could create a separate hidden transcript.

**Fix:** in bridge mode, submit must resolve an existing binding/principal. Unknown inbound address = denied/unlinked, not new principal.

### 5.3. Follow-up inputs may be consumed without execution

Current worker can drain queued follow-up inputs and mark/link them, while executor receives only the originally claimed input content.

**Fix:** preferred bridge contract is:

```text
one queued input = one agent run
```

If a run is active, later inputs stay queued until the active run completes. No input is consumed unless it is actually executed or explicitly dead-lettered.

### 5.4. Active run crash recovery is incomplete

If process dies while a run is `running`, a future input can attach to a run whose worker no longer exists.

**Fix:** run lease/reaper:

```text
life_runs.lease_owner
life_runs.lease_expires_at
life_runs.heartbeat_at
```

Expired running runs are marked interrupted/failed and queued work continues.

### 5.5. Delivery must not live inside executor

Executor must only write canonical assistant turn. It must not call Telegram/Android/Linux APIs.

**Fix:** durable delivery outbox.

### 5.6. Telegram MarkdownV2 is not the first milestone

Telegram `sendMessage` has a 4096-char limit and MarkdownV2 escaping rules. Raw assistant markdown can 400.

**Fix:** first Telegram delivery sends plain text chunks <= 4096 chars. Rich Telegram rendering is a later deterministic renderer, not bridge core.

## 6. Target bridge architecture

### 6.1. Process model

```text
┌─────────────────────────────────────────────────────────────────────────────┐
│                              Postgres source of truth                       │
│                                                                             │
│ life_principals        life_transport_bindings      life_delivery_outbox     │
│ life_turns             life_inputs                  life_runs                │
│ life_events            existing checkpoints/snapshots                       │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │
              ┌────────────────────────┼────────────────────────┐
              │                        │                        │
              ▼                        ▼                        ▼
     ┌─────────────────┐      ┌──────────────────┐      ┌──────────────────┐
     │ Web /life UI    │      │ Web Life Runtime │      │ Transport adapters│
     │ read/write/SSE  │      │ executor+worker  │      │ Telegram/Linux/   │
     │                 │      │ poller+reaper    │      │ Android/system    │
     └─────────────────┘      └──────────────────┘      └──────────────────┘
```

### 6.2. Submit flow

Transport adapter submits only what it actually knows:

```text
submit_life_input(
  transport_id,
  inbound_address,
  source_ref,
  content,
  attachments,
  metadata
)
```

Core resolves `transport_id + inbound_address` through configured bindings to the solo principal.

No adapter sends arbitrary `principal_user_id`.

### 6.3. Delivery outbox

Assistant turn persistence enqueues delivery work for enabled bindings:

```text
life_delivery_outbox
  delivery_id        UUID PRIMARY KEY
  turn_id            UUID NOT NULL
  binding_id         UUID NOT NULL
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

Transport delivery worker:

```text
claim outbox rows for transport_id
→ load binding.delivery_address + assistant turn content
→ send via transport API
→ mark delivered/failed/dead
```

This is the core bridge: once assistant output exists, all enabled interfaces can receive/sync it.

## 7. Data flows

### 7.1. Telegram → Web + Telegram

```text
Telegram dedicated Life bot receives DM text from chat_id=424242
  │
  ▼ adapter checks chat_id == LIFE_TELEGRAM_CHAT_ID
  │
  ▼ submit_life_input(transport_id="telegram", inbound_address={chat_id:424242})
  │  → resolve binding → principal
  │  → INSERT life_turns(role=user, source_transport=telegram, source_ref=message_id)
  │  → INSERT life_inputs(status=queued)
  │
  ▼ Telegram ack: "💭 Обрабатываю..."
  │
  ▼ Web runtime/poller claims queued input
  │  → one input = one run
  │
  ▼ AgentExecutor runs ordinary chat turn
  │
  ▼ persist assistant turn(source=internal)
  │  → enqueue outbox for enabled bindings
  │
  ├─ Web UI sees user+assistant turns through SSE/Postgres polling
  └─ Telegram delivery worker sends assistant response to chat_id=424242
```

### 7.2. Web → Web + Telegram

```text
Web /life submit by authenticated LIFE_OWNER_WEB_LOGIN user
  │
  ▼ INSERT life_turns(role=user, source_transport=web)
  ▼ INSERT life_inputs(status=queued)
  ▼ runtime claims input immediately
  ▼ AgentExecutor runs ordinary chat turn
  ▼ assistant turn persisted + outbox enqueued
  │
  ├─ Web UI receives SSE turn
  └─ Telegram delivery worker sends assistant response to configured chat_id
```

### 7.3. Future Linux/Android

Future transports follow the same pattern:

```text
new transport config/binding
→ adapter maps local input to submit_life_input
→ shared queue/executor/transcript
→ delivery worker or client polling reads outputs
```

No changes to executor semantics should be required.

## 8. Implementation plan

### B1: Bridge-first terminology and config

- Rename docs/config from memory-first wording to bridge/chat wording.
- Introduce `LIFE_OWNER_WEB_LOGIN`.
- Dedicated Telegram bot uses `LIFE_TELEGRAM_BOT_TOKEN` + `LIFE_TELEGRAM_CHAT_ID`.
- Remove command-prefixed input requirement for dedicated Telegram bot.

### B2: Open transport id

- Replace closed provider/source enums with open `TransportId` newtype/reserved `internal` source.
- Remove SQL CHECKs that enumerate concrete transports.

### B3: Solo transport bindings

- Add/load `life_transport_bindings` from env/config at web startup.
- Web login binding resolves owner principal.
- Telegram chat binding maps configured chat id to same principal.
- Unknown Telegram chat id is denied/ignored, not auto-linked.

### B4: Narrow submit path

- Submit resolves existing binding.
- No accidental principal allocation from random transport input.
- Web submit remains direct for authenticated owner but still maps into same principal/binding model.

### B5: Queue correctness

- Implement one-input-one-run or ordered batch execution.
- Preferred: one-input-one-run.
- Add tests: two fast Telegram messages are both executed exactly once and in order.

### B6: Run lease/reaper

- Add lease fields and heartbeat.
- Startup/poller reaps expired running runs.
- Tests: crashed active run does not block later inputs.

### B7: Delivery outbox

- Assistant turn persistence enqueues outbox rows for enabled transport bindings.
- Executor does not call external transport APIs.
- Delivery workers claim/send/retry/dead-letter.

### B8: Dedicated Telegram adapter

- Every non-command private DM from configured chat id is Life input.
- Commands: `/start`, `/help`, `/status`.
- Ack: `💭 Обрабатываю...`.
- Delivery: plain text chunks <= 4096 chars.
- No MarkdownV2 in first milestone.

## 9. Validation

### Design validation

- User can write in Telegram and see the turn in Web transcript.
- User can write in Web and receive assistant response in Telegram.
- Telegram dedicated bot does not require `/life`.
- Unknown Telegram chat cannot create a hidden new principal.
- New future transport can be added by config/binding + adapter, not executor rewrite.
- Queued input cannot be consumed unless executed or dead-lettered.
- Expired running run cannot block the solo principal forever.

### Code gates

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
- If touching web UI: `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`
- If touching web UI: `trunk build --release` from `crates/oxide-agent-web-ui`

## 10. Scope

### В scope

- Web permanent chat `/life` as full console.
- Dedicated Telegram Life bot with no `/life` command requirement.
- Solo-owner env/config bindings.
- Cross-interface transcript sync.
- Durable input queue and delivery outbox.
- Future Linux/Android/system transports via same bridge pattern.

### Out of scope for bridge milestone

- Engram / derived memory index / recall engine.
- Post-run memory curator.
- Memory generations.
- Memory inspector/editor/conflict review.
- User-facing token linking flow.
- Multi-user SaaS account/device linking.
- Telegram groups/multi-chat routing.
- Rich Telegram Markdown rendering.
- Cross-transport reminders.

These can be separate goals after the bridge is correct.

## 11. История коммитов

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
| Scalable PRD | `c29d15df` | Redesign around scalable transports/outbox |

Goal doc: `docs/goals/2026-06-28-web-permanent-chat.md` (status: complete for C1-C9)
