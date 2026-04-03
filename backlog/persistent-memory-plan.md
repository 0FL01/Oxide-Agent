# Memory / RAG Plan for Oxide Agent

## Goal

Сделать память агента полезной для long-running assistant workflows без превращения истории чатов в шумный архив.

Ключевая идея:
- не использовать один общий RAG по всем старым сообщениям;
- разделить память на типы;
- использовать hybrid retrieval: lexical + semantic + rerank;
- хранить raw history отдельно от индексируемой памяти;
- держать hot context маленьким и агрессивно чистить его.

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
- normal hot size: **10k–18k tokens**
- soft limit: **12k–14k**
- hard limit: **18k–20k**
- emergency threshold: **24k**

Нельзя позволять active agent loop стабильно жить на 40k–50k+ hot context, даже если модель формально поддерживает большой контекст.

---

## Automatic cleanup rules

### 1. End-of-task cleanup
Когда задача завершена:
- сохранить episode summary;
- сохранить artifact refs;
- извлечь reusable memories;
- сократить hot context почти до baseline.

После завершения задачи в hot остаются только:
- system instructions;
- topic / AGENTS essentials;
- active constraints;
- short session summary;
- минимальный recent window.

### 2. Preflight cleanup
Перед каждым вызовом модели:
- оценить размер hot context;
- если превышен soft limit — выполнить normal compaction;
- если превышен hard limit — выполнить forced compaction.

### 3. Background cleanup
Если end-of-task cleanup не произошёл:
- фоновый watchdog находит idle / stuck sessions;
- выполняет deferred compaction;
- при необходимости финализирует episode.

### 4. Emergency shrink
Если normal compaction не удалась:
- применить deterministic fallback без LLM;
- оставить только short summary, active todo state, latest user turn, latest assistant intent и safety window;
- остальной контекст удалить из hot и оставить в archive / episode records.

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

---

## Tooling

Нужны отдельные memory tools.

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
Пишет факт / preference / constraint.

### `memory_write_procedure`
Пишет reusable procedure / playbook.

### `memory_link_artifact`
Связывает episode с sandbox/file artefact.

### `memory_finalize_session`
Финализирует текущую сессию, пишет episodic record и инициирует cleanup.

### `memory_emergency_shrink`
Аварийно уменьшает hot context без LLM-суммаризации.

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
Сделать базовые сущности и write path.

Что делаем:
- вводим `threads`, `episodes`, `memories`, `session_state`;
- сохраняем episode summary при завершении задачи;
- отделяем raw archive в R2 от retrieval metadata;
- делаем lexical search по episodes/memories;
- делаем manual read tools;
- вводим end-of-task cleanup и preflight cleanup.

Результат:
- агент уже может находить старые задачи и решения без full chat scan;
- hot context перестаёт раздуваться после завершённых задач.

## Phase 2 — Hybrid retrieval
Добавляем semantic retrieval.

Что делаем:
- embeddings для episodes/memories;
- pgvector search;
- weighted fusion;
- optional rerank;
- context injection policy.

Результат:
- поиск работает и по exact match, и по смыслу.

## Phase 3 — Consolidation
Добавляем memory hygiene.

Что делаем:
- deduplication;
- extraction episode -> reusable memory;
- importance scoring;
- decay / TTL;
- merge похожих записей;
- background cleanup watchdog.

Результат:
- память растёт медленно и остаётся полезной;
- cleanup перестаёт зависеть только от успешного конца сессии.

## Phase 4 — Agent-native memory behavior
Даём агенту явный memory workflow.

Что делаем:
- router deciding when retrieval is needed;
- explicit memory write hooks;
- topic-aware memory policies;
- optional user-facing “memory cards” / “chat history cards”;
- emergency shrink fallback.

Результат:
- память становится управляемой подсистемой, а не пассивным архивом.

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
- держать emergency shrink как обязательный fallback

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
  - background cleanup,
  - emergency shrink.

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
1. маленький hot context;
2. structured episodic writes;
3. reusable memory extraction;
4. hybrid retrieval поверх чистой памяти.