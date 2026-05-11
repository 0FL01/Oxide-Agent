# PRD: Deep Refactor Program for `Oxide-Agent` (`feature/memento-mori`)

## Context

Ветка уже оформлена как workspace из нескольких crates, а README декларирует модульную архитектуру, transport-agnostic runtime, topic-scoped infrastructure, manager control plane и sandbox broker. Но фактический центр тяжести всё ещё в `oxide-agent-core`: crate экспортирует одновременно `agent`, `config`, `llm`, `sandbox`, `storage`, `utils` и `testing`, а его зависимости включают и инфраструктурные вещи вроде S3/AWS SDK, и LLM SDK. Это означает, что `core` сейчас играет роль внутреннего интеграционного монолита, хотя внешне репозиторий выглядит модульным. ([GitHub][1])

Самый токсичный узел текущей архитектуры — `StorageProvider`. Он покрывает user config, chat history, agent memory, flow state, persistent memory, embeddings, artifacts, profiles, reminders и topic control plane. При этом часть методов дефолтно возвращает “not implemented”, то есть отсутствие capability обнаруживается поздно, уже в рантайме. На этом фоне `oxide-agent-memory` уже содержит более здоровую и узкую абстракцию `MemoryRepository`, что показывает правильное направление для декомпозиции storage-слоя. ([GitHub][2])

Execution-поведение размазано между `AgentExecutor`, `AgentRunner` и `tool_bridge`. `AgentExecutor` одновременно обрабатывает обычный запуск, resume после approval, resume после user input и continuation после runtime context, а также участвует в topic infra preflight и persistent-memory retrieval. `AgentRunner` отвечает за hooks, compaction, loop detection, timeout/iteration management и основной LLM loop. `tool_bridge` отдельно занимается вызовом tools, нормализацией результатов, timeout-обёртками и memory checkpoint side effects. Это слишком много причин для изменения на один execution pipeline. ([GitHub][3])

Tool subsystem уже богатый по набору провайдеров, но архитектурно хрупкий. `ToolProvider` опирается на строковые `tool_name`, `ToolRegistry` ищет обработчик линейным сканированием провайдеров, а `ToolAccessPolicy` хранит allow/block списки как `HashSet<String>`. Пока tool identity живёт в строках, любой rename имени инструмента остаётся потенциальным policy/security change. ([GitHub][4])

Topic control plane тоже размазан поперёк слоёв. В storage уже есть типизированные records для topic context, topic AGENTS.md, topic bindings, audit events и infra-mode настроек; в providers есть `ManagerControlPlaneProvider` и `AgentsMdProvider`; в Telegram transport лежат `manager_topic_lifecycle.rs` и `topic_route.rs`. Из этого следует, что один бизнес-контекст сейчас распределён между storage, tools, execution и transport. ([GitHub][5])

При этом у репозитория уже есть хороший тестовый шов: `oxide-agent-runtime` задуман как transport-agnostic runtime, а `oxide-agent-transport-web` специально описан как harness для E2E, benchmarks и deterministic tooling с timeline/events API. Именно он должен стать базой для characterization tests перед любым глубоким переносом логики. ([GitHub][6])

---

## Goal

Снизить архитектурный техдолг без пользовательского регресса за счёт пяти целевых изменений:

1. Разрезать storage на узкие capability interfaces.
2. Формализовать execution как явную state machine.
3. Вынести topic control plane в отдельный bounded context.
4. Сделать tools policy-safe через стабильный `ToolId` и capability model.
5. Превратить Telegram-слой в адаптер и composition root, а не в место, где живёт бизнес-логика.

Главный ожидаемый результат — чтобы новые фичи перестали требовать правки “чуть-чуть везде” и начали добавляться в один явно определённый контекст.

## Non-Goals

* Не расширять продуктовый функционал в рамках этой программы, кроме стабильности, наблюдаемости и упрощения сопровождения.
* Не менять transport-web API в фазах 0–7: текущие session/task/progress/events/timeline endpoints считаются стабильным тестовым швом. ([GitHub][7])
* Не менять пользовательскую семантику Telegram-потока в ранних фазах: бот по-прежнему стартует через `run_bot(settings)`, а reminder/startup maintenance должны сохранить текущее поведение до выделения сервисных границ. ([GitHub][8])
* Не делать “true parallel tool execution” в рамках основного рефакторинга. README говорит о parallel tool execution, но текущий `tool_bridge` выполняет tool calls последовательным циклом; на время программы источником истины считается текущее runtime-поведение, а не маркетинговое описание. ([GitHub][9])
* Не удалять `StorageProvider` флаг-днём в фазе 1; сначала нужен compatibility layer, иначе миграция разорвёт слишком много call sites одновременно. ([GitHub][2])

---

## Success Metrics

Успех программы измеряется не “красотой кода”, а конкретными эффектами:

* Все текущие CI-команды остаются зелёными на каждой фазе: `cargo fmt --all -- --check`, `cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace`. В репозитории уже есть и отдельная тяжёлая validation-проверка для Telegram integration test в ignored-режиме. ([GitHub][10])
* До начала crate extraction появляется characterization suite на базе `transport-web`, покрывающий new task, tool calls, timeline milestones, cancellation, compaction-related переходы и все public resume entrypoints executor’а. `transport-web` уже предоставляет подходящие timeline/events surfaces, а `AgentExecutor` уже имеет четыре явные точки входа для normal/approval/user-input/runtime-context сценариев. ([GitHub][7])
* После фазы 1 production-код больше не должен зависеть от “широкого” storage API там, где достаточно одного узкого capability trait.
* После фазы 3 pause/resume semantics должны быть выражены как явные state transitions, а не как скрытые боковые ветки в executor/runner/bridge.
* После фазы 6 Telegram transport не должен содержать topic business rules и прямые storage orchestration paths.
* После фазы 8 строковые allow/block tool checks и broad legacy storage usage либо полностью удалены, либо локализованы только внутри compatibility shim.

---

## Public API / Behavior Invariants

* Публичные входы `AgentExecutor::execute`, `resume_ssh_approval`, `resume_after_user_input` и `continue_after_runtime_context` сохраняются до финальной cleanup-фазы, даже если под капотом они станут thin wrappers над state machine. ([GitHub][3])
* `transport-web` сохраняет текущую поверхность session/task/progress/events/timeline/cancel API на протяжении всей программы, потому что она нужна как эталонный тестовый контур. ([GitHub][7])
* Имена инструментов, которые уже участвуют в policy и profile-конфигурации, должны оставаться обратнос совместимыми через alias map до завершения миграции на `ToolId`, потому что текущая policy-модель работает на строках и env vars вроде `DM_ALLOWED_TOOLS`/`DM_BLOCKED_TOOLS`. ([GitHub][11])
* Topic control plane record shapes не должны разрушительно переписываться в ранних фазах; существующие records уже несут `schema_version` и `version`, что позволяет делать additive migration path. ([GitHub][5])
* Memory subsystem не должен “раствориться обратно” в generic storage abstraction: `oxide-agent-memory` уже имеет собственный typed repository surface и должен остаться специализированным контекстом. ([GitHub][12])

---

## Validation Commands

CI уже валидирует проект стандартным Rust-набором. Для этой программы они становятся обязательным gate на каждом phase boundary. ([GitHub][10])

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo test -p oxide-agent-telegram-bot --test integration_validation -- --ignored --nocapture
```

Дополнительно для refactor-программы вводится обязательный characterization suite на базе `oxide-agent-transport-web`, использующий timeline/events endpoints и deterministic/scripted LLM tooling как эталон поведенческой совместимости. ([GitHub][7])

---

## Target Architecture

Целевая форма системы — не “разнести всё по папкам”, а закрепить compile-time boundaries.

* `oxide-agent-storage-api`: только узкие storage traits.
* `oxide-agent-storage-r2`: concrete storage/backing implementation.
* `oxide-agent-execution`: state machine, pause/resume orchestration, hooks/compaction coordination.
* `oxide-agent-tools`: `ToolId`, capabilities, registry, provider contracts.
* `oxide-agent-topic-control-plane`: topic context, topic AGENTS.md, infra config, bindings, audit, lifecycle orchestration.
* `oxide-agent-memory`: остаётся отдельным специализированным контекстом.
* `oxide-agent-runtime`: остаётся transport-agnostic coordinator.
* `transport-telegram`, `transport-web`, `telegram-bot`, `sandboxd`: только адаптеры и entrypoints.

Такой разрез опирается на уже существующее разделение workspace, на transport-agnostic runtime и на то, что memory crate уже отделён от остального storage мира. ([GitHub][1])

---

## Implementation Phases

Порядок ниже выбран специально: сначала фиксируем текущее поведение через `transport-web`, потом дробим интерфейсы, и только после этого выносим логику в отдельные crates. Иначе получится distributed monolith с теми же зависимостями, только растянутыми по workspace. ([GitHub][7])

| Phase | Name                                      | Status  | Depends On | Goal                                                                 |
| ----: | ----------------------------------------- | ------- | ---------- | -------------------------------------------------------------------- |
|     0 | Baseline normalization & characterization | pending | -          | Зафиксировать текущее поведение и сделать большие diff’ы безопасными |
|     1 | Storage capability split                  | pending | 0          | Убрать god-interface и заменить его набором узких контрактов         |
|     2 | Tool identity & policy normalization      | pending | 0          | Ввести стабильный `ToolId`, capabilities и безопасную policy-модель  |
|     3 | Execution state machine                   | pending | 0, 1, 2    | Сделать pause/resume/iteration/tool lifecycle явным                  |
|     4 | Topic control plane extraction            | pending | 1, 2       | Собрать topic business logic в один сервисный контекст               |
|     5 | Crate extraction & import boundaries      | pending | 1, 2, 3, 4 | Закрепить новые границы на уровне workspace                          |
|     6 | Telegram transport thinning               | pending | 5          | Свести Telegram к adapter/composition-root роли                      |
|     7 | Durable side effects & recovery           | pending | 3, 5       | Сделать checkpoint/finalize/cleanup crash-safe                       |
|     8 | Legacy path removal & hardening           | pending | 6, 7       | Удалить совместимые шины и мёртвые ветки                             |

---

## Phase 0 — Baseline normalization & characterization

Эта фаза касается самых горячих execution/bootstrap файлов: `agent/executor/execution.rs`, `agent/runner/execution.rs`, `agent/tool_bridge.rs`, а также Telegram bootstrap и `transport-web` как поведенческого harness. Сейчас именно эти части определяют основную runtime-семантику задачи, tools, hooks и старта приложения. ([GitHub][3])

**Deliverables**

* Механический PR с нормализацией форматирования и физической структуры файлов без изменения поведения.
* Characterization tests на базе `transport-web`:

  * normal execution;
  * multiple tool calls;
  * cancellation;
  * timeout/iteration stop;
  * compaction path;
  * `resume_ssh_approval`;
  * `resume_after_user_input`;
  * `continue_after_runtime_context`.
* Architectural dependency checks: запрет новых прямых зависимостей transport → storage implementation и domain → concrete infra.
* ADR-0 с фиксацией “current runtime behavior is oracle”.

**Acceptance Criteria**

* Diff этой фазы по смыслу no-op, кроме тестовой обвязки.
* Все CI-команды зелёные.
* Characterization suite ловит хотя бы один timeline/event regression при намеренной поломке.

**Мины**

* Нельзя смешивать reformat и semantic changes в одном PR.
* Нельзя писать golden tests поверх README-ожиданий; oracle — только реальный runtime code path.

---

## Phase 1 — Storage capability split

`StorageProvider` сегодня совмещает user config, history, flows, reminders, profile policy, artifacts, embeddings, topic context и другие вещи, а storage-модуль уже разросся на набор файлов вроде `control_plane.rs`, `flows.rs`, `persistent_memory.rs`, `reminder.rs`, `user.rs`, `r2.rs` и сопутствующие реализации. Это явный признак того, что единая абстракция больше не отражает реальные bounded contexts. ([GitHub][2])

**Deliverables**

Вводятся новые traits:

* `ChatHistoryStore`
* `UserConfigStore`
* `FlowStore`
* `ReminderStore`
* `ProfileStore`
* `ArtifactStore`
* `ControlPlaneStore`
* `TopicBindingStore` или как часть control-plane API
* `CheckpointStore` для execution-related persistence
* `MemoryRepository` используется напрямую там, где речь о persistent memory, без возврата к broad storage

Плюс создаётся:

* `LegacyStorageProviderAdapter`, который реализует новые traits через старый `StorageProvider`;
* capability validation на bootstrap;
* минимальный `StorageFacade` только для composition root, если нужен transitional агрегат.

**Acceptance Criteria**

* Новый production-код больше не принимает `Arc<dyn StorageProvider>`, если ему нужен один конкретный capability trait.
* Default “not implemented” методы не участвуют в happy-path доменной логике.
* Missing capability валится на startup/assembly, а не в середине user flow.

**Мины**

* Нельзя тащить persistent memory назад в generic storage; используем уже существующий `MemoryRepository` как отдельную ось дизайна. ([GitHub][12])
* Нельзя делать “один новый trait на всё”: это просто переименование старой проблемы.
* На transitional этапе придётся терпеть двойной слой абстракций; это нормально и дешевле, чем flag-day migration.

---

## Phase 2 — Tool identity & policy normalization

Сейчас `ToolProvider` определяет обработку по строковому имени, `ToolRegistry` пробегает массив провайдеров в поиске первого `can_handle`, а policy-профили работают со строковыми allow/block списками. При большом количестве providers это создаёт и избыточную связанность, и policy-риски. ([GitHub][4])

**Deliverables**

* `ToolId` как стабильный newtype.
* `ToolCapability` и `ToolScope`, например:

  * `filesystem`
  * `network`
  * `topic_control_plane`
  * `sandbox`
  * `search`
  * `memory`
  * `reminder`
  * `media`
  * `delegation`
* Новый registry index: `ToolId -> HandlerBinding`.
* Alias map: legacy string name → canonical `ToolId`.
* Collision detector на bootstrap: один alias не может бесшумно указывать на два обработчика.
* Новая policy-модель:

  * allow/block по `ToolId`;
  * capability-level deny rules;
  * migration parser для старых string-based profile entries.

**Acceptance Criteria**

* В hot path нет линейного перебора провайдеров по строке имени.
* Ни один новый policy-check не использует raw string name напрямую.
* Rename tool name не меняет policy semantics без явного alias/migration шага.

**Мины**

* Старые профили и env-конфиги нельзя ломать сразу.
* Optional feature providers должны регистрировать capabilities одинаково строго, как и built-in инструменты.
* Нужно явно зафиксировать, что tool identity — это security boundary, а не косметический label.

---

## Phase 3 — Execution state machine

Сейчас execute/resume semantics распределены между `AgentExecutor`, `AgentRunner` и `tool_bridge`, а approval, user-input continuation, runtime context injection, loop detection, compaction и persistent-memory retrieval переплетены в одном execution потоке. Дополнительно в `tool_bridge` уже видны следы нестабильного approval-flow: есть `WaitingForApproval`, хранение pending approval и отдельные ветки с пометкой `[APPROVAL DISABLED]`. ([GitHub][3])

**Deliverables**

Вводится явная state model:

* `ExecutionRequest`
* `PreparedExecution`
* `ContextAugmentedExecution`
* `RunningIteration`
* `RunningTools`
* `Paused(ApprovalRequired | UserInputRequired | RuntimeContextPending)`
* `Compacting`
* `Finalizing`
* `Completed`
* `Cancelled`
* `Failed`

Плюс:

* thin wrappers в `AgentExecutor`, чтобы сохранить существующий public API;
* явные transition handlers;
* сериализуемый payload pause/resume;
* отдельный `ExecutionSemantics` документ, фиксирующий порядок:

  1. prepare
  2. context augmentation
  3. pre-LLM maintenance
  4. LLM iteration
  5. tool phase
  6. compaction/finalize
  7. complete/fail/cancel

**Acceptance Criteria**

* Все текущие entrypoints executor’а работают через единый state engine.
* Characterization tests подтверждают идентичный observable outcome для normal run и всех resume веток.
* Approval path либо восстановлен как поддерживаемый state, либо официально выведен из рантайма и удалён из dead-code хвостов.

**Мины**

* Здесь нельзя одновременно менять semantics и extraction boundaries: сначала state shell, потом вынос в crate.
* Нельзя “случайно улучшить” tool scheduling. До отдельного решения текущая последовательная обработка tool calls должна остаться как oracle. ([GitHub][13])
* Loop detection, timeout и compaction должны стать состояниями/переходами, а не скрытыми побочными проверками.

---

## Phase 4 — Topic control plane extraction

Topic control plane уже имеет собственные типы данных: `TopicContextRecord`, `TopicAgentsMdRecord`, `TopicBindingRecord`, `AuditEventRecord`, а также типы infra auth/tool modes. Одновременно lifecycle операции для manager topics и routing logic лежат в Telegram transport. Это прямой сигнал, что topic — отдельный бизнес-контекст, а не “ещё одна storage-табличка” или “пара tool handlers”. ([GitHub][5])

**Deliverables**

Новый сервисный контекст:

* `TopicContextService`
* `TopicAgentsMdService`
* `TopicInfraConfigService`
* `TopicBindingService`
* `TopicAuditService`
* `TopicLifecycleService`
* `TopicPreflightPlanner`

Adapters:

* `ManagerControlPlaneProvider` становится thin adapter.
* `AgentsMdProvider` становится thin adapter.
* Telegram `manager_topic_lifecycle` и `topic_route` переходят на вызов сервисов, а не на прямое знание storage/provider деталей.

**Acceptance Criteria**

* Все topic rules описаны в одном сервисном слое.
* Transport не принимает решений о topic infra/auth/tool policy.
* Topic record migrations additive и версионируемые, без destructive rewrite в early phases.

**Мины**

* Нужно один раз зафиксировать canonical identity mapping: `user_id`, `topic_id`, `chat_id`, `thread_id`, `session_id`.
* Нельзя дублировать audit writing в transport и service одновременно.
* Sandbox cleanup и topic lifecycle должны иметь одну точку оркестрации, иначе будут гонки и двойные side effects.

---

## Phase 5 — Crate extraction & import boundaries

После стабилизации interfaces можно закрепить архитектуру на уровне workspace. Сейчас workspace уже состоит из `core`, `memory`, `runtime`, `transport-web`, `transport-telegram`, `telegram-bot`, `sandboxd`, но `core` всё ещё несёт и domain, и infra, и execution orchestration. ([GitHub][1])

**Deliverables**

Новые crates:

* `oxide-agent-storage-api`
* `oxide-agent-storage-r2`
* `oxide-agent-execution`
* `oxide-agent-tools`
* `oxide-agent-topic-control-plane`

Возможные transitional решения:

* `oxide-agent-core` временно остаётся как compatibility facade/re-export crate;
* import lint rules не позволяют новым crates импортировать concrete infra друг друга напрямую;
* reverse dependencies проверяются в CI.

**Acceptance Criteria**

* У каждого нового crate есть чёткий owner-context.
* Нет обратных зависимостей `execution -> transport-*` или `domain/service -> storage-r2`.
* `core` либо заметно худеет до фасада, либо получает официальный план на исчезновение.

**Мины**

* Нельзя выносить код в crates до стабилизации интерфейсов — иначе получится просто логистический шум.
* Каждый extraction PR должен быть semantically boring: перенос + wiring, а не перенос + redesign + новые фичи.

---

## Phase 6 — Telegram transport thinning

`run_bot` сегодня поднимает storage, persistent memory store, LLM client, выполняет startup maintenance, запускает reminder scheduler и собирает handler tree. Отдельно в Telegram transport уже лежат `agent_transport.rs`, `manager_topic_lifecycle.rs`, `topic_route.rs` и другие модули. Это указывает, что transport layer слишком много знает о жизненном цикле системы. ([GitHub][8])

**Deliverables**

* `telegram-bot` становится composition root:

  * config loading;
  * logging;
  * wiring services;
  * process startup/shutdown.
* `transport-telegram` становится adapter layer:

  * inbound command mapping;
  * callback/query translation;
  * progress rendering;
  * auth/access checks;
  * transport-specific state handling.
* Topic business logic уходит в topic-control-plane service.
* Reminder/startup maintenance orchestration выносится в app bootstrap services.

**Acceptance Criteria**

* Telegram transport больше не знает concrete storage implementation details.
* Внутри transport нет direct business decisions по topics, tools policy и execution transitions.
* Поведение бота для пользователя не меняется.

**Мины**

* В этом слое легко случайно сломать progress UX и callback wiring.
* Reminder scheduler нельзя оставить “висящим” между transport и app bootstrap; у него должен быть один lifecycle owner.

---

## Phase 7 — Durable side effects & recovery

В `tool_bridge` есть fire-and-forget background checkpoint с явным комментарием про снижение latency, а approval flow частично отключён. Это типичный маркер temporal coupling: видимое состояние execution и фактическое состояние side effects могут расходиться при сбое или рестарте. ([GitHub][13])

**Deliverables**

* Durable outbox/event-log для критичных side effects:

  * checkpoint requested
  * checkpoint completed
  * compaction completed
  * memory finalize requested
  * reminder scheduled
  * sandbox cleanup requested
* Idempotent handlers для каждого side effect.
* Recovery worker, который может безопасно replay-ить невыполненные side effects после рестарта.
* Явное разделение:

  * execution state transition;
  * background best-effort follow-up.

**Acceptance Criteria**

* Сбой между tool execution и checkpoint/finalize не ломает консистентность сессии.
* Повторный запуск side-effect handler не приводит к двойной записи.
* Latency не ухудшается заметно на happy path за счёт выноса тяжёлых side effects в durable async pipeline.

**Мины**

* Самый большой риск этой фазы — незаметная latency/regression цена.
* Нельзя реализовывать durable side effects до state machine: иначе не будет надёжного источника истины для replay.

---

## Phase 8 — Legacy path removal & hardening

К этому моменту все compatibility layers уже отработали своё. Финальная фаза нужна не для “полировки”, а для того, чтобы техдолг не вернулся через старые входы.

**Deliverables**

* Удаление broad `StorageProvider` из production paths.
* Удаление string-first policy checks из production paths.
* Удаление dead approval code, если approval flow окончательно не поддерживается.
* Обновление README/архитектурной документации под реальное runtime-поведение.
* Жёсткие lint/dependency guards, чтобы новые широкие interfaces не появились снова.

**Acceptance Criteria**

* Legacy adapter либо удалён, либо изолирован только для временной backward-compat ветки.
* Нет новых import paths, которые пробивают compile-time boundaries.
* Документация совпадает с кодом, включая semantics tool execution и approval lifecycle.

**Мины**

* Документация уже сейчас расходится с кодом по tool execution semantics; этот хвост нельзя оставлять после завершения программы. ([GitHub][9])

---

## Cross-Cutting Migration Rules

### 1. Freeze current behavior before redesign

README заявляет parallel tool execution, а текущий runtime выполняет tool calls последовательно. Для refactor-программы это означает простое правило: сначала фиксируем фактическое поведение тестами, потом принимаем отдельное продуктово-техническое решение, хотим ли мы реально вводить параллелизм. ([GitHub][9])

### 2. Missing capability must fail at bootstrap

Пока в `StorageProvider` есть default “not implemented” paths, система уязвима к поздним runtime-failures. После phase 1 любые отсутствующие capability должны детектироваться в composition root при сборке приложения. ([GitHub][2])

### 3. Tool identity is a policy boundary

Поскольку текущий profile/access layer опирается на строковые имена, миграция на `ToolId` обязана идти через alias map, validator и collision detection. Иначе безобидный rename станет невидимым security change. ([GitHub][11])

### 4. Approval must be either restored or deleted

Существование `WaitingForApproval` рядом с `[APPROVAL DISABLED]` — плохое промежуточное состояние. После phase 3 должно быть только два варианта: поддерживаемый state machine path или полное удаление мёртвой ветки. ([GitHub][13])

### 5. Memory stays specialized

`oxide-agent-memory` уже оформлен как отдельный subsystem с typed repository abstractions и finalize/consolidation pipeline. Возвращать его обратно под generic storage umbrella было бы шагом назад. ([GitHub][12])

---

## Definition of Done

Рефакторинг считается завершённым, когда выполняются все условия ниже:

* execution lifecycle выражен как state machine, а public executor entrypoints только делегируют ей;
* topic context, AGENTS.md, infra config, bindings и audit живут в одном сервисном контексте;
* Telegram transport не содержит domain/business orchestration;
* production paths не зависят от broad `StorageProvider`;
* policy не зависит от сырых строк tool names;
* crash-sensitive side effects проходят через durable outbox/recovery path;
* characterization suite через `transport-web` и все CI-команды стабильно зелёные. ([GitHub][7])

---

## Самые опасные мины, которые нужно признать заранее

1. **Документация расходится с кодом.** До начала migration нужно решить, где источник истины: для рефакторинга — только код и characterization suite. ([GitHub][9])
2. **Approval flow уже в переходном состоянии.** Его нельзя оставлять “полуживым”, иначе state machine получится ложной. ([GitHub][13])
3. **String tool names — это скрытая security boundary.** Любой rename до введения `ToolId` опасен. ([GitHub][11])
4. **Storage missing capabilities сейчас ловятся слишком поздно.** Это источник неочевидных runtime падений. ([GitHub][2])
5. **Telegram bootstrap перегружен инфраструктурными обязанностями.** Его трогать раньше времени опасно; сначала нужны service boundaries. ([GitHub][8])
6. **Fire-and-forget checkpoint скрывает temporal coupling.** Переход к durable side effects обязателен, но только после formal execution state model. ([GitHub][13])

