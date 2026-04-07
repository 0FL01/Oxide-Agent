# Memory / RAG Plan for Oxide Agent

## Goal

Сделать память агента полезной для long-running assistant workflows без превращения истории чатов в шумный архив.

Ключевая идея:
- не использовать один общий RAG по всем старым сообщениям;
- разделить память на типы;
- использовать hybrid retrieval: lexical + semantic + fusion;
- хранить raw history отдельно от индексируемой памяти;
- держать hot context маленьким и агрессивно чистить его.

## Current Status In Code

### Implemented
- `compress` tool exists and is registered in the main agent registry.
- `compress` is handled by the runner and blocked for sub-agents.
- `HotContextHealthHook` is implemented and wired into agent execution.
- Soft/hard hot-context limits are config-backed (`soft_warning_tokens`, `hard_compaction_tokens`).
- Transient warning injection exists and does not persist into agent memory.
- Typed long-term memory exists in `oxide-agent-memory` (`threads`, `episodes`, `memories`, `session_state`).
- Real memory scope plumbing is in place (`user_id`, `context_key`, `flow_id`).
- Durable PostRun write path exists: `EpisodeRecord` + `SessionStateRecord` + thread metadata.
- Archive/blob persistence is wired through the existing storage/R2 path.
- Conservative reusable-memory extraction is implemented without embeddings.
- PG backend skeleton exists for memory write path.
- Core integration tests cover final response, archive refs, scope isolation, sub-agent no-write, and waiting-state finalization.

### Not Implemented Yet
- Memory write tools.
- Hybrid retrieval pipeline (lexical + vector fusion, без rerank).
- Query router for deciding when retrieval is needed.
- pgvector / semantic retrieval.
- Background consolidation / dedup / TTL / decay.
- Higher-signal extraction beyond the conservative Stage 5 baseline.

---

## Decision

Для Oxide Agent принимаем упрощённый стек:

- **Hybrid RAG**
  - full-text / lexical search
  - embeddings / vector search
  - fusion lexical/vector кандидатов без отдельного reranker-а

- **Typed Memory**
  - `working memory`
  - `episodic memory`
  - `semantic/procedural memory`

- **Storage split**
  - **Postgres** — metadata, thread / episode / memory records, retrieval index
  - **Postgres full-text search** — lexical retrieval
  - **pgvector** — vector retrieval
  - **R2** — cold archive для raw chat history, tool traces, больших payloads и artefacts

- **Extensibility**
  - retrieval должен идти через abstraction layer;
  - в будущем можно подключить отдельный search engine без смены memory model.

---

## Why this choice

### Почему не плодим больше зависимостей
На старте не нужен отдельный search engine.

Причины:
- меньше operational complexity;
- проще backup / migration;
- проще отлаживать consistency между raw archive и индексом;
- достаточно для первого production-grade memory layer.

### Почему не чистый vector RAG
Плохо работает на:
- exact match по error codes, env vars, file paths, topic names, issue ids;
- retrieval по tool-heavy history;
- поиске по старым execution traces.

### Почему не чистый vectorless RAG
Плохо работает на:
- переформулированных запросах;
- поиске “по смыслу” в старых чатах;
- извлечении похожего опыта, когда формулировки отличаются.

### Почему hybrid
Hybrid retrieval даёт лучшее покрытие:
- lexical ловит точные сущности;
- vector ловит semantic similarity;
- fusion объединяет оба сигнала без отдельного latency/cost layer.

---

## Core rule

Память агента — это не архив чатов.

Память агента — это:
- структурированные эпизоды;
- переиспользуемые знания;
- управляемое забывание;
- точный retrieval по типам данных;
- маленький hot context для ближайших шагов.

---

## Hook vs Tool: принцип разделения

**Hook** — детерминированное действие, которое должно происходить автоматически.
Агент не тратит attention на решение, когда и что делать.

**Tool** — интерактивное действие, где агент решает вызвать или нет.

### Детерминированные операции → Hooks
- Финализация сессии при завершении задачи
- Emergency shrink при превышении порога
- Preflight hot context check
- Запись episode при task complete / known failure / artifact created
- Извлечение preference при repeated pattern

### Интерактивные операции → Tools
- Поиск по памяти (агент решает нужен ли retrieval)
- Чтение конкретного episode / thread
- Явная запись факта / процедуры по запросу агента
- Линковка artifact'а
- Явный compress контекста по решению агента (soft limit warning → agent decides)

### Профит
- Снижение agent effort: рутина уходит в hooks
- Гарантированное выполнение: hooks нельзя пропустить
- Лучший attention allocation: агент фокусируется на задаче

---

## Embeddings:

* **model:** `gemini-embedding-001` (Gemini provider)
* **document embeddings:** `1536`
* **query embeddings:** `1536`
* **task_type для индексации:** `RETRIEVAL_DOCUMENT`
* **task_type для поиска:** `RETRIEVAL_QUERY`

## Memory model

### 1. Working memory
Горячий контекст активной сессии.

Содержит:
- последние сообщения;
- protected tool window;
- текущий plan / todos;
- незавершённые действия;
- краткое session summary;
- актуальные ограничения и state текущего шага.

Не индексируется как long-term memory напрямую.

### 2. Episodic memory
Память о завершённых эпизодах работы.

Один эпизод = одна заметная задача / подзадача / рабочая сессия.

Храним:
- что хотел пользователь;
- какой был план;
- что делали;
- какие инструменты использовали;
- что сработало / не сработало;
- какие были ошибки;
- какие артефакты создали;
- итог.

Это основной слой для “мы уже делали похожее”.

### 3. Semantic / procedural memory
Нормализованная переиспользуемая память.

Храним:
- факты о проекте / topic;
- предпочтения пользователя;
- устойчивые решения;
- рабочие процедуры;
- playbooks;
- полезные правила и ограничения.

Это ближе к skills, чем к chat history.

---

## What not to do

Не делать:
- один общий embeddings-index по всем старым сообщениям;
- прямое превращение старых чатов в skills;
- retrieval по narrator text и мусорным tool traces без фильтрации;
- full chat injection обратно в prompt по умолчанию;
- хранение раздутого hot context “на всякий случай”.

Правильный путь:
- `chat/thread -> episode -> extracted memory -> retrieval`

---

## Storage layout

### Postgres
Используем для:
- thread registry;
- episode records;
- memory records;
- metadata;
- filters по user/topic/context/type/time;
- lexical retrieval;
- vector retrieval;
- tracking cleanup / indexing state.

### R2
Используем для:
- raw chat history;
- полных tool payloads;
- больших summaries;
- archived traces;
- вложений и внешних artefact references.

R2 не используется как primary retrieval engine.

---

## Minimal entities

### `threads`
Карточка диалога / topic thread.

Поля:
- `thread_id`
- `user_id`
- `context_key`
- `title`
- `short_summary`
- `created_at`
- `updated_at`
- `last_activity_at`

### `episodes`
Компактные записи о завершённых задачах / этапах.

Поля:
- `episode_id`
- `thread_id`
- `context_key`
- `goal`
- `summary`
- `outcome`
- `tools_used`
- `artifacts`
- `failures`
- `importance`
- `created_at`

### `memories`
Нормализованные memory records.

Поля:
- `memory_id`
- `context_key`
- `source_episode_id`
- `memory_type` (`fact`, `preference`, `procedure`, `decision`, `constraint`)
- `title`
- `content`
- `short_description`
- `importance`
- `confidence`
- `tags`
- `created_at`
- `updated_at`

### `memory_embeddings`
Вектора для `episodes` и `memories`.

Поля:
- `owner_id`
- `owner_type` (`episode`, `memory`)
- `embedding`

### `session_state`
Служебное состояние активной сессии.

Поля:
- `session_id`
- `context_key`
- `hot_token_estimate`
- `last_compacted_at`
- `last_finalized_at`
- `cleanup_status`
- `pending_episode_id`

---

## Retrieval flow

### Step 1. Query router
Перед поиском определяем:
- нужен ли retrieval вообще;
- искать ли только в active thread;
- искать ли в episodes;
- искать ли в reusable memories;
- нужен ли full thread read.

### Step 2. Candidate generation
Для `episodes` и `memories` запускаем:
- lexical full-text search;
- vector search;
- filters по:
  - `context_key`
  - `user_id`
  - `memory_type`
  - `time range`
  - `importance`

### Step 3. Fusion
Объединяем кандидатов через weighted merge.

RRF можно добавить позже, но на старте не обязателен.

### Step 4. Final ranking
На текущем этапе отдельный reranker исключён из дизайна.

Финальный top-K формируется fusion-ранжированием lexical/vector кандидатов.

### Step 5. Context injection
В prompt отдаём:
- 3–8 лучших memory items;
- короткие evidence snippets;
- source refs (`thread_id`, `episode_id`);
- инструкцию “open full thread only if needed”.

---

## Write path

### On every turn
Не записываем всё подряд в long-term memory.

### On meaningful event
Пишем structured episodic record, когда:
- задача завершена;
- найдено рабочее решение;
- произошёл заметный фейл;
- создан артефакт;
- принято решение;
- пользователь сообщил устойчивое предпочтение.

### Async consolidation
Фоново делаем:
- deduplication;
- merge похожих memories;
- extraction из episodes в semantic/procedural memory;
- decay / TTL для слабополезных записей;
- reindex.

---

## Hot context policy

Hot context не должен расти бесконтрольно.

Целевая политика:
- **Normal hot size**: 12k – 60k tokens
- **Soft limit (warning)**: 60k tokens → inject warning, агент решает вызвать `compress`
- **Hard limit (auto-compaction)**: 80k tokens → hook автоматически запускает compaction с LLM summary + truncate

Управление hot context — двухуровневое:
1. Агент получает warning при 60k и может вызвать `compress` tool добровольно
2. Если агент игнорирует warning и контекст достигает 80k — hook принудительно запускает compaction

Нельзя позволять active agent loop стабильно жить на 100k–120k+ hot context, даже если модель формально поддерживает большой контекст. (`Архитектура делается под дешёвые модели, а дешёвые модели теряют attention начиная от 80к токенов контекста, это выливается в тот факт, что агент не может вызывать инструменты и начинается лениться`)

---

## Automatic cleanup rules

### 1. End-of-task cleanup
Когда задача завершена:
- сохранить episode summary;
- сохранить artifact refs;
- извлечь reusable memories;
- сократить hot context почти до 12-16k tokens.

После завершения задачи в hot остаются только:
- system instructions;
- topic / AGENTS essentials;
- active constraints;
- short session summary;
- минимальный recent window.

### 2. Preflight cleanup
Перед каждой итерацией:
- оценить размер hot context;
- если `token_count >= 60k` (soft limit) → inject warning в prompt:
  `"Context is growing (Nk tokens). Consider calling compress to free up space. At 80k tokens, compaction will be triggered automatically."`;
- если `token_count >= 80k` (hard limit) → hook принудительно запускает compaction
  с LLM summary + truncate + retry с backoff.

### 3. Background cleanup
Если end-of-task cleanup не произошёл:
- фоновый watchdog находит idle / stuck sessions;
- выполняет deferred compaction;
- при необходимости финализирует episode.

---

## Compaction policy

Текущий compaction сохраняем, но меняем роль:

- compaction нужен для hot context;
- long-term memory строится не только из compaction summary;
- summary не является единственным источником памяти;
- записи в episodic/semantic memory должны создаваться отдельно.

Иными словами:
- **compaction = уборка активного контекста**
- **memory pipeline = формирование долговременной памяти**

### Compaction side-effects

Каждая compression operation (вызванная инструментом `compress`, auto-compaction хуком
или PostRun cleanup path) обязана:

1. **Persist to long-term memory** — перед truncate, извлечь из удаляемого контекста
   high-signal данные и записать в episodic/semantic memory:
   - ключевые решения и их обоснование
   - важные находки и результаты
   - procedure candidates (что сработало)
   - constraint/fact candidates (что важно помнить)
   - artifact refs
2. **Не шуметь** — не писать в memory:
   - промежуточные tool traces
   - повторяющиеся/duplicate факты
   - сырые output без нормализации
3. **Оставить hint в текущей сессии** — после compaction добавить в hot context
   `ArchiveReference` с кратким описанием того, что было заархивировано:
   - типы данных (decisions, procedures, artifacts)
   - episode_id / thread_id для retrieval
   - короткий список ключевых тем
   Это позволяет агенту в текущей сессии знать, что было сжато, и при необходимости
   достать детали через `memory_search` / `memory_read_episode`.

---

## Memory Hooks (automatic)

Реализуются в `agent/hooks/memory/`.

### PostRun finalization path
Отдельный `EndOfTaskMemoryHook` больше не является частью дизайна.

Canonical lifecycle теперь идёт через runner PostRun path:
- `handle_final_response` / `handle_waiting_for_user_input`
- `run_compaction_checkpoint(..., PostRun)`
- `persist_post_run_memory(...)`

Этот путь обязан:
- запускать end-of-task / pause cleanup без отдельного hook;
- писать episode summary / session state / reusable memories через top-level PostRun coordinator;
- сбрасывать hot context до малого остаточного бюджета, насколько это позволяют pinned/protected-live entries;
- оставлять structured summary + `ArchiveReference` в hot context;
- оставаться top-level only: sub-agent'ы durable memory напрямую не пишут.

### `HotContextHealthHook` (`BeforeIteration`)
Проверяет перед каждой итерацией:
- Если `token_count >= soft_limit` (60k) → inject warning в prompt:
  ```
  [Context Health Warning] Hot context is at {N}k tokens (soft limit: 60k).
  Consider calling compress to summarize and free space.
  At 80k tokens, compaction will be triggered automatically.
  ```
  Агент решает — вызвать `compress` tool или продолжить.
- Если `token_count >= hard_limit` (80k) → автоматически запускает compaction pipeline:
  LLM summary → extract high-signal data → persist to long-term memory → truncate
  → rebuild hot context с ArchiveReference hints.
  
  Если LLM summarization не удалась (timeout/API error), используется retry с backoff.
  После исчерпания retry — deterministic fallback summary (без отдельного hook).

### `EpisodicExtractHook` (`AfterTool`)
Извлекает память после определённых tool calls:
- `file_write` / `apply_file_edit` → procedure candidate
- `sandbox_exec` с error exit → failure memory
- repeated pattern detection → preference extraction

---

## Memory Tools (interactive)

### `memory_search`
Ищет по episodes и memories.

Аргументы:
- `query`
- `types`
- `context_key`
- `time_range`
- `limit`

### `memory_read_episode`
Читает полный episodic record.

### `memory_read_thread_summary`
Возвращает summary старого thread.

### `memory_read_thread_window`
Читает кусок полного старого чата по диапазону.

### `memory_write_fact`
Пишет факт / preference / constraint по явному запросу агента.

### `memory_write_procedure`
Пишет reusable procedure / playbook по явному запросу.

### `memory_link_artifact`
Связывает episode с sandbox/file artefact.

### `compress`
Агент инициирует сжатие hot context. Вызывается добровольно при получении
soft limit warning или по решению агента.

Аргументы:
- `reason` (optional) — почему агент решил сжать (для audit/observability)

Поведение:
1. Запускает compaction pipeline: LLM summary + truncate
2. Извлекает high-signal данные из удаляемого контекста → persist в long-term memory
3. Добавляет ArchiveReference hints в hot context
4. Возвращает результат агенту: сколько токенов было, сколько стало, что заархивировано

Гарантии:
- Неблокирующий для agent loop — agent продолжает после compress
- Данные не теряются — всё заархивированное доступно через `memory_search`
- Можно вызывать несколько раз за сессию

---

## Indexing rules

Индексируем:
- episode summaries;
- reusable memory records;
- title / tags / short descriptions;
- extracted decisions / constraints / procedures.

Не индексируем напрямую как first-class memory:
- сырые tool results без нормализации;
- narrator output;
- повторяющиеся progress messages;
- шумные промежуточные chain-like traces;
- большие raw payloads.

---

## Phased implementation

## Phase 1 — Foundation
Сделать базовые сущности, hooks, tool и write path.

Что уже есть в коде:
- `compress` tool;
- `HotContextHealthHook`;
- transient warning path;
- config/env для soft/hard limits;
- tests for the hot-context path.

Что делаем:
- [x] вводим `threads`, `episodes`, `memories`, `session_state`;
- [x] реализуем durable PostRun finalization для episode/thread/session_state;
- [x] реализуем `HotContextHealthHook` — warning при 60k, auto-compaction при 80k (с retry fallback);
- [x] реализуем `compress` tool — интерактивное сжатие по решению агента;
- [x] добавляем compaction side-effects: persist high-signal data → long-term memory;
- [x] добавляем ArchiveReference hints в hot context после каждой compression;
- [x] отделяем raw archive in R2 from retrieval metadata;
- [x] делаем lexical search по episodes/memories;
- [x] делаем manual read tools.

Результат:
- hot context управляется автоматически (hooks) и интерактивно (tool);
- long-term memory write path и archive persistence уже реализованы;
- retrieval layer и advanced consolidation ещё не готовы.

## Phase 2 — Hybrid retrieval
Добавляем semantic retrieval.

Статус: не реализовано.

Что делаем:
- [ ] embeddings для episodes/memories;
- [ ] pgvector search;
- [ ] weighted fusion;
- [ ] context injection policy.

Результат:
- поиск работает и по exact match, и по смыслу.

## Phase 3 — Consolidation
Добавляем memory hygiene.

Статус: частично реализовано.

Что делаем:
- [x] extraction episode -> reusable memory (conservative baseline);
- [ ] deduplication;
- [ ] importance scoring;
- [ ] decay / TTL;
- [ ] merge похожих записей;
- [ ] background cleanup watchdog.

Результат:
- память растёт медленно и остаётся полезной;
- cleanup перестаёт зависеть только от успешного конца сессии.

## Phase 4 — Agent-native memory behavior
Уточняем memory workflow для агента.

Статус: не реализовано.

Что делаем:
- [ ] `EpisodicExtractHook` — extraction из tool calls в reusable memories;
- [ ] retrieval advisor hook — подсказывает агенту "consider memory search", но агент решает;
- [ ] topic-aware memory policies;
- [ ] optional user-facing "memory cards" / "chat history cards".

Результат:
- память становится управляемой подсистемой, а не пассивным архивом;
- агент тратит минимум attention на memory management;

---

## Recommended defaults

### Retrieval
- сначала lexical + vector
- потом top-K injection

### Scope
- memory строго topic-aware / context-aware
- cross-topic retrieval только по явному разрешению

### Writes
- писать только high-signal records
- избегать “запомнить всё”

### Retention
- episodes хранить долго
- low-value traces архивировать
- reusable memories хранить с importance/confidence

### Cleanup
- всегда проверять budget перед model call
- после task completion почти полностью сбрасывать hot context
- при auto-compaction использовать retry + deterministic fallback summary

---

## Practical recommendation for Oxide

Итоговое решение для Oxide Agent:

- оставить текущий hot-context + compaction pipeline;
- добавить typed long-term memory;
- использовать **Postgres + full-text search + pgvector** для retrieval;
- использовать **R2 как cold archive**;
- строить память через:
  - episodic summaries,
  - extracted reusable memories,
  - hybrid retrieval с fusion-ранжированием;
- ввести aggressive hot-context control:
  - end-of-task reset,
  - preflight compaction,
  - background cleanup.

Это лучший баланс между:
- качеством retrieval;
- простотой эксплуатации;
- explainability;
- стоимостью внедрения;
- совместимостью с текущей архитектурой Oxide;
- минимизацией новых зависимостей.

---

## Final rule

Memory system должна начинаться не с retrieval, а с контроля hot context.

Если hot context раздут, агент хуже:
- использует инструменты;
- вспоминает нужную память;
- планирует следующие шаги.

Поэтому для Oxide приоритет такой:
1. **автоматическое управление hot context** (hooks);
2. structured episodic writes (hooks);
3. reusable memory extraction (hooks + tools);
4. interactive retrieval (tools).

Принцип: рутина — в hooks, интерактив — в tools.

---

## Remaining work for full agent-memory functionality

Audit baseline: `2026-04-07`, branch `feature/memento-mori`.

Этот раздел важнее старых phase-status выше: часть из них уже устарела.

### Что уже есть в ветке

- hot-context warning + hard compaction path;
- durable PostRun persistence для `threads` / `episodes` / `memories` / `session_state`;
- automatic durable-memory retrieval перед model call;
- explicit memory tools (`memory_search`, `memory_read_*`, `memory_write_*`, `memory_link_artifact`);
- embeddings indexing и vector search API;
- consolidation watchdog: dedup, merge, rescoring, TTL/expiration;
- retrieval advisor и tool-derived memory extraction hooks.

### P0 — довести retrieval до консистентного production-state

1. **Сделать tool-level retrieval таким же сильным, как automatic retrieval.**
   - Сейчас automatic prompt injection использует hybrid lexical + vector fusion.
   - Но `memory_search` всё ещё lexical-only.
   - Нужно перевести `memory_search` на общий retrieval engine, чтобы агент получал одинаковое качество поиска и в auto-path, и в explicit tool-path.

2. **Rerank исключён из текущего дизайна.**
   - Флаг `rerank_requested` и связанные misleading references нужно удалить из контракта и документации.
   - Текущий production-target: hybrid lexical + vector fusion без отдельного reranker pipeline.

3. **Добавить retrieval quality tests.**
   - Нужны интеграционные тесты на lexical-only edge cases, semantic-match cases и hybrid fusion ranking.
   - Отдельно нужно проверить parity между automatic retrieval injection и `memory_search`.

### P0 — завершить storage/backend story

4. **Выбрать и закрепить production backend для typed memory.**
   - Решение принято: canonical production backend — `Postgres + full-text + pgvector`.
   - Telegram runtime должен инициализировать общий `PersistentMemoryStore` через Postgres на старте и падать при недоступной БД.
   - `StorageMemoryRepository` остаётся compatibility/test adapter'ом и источником artifact blobs, но не production retrieval/write path.

5. **Если целевой backend остаётся Postgres/pgvector — довести его до deployable состояния.**
   - [x] Добавить `postgres`/`pgvector` service в `docker-compose.yml`.
   - [x] Добавить все нужные env vars в `.env.example`.
   - [x] Описать bootstrap/migrations/backup/restore.
   - [x] Добавить startup health checks и failure handling на случай недоступной БД.

   Operational notes:
   - **Bootstrap**: canonical local/prod path идёт через `docker-compose.yml` service `postgres` на образе `pgvector/pgvector:pg17` и `MEMORY_DATABASE_URL`. При первом старте volume `postgres-data` инициализируется самим Postgres.
   - **Migrations**: schema bootstrap не требует отдельного manual SQL step. При `MEMORY_DATABASE_AUTO_MIGRATE=true` runtime вызывает embedded SQLx migrations из `crates/oxide-agent-memory/migrations/` и сам создаёт `vector` extension, typed-memory tables, lexical indexes и consolidation columns.
   - **Pre-provisioned DB**: если rollout требует отдельного change window, можно заранее прогнать те же embedded migrations вне runtime и запускать приложение с `MEMORY_DATABASE_AUTO_MIGRATE=false`; startup health-check всё равно валидирует наличие `pgvector`, базовых таблиц и последних schema columns.
   - **Startup failure handling**: runtime должен ретраить transient startup failures через `MEMORY_DATABASE_STARTUP_MAX_ATTEMPTS`, `MEMORY_DATABASE_STARTUP_RETRY_DELAY_MS` и `MEMORY_DATABASE_STARTUP_TIMEOUT_SECS`, но после исчерпания попыток завершаться fail-fast с actionable error. Это защищает от тихого запуска без typed memory.
   - **Health checks**: Compose health-check для `postgres` должен подтверждать не только `pg_isready`, но и реальный SQL query. Runtime дополнительно выполняет app-level health check после connect/migrate.
   - **Backup**: основной backup unit — Postgres volume/database. Для docker-compose-окружения: `pg_dump --format=custom --dbname "$MEMORY_DATABASE_URL" > memory.dump` или filesystem snapshot `postgres-data` при остановленном контейнере. R2 archive/blobs бэкапятся отдельно, потому что durable metadata и cold artifacts разнесены.
   - **Restore**: поднять `postgres`/pgvector, создать пустую БД/роль при необходимости, затем `pg_restore --clean --if-exists --dbname "$MEMORY_DATABASE_URL" memory.dump`. После restore запускать агент либо с `MEMORY_DATABASE_AUTO_MIGRATE=true`, либо с ручной проверкой, что embedded migrations уже на месте.

6. **Если остаётся storage-backed backend — зафиксировать operational guarantees.**
   - Явно описать консистентность, latency, limits по list/search, strategy для embedding backfill и cleanup.
   - Проверить, что R2-backed lexical/vector search масштабируется на ожидаемый объём памяти.

### P1 — закрыть lifecycle/product gaps

7. [x] **Решить, нужен ли отдельный `EndOfTaskMemoryHook`.**
   - Решение: **нет**, отдельный hook убран из дизайна.
   - Canonical lifecycle/finalization path закреплён за PostRun runner flow + persistent-memory coordinator.
   - Observability и residual-budget verification добавляются в PostRun path, а не в отдельный hook.

8. [x] **Жёстко верифицировать end-of-task cleanup target.**
   - Добавить runner telemetry around pre/post PostRun cleanup snapshots.
   - Держать operational target: residual hot context `<= 16k` tokens для обычного top-level финала, если это не блокируется pinned/protected-live state.
   - Проверять это тестами на PostRun path, а не только по факту `CleanupStatus::Finalized`.

9. **Решить судьбу user-facing memory/history cards.**
   - Prompt-side advisor cards уже есть.
   - Но transport-level UI cards для Telegram/Web пока не оформлены как отдельная продуктовая функция.
   - Нужно либо реализовать их, либо убрать из плана как неактуальное требование.

### P1 — observability и эксплуатация

10. **Добавить telemetry для memory subsystem.**
    - retrieval hit/miss;
    - lexical vs vector contribution;
    - number of retrieved items injected into prompt;
    - memory write counts by type;
    - consolidation merges/deletes/expirations;
    - embedding backfill failures/retries.

11. **Добавить operator-facing diagnostics.**
    - Нужен понятный способ посмотреть: какие memories были созданы, что удалил consolidator, почему retrieval ничего не вернул, сколько embeddings pending/failed.

### P1 — тестовое покрытие

12. **Расширить e2e и integration coverage.**
    - hybrid retrieval в runner path;
    - explicit `memory_search` после перевода на hybrid;
    - Postgres backend path, если он остаётся в scope;
    - scope isolation для vector retrieval;
    - maintenance/watchdog на больших наборах memories;
    - recovery после embedding/indexing errors.

### P2 — rollout и cleanup

13. **Перенести/влить работу из `feature/memento-mori` в рабочую ветку rollout.**
    - Пока что значительная часть memory stack живёт не в `testing`.
    - До фактического rollout это остаётся branch-local capability.

14. **Актуализировать документацию.**
    - Обновить status-блоки и phase checkboxes в этом файле.
    - Синхронизировать `docker-compose.yml`, `.env.example` и runtime-доки с реальным backend choice.
    - Явно описать, что уже автоматическое, а что остаётся tool-driven.

### Definition of done для “memory system complete”

Систему можно считать доведённой до полного функционала, когда одновременно выполнены все условия:

- automatic retrieval и `memory_search` используют один и тот же hybrid retrieval core;
- выбран и документирован один production backend story;
- deploy-конфигурация (`docker-compose.yml`, `.env.example`, migrations) соответствует выбранному backend;
- отдельный rerank честно исключён из текущего дизайна, пока не появится eval-set и понятный budget на latency/cost;
- PostRun lifecycle, cleanup и consolidation покрыты e2e/integration тестами;
- есть telemetry и operator diagnostics для retrieval / writes / cleanup / embeddings;
- ветка с памятью влита в реальный rollout branch, а не существует отдельно от production path.
