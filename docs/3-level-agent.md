### RFC: Эволюция к трёхуровневой системе агентов (Agent Mode v2)

Статус: Draft

Цель этого RFC - описать реалистичный путь от текущей архитектуры Oxide Agent к трёхуровневой модели выполнения задач:

1. Архитектор (user-facing агент в Telegram)
2. Исполнитель (background worker для длительной задачи)
3. Саб-агент (узкоспециализированный дочерний исполнитель)

Документ не утверждает, что эта архитектура уже существует. На момент написания в кодовой базе есть только часть необходимых примитивов: session registry, cancellation tokens, loop detection, progress events и синхронная delegation в sub-agent. Detached workers, task state machine, task persistence, Telegram poll flow и web monitoring еще не реализованы.

---

### 1. Зачем нужен Agent Mode v2

Текущая модель хорошо подходит для коротких и средних запросов, но имеет ограничения для длительных сценариев:

- долгие задачи блокируют пользовательский сценарий сильнее, чем хотелось бы;
- sub-agent delegation выполняется синхронно внутри родительского исполнения;
- нет отдельной сущности Task с собственным жизненным циклом;
- нет персистентного восстановления фоновой задачи после рестарта;
- нет нативного механизма Human-in-the-Loop для долгих задач;
- progress stream ориентирован на одного потребителя, а не на Telegram + Web одновременно.

Цель Agent Mode v2 - добавить фоновые задачи без разрушения текущих контрактов session/runtime/transport.

---

### 2. Что есть в системе сейчас

#### 2.1 Уже существует

- `SessionRegistry` и session-scoped cancellation через `CancellationToken`.
- Loop detection на уровне runner с жесткой остановкой исполнения при детекте цикла.
- Progress events и runtime для доставки прогресса в transport.
- Sub-agent delegation с защитой от рекурсивной делегации инструментов.
- Базовый storage для agent memory и user config.

#### 2.2 Чего пока нет

- отдельного `TaskId`, не зависящего от `SessionId`;
- state machine для фоновой задачи (`Pending`, `Running`, `WaitingInput`, `Completed`, `Failed`, `Cancelled`);
- detached background worker manager;
- persistence schema для task state и scratchpad;
- event fan-out для нескольких подписчиков;
- transport flow для Telegram polls;
- graceful stop с итоговым отчетом;
- web monitoring crate или встроенного web-модуля.

Это важное ограничение: дальнейшее описание ниже - целевая архитектура и rollout plan, а не описание уже существующей реализации.

---

### 3. Термины и сущности

#### 3.1 Session

Пользовательская сессия в runtime. Уже существует.

#### 3.2 Task

Новая сущность, которую нужно ввести. Task представляет длительную работу, которая может жить дольше одного активного пользовательского запроса.

Минимальные поля:

- `task_id: Uuid`
- `session_id`
- `user_id`
- `parent_task_id: Option<Uuid>`
- `state: TaskState`
- `created_at`
- `updated_at`
- `last_error: Option<String>`
- `retry_count: u32`
- `awaiting_input: Option<PendingInput>`

#### 3.3 Архитектор

User-facing агент, который принимает запрос пользователя, принимает решение о запуске фоновой задачи, показывает статус и доставляет итог пользователю.

#### 3.4 Исполнитель

Detached background worker, который исполняет длительную задачу и имеет право порождать только один следующий уровень - саб-агентов.

#### 3.5 Саб-агент

Ограниченный исполнитель атомарной подзадачи. Не имеет права делегировать дальше.

---

### 4. Инварианты архитектуры

Следующие инварианты обязательны. Если реализация им не соответствует, она не считается Agent Mode v2.

#### 4.1 `TaskId` отделен от `SessionId`

Нельзя строить long-running execution только вокруг session identity. Фоновая задача должна иметь собственный идентификатор, собственное состояние и собственную persistence model.

#### 4.2 Максимальная глубина делегации равна 2

Допустимая цепочка:

- Архитектор -> Исполнитель
- Исполнитель -> Саб-агент

Запрещено:

- Саб-агент -> Саб-агент
- Саб-агент -> Исполнитель
- любая рекурсивная делегация глубже 2

Это должно валидироваться не только промптом, но и runtime/tool policy.

#### 4.3 Loop detection остается жесткой остановкой

В текущей системе loop detection уже существует и при срабатывании отменяет выполнение. В этом RFC loop detection не считается механизмом автоматического recovery.

Следствие: никаких неявных `auto-retry`, `max retries = 3`, `resume after compaction` в базовой версии Agent Mode v2 нет.

Если retry понадобится позже, это оформляется отдельным RFC после появления task persistence и restart-safe execution.

#### 4.4 Cancellation и Graceful Stop - разные режимы

- `cancel_task(task_id)` - немедленная отмена задачи.
- `stop_and_report(task_id)` - мягкая остановка на безопасной точке с последующей саммаризацией накопленного состояния.

Они не должны делить одну и ту же семантику.

#### 4.5 Один источник истины для task state

Состояние задачи должно читаться и восстанавливаться из персистентного состояния, а не из случайных in-memory структур transport layer.

#### 4.6 Event delivery должен поддерживать fan-out

Если у задачи есть несколько подписчиков (Telegram UI, web UI, логгер), event stream должен либо поддерживать broadcast/fan-out, либо иметь явный relay layer. Single-consumer `mpsc` недостаточен для финальной архитектуры.

#### 4.7 HITL требует явного состояния `WaitingInput`

Нельзя моделировать ожидание ответа пользователя как "задача просто зависла". Это отдельное состояние state machine, которое должно переживать рестарт процесса.

---

### 5. Целевая архитектура

### 5.1 Уровень 1: Архитектор

Архитектор остается user-facing агентом в Telegram и получает новые обязанности:

- принять запрос пользователя;
- решить, требуется ли background execution;
- создать `TaskId` и зарегистрировать задачу;
- запустить Исполнителя через runtime background manager;
- принимать task events и обновлять пользователя;
- доставлять пользователю запросы HITL;
- передавать `cancel_task`, `stop_and_report`, `resume_task` в runtime.

Архитектор не исполняет длительную задачу сам. Его ответственность - orchestration и user communication.

### 5.2 Уровень 2: Исполнитель

Исполнитель - это отдельная фоновая задача runtime, привязанная к `TaskId`.

Свойства Исполнителя:

- запускается через `tokio::spawn` или эквивалентный background manager;
- имеет собственный task state;
- умеет сохранять checkpoint state после успешных шагов;
- умеет переходить в `WaitingInput`;
- умеет исполнять `stop_and_report`;
- имеет право запускать саб-агентов.

Важно: это целевое состояние. Текущая синхронная delegation не является таким Исполнителем.

### 5.3 Уровень 3: Саб-агент

Саб-агент решает ограниченную подзадачу и наследует cancellation context родительского Исполнителя.

Свойства:

- не может порождать следующий уровень агентов;
- работает внутри budget и timeout, заданных Исполнителем;
- возвращает строго типизированный результат или ошибку;
- может иметь локальный compaction, но не меняет state machine родительской задачи напрямую.

---

### 6. Task State Machine

Базовая state machine:

- `Pending` - задача создана, но worker еще не стартовал;
- `Running` - задача выполняется;
- `WaitingInput` - задача приостановлена и ждет пользовательский ответ;
- `Completed` - задача завершилась успешно;
- `Failed` - задача завершилась ошибкой;
- `Cancelled` - задача принудительно отменена;
- `Stopped` - задача мягко остановлена и завершена отчетом.

Разрешенные переходы:

- `Pending -> Running`
- `Running -> WaitingInput`
- `WaitingInput -> Running`
- `Running -> Completed`
- `Running -> Failed`
- `Running -> Cancelled`
- `WaitingInput -> Cancelled`
- `Running -> Stopped`

Не допускается:

- возврат из terminal state в non-terminal без отдельного механизма resume-from-checkpoint;
- неявный переход в `WaitingInput` без persisted `PendingInput` payload;
- неявный `Completed` после `Cancelled`.

---

### 7. Persistence contract

Для Agent Mode v2 требуется новый storage contract. Текущий storage для памяти агента недостаточен.

Минимальные операции:

- `save_task_state(task_id, snapshot)`
- `load_task_state(task_id)`
- `list_tasks_by_session(session_id)`
- `delete_task_state(task_id)`
- `append_task_event(task_id, event)` или эквивалентный event log

Минимальный snapshot должен содержать:

- metadata задачи;
- текущий `TaskState`;
- scratchpad/checkpoint данных;
- pending HITL payload;
- summary последнего безопасного checkpoint;
- информацию о дочерних задачах, если они есть.

Это не обязательно должен быть R2-only дизайн, но persistence обязан переживать рестарт процесса.

---

### 8. Event model

В базовой реализации Agent Mode v2 события задачи должны быть отделены от UI transport layer.

Минимальные типы событий:

- `TaskCreated`
- `TaskStarted`
- `TaskProgress`
- `TaskWaitingInput`
- `TaskResumed`
- `TaskCompleted`
- `TaskFailed`
- `TaskCancelled`
- `TaskStopped`

Требования:

- transport не должен быть единственным владельцем progress stream;
- событие должно быть привязано к `TaskId`;
- опоздавший consumer должен иметь возможность хотя бы частично восстановить состояние через persistence layer;
- backpressure policy должна быть описана явно.

В первой версии допустим relay layer поверх существующего progress runtime. Полный broadcast refactor не обязателен в том же этапе, если fan-out обеспечен иначе.

---

### 9. Human-in-the-Loop через Telegram

HITL вводится только после появления `TaskId`, `TaskState`, persistence и resume semantics.

#### 9.1 Новая команда/инструмент runtime

Исполнитель должен уметь сформировать запрос на ввод пользователя:

```json
{
  "kind": "poll",
  "question": "Какие логи собрать?",
  "options": ["System", "App", "DB"],
  "multi_select": true,
  "min_choices": 1,
  "max_choices": 3
}
```

После этого task обязан перейти в `WaitingInput`, а payload должен быть сохранен в persistence.

#### 9.2 Telegram transport requirements

Transport layer должен:

- отправить `sendPoll` или другой поддерживаемый Telegram UX;
- сохранить mapping `poll_id -> task_id`;
- валидировать, что ответ пришел от ожидаемого пользователя;
- предотвратить двойной resume одной и той же задачи;
- уметь закрыть poll после получения валидного ответа.

#### 9.3 Что не допускается

- хранить единственную копию mapping только в volatile memory без плана на рестарт;
- возобновлять задачу без проверки user identity;
- трактовать отсутствие ответа как silent failure.

---

### 10. Cancellation и Graceful Stop

#### 10.1 `cancel_task(task_id)`

Немедленно переводит задачу в terminal state `Cancelled` и инициирует каскадную отмену дочерних исполнений.

Гарантии:

- после завершения cleanup новые tool calls не стартуют;
- дочерние саб-агенты получают cancel;
- transport получает terminal event.

#### 10.2 `stop_and_report(task_id)`

Переводит задачу в режим мягкой остановки.

Ожидаемое поведение:

- worker замечает сигнал на безопасной точке;
- прекращает дальнейшее планирование;
- формирует summary из накопленного checkpoint/scratchpad;
- переводит задачу в `Stopped`;
- передает отчет Архитектору.

Этот режим нельзя строить только на `CancellationToken`. Нужен отдельный сигнал или отдельная команда в state machine.

---

### 11. Наблюдаемость и web monitoring

Web monitoring не является prerequisite для Agent Mode v2 core, но может быть добавлен после появления task identity, persistence и event fan-out.

Требования к web monitoring:

- доступ по короткоживущему токену, а не по открытому `task_id`;
- явная авторизация пользователя;
- read-only доступ к progress/event stream;
- отсутствие зависимости core execution от web-сервера.

На первом этапе допустимо отложить отдельный crate и реализовать только transport/runtime contracts, необходимые для будущего web UI.

---

### 12. Rate limiting

Параллельные фоновые задачи увеличат нагрузку на LLM providers.

Этот RFC фиксирует только требование: до широкого включения parallel background execution должна существовать стратегия ограничения конкурентных LLM вызовов.

Возможные варианты:

- глобальный semaphore;
- per-provider budget;
- приоритетные очереди для user-facing и background traffic.

Конкретная стратегия не считается частью первой фазы, но отсутствие такой стратегии блокирует rollout массового parallel execution.

---

### 13. Rollout plan

#### Фаза 1: Task identity и state contract

Crates:

- `oxide-agent-core`
- `oxide-agent-runtime`

Deliverables:

- `TaskId`, `TaskState`, `TaskMetadata`;
- storage contract для task persistence;
- runtime registry для задач;
- базовые task events.

Exit criteria:

- runtime умеет создать задачу, прочитать ее состояние и завершить ее без Telegram-specific логики.

#### Фаза 2: Background execution

Crates:

- `oxide-agent-runtime`
- `oxide-agent-core`

Deliverables:

- background worker manager;
- detached execution для длительных задач;
- каскадная отмена по `TaskId`;
- checkpoint persistence на безопасных шагах.

Exit criteria:

- задача переживает временную потерю transport interaction и продолжает жить как runtime entity.

#### Фаза 3: HITL и resume flow

Crates:

- `oxide-agent-transport-telegram`
- `oxide-agent-runtime`
- `oxide-agent-core`

Deliverables:

- `WaitingInput` state;
- poll/request mapping;
- resume flow;
- user identity validation.

Exit criteria:

- задача может надежно перейти в `WaitingInput`, получить ответ и вернуться в `Running`.

#### Фаза 4: Graceful stop и observability

Crates:

- `oxide-agent-runtime`
- `oxide-agent-transport-telegram`

Deliverables:

- `stop_and_report`;
- terminal summary for stopped tasks;
- transport-visible task controls;
- event relay/fan-out для нескольких consumers.

Exit criteria:

- пользователь может безопасно остановить долгую задачу и получить частичный отчет.

#### Фаза 5: Web monitoring (optional)

Crates:

- `oxide-agent-runtime`
- отдельный web module/crate при необходимости

Deliverables:

- token-based web access;
- live event streaming;
- read-only task view.

Exit criteria:

- web UI не влияет на корректность основного runtime пути и может быть отключен без деградации core execution.

---

### 14. Что явно не входит в этот RFC

- автоматический recovery после loop detection;
- неограниченное параллельное исполнение задач на одну сессию;
- распределенное исполнение по нескольким процессам или нодам;
- полноценный workflow engine;
- гарантии exactly-once delivery для UI events.

---

### 15. Критерии готовности Agent Mode v2

Agent Mode v2 можно считать внедренным только если выполнены все условия:

- long-running task имеет собственный `TaskId` и persisted state;
- background worker отделен от user-facing request flow;
- task lifecycle наблюдаем и управляем через runtime;
- саб-агенты не могут делегировать дальше;
- HITL переживает рестарт процесса без потери контекста;
- cancel и graceful stop имеют разные семантики;
- transport не является единственным хранилищем истины о состоянии задачи.

До этого момента любые отдельные элементы (polls, web UI, summary hooks) считаются промежуточными фичами, а не полной реализацией Agent Mode v2.
