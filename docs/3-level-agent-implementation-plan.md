### Agent Mode v2: Implementation Plan

Status: Draft

Этот документ дополняет `docs/3-level-agent.md` и раскладывает внедрение Agent Mode v2 на конкретные стадии и небольшие auditable slices.

План построен от текущего состояния кодовой базы:

- есть `SessionRegistry`, cancellation tokens, loop detection и progress events;
- есть синхронная delegation в sub-agent;
- нет `TaskId`, task state machine, task persistence, detached workers, HITL resume flow и web monitoring.

Главный принцип: не внедрять transport UX, пока нет task identity и runtime state contract.

---

### 1. Правила выполнения плана

- каждый slice должен быть маленьким, reviewable и testable;
- нельзя начинать transport-first реализацию без foundation в `core` и `runtime`;
- loop detection в рамках этого плана остается hard abort, без auto-retry;
- `cancel_task` и `stop_and_report` реализуются как разные механики;
- web monitoring не блокирует rollout базового Agent Mode v2;
- rollout идет через feature flags или staged enablement, а не через мгновенное включение для всех.

---

### 2. Stage 1 - Foundation: Task identity и state contract

Цель stage: ввести новую domain model для фоновой задачи, не ломая текущий session-centric runtime.

#### Slice 1.1 - Task Domain Model

Crates:

- `oxide-agent-core`

Deliverables:

- `TaskId` на базе UUID;
- `TaskState` с terminal и non-terminal состояниями;
- `TaskMetadata` с минимальным набором полей;
- валидатор переходов между состояниями.

Acceptance criteria:

- все допустимые переходы state machine покрыты unit-тестами;
- недопустимые переходы возвращают явную ошибку;
- типы не завязаны на Telegram transport.

Verification:

```bash
cargo test -p oxide-agent-core task_state
cargo clippy -p oxide-agent-core
```

#### Slice 1.2 - Task Persistence Contract

Crates:

- `oxide-agent-core`

Depends on:

- Slice 1.1

Deliverables:

- расширение `StorageProvider` методами для task state;
- `TaskSnapshot` и schema для persisted checkpoint;
- key naming contract для task storage;
- базовый event log contract для задач.

Acceptance criteria:

- task snapshot можно сохранить и прочитать без transport-specific данных;
- структура пригодна для recovery после рестарта;
- storage API документирован как additive extension.

Verification:

```bash
cargo test -p oxide-agent-core storage
cargo clippy -p oxide-agent-core
```

#### Slice 1.3 - Task Registry

Crates:

- `oxide-agent-runtime`

Depends on:

- Slice 1.1
- Slice 1.2

Deliverables:

- `TaskRegistry`, отдельный от `SessionRegistry`;
- создание, поиск, обновление и listing задач;
- task-scoped cancellation token management;
- связка `TaskId <-> SessionId`.

Acceptance criteria:

- runtime умеет создать task и сменить его состояние;
- task registry не ломает существующие session flows;
- параллельные операции по нескольким задачам корректно синхронизированы.

Verification:

```bash
cargo test -p oxide-agent-runtime task_registry
cargo clippy -p oxide-agent-runtime
```

#### Slice 1.4 - Task Events

Crates:

- `oxide-agent-core`
- `oxide-agent-runtime`

Depends on:

- Slice 1.1

Deliverables:

- новый `TaskEvent`, отделенный от текущего `AgentEvent`;
- базовые event kinds для жизненного цикла задачи;
- serialization contract и timestamping.

Acceptance criteria:

- события привязаны к `TaskId`;
- runtime может публиковать task events независимо от Telegram transport;
- event contract пригоден для будущего fan-out.

Verification:

```bash
cargo test -p oxide-agent-core task_events
cargo test -p oxide-agent-runtime task_events
```

Exit criteria for Stage 1:

- в системе есть минимальная task-centric модель, persistence contract и runtime registry;
- transport еще не знает про polls/web, но runtime уже знает про задачи.

---

### 3. Stage 2 - Background execution

Цель stage: отделить long-running execution от user-facing request flow.

#### Slice 2.1 - Background Worker Manager

Crates:

- `oxide-agent-runtime`

Depends on:

- Slice 1.3

Deliverables:

- manager для detached worker tasks;
- tracking `TaskId -> JoinHandle`;
- лимиты на количество фоновых workers;
- cleanup завершенных workers.

Acceptance criteria:

- worker запускается через runtime как отдельная async задача;
- завершенные worker handles очищаются;
- failure одного worker не валит весь runtime.

Verification:

```bash
cargo test -p oxide-agent-runtime worker_manager -- --test-threads=1
```

#### Slice 2.2 - Detached Task Executor

Crates:

- `oxide-agent-runtime`
- `oxide-agent-core`

Depends on:

- Slice 1.2
- Slice 2.1

Deliverables:

- runtime executor для long-running task;
- интеграция с существующим `AgentRunner` без transport coupling;
- переходы `Pending -> Running -> terminal state`;
- checkpoint persistence после безопасных шагов.

Acceptance criteria:

- задача исполняется без удержания user-facing handler path;
- runtime фиксирует состояние и может прочитать checkpoint;
- transport не является владельцем жизненного цикла worker.

Verification:

```bash
cargo test -p oxide-agent-runtime detached_executor
```

#### Slice 2.3 - Restart Recovery and Reconciliation

Crates:

- `oxide-agent-runtime`
- `oxide-agent-core`

Depends on:

- Slice 1.2
- Slice 1.3
- Slice 2.2

Deliverables:

- boot-time reconciliation для persisted tasks;
- правила восстановления состояний после рестарта процесса;
- политика для задач, которые были `Running` в момент падения процесса;
- восстановление runtime ownership для задач, которые можно безопасно продолжить;
- перевод невосстановимых задач в явный terminal/error state.

Acceptance criteria:

- после рестарта runtime не теряет knowledge о persisted tasks;
- `Running` task не остается в подвешенном состоянии без owner/worker semantics;
- recovery policy документирована и детерминирована.

Verification:

```bash
cargo test -p oxide-agent-runtime task_recovery
```

#### Slice 2.4 - Cascading Cancellation

Crates:

- `oxide-agent-runtime`

Depends on:

- Slice 2.2
- Slice 2.3

Deliverables:

- `cancel_task(task_id)`;
- каскадная отмена дочерних исполнений;
- перевод задачи в `Cancelled`;
- cleanup при гонке cancel vs complete.

Acceptance criteria:

- отмена по `TaskId` работает независимо от transport message state;
- дочерние execution branches не остаются orphaned;
- terminal event гарантированно публикуется.

Verification:

```bash
cargo test -p oxide-agent-runtime cancellation
```

Exit criteria for Stage 2:

- background worker живет как runtime entity;
- persisted tasks проходят boot-time reconciliation после рестарта;
- базовые create/run/cancel flows работают без Telegram-specific HITL.

---

### 4. Stage 3 - Human-in-the-Loop

Цель stage: дать задаче возможность безопасно приостанавливаться и ждать ответ пользователя.

#### Slice 3.1 - Pending Input Model

Crates:

- `oxide-agent-core`

Depends on:

- Slice 1.1

Deliverables:

- `PendingInput` и payload schema;
- поля для `WaitingInput` в task snapshot;
- validation для poll/request constraints.

Acceptance criteria:

- payload сериализуется и хранится в persistence;
- invalid payload отвергается до transport layer;
- модель не зависит только от Telegram и допускает будущие UX варианты.

Verification:

```bash
cargo test -p oxide-agent-core pending_input
```

#### Slice 3.2 - Runtime HITL Tool

Crates:

- `oxide-agent-runtime`
- `oxide-agent-core`

Depends on:

- Slice 2.2
- Slice 3.1

Deliverables:

- инструмент/runtime command для запроса пользовательского ввода;
- переход `Running -> WaitingInput`;
- persistence pending input;
- защита от повторного входа в `WaitingInput` без resume.

Acceptance criteria:

- worker может корректно приостановить задачу;
- pending input доступен runtime и transport;
- после перезапуска состояние `WaitingInput` не теряется.

Verification:

```bash
cargo test -p oxide-agent-runtime hitl_tool
```

#### Slice 3.3 - Telegram Poll Integration

Crates:

- `oxide-agent-transport-telegram`

Depends on:

- Slice 3.2

Deliverables:

- отправка poll в Telegram;
- mapping `poll_id -> task_id`;
- persistence/recovery для `poll_id -> task_id` mapping или эквивалентного resume key;
- валидация user identity;
- защита от duplicate answer/resume;
- закрытие poll после корректного ответа.

Acceptance criteria:

- transport может восстановить `TaskId` по ответу в poll;
- mapping переживает рестарт процесса или имеет документированный recovery path;
- чужой пользователь не может resume чужую задачу;
- invalid/late response обрабатывается безопасно.

Verification:

```bash
cargo test -p oxide-agent-transport-telegram poll
```

#### Slice 3.4 - Resume Flow

Crates:

- `oxide-agent-runtime`
- `oxide-agent-transport-telegram`

Depends on:

- Slice 3.3

Deliverables:

- `resume_task(task_id, input)`;
- переход `WaitingInput -> Running`;
- очистка pending input и transport mappings;
- возобновление worker из checkpoint.

Acceptance criteria:

- полный HITL цикл проходит end-to-end;
- повторный resume для той же задачи блокируется;
- задача не resume'ится из terminal state.

Verification:

```bash
cargo test -p oxide-agent-runtime hitl_resume
cargo test -p oxide-agent-transport-telegram poll_resume
```

Exit criteria for Stage 3:

- задача может приостановиться, дождаться ответа пользователя и продолжить исполнение.

---

### 5. Stage 4 - Graceful stop и task observability

Цель stage: добавить управляемую остановку и отделить event delivery от одного transport consumer.

#### Slice 4.1 - Stop Signal Contract

Crates:

- `oxide-agent-core`

Depends on:

- Slice 1.1

Deliverables:

- отдельный stop signal contract;
- типы данных для partial summary/report;
- правила safe-point обработки.

Acceptance criteria:

- `stop_and_report` семантически отделен от `cancel_task`;
- safe-point contract формализован и тестируем;
- нет неявного смешения soft-stop и hard-cancel.

Verification:

```bash
cargo test -p oxide-agent-core stop_signal
```

#### Slice 4.2 - Graceful Stop Runtime Flow

Crates:

- `oxide-agent-runtime`

Depends on:

- Slice 2.2
- Slice 4.1

Deliverables:

- `stop_and_report(task_id)`;
- перевод задачи в `Stopped`;
- генерация partial summary;
- доставка terminal event и отчета.

Acceptance criteria:

- worker способен остановиться на безопасной точке;
- пользователь получает итоговый частичный отчет;
- остановленная задача не продолжает планирование после завершения stop flow.

Verification:

```bash
cargo test -p oxide-agent-runtime graceful_stop
```

#### Slice 4.3 - Event Fan-Out Layer

Crates:

- `oxide-agent-runtime`

Depends on:

- Slice 1.4
- Slice 1.2

Deliverables:

- multi-subscriber event relay или broadcaster;
- подписка по `TaskId`;
- backpressure policy;
- cleanup подписчиков после terminal state.

Acceptance criteria:

- Telegram transport больше не является единственным consumer;
- несколько observers могут одновременно читать task events;
- late subscribers могут восстановить snapshot состояния через persistence.

Verification:

```bash
cargo test -p oxide-agent-runtime event_broadcaster
```

#### Slice 4.4 - Telegram Task Controls

Crates:

- `oxide-agent-transport-telegram`

Depends on:

- Slice 4.2
- Slice 4.3

Deliverables:

- UI controls для `cancel_task` и `stop_and_report`;
- live task status updates;
- terminal notifications для `Completed`, `Failed`, `Cancelled`, `Stopped`.

Acceptance criteria:

- пользователь видит управляемый lifecycle задачи;
- кнопки работают только для владельца задачи;
- transport UI не рассинхронизируется с runtime state.

Verification:

```bash
cargo test -p oxide-agent-transport-telegram task_controls
```

Exit criteria for Stage 4:

- задача управляется пользователем через runtime-backed controls;
- progress/event model пригоден для дополнительных consumers.

---

### 6. Stage 5 - Web monitoring (optional)

Цель stage: добавить read-only web access без влияния на core execution path.

#### Slice 5.1 - Web Access Contracts

Crates:

- `oxide-agent-runtime`

Depends on:

- Slice 4.3

Deliverables:

- short-lived task-scoped access token;
- read-only observer contract;
- API contract для snapshot + live events.

Acceptance criteria:

- web consumer не получает write access;
- токен нельзя заменить простым `TaskId`;
- runtime может отключить web integration без потери core functionality.

Verification:

```bash
cargo test -p oxide-agent-runtime web_contracts
```

#### Slice 5.2 - Web Monitor Module/Crate

Crates:

- новый web module или отдельный crate

Depends on:

- Slice 5.1

Deliverables:

- HTTP server;
- endpoint для task snapshot;
- live event streaming;
- token validation middleware.

Acceptance criteria:

- web UI работает как optional layer;
- падение web-модуля не рушит execution runtime;
- наблюдаемость read-only и безопасна по умолчанию.

Verification:

```bash
cargo test -p oxide-agent-runtime
# или для отдельного крейта:
# cargo test -p oxide-agent-web-monitor
```

Exit criteria for Stage 5:

- есть безопасный optional web observer path поверх runtime events.

---

### 7. Stage 6 - Integration, guards и rollout

Цель stage: довести систему до production-shaped состояния и аккуратно включить для пользователей.

#### Slice 6.1 - Architect Integration in Telegram

Crates:

- `oxide-agent-transport-telegram`

Depends on:

- Slice 4.4

Deliverables:

- user-facing flow для создания task;
- различение sync и async сценариев;
- финальная доставка результата пользователю.

Acceptance criteria:

- long-running requests могут перейти в background mode;
- пользователь получает понятный feedback о создании и завершении task;
- ошибки surfaced без потери task identity.

Verification:

```bash
cargo test -p oxide-agent-transport-telegram task_background_flow
```

#### Slice 6.2 - Delegation Depth Enforcement

Crates:

- `oxide-agent-core`
- `oxide-agent-runtime`

Depends on:

- Slice 2.2

Deliverables:

- явный depth tracking;
- runtime/tool-level запрет делегации глубже 2;
- диагностические ошибки для forbidden delegation.

Acceptance criteria:

- саб-агент не может породить новый саб-агент;
- depth policy проверяется кодом, а не только инструкциями в prompt;
- нарушения политики явно логируются.

Verification:

```bash
cargo test -p oxide-agent-core delegation_depth
cargo test -p oxide-agent-runtime delegation_depth
```

#### Slice 6.3 - Rate Limiting Strategy

Crates:

- `oxide-agent-runtime`
- `oxide-agent-core`

Depends on:

- Slice 2.1

Deliverables:

- стратегия ограничения конкурентных LLM вызовов;
- приоритет user-facing vs background traffic;
- конфигурируемые лимиты и базовая телеметрия.

Acceptance criteria:

- background tasks не могут полностью съесть capacity user-facing path;
- лимиты конфигурируются без перекомпиляции;
- saturation не приводит к silent failure.

Verification:

```bash
cargo test -p oxide-agent-runtime rate_limiting
```

#### Slice 6.4 - Feature Flags и Rollout Safety

Crates:

- все затронутые crates

Depends on:

- все предыдущие slices

Deliverables:

- feature flags или staged enablement;
- rollout checklist;
- rollback procedure;
- документация по observability и support playbook.

Acceptance criteria:

- Agent Mode v2 можно включать поэтапно;
- rollback не требует ручного восстановления данных;
- production rollout имеет явные guardrails.

Verification:

```bash
cargo test --all-features
cargo clippy --all-targets --all-features
cargo fmt --check
```

Exit criteria for Stage 6:

- система готова к staged rollout и supportable в эксплуатации.

---

### 8. Dependency summary

Критический путь:

1. `1.1 -> 1.2 -> 1.3`
2. `1.3 -> 2.1 -> 2.2 -> 2.3 -> 2.4`
3. `1.1 -> 3.1` и `2.2 -> 3.2 -> 3.3 -> 3.4`
4. `1.4 -> 4.3 -> 4.4`
5. `4.3 -> 5.1 -> 5.2`
6. Stage 6 начинается только после рабочих foundation/runtime flows.

Наиболее чувствительные blockers:

- storage schema design для task persistence;
- корректное разделение `TaskId` и `SessionId`;
- race conditions в cancel/resume/stop flows;
- backpressure и cleanup в event fan-out;
- priority control для user-facing и background LLM traffic.

---

### 9. Recommended review gates

Каждый slice должен пройти отдельный review gate:

- domain correctness;
- state transition safety;
- concurrency safety;
- recovery semantics;
- transport identity/security checks;
- tests for touched scope.

Дополнительно по stage:

- после Stage 2 нужен integration review detached execution;
- после Stage 3 нужен E2E review HITL cycle;
- после Stage 4 нужен review cancel/stop/event fan-out semantics;
- перед rollout нужен production-readiness review.

---

### 10. Minimal definition of done

Agent Mode v2 можно считать внедренным только если выполнены следующие условия:

- long-running task имеет persisted `TaskId` и runtime-owned lifecycle;
- detached background execution работает независимо от transport handler path;
- `cancel_task` и `stop_and_report` различаются и надежно работают;
- HITL pause/resume переживает рестарт процесса;
- delegation depth ограничен кодом;
- progress/task events доступны более чем одному consumer или через явный relay;
- rollout управляется через feature flags или staged enablement.
