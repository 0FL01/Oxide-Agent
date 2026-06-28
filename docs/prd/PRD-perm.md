# PRD: Permanent Life Mode — Web Chat + Telegram Bridge

Статус: `web permanent chat shipped (C1-C9); Telegram bridge in planning`

Дата: `2026-06-28`

## 1. Что это

Permanent Life Mode — отдельный продуктовый режим: один непрерывный чат с агентом, без сброса контекста между сессиями. Агент использует те же инструменты, что и в обычных сессиях, но с стабильной памятью `(principal, "life", "main")` и транскриптом в Postgres.

Источником истины является Postgres. Никакого Engram, curator, memory generations, friction patterns, support protocols — этот слой удалён как мёртвый код (commit `f71ed6ac`, -7765 строк).

Два интерфейса доступа:
- **Web UI** (`/life`) — полный чат-интерфейс с transcript, composer, activity, paging, SSE
- **Telegram DM** (`/life <text>`) — thin client: сабмитит в Postgres, получает ack, ответ доставляется notifier-ом

## 2. Что уже сделано (C1-C9)

### 2.1. Backend (oxide-agent-life + oxide-agent-transport-web)

| Компонент | Назначение |
|-----------|-----------|
| `0010_life_mode.sql` | Миграция: 6 таблиц (`life_principals`, `life_identity_links`, `life_turns`, `life_runs`, `life_inputs`, `life_events`) |
| `LifeGateway` | Transport-neutral вход: `(provider, provider_subject, content, attachments, metadata)` → principal → turn + queued input |
| `LifeRuntimeHandle` | Wake: claim input + start run (advisory lock), link turn to run |
| `LifeWorker` | Execute claimed run: drain follow-up inputs, link turns, append events, complete run |
| `LifeAgentExecutor` | Real `LifeRunExecutor` over `AgentExecutor`: stable scope `(principal, "life", "main")`, hydrate from `agent_memory_snapshots`, execute with all ordinary tools, persist assistant turn, force final checkpoint |
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

## 3. Текущая архитектура

```
┌──────────────────────────────────────────────────────────────────────┐
│                        Postgres (source of truth)                     │
│                                                                       │
│  life_principals          life_identity_links                          │
│  life_turns               life_runs                                    │
│  life_inputs              life_events                                  │
│  agent_memory_snapshots   (existing, scope=(principal,"life","main")) │
└───────────────────────────────────┬───────────────────────────────────┘
                                    │
           ┌────────────────────────┼────────────────────────┐
           │                        │                        │
           ▼                        ▼                        ▼
  ┌─────────────────┐    ┌──────────────────┐    ┌──────────────────┐
  │  Web Process    │    │  Web Process     │    │  Telegram Process│
  │  (HTTP server)  │    │  (executor +     │    │  (teloxide bot)  │
  │                 │    │   poller + SSE)  │    │                  │
  │  POST /inputs   │    │                  │    │  /life <text>    │
  │  GET  /turns    │    │  LifeAgentExec   │    │  → gateway       │
  │  GET  /events   │    │  LifeWorker      │    │  → ack "💭"      │
  │  GET  /stream   │    │  LifeRuntime     │    │  (no executor)   │
  │  POST /uploads  │    │  Input Poller    │    │  (thin client)   │
  │                 │    │  TelegramNotifier│    │                  │
  └─────────────────┘    └──────────────────┘    └──────────────────┘
```

### 3.1. Stable life scope

```
AgentMemoryScope(
    user_id    = principal_user_id,
    context_key = "life",
    flow_id     = "main",
)
```

Гидрируется из `agent_memory_snapshots` в начале каждого run, коммитится синхронно в конце.

### 3.2. Identity model

```
life_identity_links
┌──────────┬──────────────────┬────────────────────┐
│ provider │ provider_subject │ principal_user_id  │
├──────────┼──────────────────┼────────────────────┤
│ web      │ "<web_user_id>"  │ <principal>        │  ← FixedPrincipalAllocator
│ telegram │ "<tg_user_id>"   │ <principal>        │  ← env link at startup (T1)
└──────────┴──────────────────┴────────────────────┘
```

Web transport: `provider=Web`, `provider_subject=web_user_id.to_string()`.
Telegram transport: `provider=Telegram`, `provider_subject=tg_user_id.to_string()`.

### 3.3. source_transport на assistant turn

```
life_turns.source_transport:
  user turn from web      → "web"
  user turn from telegram → "telegram"
  assistant turn          → "internal"  (T4: fix hardcoded "Web")
```

### 3.4. Restart survival

Каждый read path (transcript, memory, events, SSE) читает из Postgres напрямую. Нет process-local state как источника истины. После рестарта:
- Transcript: `SELECT FROM life_turns WHERE principal = $1`
- Hot memory: `SELECT FROM agent_memory_snapshots WHERE (user_id, context_key, flow_id) = ($1, 'life', 'main')`
- Events: `SELECT FROM life_events WHERE run_id = $1`
- SSE: DB-poll каждые 2с

## 4. Telegram Bridge — план

### 4.1. Контракт

**Передающая сторона:** web/telegram transport → `submit_life_input(provider, provider_subject, content, attachments, metadata)`
**Принимающая:** `LifeGateway` → `life_identity_links` → principal → `life_turns` + `life_inputs`

Transport не знает:
- principal_user_id (резолвится через identity links)
- состояние текущего run (claim/attach решает runtime)
- содержимое памяти (гидрируется executor-ом из Postgres)

### 4.2. Env vars

```env
# Web process
LIFE_WEB_USER_LOGIN=alice              # web login → resolve to user_id → principal
LIFE_TELEGRAM_CHAT_ID=424242           # Telegram DM chat_id (identity link + delivery target)
LIFE_TELEGRAM_BOT_TOKEN=123456:ABC...  # for notifier (POST api.telegram.org)

# Telegram process
TELEGRAM_TOKEN=123456:ABC...           # same bot, long-polling
```

`LIFE_TELEGRAM_CHAT_ID` работает для обоих назначений:
1. **Identity:** `link_identity(Telegram, "424242", principal)` — для DM `user.id == chat.id`
2. **Delivery:** `sendMessage(chat_id=424242, text)` — тот же id

`LIFE_WEB_USER_LOGIN` — человекочитаемый, стабильный. Резолвится через `web_store.load_login_index(login) → user_id → principal`. Не зависит от database-internal id.

### 4.3. Фазы реализации

#### T1: Identity unification (env-configured)

At web startup:
1. `normalize_login(LIFE_WEB_USER_LOGIN)` → normalized login
2. `web_store.load_login_index(normalized_login)` → `user_id`
3. `principal = PrincipalUserId::new(user_id)`
4. `life_storage.link_identity(Telegram, LIFE_TELEGRAM_CHAT_ID, principal)` — upsert в `life_identity_links` (INSERT ... ON CONFLICT DO UPDATE)

Оба транспорта разделяют один principal → одну транскрипцию → одну память.

**Контракт `link_identity`:** должен быть upsert (not insert-only), чтобы при рестарте web-процесса с изменённым env не создавать duplicate link.

#### T2: Input poller в web-процессе

Background task в web-процессе, поллит `life_inputs` каждые 2с:
1. `list_queued_inputs() -> Vec<(PrincipalUserId, InputId)>` — новый метод в `LifeStorageRepository` + `SqlxLifeStorage`
2. Для каждого queued input → `runtime.wake(principal, input_id)`
3. `WakeOutcome::Started` → `tokio::spawn(worker.execute_claimed_run(*claimed))`
4. `WakeOutcome::AttachedToActive` → input будет drained активным run

Web-сабмиты уже обработаны мгновенно через `submit_life_input_for_user → wake()`. Poller подхватывает только orphaned queued inputs (Telegram-сабмиты, crash recovery).

**Poll interval:** 2с (приемлемо для personal use, 5 RPS).

#### T3: Telegram notifier в web-процессе

`TelegramLifeNotifier` в `life_executor.rs`:
- После `persist_assistant_turn` (assistant content + principal available)
- Если `LIFE_TELEGRAM_BOT_TOKEN` + `LIFE_TELEGRAM_CHAT_ID` настроены
- `POST https://api.telegram.org/bot{token}/sendMessage` с `chat_id` и `text` (assistant response)
- Использует `reqwest` (уже зависимость) — **не** teloxide, не нарушает архитектурный инвариант
- `parse_mode=MarkdownV2` для рендеринга
- Fire-and-forget: ошибка отправки логируется, не блокирует run completion

Покрывает оба направления: notifier срабатывает на каждый assistant turn независимо от источника input.

**Контракт:** notifier не знает, откуда пришёл input. Он отправляет каждый assistant turn. Если сообщение пришло из Telegram, пользователь увидит ответ в Telegram. Если из Web — увидит в обоих местах (Web через SSE, Telegram через notifier).

#### T4: Assistant turn source_transport fix

`persist_assistant_turn` в `life_executor.rs`:
- `LifeSourceTransport::Web` → `LifeSourceTransport::Internal`
- Корректно: ответ агента не привязан к транспорту

#### T5: Telegram handler refinement

`handle_life_command` в `transport-telegram/runner.rs`:
- Ack: `"💭 Обрабатываю..."` вместо `"Life input queued. input_id=..."`
- Никаких структурных изменений — notifier доставляет ответ
- User turn уже попадает в web-транскрипцию через SSE (DB-poll подхватит `life_turns` с `source_transport='telegram'`)

### 4.4. Потоки данных

#### Сценарий 1: Telegram → Web + Telegram

```
Telegram DM: "Привет"
  │
  ▼ gateway.submit_life_input(provider=Telegram, subject=424242)
  │  → resolve principal via life_identity_links
  │  → INSERT life_turns (role=user, source_transport=telegram)
  │  → INSERT life_inputs (status=queued)
  │
  ▼ ack: "💭 Обрабатываю..."
  │
  ▼ [Web poller, 2с] list_queued_inputs → wake(principal, input_id)
  │  → claim_input_and_start_run
  │  → spawn worker.execute_claimed_run
  │
  ▼ LifeAgentExecutor.execute_life_run
  │  → hydrate AgentMemory from agent_memory_snapshots
  │  → AgentExecutor.execute_user_input_with_options
  │  → Completed("Привет! Я Oxide...")
  │
  ▼ persist_assistant_turn (source_transport=internal)
  │  → INSERT life_turns (role=assistant)
  │
  ▼ TelegramLifeNotifier
  │  → POST api.telegram.org/bot{token}/sendMessage
  │  → chat_id=424242, text="Привет! Я Oxide..."
  │
  ▼ [Web SSE, 2с] list_turns_ascending → emit turn event
  │  → Web UI видит: [Telegram] Привет / [Assistant] Привет! Я Oxide...
```

#### Сценарий 2: Web → Web + Telegram

```
Web UI /life: "Что нового?"
  │
  ▼ POST /api/v1/life/inputs
  │  → gateway.submit_life_input(provider=Web, subject=web_user_id)
  │  → INSERT life_turns (role=user, source_transport=web)
  │  → INSERT life_inputs (status=queued)
  │
  ▼ runtime.wake() → claim → spawn execute_claimed_run
  │
  ▼ LifeAgentExecutor.execute_life_run
  │  → Completed("Всё goed, работаю...")
  │
  ▼ persist_assistant_turn (source_transport=internal)
  │
  ▼ TelegramLifeNotifier
  │  → POST sendMessage → Telegram видит: "Всё goed, работаю..."
  │
  ▼ [Web SSE] → Web UI видит: [Web] Что нового? / [Assistant] Всё goed...
```

#### Единая транскрипция в Postgres

```
life_turns
┌─────────┬───────────┬──────────────────┬─────────────────────┐
│ turn_id │ role      │ source_transport │ content             │
├─────────┼───────────┼──────────────────┼─────────────────────┤
│ uuid-1  │ user      │ telegram         │ "Привет"            │
│ uuid-2  │ assistant │ internal         │ "Привет! Я Oxide.." │
│ uuid-3  │ user      │ web              │ "Что нового?"       │
│ uuid-4  │ assistant │ internal         │ "Всё goed, работаю" │
└─────────┴───────────┴──────────────────┴─────────────────────┘

life_identity_links
┌──────────┬──────────────────┬────────────────────┐
│ provider │ provider_subject │ principal_user_id  │
├──────────┼──────────────────┼────────────────────┤
│ web      │ "42"             │ 42                 │
│ telegram │ "424242"         │ 42                 │
└──────────┴──────────────────┴────────────────────┘
         │
         ▼
  Один principal = одна транскрипция, одна память
  AgentMemoryScope(42, "life", "main")
```

### 4.5. Что НЕ делаем

- Не извлекаем `LifeAgentExecutor` в shared crate — web-only executor проще, poller latency 2с приемлема
- Не добавляем Postgres LISTEN/NOTIFY — можно апгрейдить позже если latency будет проблемой
- Не меняем архитектуру Telegram-бота (остаётся thin client для life mode)
- Не добавляем teloxide зависимость в web transport — notifier использует raw HTTP (reqwest)
- Не добавляем Telegram в web-contracts — notifier использует raw HTTP, не контракты
- Не делаем linking flow через token exchange — env-configured identity link достаточно для personal use

### 4.6. Валидация

- `cargo fmt --all -- --check`
- `cargo clippy` (scoped: life, transport-web, transport-telegram, web-contracts)
- `cargo test -p oxide-agent-life` (real Postgres)
- `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local --lib`
- `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
- `trunk build --release`
- `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`

## 5. Границы

### В scope

- Web permanent chat: `/life` UI, transcript, composer, activity, paging, SSE
- Telegram bridge: bidirectional message duplication
- Postgres-backed runtime: turns, inputs, runs, events, memory snapshots
- Stable identity: web login + Telegram chat_id → one principal

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
- Link tokens (Telegram linking flow via `/link` command)
- Cross-transport reminders

Эти компоненты могут вернуться в будущем как отдельный memory-tool goal, но не как часть web chat + Telegram bridge.

## 6. История коммитов

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
| C9 | `5307e91e` | Restart survival + final audit |
| Cleanup | `f71ed6ac` | Dead code removal (-7765 lines, 15→6 tables) |

Goal doc: `docs/goals/2026-06-28-web-permanent-chat.md` (status: complete)
