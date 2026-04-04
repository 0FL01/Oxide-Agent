# Memory / RAG Plan for Oxide Agent

## Goal

Сделать память агента полезной для long-running assistant workflows без превращения истории чатов в шумный архив.

Ключевая идея:
- не использовать один общий RAG по всем старым сообщениям;
- разделить память на типы;
- использовать hybrid retrieval: lexical + semantic + rerank;
- хранить raw history отдельно от индексируемой памяти;
- держать hot context маленьким и агрессивно чистить его.

## Current Status In Code

### Implemented
- `compress` tool exists and is registered in the main agent registry.
- `compress` is handled by the runner and blocked for sub-agents.
- `HotContextHealthHook` is implemented and wired into agent execution.
- Soft/hard hot-context limits are config-backed (`soft_warning_tokens`, `hard_compaction_tokens`).
- Transient warning injection exists and does not persist into agent memory.
- Regression tests cover `compress`, hot-context warnings, compaction behavior, and transport-web E2E.

### Not Implemented Yet
- Typed long-term memory model (`threads`, `episodes`, `memories`, `session_state`).
- Memory search/read/write tools.
- Hybrid retrieval pipeline (lexical + vector + rerank).
- End-of-task memory finalization hook.
- Episodic extraction / consolidation hooks.
- Long-term memory persistence and indexing for compaction outputs.

---

## Decision

Для Oxide Agent принимаем упрощённый стек:

- **Hybrid RAG**
  - full-text / lexical search
  - embeddings / vector search
  - optional reranking поверх объединённых кандидатов

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
- reranker повышает качество top-K.

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

### Step 4. Rerank
Опционально прогоняем top-N кандидатов через reranker.

На первом этапе можно отключить, если latency или цена важнее.

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

Каждая compression operation (вызванная инструментом `compress`, auto-compaction хуком,
или `EndOfTaskMemoryHook`) обязана:

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

### `EndOfTaskMemoryHook` (`AfterAgent`)
Триггерит при финальном ответе (без pending tool calls):
- Пишет episode summary в storage
- Извлекает reusable memories (fact, procedure, constraint)
- Запускает compaction pipeline с persist to long-term memory
- Сбрасывает hot context до ~12-16k tokens
- Сохраняет artifact refs
- Оставляет в hot context ArchiveReference на записанный episode

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
- [ ] вводим `threads`, `episodes`, `memories`, `session_state`;
- [ ] реализуем `EndOfTaskMemoryHook` — автоматическая финализация + compaction + persist memory;
- [x] реализуем `HotContextHealthHook` — warning при 60k, auto-compaction при 80k (с retry fallback);
- [x] реализуем `compress` tool — интерактивное сжатие по решению агента;
- [ ] добавляем compaction side-effects: persist high-signal data → long-term memory;
- [ ] добавляем ArchiveReference hints в hot context после каждой compression;
- [ ] отделяем raw archive в R2 от retrieval metadata;
- [ ] делаем lexical search по episodes/memories;
- [ ] делаем manual read tools.

Результат:
- hot context управляется автоматически (hooks) и интерактивно (tool);
- long-term memory pipeline ещё не реализован;
- hints об архиве и эпизоды ещё не пишутся автоматически.

## Phase 2 — Hybrid retrieval
Добавляем semantic retrieval.

Статус: не реализовано.

Что делаем:
- [ ] embeddings для episodes/memories;
- [ ] pgvector search;
- [ ] weighted fusion;
- [ ] optional rerank;
- [ ] context injection policy.

Результат:
- поиск работает и по exact match, и по смыслу.

## Phase 3 — Consolidation
Добавляем memory hygiene.

Статус: не реализовано.

Что делаем:
- [ ] deduplication;
- [ ] extraction episode -> reusable memory;
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
- потом optional rerank
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
  - hybrid retrieval,
  - optional reranking;
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
