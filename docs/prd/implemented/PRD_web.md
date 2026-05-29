# PRD: Rust-only Web Console for Oxide-Agent (V1)

## 1. Title

Rust-only Web Console for Oxide-Agent: authentication, sessions, task execution, live events, final answers, and safe Markdown rendering.

## 2. Summary

Цель этого scope — добавить в репозиторий полноценный веб-интерфейс, которым реально можно пользоваться для работы с агентом: логиниться, создавать и открывать сессии, отправлять задачи и вопросы, смотреть live progress и tool events, получать финальный ответ, отменять задачу и продолжать работу в рамках той же сессии.

Ключевое архитектурное требование: frontend должен быть написан полностью на Rust. Для первой версии не нужен сложный дизайн, SSR-магия, TypeScript SPA или новый transport с нуля. Нужен аккуратный, читаемый и безопасный продукт, опирающийся на существующие `oxide-agent-core`, `oxide-agent-runtime` и `oxide-agent-transport-web`, но доведённый до production-usable уровня для браузера.

## 3. Goals

- Сделать usable веб-консоль для Oxide-Agent без смены текущей core/runtime архитектуры.
- Сохранить Rust-only stack на frontend и не вводить TypeScript/React/Vue/Svelte стек.
- Переиспользовать существующий `oxide-agent-transport-web` как основу backend/API вместо разработки нового transport с нуля.
- Добавить безопасную login/password аутентификацию с cookie-сессиями.
- Добавить регистрацию пользователей, если она включена конфигом.
- Привязать agent sessions и tasks к authenticated user и обеспечить строгую изоляцию между пользователями.
- Добавить durable persistence для пользовательских данных веб-консоли через S3/R2, а не через in-memory `HashMap`.
- Сохранять историю сессий и задач, финальный ответ агента, состояние задачи, event log и прогресс так, чтобы UI переживал refresh страницы и перезапуск backend.
- Показывать live progress и события агента в реальном времени через SSE с корректным reconnect/replay.
- Поддержать полноценный Markdown rendering в сообщениях пользователя и агента с sanitization и XSS-защитой.
- Держать первую версию визуально простой, но аккуратной и удобной.

## 4. Non-goals

- Pixel-perfect дизайн, продвинутая дизайн-система, темы, тёмные/светлые палитры и сложные анимации.
- TypeScript/React/Vue/Svelte/Solid/Next.js/Nuxt/Vite+TS frontend.
- Полная замена `oxide-agent-core`, `oxide-agent-runtime` или модели `SessionRegistry` / `AgentExecutor`.
- Новый transport с нуля, если существующий `oxide-agent-transport-web` можно эволюционно доработать.
- Полноценная административная консоль, управление всеми внутренностями агента, observability dashboard.
- OAuth/SAML/SSO.
- Многотенантная org/multi-workspace модель.
- Визуальный workflow builder.
- Полноценный mobile app.
- Файловые upload workflows, если они не оказываются уже тривиальны в текущем backend.
- Сложная role/permission matrix. Для V1 достаточно заложить `user` и `admin` в data model без отдельного admin UI.
- Approve/reject UI. Более того, V1 работает в **YOLO (Full permission)** mode: web-транспорт не использует `WaitingForApproval` state — агент всегда запускается с полным набором разрешений и никогда не ждёт approve на web-канале. Это осознанное упрощение: core approval path не готов как браузерный сценарий, а для консольного использования YOLO mode предпочтительнее.
- SQL база, migrations и schema migration framework. В этом scope persistence должен опираться на versioned JSON документы в S3/R2.

## 5. Current Repository Findings

### 5.1 Workspace и общая структура

Проверено по `Cargo.toml` в корне workspace:

- `crates/oxide-agent-core`
- `crates/oxide-agent-runtime`
- `crates/oxide-agent-transport-telegram`
- `crates/oxide-agent-transport-web`
- `crates/oxide-agent-telegram-bot`
- `crates/oxide-agent-sandboxd`

Что важно:

- В репозитории сейчас нет отдельного frontend crate.
- В репозитории не найдено `package.json`, `tsconfig.json`, `.ts`, `.tsx`, `.js`, `.jsx` файлов. То есть TypeScript/JS frontend сейчас отсутствует вообще.
- `README.md` называет `oxide-agent-transport-web` “E2E testing infrastructure with HTTP API”, а не production web app.

### 5.2 Что уже есть в `crates/oxide-agent-transport-web`

Проверено по `crates/oxide-agent-transport-web/src/lib.rs`, `src/server.rs`, `src/session.rs`, `src/web_transport.rs`.

Текущее состояние:

- crate прямо документирован как transport для E2E тестов и benchmarks;
- HTTP server уже есть на Axum;
- уже есть базовые endpoint-ы для session/task lifecycle;
- уже есть SSE endpoint;
- уже есть event collection поверх `AgentEvent`;
- уже есть e2e тесты для web transport.

Существующие endpoint-ы сейчас:

- `POST /sessions`
- `GET /sessions/:id`
- `DELETE /sessions/:id`
- `POST /sessions/:session_id/tasks`
- `GET /sessions/:session_id/tasks/:task_id/progress`
- `GET /sessions/:session_id/tasks/:task_id/events`
- `GET /sessions/:session_id/tasks/:task_id/stream`
- `GET /sessions/:session_id/tasks/:task_id/timeline`
- `POST /sessions/:session_id/tasks/:task_id/cancel`
- `GET /health`
- `GET /debug/event_logs`

Это полезная основа, но не готовый user-facing backend.

### 5.3 Критические ограничения текущего web transport

#### 5.3.1 Это E2E/test transport, а не production web backend

`src/lib.rs` и `README.md` прямо описывают его как транспорт для E2E/benchmark сценариев. Значит, PRD должен опираться на него как на базу, но не считать его уже готовым продуктовым API.

#### 5.3.2 Нет аутентификации и user isolation уровня браузера

Сейчас:

- нет login/password auth;
- нет register/logout;
- нет cookie sessions;
- нет auth middleware;
- нет CSRF защиты;
- нет роли пользователя и статуса `disabled`;
- нет current-user endpoint.

Особенно опасно: `CreateSessionBody` в `src/server.rs` принимает `user_id` прямо из browser body. Для production web UI это запрещено. `user_id` должен определяться только из authenticated server-side session.

#### 5.3.3 Нет durable storage для web UI

`WebSessionManager::new()` в `src/session.rs` жёстко использует `InMemoryStorage::new()`.

В памяти процесса сейчас живут:

- sessions map;
- tasks map;
- running tasks map;
- `AppState.task_progress`;
- `AppState.task_timeline`;
- глобальный `EVENT_LOGS`;
- отдельные per-session in-memory memory checkpoints.

Следствия:

- refresh backend теряет session/task history;
- финальный ответ не переживает restart;
- event log не переживает restart;
- multi-user production эксплуатация невозможна;
- accidental in-memory mode в prod сейчас слишком вероятен.

#### 5.3.4 Финальный ответ агента сейчас не сохраняется

Это одна из главных “мин”.

Проверено по `src/server.rs`: `spawn_executor_task()` вызывает `executor.execute(&task_text, Some(tx)).await`, но затем просто делает:

- `complete_task(...)` если `Ok(_)`
- `fail_task(...)` если `Err(_)`

То есть `AgentExecutionOutcome::Completed(String)` не извлекается и не сохраняется. Финальный текст ответа теряется.

Следствия:

- UI не сможет гарантированно показать final answer после refresh;
- backend не сможет отдать final response отдельным endpoint-ом;
- невозможно построить корректную историю сессии только по persisted данным;
- `WaitingForUserInput` и `WaitingForApproval` схлопываются в “успех”/`Completed`, что некорректно.

Это must-fix требование V1.

#### 5.3.5 Статусная модель задачи слишком бедная

В `src/session.rs` сейчас есть только:

- `Running`
- `Completed`
- `Cancelled`
- `Failed`

Но core/runtime реально умеет больше:

- `Completed(String)`
- `WaitingForUserInput(PendingUserInput)`
- `WaitingForApproval`

Для usable web UI этого недостаточно. Нужен как минимум отдельный status для waiting states, а также статус `interrupted` для crash/restart scenario.

#### 5.3.6 Event log слишком бедный для UI

В `src/web_transport.rs` `TaskEventEntry` содержит только:

- `timestamp`
- `event_name`

Это непригодно для полноценного UI, потому что теряется payload:

- input/output tool events;
- reasoning summary;
- todos updates;
- waiting-for-user-input prompt;
- waiting-for-approval metadata;
- token snapshot;
- provider failover / compaction / retry notice;
- file metadata.

UI не должен собирать всё только из aggregate `ProgressState`. Для нормального event panel нужен богатый event payload.

#### 5.3.7 `/progress` сейчас фактически не live

`spawn_event_collector()` обновляет `task_progress` только после того, как `collect_events()` завершил сбор событий. Значит `GET /progress` во время выполнения задачи может быть пустым или stale.

Для V1 это надо исправить: progress snapshot должен обновляться во время run, а не только в конце.

#### 5.3.8 SSE сейчас недостаточно надёжен для browser UI

Проблемы в `src/server.rs`:

- SSE ждёт регистрацию event log polling-ом до 30 секунд;
- после этого stream принудительно закрывается через 60 секунд;
- отсутствует replay по sequence number;
- нет поддержки `Last-Event-ID` / `after_seq`;
- нет keepalive contract;
- snapshot берётся только при подключении и дальше идёт broadcast без backfill.

Для коротких e2e это приемлемо. Для реального UI — нет.

#### 5.3.9 Нет session list/history API для UI

Сейчас нет:

- списка сессий пользователя;
- списка задач в сессии;
- endpoint-а финального ответа;
- endpoint-а полного task detail;
- endpoint-а current user;
- endpoint-а public config;
- resume endpoint-а для `WaitingForUserInput`.

#### 5.3.10 Нет ограничений на “одна активная задача на сессию”

`register_task()` в `src/session.rs` не блокирует запуск новой задачи, если предыдущая ещё работает. Существующий e2e тест прямо фиксирует текущую дыру: follow-up во время run становится отдельной top-level задачей.

Для браузерного чата V1 это плохое UX-поведение. Нужен явный policy:

- одна активная задача на сессию;
- follow-up во время `running` должен возвращать `409 session_busy`;
- если задача ждёт user input, должен использоваться resume endpoint того же task.

### 5.4 Что уже есть полезного в `oxide-agent-core`

Проверено по `crates/oxide-agent-core/src/agent/executor.rs`, `src/agent/executor/execution.rs`, `src/agent/progress.rs`, `src/agent/session.rs`, `src/agent/memory.rs`.

Полезные существующие контракты:

- `AgentExecutionOutcome` уже различает:
  - `Completed(String)`
  - `WaitingForApproval`
  - `WaitingForUserInput(PendingUserInput)`
- `AgentEvent` уже богатый и покрывает нужды UI:
  - thinking;
  - token snapshot updates;
  - tool call / tool result;
  - waiting for approval;
  - continuation;
  - todos updated;
  - file to send;
  - cancelling / cancelled;
  - error;
  - reasoning;
  - loop detection;
  - compaction and retry events;
  - milestones.
- `ProgressState` уже хранит полезный aggregate state:
  - current iteration;
  - max iterations;
  - current todos;
  - current thought;
  - error;
  - compaction status;
  - repeated compaction warning;
  - history repair status;
  - latest token snapshot;
  - retry / provider failover notices.
- `PendingUserInput` и `UserInputKind` уже есть.
- `restore_last_task_from_memory()` уже есть на уровне session model.
- `AgentMemory` умеет `replace_messages()`, но в репозитории нет устойчивой web-facing message-level модели с постоянными message IDs.

Вывод: core уже содержит почти весь runtime semantics, который нужен UI. Проблема не в core, а в web transport / persistence / auth / API surface.

### 5.5 Что уже есть полезного в `oxide-agent-runtime`

Проверено по `crates/oxide-agent-runtime/src/session_registry.rs`.

`SessionRegistry` уже умеет:

- хранить executors;
- `get_or_create`, `get`, `insert`, `remove`, `remove_if_idle`;
- `cancel`;
- `renew_cancellation_token`;
- `resume_with_user_input`;
- `enqueue_runtime_context`;
- `is_running`;
- reset / clear todos.

Это важная опора. PRD не предлагает её заменять. Наоборот, backend web UI должен использовать ту же модель orchestration.

### 5.6 Telegram transport — полезный reference для durable/session behavior

Проверено по `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/session.rs` и `task_runner.rs`.

Что там уже сделано лучше, чем в web transport:

- session bootstrap использует durable storage, а не in-memory-only модель;
- agent memory загружается из storage на старте session;
- после задачи вызывается flush memory checkpoint;
- `Completed`, `WaitingForApproval`, `WaitingForUserInput` обрабатываются раздельно;
- сессия умеет быть восстановлена из persisted memory.

Вывод: web transport должен заимствовать эти паттерны, а не изобретать вторую runtime модель.

### 5.7 Storage и config в репозитории уже позволяют durable strategy

Проверено по `crates/oxide-agent-core/src/storage/modules.rs`, `r2_config.rs`, `r2_base.rs`, `keys.rs`.

Факты:

- S3/R2 уже является целевой durable storage стратегией репозитория;
- `build_primary_storage()` уже существует;
- `R2StorageConfig` уже читает `OXIDE_R2_*` / module config;
- `R2Storage` уже имеет полезные низкоуровневые операции:
  - `save_json`;
  - `load_json`;
  - `save_text`;
  - `load_text`;
  - conditional write с etag;
  - delete object / delete prefix;
  - listing JSON under prefix.
- существующие key helpers уже используют `users/{user_id}/...` namespace.

Ограничение: текущий `StorageProvider` не содержит методов для web auth / browser sessions / task event persistence. Значит V1 нужен отдельный web persistence слой, а не голые `HashMap` внутри transport.

### 5.8 Что показывают текущие E2E тесты web transport

Проверено по `crates/oxide-agent-transport-web/tests/e2e/*`.

Полезно для V1:

- уже есть рабочий e2e-ish паттерн для Axum web transport;
- уже есть тесты на task lifecycle, cancellation, SSE, timeline;
- уже есть тест, который фиксирует текущую проблему follow-up while running;
- уже есть тест на resume after user input, но он обходит HTTP слой и идёт напрямую в registry.

Вывод: эти тесты надо не выкидывать, а расширить. Они хорошая база для backend integration/e2e плана.

## 6. Proposed Architecture

### 6.1 Общее направление

Для V1 нужно **не создавать новый transport**, а эволюционно доработать существующий `crates/oxide-agent-transport-web` и добавить поверх него Rust-only frontend.

Архитектурное решение:

- backend/API остаётся в `oxide-agent-transport-web`;
- frontend добавляется отдельным Rust crate;
- shared DTO contracts выносятся в отдельный shared Rust crate;
- persistence для web auth/session/task/event данных добавляется как отдельный durable слой поверх S3/R2;
- core/runtime execution semantics остаются в `oxide-agent-core` + `oxide-agent-runtime`.

### 6.2 Выбранный frontend framework

**Выбор: Leptos в режиме CSR (client-side rendering) + Trunk для dev/prod bundle сборки.**

Почему это лучший вариант для этого репозитория:

- полностью Rust-only frontend;
- хорошо подходит к существующему backend API на Axum;
- не требует вводить TypeScript runtime-код;
- позволяет писать компоненты, routing, state и API client на Rust;
- проще для V1, чем SSR/hydration путь;
- хорошо подходит к сценарию “есть уже backend API, нужен отдельный SPA-like browser client на Rust/WASM”.

### 6.3 Почему выбран именно CSR, а не Leptos SSR

SSR не нужен в первой версии, потому что:

- UI — это authenticated console, а не SEO-ориентированный сайт;
- уже есть отдельный HTTP API transport, который надо развивать;
- SSR добавит лишнюю сложность с hydration/server functions;
- CSR упрощает границу между frontend и backend: frontend просто работает поверх versioned API.

### 6.4 Рассмотренные альтернативы

#### Yew

Подходит технически: Rust/WASM, компоненты, routing, CSR. Но для этого repo он менее предпочтителен, чем Leptos, потому что в V1 нужна не просто component library, а компактный Rust SPA с удобной реактивной моделью, умеренной сложностью и хорошим DX вокруг уже существующего backend API.

#### Dioxus

Тоже подходит технически. Но в этом проекте это менее удачный выбор, потому что Dioxus тянет за собой более выраженный собственный tooling/runtime стиль. Для существующего Axum-based backend и требования “минимально, но usable” Leptos CSR выглядит проще и ближе к цели.

#### Любой TypeScript frontend

Отвергается по product/architecture requirement. Кроме того, это ухудшит согласованность workspace, вынудит тащить отдельный toolchain, генерацию/синхронизацию DTO через отдельный стек и размоет цель “Rust-only web stack”.

### 6.5 Почему не используется TypeScript frontend

Причины должны быть зафиксированы явно:

- это прямое архитектурное ограничение задачи;
- репозиторий уже является Rust workspace и не содержит JS frontend стека;
- shared DTO можно держать в одном Rust crate без кодогенерации в TS;
- один язык на backend + frontend упростит сопровождение;
- для этой консоли важнее надёжность и согласованность модели данных, чем быстрый вход через React ecosystem.

### 6.6 Новые элементы workspace

Рекомендуемые additions:

- `crates/oxide-agent-web-contracts`
  - shared request/response DTO;
  - shared enums для task/session/auth/UI API;
  - shared serde models для frontend и backend.
- `crates/oxide-agent-web-ui`
  - Leptos CSR frontend;
  - Rust/WASM app;
  - routing, state, components, markdown renderer, API client.

Существующий `crates/oxide-agent-transport-web`:

- остаётся backend web transport и HTTP server;
- получает production API `/api/v1/...`;
- получает auth middleware;
- получает static asset serving built frontend bundle;
- получает durable persistence и rich event/task/session model.

### 6.7 Высокоуровневый runtime flow

1. Пользователь открывает UI.
2. Frontend запрашивает `GET /api/v1/public-config` и `GET /api/v1/me`.
3. Если пользователь не authenticated — показывается auth flow.
4. После логина frontend запрашивает список сессий пользователя.
5. При создании новой сессии backend создаёт session record и agent memory scope.
6. При отправке задачи backend создаёт durable task record, запускает executor и начинает писать progress/events/final outcome в durable store и live SSE.
7. Frontend подписывается на SSE, показывает live events/progress.
8. После завершения backend сохраняет final response и terminal status.
9. При refresh frontend восстанавливает всё из durable task/session records и events.

## 7. Frontend Architecture

### 7.1 Расположение в workspace

Новый frontend crate:

- `crates/oxide-agent-web-ui`

Новый shared contracts crate:

- `crates/oxide-agent-web-contracts`

### 7.2 Зависимости и границы ответственности

`oxide-agent-web-ui` зависит от:

- `oxide-agent-web-contracts` для API DTO;
- выбранного Rust frontend framework (Leptos CSR);
- Rust crates для browser HTTP/SSE/clipboard/local storage, если нужны;
- Rust Markdown stack (`comrak`, `ammonia` или эквиваленты).

`oxide-agent-web-ui` **не должен**:

- содержать handwritten TS/JS код приложения;
- зависеть от React/Vue ecosystem;
- иметь отдельную schema source of truth вне Rust DTO crate.

`oxide-agent-web-contracts` должен быть единственным source of truth для browser-facing DTO.

### 7.3 Структура модулей frontend

Рекомендуемая структура:

```text
crates/oxide-agent-web-ui/
  src/
    main.rs
    app.rs
    routes.rs
    config.rs
    api/
      client.rs
      auth.rs
      sessions.rs
      tasks.rs
    auth/
      state.rs
      login_page.rs
      register_page.rs
      bootstrap_page.rs
      logout.rs
      guards.rs
    sessions/
      list.rs
      create.rs
      detail.rs
      state.rs
    tasks/
      composer.rs
      transcript.rs
      task_panel.rs
      events_panel.rs
      progress_panel.rs
      sse.rs
      state.rs
    markdown/
      renderer.rs
      sanitize.rs
      code_block.rs
    components/
      layout.rs
      sidebar.rs
      header.rs
      status_badge.rs
      error_banner.rs
      empty_state.rs
      loading.rs
      modal.rs
    pages/
      app_shell.rs
      settings.rs
      not_found.rs
    utils/
      time.rs
      clipboard.rs
      storage.rs
      errors.rs
```

### 7.4 Routing

Минимальный routing V1:

- `/login`
- `/register` — только если registration enabled
- `/bootstrap` — только если bootstrap required / allowed
- `/app`
- `/app/session/:session_id`
- `/settings`

Дополнительно:

- redirect `/` в зависимости от auth state;
- protected routes для `/app/*`;
- 404 page.

### 7.5 State management

Нужны следующие доменные state slices:

- `AuthState`
  - current user;
  - auth loading;
  - csrf token;
  - session expired flag.
- `SessionListState`
  - список сессий;
  - loading/error;
  - selected session id.
- `SessionDetailState`
  - session metadata;
  - список задач/история;
  - active task id.
- `TaskRuntimeState`
  - running status;
  - last progress snapshot;
  - event list;
  - SSE connection status;
  - reconnect attempts;
  - cancel in-flight state.
- `UiState`
  - banners;
  - backend unavailable;
  - unauthorized redirect reason;
  - narrow viewport toggles.

Источник истины для user/session/task data — backend API. Локальное состояние должно быть cache/view-model, а не permanent source of truth.

### 7.6 Session/task history model на frontend

Для V1 история сессии в UI должна строиться по persisted task records, а не по прямому чтению сырой `AgentMemory`.

Одна user-visible task единица должна содержать:

- user input markdown;
- task status;
- timestamps;
- final response markdown, если есть;
- error summary, если есть;
- pending user input metadata, если есть;
- waiting approval metadata, если есть.

Это даёт понятную чат-модель без попытки превратить внутреннюю memory структуру агента в browser transcript API.

### 7.7 SSE client

Frontend должен иметь отдельный Rust SSE client module.

Требования:

- подключение к task-specific SSE endpoint;
- поддержка reconnect;
- восстановление после разрыва;
- перед reconnect сначала запросить missed events через REST `after_seq`;
- хранить последний подтверждённый `seq` для каждого task;
- различать состояния:
  - connected;
  - disconnected;
  - reconnecting;
  - terminal closed.
- не дублировать already received events при reconnect.

### 7.8 Reconnect logic

Алгоритм reconnect V1:

1. SSE разорвался.
2. UI помечает задачу как `sse_disconnected` и показывает banner.
3. UI делает `GET task detail`.
4. UI делает `GET events?after_seq=<last_seen_seq>`.
5. UI дополняет локальный event list.
6. Если задача ещё `running`, UI переподключает SSE с `after_seq=<last_seen_seq>`.
7. Если задача уже terminal, SSE больше не нужен.

### 7.9 Markdown renderer component

Frontend должен иметь единый `MarkdownContent` component как security boundary.

Правила:

- на вход получает raw markdown string;
- в Rust превращает markdown в HTML;
- sanitizes HTML;
- только после этого вставляет fragment в DOM;
- больше нигде в приложении raw HTML insertion не допускается.

### 7.10 Error boundaries и loading states

Frontend должен явно обрабатывать:

- loading current user;
- login failed;
- registration failed;
- bootstrap required;
- sessions loading;
- session not found;
- task loading;
- SSE disconnected;
- backend unavailable;
- unauthorized / expired session;
- generic API errors.

### 7.11 Build and run: dev/prod

#### Dev

- backend запускается обычной Rust командой из `oxide-agent-transport-web` binary target;
- frontend запускается отдельно через Trunk;
- frontend использует dev proxy на backend `/api/...` и `/assets/...`, чтобы браузер работал как будто всё same-origin;
- handwritten JS не добавляется.

#### Prod

- frontend собирается в статические assets (WASM + generated loader + CSS);
- backend server из `oxide-agent-transport-web` раздаёт эти assets и API с того же origin;
- production режим не должен зависеть от permissive CORS.

### 7.12 CSS / styling policy

Для V1:

- простой CSS без тяжёлой дизайн-системы;
- базовая типографика;
- аккуратный spacing;
- читаемые code blocks;
- понятные status colors;
- usable narrow/mobile layout.

Сложные theming systems и design tokens не нужны.

## 8. Backend/API Requirements

### 8.1 Общий принцип

Backend V1 должен развивать существующий `oxide-agent-transport-web`, а не заменять его.

Рекомендуемый подход:

- существующие unversioned e2e endpoint-ы **удалить** (см. resolved decision в Section 20);
- добавить новый production/user-facing namespace `/api/v1/...`;
- новый frontend работает только с `/api/v1/...`.

### 8.2 Session lifecycle

Backend должен поддерживать:

- создание новой пользовательской session;
- список всех сессий authenticated user;
- чтение одной session;
- удаление session — обязательно для V1 (delete UI и API); архивирование не нужно;
- восстановление session history после refresh/restart.

Обязательные правила:

- session record привязан к `authenticated user_id`;
- `user_id` не принимается из browser body;
- backend всегда проверяет ownership;
- доступ к чужой session должен возвращать `404`, а не раскрывать существование ресурса через `403`.

### 8.3 Agent memory scope для web sessions

Текущий web transport использует дефолтные `context_key = "default"` и `agent_flow_id = "default"`. Для браузерных сессий это слишком грубо.

Требование V1:

- каждая web session должна иметь собственный `context_key`;
- рекомендуемый формат: `context_key = "web-session-{session_id}"`;
- `agent_flow_id` для V1 можно фиксировать как `main`, сохранив поле в session record для future compatibility.

Это позволит:

- держать контекст изолированным между чатами;
- использовать уже существующую flow memory storage модель;
- безопасно восстанавливать session memory из durable store.

### 8.4 Task lifecycle

Backend должен поддерживать следующие статусы задачи:

- `queued` — optional but recommended;
- `running`;
- `waiting_for_user_input`;
- `completed`;
- `failed`;
- `cancelled`;
- `interrupted` — для crash/restart/неполного terminal persistence.

`waiting_for_approval` не входит в V1: web-транспорт работает в YOLO (Full permission) mode, агент никогда не ждёт approve.

Обязательные правила:

- в одной session в V1 одновременно может быть только одна active task;
- `POST new task` при `running` должен вернуть `409 session_busy`;
- `POST new task` при `waiting_for_user_input` должен вернуть `409 task_waiting_for_user_input` и указать `task_id` для resume;
- после `cancelled`, `completed`, `failed`, `interrupted` пользователь может запускать новую задачу в той же session.

### 8.5 Создание задачи

При создании задачи backend обязан:

1. Проверить auth и ownership session.
2. Проверить policy “одна активная задача на сессию”.
3. Создать durable task record до запуска executor.
4. Инициализировать event log storage и `last_event_seq = 0`.
5. Обновить session metadata (`updated_at`, `active_task_id`, preview/title if needed).
6. Запустить executor.
7. Немедленно вернуть `task_id` и начальный status.

### 8.6 Получение task status и финального ответа

Нужен task detail endpoint, который возвращает:

- task metadata;
- raw user input markdown;
- final response markdown, если есть;
- status;
- timestamps;
- last progress snapshot;
- waiting payload, если есть;
- last known event sequence.

Важно: `final_response_markdown` должен persist-иться отдельно и не восстанавливаться постфактум из event log best-effort логикой.

### 8.7 `AgentExecutionOutcome` mapping обязателен

Backend V1 обязан корректно различать:

- `AgentExecutionOutcome::Completed(String)`
- `AgentExecutionOutcome::WaitingForUserInput(PendingUserInput)`
- `AgentExecutionOutcome::WaitingForApproval` — маппится в `failed` (см. YOLO mode ниже)

Нельзя, как сейчас, схлопывать всё `Ok(_)` в `Completed`.

Нормативное поведение:

- `Completed(String)`
  - сохранить `final_response_markdown`;
  - сохранить status `completed`;
  - очистить `active_task_id` у session.
- `WaitingForUserInput(...)`
  - сохранить status `waiting_for_user_input`;
  - сохранить prompt/kind metadata;
  - не создавать новую task при продолжении, а резюмировать текущую.
- `WaitingForApproval` — **YOLO mode**: не используется на web-канале.
  - backend маппит в `failed` с диагностическим сообщением: `"The agent requested approval, but web console runs in YOLO (full permission) mode. Reconfigure the agent or retry without an approval-requiring setup."`;
  - сохранить status `failed` и error_message;
  - не создавать отдельный `waiting_for_approval` status.

### 8.8 Resume after user input

Backend V1 должен добавить HTTP API для resume paused task.

Требование:

- если session/task находятся в `waiting_for_user_input`, пользователь может отправить новый текст через `POST /resume`;
- backend должен использовать `SessionRegistry.resume_with_user_input(...)` или эквивалентный executor path;
- тот же `task_id` должен продолжиться;
- history и event stream продолжаются на той же задаче;
- frontend не должен создавать новую top-level task для этого кейса.

### 8.9 Waiting for approval — YOLO (Full permission)

В V1 web-транспорт **не использует** `WaitingForApproval`. Решение: **YOLO (Full permission)**.

Принцип:

- web-сессии запускают агента в режиме полных разрешений — агент никогда не ждёт approve на web-канале;
- `WaitingForApproval` из core не маппится в web-статус задачи; если core всё же вернёт это состояние (например, через общий execution path), backend маппит его в `failed` с диагностическим сообщением, т.к. полноценный approval workflow не реализован для браузера;
- вся `waiting_for_approval`-семантика (статус, поля в record, API conflict, UI-состояния) **исключена** из V1 web scope.

Обоснование: core approval resume path не готов как браузерный сценарий, а для консольного использования YOLO mode даёт лучший UX, чем display-only блокировка задачи.

### 8.10 Cancel running task

Существующий cancel endpoint можно сохранить концептуально, но V1 должен обеспечить:

- ownership check;
- idempotent cancel behavior;
- корректную гонку `cancel vs complete`;
- сохранение terminal state `cancelled`, если cancel победил;
- невозможность зависания задачи в `running` после отмены.

### 8.11 Progress snapshot persistence

`ProgressState` должен persist-иться во время выполнения, а не только по завершении.

Минимум, что должно быть доступно UI:

- current iteration / max iterations;
- current thought;
- current todos;
- latest token snapshot;
- current error, если есть;
- compaction/retry/provider failover statuses.

UI endpoint не должен зависеть от in-memory-only aggregate map.

### 8.12 Rich event payload для UI

Нужен отдельный browser-facing event model, richer than current `TaskEventEntry`.

Требования:

- у каждого event есть `seq`;
- у каждого event есть `kind`;
- у event есть timestamp;
- у event есть payload/body для UI;
- payload допускает redaction/truncation;
- payload serializable и стабильный для frontend.

Нельзя оставлять только `event_name`.

### 8.13 SSE requirements

SSE остаётся предпочтительным live transport для V1, но должен быть доработан.

Обязательно:

- task-specific SSE endpoint;
- отсутствие жёсткого 60-second cutoff;
- keepalive comments/events;
- поддержка reconnect/backfill;
- sequence-based replay;
- поддержка `after_seq` query и желательно `Last-Event-ID`;
- initial snapshot или обязательный prefetch missed events через REST.

### 8.14 Event replay contract

При reconnect/page refresh UI должен иметь возможность:

- узнать last persisted task status;
- получить missed events начиная с `after_seq`;
- продолжить live SSE без потери видимости задачи.

Это обязательное свойство V1.

### 8.15 Final response persistence contract

Если agent завершился `Completed(String)`, backend обязан сделать финальный текст доступным через API даже если:

- пользователь обновил страницу;
- SSE уже закрыт;
- backend перезапустился;
- пользователь открыл сессию в другой вкладке.

Это must-have acceptance criterion.

### 8.16 Event log retention и объём

Event log не должен расти без ограничений.

Требования:

- события хранятся в chunked виде, а не бесконечным одним blob;
- на уровне task record хранится metadata о последнем seq и числе chunks;
- вводится retention/size policy для очень длинных задач;
- UI получает truncated indicators, если payload был урезан.

### 8.17 Errors

API должен возвращать единый error envelope, содержащий:

- machine-readable `code`;
- человекочитаемый `message`;
- `retryable` flag, если применимо;
- optional `details` для безопасных кейсов.

Нельзя отдавать сырые internal stack traces в браузер.

### 8.18 Auth middleware и user isolation

Все `/api/v1/sessions/*` и `/api/v1/tasks/*` endpoint-ы должны быть защищены auth middleware.

Backend должен:

- извлекать current user из server-side session cookie;
- подставлять user_id сам;
- проверять ownership каждого session/task;
- никогда не принимать `user_id` из browser body как источник истины.

### 8.19 CORS и dev setup

Для production:

- frontend и backend должны обслуживаться с одного origin;
- permissive CORS запрещён.

Для dev:

- допустим Trunk proxy на локальный backend;
- если нужен CORS без proxy, то только явно разрешённые dev origins, не `*`.

### 8.20 Static assets serving

Production backend должен уметь:

- раздавать собранные frontend assets;
- отдавать index file для browser routes;
- корректно обслуживать WASM / JS loader / CSS assets;
- fail-fast, если assets отсутствуют в production сборке.

## 9. Authentication & Registration

### 9.1 Scope V1

V1 должен включать:

- login page;
- register page, если registration enabled;
- logout;
- current user endpoint;
- browser session cookie;
- password hashing;
- user persistence;
- disabled user behavior;
- bootstrap первого пользователя/admin.

### 9.2 Минимальная модель пользователя

Нужна следующая минимальная user model:

- `user_id: i64`
- `login: String`
- `normalized_login: String`
- `password_hash: String`
- `role: user | admin`
- `status: active | disabled`
- `created_at`
- `updated_at`
- `last_login_at: Option<DateTime>`
- `schema_version`

`user_id` нужен именно числовой, чтобы не ломать существующие core/runtime storage conventions.

### 9.3 Генерация user_id

V1 не должен брать `user_id` из browser.

Рекомендуемое поведение:

- генерировать случайный положительный 63-bit `i64`;
- проверять отсутствие коллизии в storage;
- при коллизии повторять генерацию.

Это проще и безопаснее, чем вводить SQL sequence или migration-dependent counter.

### 9.4 Login normalization policy

Для V1 логин лучше сделать intentionally narrow:

- ASCII only;
- допустимые символы: буквы, цифры, `.`, `_`, `-`;
- без пробелов;
- case-insensitive matching через `normalized_login = lowercase(login)`.

Причина: уменьшение риска Unicode confusables и user enumeration edge cases.

### 9.5 Password policy

Минимальные требования V1:

- password хранится только как hash;
- plaintext storage запрещён;
- использовать Argon2id;
- минимальная длина — не меньше 12 символов;
- максимальная длина — ограничена разумным upper bound, чтобы избежать DoS на hashing path;
- complexity rules вида “обязательно цифра/символ” не обязательны; passphrase acceptable.

### 9.6 Password hashing

Требование V1:

- использовать Argon2id;
- хранить hash в self-describing формате, включающем соль и параметры;
- предусмотреть upgrade path параметров в будущем;
- сравнение только через verify API, без ручных string compare трюков.

### 9.7 Browser session model

Рекомендуемая модель: **server-side auth sessions**.

Почему не JWT для V1:

- проще revoke/logout;
- проще disabled user handling;
- проще CSRF model с server-side session metadata;
- лучше подходит для same-origin console.

Требования:

- в cookie хранится только opaque random token;
- в storage хранится только hash token, не raw token;
- session record содержит `user_id`, `csrf_token`, timestamps, expiry, revoked flag.

### 9.8 Cookie requirements

Обязательно:

- `HttpOnly`;
- `SameSite=Lax`;
- `Secure=true` в production;
- `Path=/`;
- ограниченный `Max-Age` / `Expires`;
- session rotation при login.

### 9.9 CSRF requirements

Так как V1 использует cookie auth, нужно явно закрыть CSRF.

Минимальная схема:

- server-side session хранит `csrf_token`;
- frontend получает его через `GET /api/v1/me` или login response body;
- все mutating endpoints требуют `X-CSRF-Token` header;
- backend проверяет header against session;
- дополнительно проверяются `Origin`/`Referer` для same-origin POST/DELETE endpoints, где это возможно.

### 9.10 Registration enabled / disabled

Нужен config/env флаг уровня web transport, например:

- `OXIDE_WEB_REGISTRATION_ENABLED=true|false`

Требуемое поведение:

- если `true` — `/register` доступен;
- если `false` — `/register` недоступен и API возвращает явную controlled ошибку;
- frontend должен узнавать это через `GET /api/v1/public-config`.

### 9.11 Bootstrap первого пользователя/admin

Предлагаемый пользователем вариант `admin:admin` **не должен использоваться**. Предсказуемые default credentials недопустимы.

Требование V1:

- если пользователей нет вообще, система должна уметь безопасно создать первого пользователя;
- если public registration enabled и users count == 0, первый успешно зарегистрированный пользователь получает роль `admin`;
- если public registration disabled и users count == 0, должен существовать **отдельный bootstrap flow**, а не скрытая дыра в обычной регистрации.

Рекомендуемая минимальная реализация bootstrap:

- config/env `OXIDE_WEB_BOOTSTRAP_TOKEN`;
- если users count == 0 и token задан, доступен `POST /api/v1/auth/bootstrap`;
- bootstrap endpoint принимает login/password + bootstrap token;
- после первого успешного bootstrap endpoint выключается автоматически.

Это закрывает кейс “registration disabled, but no users exist” без unsafe default credentials.

### 9.12 Login behavior

Требования:

- login по `login + password`;
- одинаковое внешнее сообщение на “wrong password”, “unknown login”, “disabled user”, если пользователь не уже authenticated как admin;
- отсутствие user enumeration через разные сообщения и HTTP timing by design настолько, насколько это практично;
- rate limit по IP/login key;
- optional small fixed jitter на error path допустим.

### 9.13 Register behavior

Требования:

- register доступен только если enabled или если это first-user bootstrap path;
- при существующем login вернуть controlled conflict без утечки лишней информации;
- слабый/короткий пароль отклонять;
- слишком длинные login/password отклонять;
- входные поля валидировать и на frontend, и на backend.

### 9.14 Disabled/deleted user behavior

Если пользователь disabled или удалён, но cookie ещё есть:

- backend должен возвращать `401 unauthorized`;
- текущая browser session должна считаться недействительной;
- frontend должен очистить auth state и отправить на login;
- existing running task не должен автоматически cancel-иться только из-за logout/expiry.

### 9.15 Logout behavior

Logout должен:

- ревокнуть текущую browser auth session;
- очистить cookie;
- не ломать другие browser sessions того же пользователя;
- не отменять running agent task автоматически.

### 9.16 Multi-tab / multi-user

Требования:

- несколько вкладок одного пользователя должны работать корректно;
- один пользователь может иметь несколько active browser sessions;
- разные пользователи должны быть строго изолированы;
- task/session access by guessed ID между пользователями невозможен.

### 9.17 Минимальная модель ролей

Для V1 достаточно:

- `user`
- `admin`

Admin UI не входит в scope. Но роль должна быть заложена в data model для:

- future user management;
- bootstrap первого администратора;
- возможной диагностики и restricted operator actions позже.

## 10. Markdown Rendering Requirements

### 10.1 Общий принцип

Markdown в сообщениях пользователя и агента — обязательный функционал V1, а не nice-to-have.

### 10.2 Выбор parser/render stack

Предпочтительный вариант V1:

- `comrak` для CommonMark + GFM-friendly parsing/rendering;
- `ammonia` для HTML sanitization.

Альтернатива:

- `pulldown-cmark` допустим, но для V1 менее предпочтителен, потому что потребуется больше ручной работы для parity с GFM features, которые прямо важны в этой задаче.

### 10.3 Поддерживаемые возможности Markdown

В V1 должны корректно рендериться:

- CommonMark базовый синтаксис;
- GitHub Flavored Markdown, где поддержка реалистична без раздувания scope;
- fenced code blocks;
- inline code;
- headings;
- bold / italic / strikethrough;
- ordered / unordered lists;
- nested lists;
- blockquotes;
- links;
- images, только если backend их безопасно отдаёт;
- tables;
- task lists;
- horizontal rules;
- code block language labels.

### 10.4 Code blocks

Обязательные требования:

- блоки кода визуально отделены;
- есть горизонтальный scroll для длинных строк;
- язык блока отображается, если label указан;
- есть кнопка `Copy` у fenced code blocks.

Syntax highlighting:

- nice-to-have, но не must-have V1;
- если Rust/WASM highlighting заметно раздувает bundle или осложняет безопасность/скорость, V1 ограничивается language label + copy button + хорошим monospace rendering.

### 10.5 Длинные строки и большие блоки

UI должен корректно вести себя при:

- очень длинных code lines;
- очень длинных plain-text строках;
- больших ответах с десятками/сотнями строк кода.

Требования:

- code blocks не ломают layout и не растягивают страницу бесконечно по горизонтали;
- prose content переносится (`overflow-wrap` / `word-break` политика для обычного текста);
- code content остаётся читаемым через scroll container.

### 10.6 Streaming Markdown

Во время SSE streaming markdown может быть:

- незавершённым;
- временно невалидным;
- обрываться посередине fenced block.

Требование V1:

- UI должен “degrade gracefully”;
- markdown должен рендериться на текущем накопленном тексте без падений интерфейса;
- при временной невалидности допустим fallback к escaped plaintext rendering данного фрагмента;
- по мере поступления новых чанков компонент должен повторно рендерить markdown.

Рекомендуется debounce/throttle parsing, чтобы не перегружать WASM при частом обновлении.

### 10.7 HTML handling

Критичное правило: **текст от LLM нельзя считать безопасным HTML**.

Даже если markdown renderer генерирует HTML, security boundary должен быть таким:

1. raw markdown string;
2. Rust markdown parser/render;
3. HTML sanitization;
4. only then DOM insertion.

### 10.8 Sanitization

Требования:

- sanitization обязательна даже если parser safe-by-default;
- raw HTML из markdown не должен проходить в DOM как trusted content;
- dangerous tags/attrs/protocols должны удаляться;
- `javascript:` и аналогичные link schemes должны блокироваться;
- `onerror`, `onclick` и прочие inline event handlers должны вычищаться.

### 10.9 Link policy

Требования:

- разрешать только безопасные схемы (`http`, `https`, при необходимости `mailto` по явному решению);
- внешние ссылки открывать безопасно;
- добавлять `rel="noopener noreferrer"` для внешних ссылок;
- malicious links не должны приводить к XSS или opener hijack.

### 10.10 Image policy

В V1 нельзя доверять произвольным remote image URL из LLM output.

Требование:

- изображения рендерятся только если URL относится к same-origin/backend-controlled path или signed safe asset path;
- иначе markdown image должен либо превращаться в обычную ссылку, либо не рендериться как `<img>`.

### 10.11 Fallback for invalid markdown

Если markdown parser или sanitizer не смогли безопасно построить HTML fragment:

- интерфейс не падает;
- сообщение отображается как escaped plaintext с сохранением переносов строк;
- ошибка логируется на клиенте/сервере диагностически, но не показывается пользователю как panic.

## 11. UX/UI Requirements

### 11.1 Общая цель UX

Интерфейс должен быть простым, аккуратным и понятным. Не нужен дизайнерский идеал. Нужен usable операторский console experience.

### 11.2 Login page

Обязательные элементы:

- поле login;
- поле password;
- submit button;
- error state;
- link на registration page, если registration enabled;
- состояние loading;
- отдельный UX для expired session / invalid credentials.

### 11.3 Register page

Показывается только если registration enabled или если это явно bootstrap flow.

Обязательные элементы:

- login;
- password;
- password confirmation;
- inline validation;
- submit button;
- controlled error states;
- понятное сообщение, если регистрация отключена.

### 11.4 Bootstrap page

Не является обычной registration page.

Нужна только если:

- пользователей нет;
- bootstrap explicitly allowed.

Элементы:

- login;
- password;
- bootstrap token field или другой подтверждающий механизм;
- message, что создаётся первый admin.

### 11.5 Main app shell

Минимальный layout:

- header/top bar с текущим пользователем и logout;
- left sidebar со списком session;
- центральная область чата/консоли;
- правая panel для events/progress на desktop;
- usable collapsible behavior на narrow viewport.

### 11.6 Session list/sidebar

Требования:

- список только сессий текущего пользователя;
- сортировка по `updated_at desc`;
- кнопка “New session”;
- empty state, если сессий нет;
- loading/error states;
- базовые session metadata:
  - title/preview;
  - last updated;
  - active status badge.

### 11.7 Session creation

При создании новой сессии:

- UI должен немедленно открыть её;
- session history изначально пустая;
- показывается `no task yet` state;
- title может быть временным (“New session”), потом обновлён по первому prompt preview.

### 11.8 Agent chat/task console

В центральной зоне должны быть:

- история user/assistant turns по task records;
- textarea для ввода raw markdown;
- send button;
- cancel/stop button для running task;
- видимый running indicator;
- final answer rendering.

Поведение:

- ввод — raw markdown, не rich text editor;
- поддержка многострочного ввода;
- отправка создаёт task или resume, в зависимости от task state;
- во время `running` composer блокируется для new top-level task в этой session;
- после terminal state composer снова активен.

### 11.9 Task event/progress panel

Отдельная panel должна показывать:

- progress summary;
- thinking/reasoning/tool events;
- milestone-like события;
- status transitions;
- disconnect/reconnect state.

Важно: tool events и internal milestones лучше не смешивать в основной чат как обычные сообщения. Их место — отдельная events/progress panel.

### 11.10 Final answer presentation

Финальный ответ агента должен:

- быть явно отделён от внутреннего event stream;
- сохраняться как часть истории сессии;
- рендериться Markdown component-ом;
- быть доступен после refresh.

### 11.11 Error states

UI должен явно различать:

- login failed;
- registration failed;
- bootstrap failed;
- backend unavailable;
- unauthorized/expired session;
- session not found;
- task failed;
- task cancelled;
- session busy;
- waiting for user input;
- SSE disconnected/reconnecting.

### 11.12 Loading/empty states

Нужны отдельные readable states для:

- loading current user;
- loading sessions;
- empty sessions;
- creating session;
- opening session;
- no task yet;
- task loading;
- waiting for stream.

### 11.13 Basic settings/about page

Полезно иметь простую страницу `/settings` или `/about` с:

- current login;
- role;
- registration enabled/disabled indicator (public info only);
- app/build version;
- **форма смены пароля** (current password + new password + confirm);
- logout button.

Смена пароля — обязательный UI V1.

### 11.14 Mobile/narrow viewport

V1 не обязан быть polished mobile app, но должен быть usable:

- sidebar может открываться как drawer;
- events panel может уходить в tab/drawer;
- composer и transcript должны оставаться удобными;
- длинные code blocks не должны ломать экран.

### 11.15 Что явно не входит в UI V1

- approve/reject UI (V1 работает в YOLO/permissionless mode — агент никогда не ждёт approve);
- advanced session filters/search;
- admin dashboard;
- advanced profile/preferences.

## 12. User Stories

### 12.1 Аутентификация

#### V1

- Как новый пользователь, я хочу зарегистрироваться по логину и паролю, когда регистрация включена, чтобы получить доступ к веб-интерфейсу агента.
- Как пользователь, я хочу входить в систему по логину и паролю, чтобы мои сессии и задачи оставались приватными.
- Как пользователь, я хочу выходить из системы, чтобы browser session аннулировалась.
- Как оператор, я хочу иметь возможность отключить публичную регистрацию через config/env, чтобы доступ к системе имели только заранее созданные пользователи.
- Как оператор, я хочу иметь безопасный bootstrap первого пользователя/admin, чтобы не заблокировать себе доступ при отключённой регистрации.
- Как пользователь, я хочу, чтобы неверный пароль не раскрывал, существует ли login, чтобы система была безопаснее.
- Как пользователь, я хочу, чтобы истёкшая browser session корректно отправляла меня на login, а не ломала интерфейс.

#### Зафиксировано, но вне V1

- Как администратор, я хочу управлять пользователями через UI.

### 12.2 Сессии

#### V1

- Как пользователь, я хочу создавать новую session агента, чтобы начинать отдельный контекст диалога или задачи.
- Как пользователь, я хочу видеть список своих sessions, чтобы быстро возобновлять предыдущую работу.
- Как пользователь, я хочу открывать session и видеть историю связанных задач и ответов.
- Как пользователь, я хочу, чтобы мои sessions были изолированы от других пользователей.
- Как пользователь, я хочу, чтобы история session сохранялась после refresh страницы и перезапуска backend, если настроено durable storage.
- Как пользователь, я хочу переименовывать session вручную (рядом с названием в sidebar/хедере), а также получать auto-title от backend по первому prompt preview, если не переименовано вручную.

### 12.3 Взаимодействие с агентом

#### V1

- Как пользователь, я хочу отправлять агенту вопросы и задачи, чтобы получать ответы и выполнять рабочие сценарии.
- Как пользователь, я хочу видеть, что задача реально запущена и находится в работе.
- Как пользователь, я хочу видеть progress и события в реальном времени во время работы агента.
- Как пользователь, я хочу видеть tool events и важные milestone-like события, чтобы работа агента не была “чёрным ящиком”.
- Как пользователь, я хочу видеть финальный ответ после завершения задачи.
- Как пользователь, я хочу отменять running task кнопкой “стоп”.
- Как пользователь, я хочу продолжить paused task после `WaitingForUserInput` в той же session и на том же `task_id`.
- Как пользователь, я хочу после `cancelled` отправить новую задачу в этой же session, чтобы продолжить работу в том же контексте.
- Как пользователь, я хочу чётко видеть разницу между `running`, `completed`, `failed`, `cancelled`, `waiting_for_user_input`. (V1 работает в YOLO/permissionless mode, поэтому `waiting_for_approval` не возникает на web-канале.)
- Как пользователь, я хочу, чтобы ошибка была показана понятно, чтобы я мог решить — повторить задачу или изменить ввод.
- Как пользователь, я хочу редактировать последнее отправленное сообщение (через pencil icon), чтобы исправить опечатку или уточнить запрос без создания новой задачи. Редактирование доступно только когда задача остановлена (terminal status). В запущенном состоянии редактирование заблокировано — сначала нужно дождаться завершения или отменить задачу.

### 12.4 Markdown

#### V1

- Как пользователь, я хочу, чтобы ответы агента корректно отображали Markdown, включая заголовки, списки, код, ссылки, таблицы и task lists.
- Как пользователь, я хочу, чтобы блоки кода были легко читаемыми и копируемыми.
- Как пользователь, я хочу, чтобы опасный HTML очищался перед рендерингом.
- Как пользователь, я хочу, чтобы streaming или partially invalid Markdown не ломал интерфейс.

### 12.5 Надёжность

#### V1

- Как пользователь, я хочу, чтобы SSE-связь восстанавливалась после временных обрывов.
- Как пользователь, я хочу, чтобы refresh страницы не терял видимость running/completed task.
- Как пользователь, я хочу, чтобы финальный результат был доступен после refresh и при повторном открытии session.
- Как оператор, я хочу, чтобы backend логировал понятные ошибки веб-консоли и API.

### 12.6 Базовое управление

#### V1

- Как пользователь, я хочу видеть базовые metadata по session/task: что произошло и когда.
- Как пользователь, я хочу легко понимать состояние каждой задачи по статусу и UI меткам.
- Как оператор, я хочу, чтобы первая версия не была перегружена ненужными административными возможностями и была готова к быстрому выпуску.

## 13. Data Model / Persistence Requirements

### 13.1 Общая стратегия persistence

Для V1 web UI persistence должен быть durable и опираться на S3/R2.

Требования:

- in-memory хранение допустимо только в тестах и explicit dev/test mode;
- production и обычный local dev UI должны работать поверх S3/R2-backed persistence;
- никаких SQL migrations;
- каждый persisted JSON document должен иметь `schema_version`.

### 13.2 Где должен жить persistence слой

Рекомендуемое решение:

- добавить в `oxide-agent-transport-web` отдельный `persistence` module с чётким интерфейсом `WebUiStore`;
- дать ему минимум две реализации:
  - `InMemoryWebUiStore` для тестов;
  - `R2WebUiStore` для dev/prod.

`R2WebUiStore` должен использовать существующий storage stack из `oxide-agent-core` и при необходимости получить небольшие additive helper methods на уровне `R2Storage`/storage modules.

Не рекомендуется:

- продолжать строить веб-консоль на внутренних `HashMap` в `AppState`;
- делать downcast-heavy хаки из `dyn StorageProvider` без явного storage layer.

### 13.3 Persisted records

#### `WebUserRecord`

```json
{
  "schema_version": 1,
  "user_id": 123,
  "login": "alice",
  "normalized_login": "alice",
  "password_hash": "...",
  "role": "admin",
  "status": "active",
  "created_at": "...",
  "updated_at": "...",
  "last_login_at": "..."
}
```

#### `LoginIndexRecord`

```json
{
  "schema_version": 1,
  "normalized_login": "alice",
  "user_id": 123
}
```

#### `WebAuthSessionRecord`

```json
{
  "schema_version": 1,
  "session_token_hash": "...",
  "user_id": 123,
  "csrf_token": "...",
  "created_at": "...",
  "last_seen_at": "...",
  "expires_at": "...",
  "revoked_at": null
}
```

#### `WebSessionRecord`

```json
{
  "schema_version": 1,
  "session_id": "uuid",
  "user_id": 123,
  "title": "Investigate failing deploy",
  "context_key": "web-session-uuid",
  "agent_flow_id": "main",
  "created_at": "...",
  "updated_at": "...",
  "active_task_id": "uuid-or-null",
  "last_task_status": "completed",
  "last_preview": "First line of last user input or answer"
}
```

#### `WebTaskRecord`

```json
{
  "schema_version": 1,
  "task_id": "uuid",
  "session_id": "uuid",
  "user_id": 123,
  "status": "running",
  "input_markdown": "...",
  "input_edited_at": null,
  "final_response_markdown": null,
  "error_message": null,
  "pending_user_input": null,
  "last_progress": {
    "current_iteration": 3,
    "max_iterations": 100,
    "is_finished": false,
    "current_thought": "..."
  },
  "last_event_seq": 42,
  "created_at": "...",
  "started_at": "...",
  "updated_at": "...",
  "finished_at": null
}
```

#### `PersistedTaskEvent`

```json
{
  "schema_version": 1,
  "task_id": "uuid",
  "session_id": "uuid",
  "user_id": 123,
  "seq": 42,
  "created_at": "...",
  "kind": "tool_call",
  "summary": "execute_command",
  "payload": {
    "name": "execute_command",
    "command_preview": "cargo test",
    "input_preview": "..."
  },
  "redacted": false,
  "truncated": false
}
```

### 13.4 Object key layout

Рекомендуемый key layout:

- `web/auth/v1/users/{user_id}.json`
- `web/auth/v1/login_index/{normalized_login}.json`
- `web/auth/v1/browser_sessions/{session_token_hash}.json`
- `users/{user_id}/web/v1/sessions/{session_id}.json`
- `users/{user_id}/web/v1/tasks/{session_id}/{task_id}.json`
- `users/{user_id}/web/v1/task_events/{session_id}/{task_id}/chunk-{chunk_no}.json`

Отдельно остаётся уже существующая agent memory storage модель:

- `users/{user_id}/topics/{context_key}/flows/{flow_id}/memory.json`
- `users/{user_id}/topics/{context_key}/flows/{flow_id}/meta.json`

### 13.5 Event chunking

Чтобы не создавать бесконечно много tiny objects:

- events должны храниться чанками;
- один chunk — массив event-ов фиксированного размера или size budget;
- task record хранит `last_event_seq` и, при необходимости, `last_chunk_no`.

### 13.6 Что хранится как source of truth

Source of truth для browser UI:

- `WebSessionRecord`
- `WebTaskRecord`
- `PersistedTaskEvent` chunks

`AgentMemory` остаётся source of truth для runtime context агента, но не для прямого browser transcript API.

### 13.7 Relationship между session/task и core memory

Для каждой web session:

- есть durable web session record;
- есть соответствующий `AgentMemoryScope`;
- runtime executor читает/пишет agent memory через существующий storage путь;
- user-facing transcript читается из task records;
- после каждой terminal или paused стадии memory checkpoint должен быть flush-нут.

### 13.8 Startup reconciliation

При старте backend должен выполнять reconciliation:

- все task records, оставшиеся в `running` или `queued` после прошлого процесса, переводятся в `interrupted` с диагностическим сообщением;
- session records с `active_task_id`, указывающим на такую задачу, очищаются;
- persisted final result/event history при этом не теряются.

### 13.9 Sensitive event payload handling

Tool events могут содержать слишком много данных или чувствительную информацию.

Требования:

- в UI event log писать preview-oriented payload, а не безусловно весь raw output;
- большие payload-ы truncation-aware;
- чувствительные поля допускают redaction policy;
- raw file bytes не пишутся в обычный event log.

### 13.10 In-memory storage guardrail

Production web UI не должен случайно подняться на in-memory store.

Требование V1:

- если web UI enabled в production-like режиме и durable storage не сконфигурирован, backend должен fail-fast на startup;
- in-memory режим должен быть явно помечен как test/dev-only.

## 14. API Contract Draft

### 14.1 Namespace

Production browser API V1:

- `/api/v1/...`

Существующие unversioned e2e/test endpoint-ы (и их e2e тесты) **удаляются полностью**: они несовместимы с новой auth/API моделью, и поддерживать их параллельно — пустая трата времени. Все e2e сценарии переписываются на `/api/v1/...`.

### 14.2 Public config

#### `GET /api/v1/public-config`

Возвращает безопасный для браузера минимум:

```json
{
  "registration_enabled": true,
  "bootstrap_required": false,
  "build_version": "..."
}
```

### 14.3 Current user

#### `GET /api/v1/me`

Если authenticated:

```json
{
  "user": {
    "user_id": 123,
    "login": "alice",
    "role": "admin"
  },
  "csrf_token": "..."
}
```

Если не authenticated — `401`.

### 14.4 Bootstrap

#### `POST /api/v1/auth/bootstrap`

Используется только если:

- users count == 0;
- bootstrap mode explicitly allowed.

Request:

```json
{
  "login": "admin",
  "password": "strong passphrase",
  "bootstrap_token": "..."
}
```

Response:

```json
{
  "user": {
    "user_id": 1,
    "login": "admin",
    "role": "admin"
  }
}
```

### 14.5 Register

#### `POST /api/v1/auth/register`

Request:

```json
{
  "login": "alice",
  "password": "strong passphrase"
}
```

Response:

```json
{
  "user": {
    "user_id": 123,
    "login": "alice",
    "role": "user"
  }
}
```

### 14.6 Login

#### `POST /api/v1/auth/login`

Request:

```json
{
  "login": "alice",
  "password": "strong passphrase"
}
```

Response:

```json
{
  "user": {
    "user_id": 123,
    "login": "alice",
    "role": "user"
  },
  "csrf_token": "..."
}
```

Cookie устанавливается server-side.

### 14.7 Logout

#### `POST /api/v1/auth/logout`

Response:

```json
{
  "ok": true
}
```

### 14.8 Change password

#### `POST /api/v1/auth/change-password`

Требует CSRF-токен.

Request:

```json
{
  "current_password": "old passphrase",
  "new_password": "new strong passphrase"
}
```

Response:

```json
{
  "ok": true
}
```

- `current_password` проверяется, при несовпадении — `403 invalid_credentials`.
- `new_password` проходит те же валидации, что при регистрации (длина, не пустой).
- После успешной смены все текущие browser sessions пользователя (кроме той, что сделала запрос) ревокаются.
- Пароль хэшируется Argon2id.

### 14.9 List sessions

#### `GET /api/v1/sessions`

Response:

```json
{
  "sessions": [
    {
      "session_id": "...",
      "title": "Investigate failing deploy",
      "last_preview": "Need help with...",
      "active_task_id": null,
      "last_task_status": "completed",
      "created_at": "...",
      "updated_at": "..."
    }
  ]
}
```

### 14.10 Create session

#### `POST /api/v1/sessions`

Request body в V1 может быть пустым или minimal:

```json
{}
```

Response:

```json
{
  "session": {
    "session_id": "...",
    "title": "New session",
    "created_at": "...",
    "updated_at": "..."
  }
}
```

`user_id` в request отсутствует.

### 14.11 Update session (rename)

#### `PATCH /api/v1/sessions/{session_id}`

Request:

```json
{
  "title": "Root cause analysis of deploy failure"
}
```

Response:

```json
{
  "session": {
    "session_id": "...",
    "title": "Root cause analysis of deploy failure",
    "updated_at": "..."
  }
}
```

- Изменяется только `title`. Остальные поля игнорируются.
- Ownership check обязателен.
- Пустой `title` или только из пробелов — `422 validation_error`.

### 14.12 Get session detail

#### `GET /api/v1/sessions/{session_id}`

Response:

```json
{
  "session": {
    "session_id": "...",
    "title": "Investigate failing deploy",
    "active_task_id": "...",
    "last_task_status": "running",
    "created_at": "...",
    "updated_at": "..."
  }
}
```

### 14.13 List tasks in session

#### `GET /api/v1/sessions/{session_id}/tasks`

Response:

```json
{
  "tasks": [
    {
      "task_id": "...",
      "status": "completed",
      "input_markdown": "Find the root cause",
      "final_response_markdown": "# Root cause\n...",
      "created_at": "...",
      "started_at": "...",
      "finished_at": "..."
    }
  ]
}
```

### 14.14 Create task

#### `POST /api/v1/sessions/{session_id}/tasks`

Request:

```json
{
  "input_markdown": "Investigate the failed deploy"
}
```

Response:

```json
{
  "task": {
    "task_id": "...",
    "status": "running",
    "created_at": "..."
  }
}
```

Conflict cases:

- `409 session_busy`
- `409 task_waiting_for_user_input`

### 14.15 Edit task input

#### `PATCH /api/v1/sessions/{session_id}/tasks/{task_id}/input`

Request:

```json
{
  "input_markdown": "Fixed: investigate the staging deploy, not production"
}
```

Response:

```json
{
  "task": {
    "task_id": "...",
    "input_markdown": "Fixed: investigate the staging deploy, not production",
    "input_edited_at": "...",
    "updated_at": "..."
  }
}
```

- Редактировать можно только последнюю задачу в session.
- `input_edited_at` проставляется при первом редактировании.
- Редактирование допустимо **только когда задача остановлена** (terminal status: `completed`, `failed`, `cancelled`). В запущенном состоянии (`running`, `waiting_for_user_input`) редактирование запрещено — `409 task_active`.
- Ownership check обязателен.

### 14.16 Resume task after user input

#### `POST /api/v1/sessions/{session_id}/tasks/{task_id}/resume`

Request:

```json
{
  "input_markdown": "Continue, the scope is specifically GPT-5.4-mini"
}
```

Response:

```json
{
  "task": {
    "task_id": "same-task-id",
    "status": "running"
  }
}
```

### 14.17 Get task detail

#### `GET /api/v1/sessions/{session_id}/tasks/{task_id}`

Response:

```json
{
  "task": {
    "task_id": "...",
    "session_id": "...",
    "status": "waiting_for_user_input",
    "input_markdown": "Investigate Codex limits",
    "final_response_markdown": null,
    "error_message": null,
    "pending_user_input": {
      "kind": "text",
      "prompt": "Send the exact scope"
    },
    "last_progress": {
      "current_iteration": 3,
      "max_iterations": 100,
      "current_thought": "Collecting evidence",
      "is_finished": false
    },
    "last_event_seq": 42,
    "created_at": "...",
    "started_at": "...",
    "updated_at": "...",
    "finished_at": null
  }
}
```

### 14.18 Get persisted events

#### `GET /api/v1/sessions/{session_id}/tasks/{task_id}/events?after_seq=42&limit=200`

Response:

```json
{
  "events": [
    {
      "seq": 43,
      "created_at": "...",
      "kind": "tool_call",
      "summary": "execute_command",
      "payload": {
        "name": "execute_command",
        "command_preview": "cargo test"
      },
      "redacted": false,
      "truncated": false
    }
  ],
  "last_seq": 43,
  "has_more": false
}
```

### 14.19 SSE stream

#### `GET /api/v1/sessions/{session_id}/tasks/{task_id}/stream?after_seq=43`

SSE contract:

- `event: snapshot`
- `event: task_event`
- `event: progress`
- `event: task_status`
- `event: keepalive`

Пример `task_event`:

```text
event: task_event
id: 44
data: {"seq":44,"kind":"tool_result","payload":{"name":"execute_command","success":true,"output_preview":"..."}}
```

Пример `task_status`:

```text
event: task_status
data: {"task_id":"...","status":"completed","final_response_available":true}
```

### 14.20 Cancel task

#### `POST /api/v1/sessions/{session_id}/tasks/{task_id}/cancel`

Response:

```json
{
  "ok": true,
  "status": "cancelled"
}
```

### 14.21 Error envelope

Все API error responses должны иметь общую форму:

```json
{
  "error": {
    "code": "task_waiting_for_user_input",
    "message": "The current task is waiting for user input.",
    "retryable": false,
    "details": {
      "task_id": "..."
    }
  }
}
```

## 15. Edge Cases & Risks

### 15.1 Auth/security

- **XSS via Markdown from LLM output**  
  Митигация: markdown → sanitize → DOM insertion only через один компонент-барьер.
- **HTML injection in Markdown**  
  Митигация: raw HTML не доверяется; sanitization обязательна.
- **Malicious links in Markdown**  
  Митигация: scheme allowlist, `rel="noopener noreferrer"`, блокировка `javascript:`.
- **Login brute force**  
  Митигация: rate limiting по IP/login key, generic invalid response.
- **User enumeration**  
  Митигация: одинаковые внешние ошибки для unknown login / wrong password / disabled user.
- **CSRF with cookie auth**  
  Митигация: `X-CSRF-Token` + Origin/Referer checks.
- **Session fixation**  
  Митигация: session rotation на login.
- **Cookie theft mitigation**  
  Митигация: HttpOnly, Secure, SameSite, short-ish TTL, server-side revoke.
- **Registration spam when enabled**  
  Митигация: rate limiting, login validation, optional reverse-proxy rate limit.
- **First-user bootstrap when registration is disabled**  
  Митигация: отдельный bootstrap flow с one-time token, не default credentials.
- **Accessing another user's session/task by ID**  
  Митигация: ownership check на каждый resource access; предпочтительно отвечать `404`.
- **Running task continues after logout**  
  Это допустимо. Logout ревокает browser auth session, но не должен неявно убивать уже запущенную агентную задачу.
- **Expired auth during active SSE stream**  
  Митигация: stream закрывается; UI показывает re-login banner; после повторного логина task state восстанавливается из durable store.

### 15.2 Runtime/task lifecycle

- **Task completes before frontend subscribes to SSE**  
  Митигация: final response и events уже persisted; UI получает task detail и historical events через REST.
- **SSE disconnects while task continues**  
  Митигация: backfill по `after_seq` + reconnect.
- **Browser refresh during running task**  
  Митигация: persisted task state + replay events + reconnect stream.
- **Multiple browser tabs subscribe to same task**  
  Допустимо. Источник истины — persisted task/event state и idempotent SSE.
- **User starts multiple tasks in same session**  
  Митигация: `409 session_busy` policy.
- **Backend restart during running task**  
  Митигация: startup reconciliation переводит task в `interrupted`.
- **Agent returns huge response**  
  Митигация: durable final response storage, scroll containers, no unbounded DOM explosions by design.
- **Agent returns malformed Markdown**  
  Митигация: graceful fallback на escaped plaintext.
- **Agent errors before emitting any event**  
  Митигация: task record всё равно должен перейти в `failed` с error message.
- **Agent returns `WaitingForUserInput`**  
  Митигация: distinct status + persisted prompt + resume API.
- **Agent returns `WaitingForApproval`**  
  Это состояние **не возникает** на web-канале в V1 (YOLO mode). Если core всё же вернул его через общий execution path — backend маппит в `failed` с диагностическим сообщением.
- **Cancel request races with task completion**  
  Митигация: terminal state transition must be idempotent and winner-defined.
- **Final result is missing but status says completed**  
  Недопустимо. `completed` разрешён только после успешной persistence final result в task record.
- **Event stream says completed but task metadata not persisted**  
  Недопустимо. Task record — source of truth. SSE не должен обгонять durable terminal write без reconciliation.
- **Tool event payload contains sensitive data**  
  Митигация: preview/redaction/truncation policy.
- **Event log grows without bounds**  
  Митигация: chunking, retention, truncation metadata.

### 15.3 Storage/config

- **In-memory storage used accidentally in production**  
  Митигация: fail-fast startup guard.
- **Durable storage unavailable**  
  Митигация: startup fail for prod mode; explicit controlled error in dev.
- **Migration/schema mismatch**  
  В V1 нет SQL migrations; вместо этого нужен `schema_version` в JSON docs и controlled backward-incompatible handling.
- **Corrupted session/task records**  
  Митигация: skip + mark record unusable + operator logs; UI должен показывать safe error, а не panic.
- **Missing config**  
  Митигация: fail-fast startup с читаемыми сообщениями.
- **Invalid config**  
  Митигация: validation на startup.
- **Registration disabled with no users**  
  Митигация: bootstrap-required mode.
- **Static assets missing in production build**  
  Митигация: startup self-check и controlled failure.

### 15.4 Frontend/Rust/WASM

- **WASM bundle too large**  
  Митигация: избегать тяжёлого highlighting в V1, следить за feature flags.
- **Markdown rendering too slow for huge messages**  
  Митигация: debounce re-render during streaming, pragmatic truncation strategy if needed.
- **SSE support differences**  
  Митигация: fallback to REST polling/backfill path.
- **API DTO mismatch between frontend and backend**  
  Митигация: shared Rust contracts crate.
- **Browser back/forward navigation**  
  Митигация: route-driven selected session state.
- **Auth cookie not sent due to SameSite/Secure config**  
  Митигация: same-origin prod, explicit dev proxy, environment-aware cookie config.
- **Long lines/code blocks break layout**  
  Митигация: dedicated CSS for code/prose overflow.
- **Mobile/narrow viewport usable enough, but not polished**  
  V1 accepts this, если сохраняется практическая usability.

## 16. Security Requirements

### 16.1 Trust boundaries

Нужно явно зафиксировать три разных trust boundary:

1. Browser input пользователя — недоверенный ввод.
2. Текст и события от LLM/agent/tools — тоже недоверенные данные.
3. Только backend-generated, sanitized, policy-checked DTO/HTML fragments допускаются к отображению или state transition.

### 16.2 Authn/Authz

- Login/password only, no plaintext password storage.
- Server-side session cookies only.
- Ownership check на каждый session/task endpoint.
- `user_id` из browser body/query/path не используется как источник истины для доступа.
- Foreign resources отвечают `404`.

### 16.3 Password security

- Argon2id.
- Salt per password.
- No plaintext.
- No reversible encryption.
- Password not logged.

### 16.4 Cookie/session security

- HttpOnly cookies.
- Secure в production.
- SameSite=Lax.
- Session rotation on login.
- Server-side revoke on logout.
- Session expiry and invalidation for disabled user.

### 16.5 CSRF

- All mutating endpoints require CSRF token.
- Origin/Referer validation where applicable.
- SSE and GET endpoints remain read-only.

### 16.6 Markdown/XSS

- No raw trusted HTML from LLM/user text.
- HTML sanitization mandatory.
- Dangerous tags/attrs/protocols removed.
- Unsafe image sources blocked.

### 16.7 Sensitive data in events/logs

- Tool inputs/outputs могут содержать секреты; UI log не должен blindly persist/display всё подряд.
- Preview/redaction strategy обязательна.
- Server logs не должны печатать raw passwords/session tokens.

### 16.8 Security headers

Production server should set sane defaults, совместимые с Rust/WASM bundle:

- `X-Content-Type-Options: nosniff`
- `Referrer-Policy: strict-origin-when-cross-origin`
- `X-Frame-Options: DENY` или эквивалент через CSP `frame-ancestors 'none'`
- CSP, совместимый с загрузкой WASM/static assets, без внешних wildcard script origins

Точный CSP-профиль — open question implementation detail, но security header policy обязателен.

### 16.9 Brute force / abuse

- Login/register rate limiting.
- Optional reverse-proxy/WAF reinforcement for internet-facing deployments.
- No user enumeration through response body.

## 17. Testing Strategy

### 17.1 Rust unit tests

Нужны unit tests для:

- password hashing/verification helpers;
- login normalization;
- auth cookie/session token utilities;
- CSRF token validation;
- task status mapping from `AgentExecutionOutcome`;
- `AgentEvent` → browser `PersistedTaskEvent` mapping;
- markdown render + sanitization pipeline;
- event truncation/redaction helpers;
- startup reconciliation logic.

### 17.2 Backend integration tests

Нужны integration tests для `/api/v1/...`:

- login success/failure;
- registration enabled/disabled;
- bootstrap flow;
- disabled user;
- logout;
- current user endpoint;
- create/list/get sessions;
- create task;
- resume task after user input;
- cancel task;
- access isolation between users;
- foreign session/task returns `404`;
- final response persisted and returned.

### 17.3 Web transport e2e-ish tests

Существующий паттерн `crates/oxide-agent-transport-web/tests/e2e/*` должен быть расширен, а не выкинут.

Добавить/обновить e2e-ish сценарии:

- SSE streaming with reconnect/backfill;
- page-refresh-like restore flow;
- final answer survives backend restart if durable storage configured;
- follow-up while running returns `409 session_busy` вместо current broken behavior;
- `WaitingForUserInput` exposed через API как distinct state;
- cancelled vs completed race;
- interrupted state after simulated restart.

### 17.4 Auth security tests

Нужны tests для:

- generic invalid credentials response;
- rate limit;
- session cookie flags;
- CSRF enforcement;
- disabled user with stale cookie;
- registration conflict;
- bootstrap token required when applicable.

### 17.5 User isolation tests

Нужны tests для:

- user A не видит sessions user B;
- user A не может читать tasks user B;
- user A не может подписаться на SSE task user B;
- guessed IDs не раскрывают существование чужих ресурсов.

### 17.6 Markdown sanitization tests

Нужны тесты на:

- `<script>`;
- `onerror`/`onclick` attrs;
- `javascript:` URLs;
- dangerous raw HTML in markdown;
- malformed fenced code blocks;
- long code blocks;
- tables and task lists;
- streaming partial markdown;
- graceful fallback when sanitizer/parser reject content.

### 17.7 Frontend component tests

Если выбранный Rust frontend stack это позволяет без оверсоупа, нужны tests для:

- auth guard;
- login form validation;
- session list rendering;
- task status badge mapping;
- markdown component;
- SSE state machine reconnect logic.

Подходящие инструменты — `wasm-bindgen-test` или framework-native component tests, если они не раздувают scope.

### 17.8 Contract tests

Нужны serialization/deserialization tests для shared DTO crate, чтобы frontend и backend не расходились по JSON schema.

### 17.9 Manual QA checklist

Минимальный ручной checklist:

- открыть UI без auth;
- зарегистрироваться при enabled registration;
- убедиться, что register hidden/disabled при disabled registration;
- войти и выйти;
- создать session;
- отправить задачу;
- увидеть running state;
- увидеть live events/progress;
- дождаться final answer;
- refresh страницы во время running;
- refresh после completed;
- cancel running task;
- resumed waiting-for-user-input task;
- попытаться открыть чужую session/task;
- проверить markdown rendering для headings/lists/code/tables/task lists/links;
- проверить XSS sanitation кейсы;
- проверить narrow viewport usability.

## 18. Acceptance Criteria

- Пользователь может зарегистрироваться, если регистрация включена.
- Пользователь не может зарегистрироваться через обычный registration endpoint, если регистрация выключена.
- Если регистрация выключена и пользователей нет, существует безопасный bootstrap flow для первого admin/user.
- Пользователь может войти и выйти.
- Пароли не хранятся в plaintext.
- Password hashing использует Argon2id или эквивалентно современный алгоритм.
- Authenticated user может создать session без передачи `user_id` из браузера.
- Authenticated user может видеть список только своих sessions.
- Пользователь не может получить доступ к session/task другого пользователя.
- Пользователь может открыть существующую свою session и увидеть историю задач.
- Пользователь может отправить задачу/вопрос агенту.
- UI показывает `running` state после отправки задачи.
- UI получает и отображает live events/progress во время выполнения.
- Backend сохраняет финальный ответ `AgentExecutionOutcome::Completed(String)` и отдаёт его через API.
- UI показывает final answer после completion.
- Page refresh не теряет видимость running/completed task.
- SSE reconnect/backfill не приводит к постоянной потере task visibility.
- Пользователь может отменить running task.
- Если задача ждёт user input, backend и UI показывают distinct status `waiting_for_user_input`, а не фальшивый `completed`.
- V1 работает в YOLO (Full permission) mode: `WaitingForApproval` не возникает на web-канале. Если core возвращает это состояние, backend маппит его в `failed`.
- Markdown корректно рендерит headings, lists, nested lists, inline code, fenced code blocks, links, blockquotes, tables, task lists, horizontal rules и inline formatting.
- Code blocks имеют copy button.
- Markdown output sanitizes unsafe HTML и защищён от XSS.
- Frontend application code написан на Rust, не на TypeScript.
- Production setup может раздавать frontend assets из Rust backend.
- Production setup не использует permissive CORS как норму.
- Durable storage используется для sessions/tasks/results/event log в dev/prod web UI режиме.
- Есть базовые automated tests для auth, session isolation, task lifecycle, SSE/reconnect и markdown sanitization.

## 19. Implementation Plan / Milestones

### Milestone 0 — Contracts and plumbing

- Добавить `oxide-agent-web-contracts` crate.
- Зафиксировать DTO и enums для auth/session/task/event API.
- Зафиксировать status model и error envelope.
- Добавить production API namespace `/api/v1` без ломания текущих e2e endpoints.

### Milestone 1 — Durable web persistence

- Добавить `WebUiStore` abstraction и R2-backed implementation.
- Добавить persisted models для user/auth session/web session/task/event chunks.
- Добавить startup reconciliation.
- Убрать implicit in-memory dependency для production web path.

### Milestone 2 — Auth foundation

- Реализовать bootstrap flow.
- Реализовать register/login/logout/current user.
- Реализовать change-password endpoint (`POST /api/v1/auth/change-password`).
- Реализовать password hashing, browser session cookies, CSRF.
- Реализовать auth middleware и ownership checks.

### Milestone 3 — Task/session API hardening

- Реализовать list sessions / get session / update session (rename) / list tasks / get task detail.
- Реализовать final response persistence.
- Реализовать rich persisted event model.
- Реализовать distinct waiting states.
- Реализовать resume-after-user-input endpoint.
- Реализовать edit-task-input endpoint (`PATCH .../tasks/{task_id}/input`).
- Сделать `one active task per session` policy.
- Убрать 60-second SSE cutoff, добавить sequence-based replay.

### Milestone 4 — Frontend shell and auth pages

- Добавить `oxide-agent-web-ui` crate на Leptos CSR.
- Реализовать login/register/bootstrap pages.
- Реализовать auth guard и app shell.
- Реализовать session sidebar и routing.
- Реализовать rename session через UI (inline edit рядом с названием в sidebar/хедере).
- Реализовать `/settings` page с формой смены пароля.

### Milestone 5 — Task console and Markdown

- Реализовать transcript/task console с возможностью редактировать последнее сообщение (pencil icon → inline edit → save).
- Реализовать SSE client, reconnect/backfill logic.
- Реализовать progress/events panel.
- Реализовать Markdown renderer с `comrak + ammonia` или эквивалентным безопасным стеком.
- Реализовать copy button у code blocks.

### Milestone 6 — Packaging, static serving, QA

- Собрать prod build pipeline frontend assets.
- Встроить static asset serving в backend.
- Добавить tests и manual QA checklist coverage.
- Проверить dev workflow (`backend + trunk serve`) и prod workflow (`backend serves built assets`).

## 20. Resolved Decisions

- **Approve/reject UI — не нужен.** V1 работает в **YOLO (Full permission)** mode: агент никогда не ждёт approval на web-канале. Если core возвращает `WaitingForApproval` через общий execution path, backend маппит его в `failed`. Это осознанное упрощение: core approval resume path не готов как браузерный сценарий, а для консольного использования YOLO mode предпочтительнее display-only блокировки.
- **Delete session — нужен. Archive — не нужен.** V1 включает delete session UI и API. Archive выкинут из реализации: нет ни модели данных, ни UI. Если понадобится в будущем — добавится отдельно.
- **Внешние markdown изображения — запрещены.** `<img>` из LLM-вывода (не same-origin, не signed backend URL) конвертируется в обычную `<a href="...">` ссылку. Никакого proxy, никакого allowlist — zero additional complexity для V1. Если появится реальный use case (например, агент сгенерировал диаграмму через backend-controlled filehoster), Section 10.10 уже разрешает backend-controlled images.
- **Change-password UI — нужен.** Пользователь должен иметь возможность сменить пароль без помощи администратора. V1 включает `/settings` страницу с формой смены пароля (текущий пароль + новый пароль + подтверждение). Backend endpoint `POST /api/v1/auth/change-password` с проверкой старого пароля и Argon2id-хэшированием нового.
- **Rename session — нужен (и auto-title, и manual rename).** Auto-title работает по preview первого user input (пока не переименован вручную). Ручной rename доступен через UI (иконка редактирования рядом с названием в sidebar / хедере) и API `PATCH /api/v1/sessions/{session_id}`. Оба механизма обязательны для V1 — без manual rename пользователь застрянет с авто-названиями, которые часто неинформативны.
- **Single-instance — OK, distributed deployment не нужен.** V1 реализуется для одного логического инстанса backend. Rate limiting — in-process (IP/login key). Session invalidation — in-process cache с проверкой в durable storage. Никакой distributed locking, memcached/redis, или cross-instance broadcast. Если в будущем понадобится горизонтальное масштабирование — это отдельный проект.
- **Unversioned e2e endpoints — удалить.** Старые endpoint-ы (и их e2e тесты) удаляются полностью. Они несовместимы с новой auth/API моделью. Все e2e сценарии переписываются на `/api/v1/...`. Никакой wire compatibility не поддерживается.
- **Редактирование сообщения — входит в V1.** Пользователь может редактировать последнее отправленное сообщение (pencil icon → inline edit). Редактирование доступно **только когда задача остановлена** (terminal status). В запущенном состоянии (`running`, `waiting_for_user_input`) — `409 task_active`. Backend: `PATCH .../tasks/{task_id}/input` + `input_edited_at`.

