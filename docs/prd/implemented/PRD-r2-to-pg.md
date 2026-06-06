# PRD: Полный переход с R2 на SQLx/Postgres/Supabase

## 1. Summary

Oxide Agent сейчас использует Cloudflare R2/S3 как durable storage и фактически моделирует поверх object storage набор таблиц, индексов, очередей и журналов. Цель этого PRD — зафиксировать карту текущей R2-поверхности, blast radius и поэтапный план полного перехода на SQLx + Postgres без миграции старых R2-данных.

Целевые режимы:

- `postgres local`: локальная разработка и локальный self-host через локальный PostgreSQL.
- `supabase web`: web/production deployment через Supabase Postgres.

Результат этой работы — только разведка и план. Реализация, dual-write, backfill и импорт старых object keys не входят в scope.

## 2. Goals

- Полностью убрать Cloudflare R2/S3 из production/runtime архитектуры durable storage.
- Заменить object-key/JSON/prefix-listing/ETag модель на SQLx-backed модель поверх Postgres.
- Сохранить fresh setup: новая база, новые таблицы, без чтения и переноса старых R2 objects.
- Сделать Postgres основным SQL dialect для локального и web/production режимов.
- Поддержать Supabase Postgres как production backend без Supabase Storage buckets.
- Ввести schema-first подход: миграции схемы, индексы, ограничения, acceptance tests.
- Перевести web users/auth/sessions/tasks/task events/task files на SQL-native persistence.
- Перевести core durable state: user config/state, agent memory, flows, profiles, topic context, topic AGENTS.md, topic infra, topic bindings, secrets, reminders, audit, wiki memory.
- Сделать task events append-only rows. Не переписывать большие JSON chunks/objects при каждом batch событий.
- Сделать reminders SQL-native due-job queue с безопасным claiming.
- Сделать audit append-only stream с индексированной пагинацией.
- Убрать AWS SDK runtime dependencies и R2 env vars из production paths, docs, examples, CI и build features.

## 3. Non-Goals

- Не мигрировать старые R2-данные.
- Не читать старые R2 objects.
- Не делать dual-write между R2 и SQL.
- Не делать backfill, importer, object-key scan tooling или data migration story.
- Не сохранять R2 как blob fallback, wiki fallback, memory fallback или emergency compatibility layer.
- Не оставлять R2 feature flags “на всякий случай”, если они не нужны для тестов удаления в промежуточных фазах.
- Не проектировать SQLite backend. SQLite отсутствует из scope.
- Не реализовывать SQLx storage в этом проходе.
- Не менять runtime behavior сейчас, кроме создания этого PRD.

## 4. Current R2 Surface Area

Разведка проводилась по Cargo features/dependencies, storage modules, web persistence, Telegram runner, docs, env examples, profiles, CI и tests. Ниже перечислены найденные зоны, которые реально завязаны на R2/S3/AWS или на object-storage модель.

### 4.1 Cargo features и AWS SDK dependencies

Затронутые файлы:

- `crates/oxide-agent-core/Cargo.toml`
- `crates/oxide-agent-telegram-bot/Cargo.toml`
- `crates/oxide-agent-transport-telegram/Cargo.toml`
- `crates/oxide-agent-transport-web/Cargo.toml`
- `Cargo.lock`

Найдено:

- В `oxide-agent-core` есть optional dependencies:
  - `aws-sdk-s3`
  - `aws-config`
  - `aws-credential-types`
  - `aws-types`
- Feature `storage-s3-r2` включает AWS SDK dependencies.
- Profile features в `oxide-agent-core` включают `storage-s3-r2` почти во всех production-like профилях: `profile-full`, `profile-embedded-opencode-local`, `profile-web-embedded-opencode-local`, `profile-lite`, `profile-search-only`, `profile-no-sandbox`, `profile-media-enabled`, `profile-host-bwrap`.
- `oxide-agent-telegram-bot` пробрасывает `storage-s3-r2` в `oxide-agent-core` и `oxide-agent-transport-telegram`.
- `oxide-agent-telegram-bot` имеет AWS SDK crates в `dev-dependencies` для ignored integration validation.
- Binary `oxide-agent-telegram-bot` сейчас имеет `required-features = ["transport-telegram", "storage-s3-r2"]`.
- `oxide-agent-transport-telegram` и `oxide-agent-transport-web` профильные features также тянут `storage-s3-r2`.

Роль зоны:

- Feature `storage-s3-r2` сейчас является production durable storage gate.
- AWS SDK попадает в runtime сборки через storage feature.
- Cargo profile topology сейчас считает R2 обязательной частью полноценных профилей.

Что это означает для перехода:

- Нужно заменить feature `storage-s3-r2` на SQLx/Postgres feature или сделать SQLx базовым durable storage для соответствующих профилей.
- Нужно убрать AWS SDK crates из runtime и dev dependencies после переписывания integration validation.
- Нужно обновить `Cargo.lock` после удаления AWS SDK dependencies.

### 4.2 Compiled capability manifest и module registry

Затронутые файлы:

- `crates/oxide-agent-core/src/capabilities/compiled.rs`
- `crates/oxide-agent-core/tests/modular_registry_snapshots.rs`
- `crates/oxide-agent-core/tests/snapshots/modular_registry_snapshots__*.snap`
- `crates/oxide-agent-core/tests/tool_runtime_static_guards.rs`

Найдено:

- `compiled.rs` регистрирует модуль `storage/r2` под cargo feature `storage-s3-r2` как `StorageBackend`.
- Тест `compiled_manifest_exposes_only_r2_as_durable_storage_backend` явно утверждает, что `storage/r2` — единственный durable storage backend.
- Snapshot tests содержат `storage/r2` и `storage-s3-r2` во всех профильных снапшотах.
- Static guard tests проверяют, что Telegram runner не импортирует concrete `R2Storage`, но всё равно ожидают `storage::build_primary_storage` и текущий R2-backed factory.

Роль зоны:

- Capability registry управляет видимостью storage backend modules и профилей.
- Snapshots и static guards будут падать при переименовании/удалении R2, пока не обновить ожидания на SQLx module.

Что это означает для перехода:

- Новый durable module должен быть явно зарегистрирован, например `storage/sqlx` или `storage/postgres`.
- Tests должны утверждать, что production durable backend — SQLx/Postgres, а не R2.
- Snapshot fixtures надо переснять только после фактического изменения feature graph.

### 4.3 Runtime storage facade и provider factory

Затронутые файлы:

- `crates/oxide-agent-core/src/storage/mod.rs`
- `crates/oxide-agent-core/src/storage/provider.rs`
- `crates/oxide-agent-core/src/storage/modules.rs`
- `crates/oxide-agent-core/src/storage/error.rs`
- `crates/oxide-agent-core/src/storage/telemetry.rs`

Найдено:

- `storage/mod.rs` сейчас документирован как Cloudflare R2 / AWS S3 storage implementation и под feature `storage-s3-r2` экспортирует `R2Storage` и `R2StorageConfig`.
- `StorageProvider` уже является широкой abstraction для user config/state, agent memory, wiki text, control-plane records, audit и reminders.
- `modules.rs` строит только `R2StorageModule` и возвращает `Arc<dyn StorageProvider>`.
- `build_primary_storage(settings)` под `storage-s3-r2` вызывает `R2StorageModule.build(settings)`.
- `R2StorageModule.module_id()` возвращает `storage/r2`.
- Если runtime module disabled, factory возвращает ошибку вида “S3/R2 is the only durable storage backend”.
- `StorageError` содержит S3-specific variants `S3Get` и `S3Put`.
- `telemetry.rs` логирует “R2 storage operation/cache hit/cache miss/summary”.

Роль зоны:

- Это главный durable storage entrypoint для core и Telegram.
- Эта зона должна стать SQLx/Postgres-backed, сохранив trait boundary для остального приложения.

Что это означает для перехода:

- `StorageProvider` стоит сохранить как бизнес-уровневый контракт, но убрать object-key semantics из методов, где они протекли наружу.
- `build_primary_storage` должен строить SQLx/Postgres backend и возвращать shared services, включая pool.
- `StorageError` нужно обобщить на DB/query/config/conflict errors.
- Telemetry должна стать storage-neutral или SQL-specific.

### 4.4 R2 raw object storage implementation

Затронутые файлы:

- `crates/oxide-agent-core/src/storage/r2.rs`
- `crates/oxide-agent-core/src/storage/r2_base.rs`
- `crates/oxide-agent-core/src/storage/r2_config.rs`
- `crates/oxide-agent-core/src/storage/r2_provider.rs`
- `crates/oxide-agent-core/src/storage/r2_user.rs`
- `crates/oxide-agent-core/src/storage/r2_memory.rs`
- `crates/oxide-agent-core/src/storage/r2_control_plane.rs`
- `crates/oxide-agent-core/src/storage/r2_reminder.rs`
- `crates/oxide-agent-core/src/storage/keys.rs`
- `crates/oxide-agent-core/src/storage/utils.rs`

Найдено:

- `R2Storage` хранит `aws_sdk_s3::Client`, `bucket`, in-memory cache, control-plane locks и telemetry.
- `r2_base.rs` реализует raw operations:
  - `save_json`, `save_text`, `save_bytes`
  - `load_json`, `load_text`, `load_bytes`
  - `load_json_with_etag`
  - `save_json_conditionally(expected_etag)`
  - `delete_object`, `delete_prefix`
  - `list_keys_under_prefix`, `list_json_under_prefix`
  - RMW helpers для reminders и user config
- Conditional writes используют ETag/If-Match/If-None-Match как optimistic locking.
- Queries реализованы через prefix listing и фильтрацию JSON objects в памяти.
- `r2_config.rs` читает module config `storage/r2` и env vars:
  - `OXIDE_R2_ACCESS_KEY_ID`
  - `OXIDE_R2_SECRET_ACCESS_KEY`
  - `OXIDE_R2_ENDPOINT_URL`
  - alias `OXIDE_R2_ENDPOINT`
  - `OXIDE_R2_BUCKET_NAME`
  - alias `OXIDE_R2_BUCKET`
  - `OXIDE_R2_REGION`
- `keys.rs` содержит object-key layout для users, memory, flows, wiki, control-plane, reminders, secrets и audit.

Роль зоны:

- Это самодельная СУБД поверх object storage: object keys как primary keys/indexes, prefix listing как query, JSON как rows, ETags как optimistic lock.

Что это означает для перехода:

- Все `r2_*` implementation modules должны быть удалены в Phase 6.
- `keys.rs` нельзя оставить как runtime addressing model для SQL, кроме временной помощи в тестах удаления. SQL schema должна адресовать entities typed columns, а не object keys.
- R2 cache/telemetry/locks должны быть заменены на DB transaction boundaries, row locks, unique constraints и indexes.

### 4.5 Object key layout, который сейчас работает как schema

Затронутый файл:

- `crates/oxide-agent-core/src/storage/keys.rs`

Найденные namespaces:

- User config:
  - `users/{user_id}/config.json`
- Agent memory:
  - `users/{user_id}/agent_memory.json`
  - `users/{user_id}/topics/{context_key}/agent_memory.json`
  - `users/{user_id}/topics/{context_key}/flows/{flow_id}/meta.json`
  - `users/{user_id}/topics/{context_key}/flows/{flow_id}/memory.json`
- Wiki memory:
  - `wiki/v1/global/{file}`
  - `wiki/v1/contexts/{context_id}/{file}`
  - `wiki/v1/contexts/{context_id}/pages/{slug}.md`
  - `wiki/v1/contexts/{context_id}/inbox/{slug}.md`
  - `wiki/v1/contexts/{context_id}/raw/{yyyy_mm}/{run_id}.md`
- Control plane:
  - `users/{user_id}/control_plane/agent_profiles/{agent_id}.json`
  - `users/{user_id}/control_plane/topic_contexts/{topic_id}.json`
  - `users/{user_id}/control_plane/topic_agents_md/{topic_id}.json`
  - `users/{user_id}/control_plane/topic_prompts/{topic_id}`
  - `users/{user_id}/control_plane/topic_infra/{topic_id}.json`
  - `users/{user_id}/control_plane/topic_bindings/{topic_id}.json`
- Reminders:
  - `users/{user_id}/control_plane/reminders/{reminder_id}.json`
- Secrets:
  - `users/{user_id}/private/secrets/{secret_ref}`
- Audit:
  - `users/{user_id}/control_plane/audit/events.json`

Роль зоны:

- Object keys выполняют роль schema, routing, partitioning и secondary indexes.
- При удалении R2 эти helpers должны перестать быть durable addressing API.

Что это означает для перехода:

- Каждая группа ключей должна получить явную SQL entity.
- Prefix delete должен стать scoped SQL delete/update с foreign keys и retention policy.
- Prefix listing должен стать indexed SQL query.

### 4.6 Core user config/state

Затронутые файлы:

- `crates/oxide-agent-core/src/storage/user.rs`
- `crates/oxide-agent-core/src/storage/r2_user.rs`
- `crates/oxide-agent-core/src/storage/provider.rs`
- Consumers в Telegram и web session manager.

Найдено:

- `UserConfig` содержит user-level `state` и `contexts: HashMap<String, UserContextConfig>`.
- `UserContextConfig` хранит topic/context durable state: state, current agent flow id, chat/thread metadata, forum topic metadata, closed flag.
- `r2_user.rs` читает/пишет весь `UserConfig` как один JSON object.
- `update_user_state` мутирует full config через read-modify-write.

Роль зоны:

- Durable user state и context routing state для transport layers.

Что это означает для перехода:

- User-level state и context rows надо разнести в SQL tables.
- Updates по одному context не должны переписывать full JSON user config.
- Multi-record updates должны идти в транзакции.

### 4.7 Agent memory и flow checkpoint persistence

Затронутые файлы:

- `crates/oxide-agent-core/src/storage/flows.rs`
- `crates/oxide-agent-core/src/storage/r2_memory.rs`
- `crates/oxide-agent-core/src/agent/memory.rs`
- `crates/oxide-agent-core/src/agent/wiki_memory/store.rs`
- `crates/oxide-agent-transport-web/src/session.rs`
- `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/session.rs`
- `crates/oxide-agent-core/tests/r2_flow_checkpoint_integration.rs`

Найдено:

- Agent memory serializes as `AgentMemory` JSON and is stored per user, per context, and per flow.
- `AgentFlowRecord` has user_id, context_key, flow_id, timestamps and schema_version.
- `WebSessionManager::create_session_with_model_selection` loads memory via `load_agent_memory_for_flow`, installs `StorageFlowCheckpoint`, and saves memory through `save_agent_memory_for_flow`.
- R2 integration test verifies background checkpoint coalescing and skipped identical writes against real R2 credentials.

Роль зоны:

- Durable checkpoint for agent memory/history between executions and restarts.

Что это означает для перехода:

- Keep checkpoint coalescing semantics, but persist to SQL row(s).
- Avoid hot-row excessive writes where possible. Memory snapshots can remain JSONB snapshots, but write frequency must be coalesced.
- R2 integration test must become SQL integration test with local Postgres test DB.

### 4.8 Control-plane state and secrets

Затронутые файлы:

- `crates/oxide-agent-core/src/storage/control_plane.rs`
- `crates/oxide-agent-core/src/storage/r2_control_plane.rs`
- `crates/oxide-agent-core/src/agent/providers/manager_control_plane/*`
- `crates/oxide-agent-transport-telegram/src/bot/topic_route.rs`
- `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/execution_config.rs`

Найдено:

- Control-plane records include:
  - `AgentProfileRecord`
  - `TopicContextRecord`
  - `TopicAgentsMdRecord`
  - `TopicInfraConfigRecord`
  - `TopicBindingRecord`
  - private secret values
- R2 implementation stores each record as an individual JSON object.
- Upserts use local keyed locks plus ETag conditional writes for optimistic concurrency.
- Agent profiles are listed via prefix listing.
- Topic prompt duplicate guard uses object-key namespace and locks.
- Secrets are stored as text objects in `users/{user_id}/private/secrets/{secret_ref}`.

Роль зоны:

- Durable manager/control-plane configuration, topic routing, infra settings, profile state and secret refs.

Что это означает для перехода:

- Typed SQL tables should replace per-record JSON objects.
- Version increments and duplicate guards should be transaction-backed.
- Secrets need a dedicated table with strict access path and future encryption hook.
- Object-key-based guard should become unique constraints, row locks or advisory locks.

### 4.9 Audit stream

Затронутые файлы:

- `crates/oxide-agent-core/src/storage/control_plane.rs`
- `crates/oxide-agent-core/src/storage/r2_control_plane.rs`
- `crates/oxide-agent-core/src/storage/utils.rs`
- Consumers in `manager_control_plane` and reminders handlers.

Найдено:

- Audit events are stored as a single JSON array at `users/{user_id}/control_plane/audit/events.json`.
- Append reads the whole array, appends one record, increments per-user version, then conditionally rewrites the full JSON object.
- Pagination uses in-memory vector sorting/windowing.

Роль зоны:

- Manager/control-plane audit trail and reminder action audit.

Что это означает для перехода:

- Audit must become append-only SQL rows.
- Version allocation must be transaction-safe.
- Pagination should use indexed queries by `(user_id, version desc)` or `(user_id, created_at desc, id desc)`.

### 4.10 Reminders scheduler durable state

Затронутые файлы:

- `crates/oxide-agent-core/src/storage/reminder.rs`
- `crates/oxide-agent-core/src/storage/r2_reminder.rs`
- `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/reminders.rs`
- `crates/oxide-agent-transport-telegram/src/bot/reminder_scheduler.rs`

Найдено:

- Each reminder job is a JSON object under the reminders prefix.
- `list_due_reminder_jobs` lists all jobs under user prefix and filters/sorts in memory.
- Claim/reschedule/complete/fail/cancel/pause/resume/retry/delete mutate individual objects via ETag conditional RMW.
- Scheduler bootstraps/reconciles via `list_reminder_jobs`.

Роль зоны:

- Durable scheduled job queue with leases and retry state.

Что это означает для перехода:

- Reminders need SQL-native due-job claiming.
- Use transactions and row locks, for example `FOR UPDATE SKIP LOCKED`, to safely claim due jobs across workers/processes.
- Add indexes for due scans, status filtering and user/context listing.

### 4.11 Wiki memory storage

Затронутые файлы:

- `crates/oxide-agent-core/src/agent/wiki_memory/*`
- `crates/oxide-agent-core/src/agent/wiki_memory/store.rs`
- `crates/oxide-agent-core/src/agent/wiki_memory/cache.rs`
- `crates/oxide-agent-core/src/agent/wiki_memory/patch.rs`
- `crates/oxide-agent-core/src/agent/wiki_memory/scope.rs`
- `crates/oxide-agent-core/src/storage/keys.rs`
- `crates/oxide-agent-core/src/storage/r2_provider.rs`
- `docs/wiki-memory.md`
- `docs/tips/cache-hit.md`
- `README.md`

Найдено:

- Wiki memory is documented as bounded Markdown wiki stored in S3/R2 object store.
- `WikiStore` delegates to `StorageProvider` methods `load_wiki_text`, `save_wiki_text`, `delete_wiki_text`, `delete_wiki_context`.
- Comments and docs refer to deterministic S3/R2 object keys.
- Hot path tries to avoid S3 LIST by maintaining `index.md`/`log.md` discoverability.
- Current docs explicitly say Postgres was removed from wiki memory stack. That statement must be reversed/replaced by this SQLx migration story.

Роль зоны:

- Durable memory pages used to assemble context for agent runs.

Что это означает для перехода:

- Wiki pages should become SQL rows with scope/context/path metadata and text content.
- `StorageProvider` wiki methods currently take `storage_key`; either adapt them to parse legacy deterministic wiki paths into SQL columns during transition or replace with typed wiki methods.
- Delete context should become scoped SQL delete/update, not prefix delete.

### 4.12 Web UI persistence

Затронутые файлы:

- `crates/oxide-agent-transport-web/src/persistence/store.rs`
- `crates/oxide-agent-transport-web/src/persistence/models.rs`
- `crates/oxide-agent-transport-web/src/persistence/mod.rs`
- `crates/oxide-agent-transport-web/src/persistence/r2.rs`
- `crates/oxide-agent-transport-web/src/server/types.rs`
- `crates/oxide-agent-transport-web/src/server/tests.rs`
- `crates/oxide-agent-transport-web/src/bin/oxide-agent-web-console.rs`
- `crates/oxide-agent-web-contracts/src/auth.rs`
- `crates/oxide-agent-web-contracts/src/sessions.rs`
- `crates/oxide-agent-web-contracts/src/tasks.rs`
- `crates/oxide-agent-web-contracts/src/events.rs`

Найдено:

- `WebUiStore` trait covers web users, login index, auth sessions, web sessions, web tasks, task event append/list, task files and startup reconciliation.
- `R2WebUiStore` wraps a generic `ObjectStoreWebUiStore<S>`.
- Web object prefixes include:
  - `web/auth/v1/users/`
  - `web/auth/v1/login_index/`
  - `web/auth/v1/browser_sessions/`
  - `web/users/{user_id}/sessions/`
  - `web/users/{user_id}/tasks/{session_id}/`
  - `web/users/{user_id}/task_events/{session_id}/{task_id}/chunk-{chunk_no}.json`
  - `web/users/{user_id}/task_files/{session_id}/{task_id}/{file_id}.json`
  - task file `.bin` blob key
- `users_count` uses object listing.
- Login uniqueness is enforced by login-index object existence.
- Revoking sessions for a user lists all browser sessions and rewrites matching JSON records.
- `delete_session` deletes the session object, task/event/file prefixes and wiki context prefixes.
- Task events are chunked into JSON arrays of 100 events. Appending events loads the chunk, merges by seq and rewrites the chunk object.
- Event listing lists all chunks, flattens, sorts, filters by `after_seq`, truncates by limit.
- Task file metadata is JSON, content is a separate object blob.
- `mark_unfinished_tasks_interrupted` lists all `web/users/` keys, filters task record keys, mutates tasks and sessions.

Роль зоны:

- Durable web console state: auth, browser sessions, chat sessions, tasks, event replay, uploaded/generated task files, restart reconciliation.

Что это означает для перехода:

- This is one of the largest blast-radius areas.
- Task events must be append-only SQL rows, not JSON chunks.
- Task file blobs need a Postgres storage policy because R2 cannot remain as blob fallback.
- Web startup should build a SQL-backed `WebUiStore` and share the same pool as core storage.

### 4.13 Web startup/runtime selection

Затронутые файлы:

- `crates/oxide-agent-transport-web/src/bin/oxide-agent-web-console.rs`
- `crates/oxide-agent-transport-web/src/server/types.rs`
- `crates/oxide-agent-transport-web/src/server/router.rs`

Найдено:

- `build_app_state` chooses R2 if `use_r2_web_store(agent_settings)` is true.
- `use_r2_web_store` returns true when:
  - `OXIDE_WEB_STORE=r2`
  - `OXIDE_WEB_REQUIRE_DURABLE_STORAGE=true`
  - profile enables `storage/r2`
- If `storage-s3-r2` feature is absent and R2 is requested, startup errors with “OXIDE_WEB_STORE=r2 requires the storage-s3-r2 feature”.
- `WebStoreKind` has variant `R2`.
- Startup error tells operators to configure R2 storage or explicitly allow in-memory.
- Health endpoint currently does not verify SQL connectivity.
- `AppState.task_progress` and `AppState.task_timeline` are in-memory maps. Durable task record currently stores `last_progress`, but live maps are not durable.

Роль зоны:

- Runtime storage selection, startup validation, health, reconciliation and web production safety.

Что это означает для перехода:

- `WebStoreKind::R2` must be replaced by SQLx/Postgres store kind.
- `OXIDE_WEB_STORE=r2` must disappear.
- Durable web startup should fail if SQL is required and unavailable.
- Health check should include database connectivity/migration status.
- Progress persistence must be consciously designed: coalesced SQL latest snapshot or strictly in-memory plus task event replay. The PRD target chooses a coalesced latest progress row separate from append-only event stream.

### 4.14 Telegram runtime and bot integration

Затронутые файлы:

- `crates/oxide-agent-transport-telegram/src/runner.rs`
- `crates/oxide-agent-transport-telegram/src/bot/context.rs`
- `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/session.rs`
- `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/execution_config.rs`
- `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/reminders.rs`
- `crates/oxide-agent-transport-telegram/src/bot/topic_route.rs`
- `crates/oxide-agent-telegram-bot/src/main.rs`

Найдено:

- `runner.rs` is mostly behind `#[cfg(feature = "storage-s3-r2")]`.
- Without `storage-s3-r2`, Telegram runtime exits with message that R2 is the only durable storage backend.
- `init_storage` calls `storage::build_primary_storage(settings.agent.as_ref())` and runs `check_connection`.
- Business logic uses `Arc<dyn StorageProvider>`, not concrete `R2Storage`, which is good for SQLx replacement.
- `main.rs` includes redaction patterns for R2 env vars and AWS-style secrets.

Роль зоны:

- Telegram production runtime is feature-gated by R2 today.
- Bot logic relies on storage provider for config, memory, control-plane and reminders.

Что это означает для перехода:

- Telegram runner must stop being gated by `storage-s3-r2`.
- The required feature should become SQLx/Postgres or the durable storage dependency should be part of selected profile.
- R2-specific redaction/env naming should be removed or replaced with DB URL redaction.

### 4.15 Tests and integration validation

Затронутые файлы:

- `crates/oxide-agent-telegram-bot/tests/integration_validation.rs`
- `crates/oxide-agent-core/tests/r2_flow_checkpoint_integration.rs`
- `crates/oxide-agent-core/src/storage/tests/*.rs`
- `crates/oxide-agent-transport-web/src/persistence/r2.rs` tests
- `crates/oxide-agent-transport-web/src/server/tests.rs`
- `crates/oxide-agent-core/tests/snapshots/modular_registry_snapshots__*.snap`
- `crates/oxide-agent-core/tests/tool_runtime_static_guards.rs`

Найдено:

- `integration_validation.rs` imports AWS SDK crates and validates R2 credentials with bucket-scoped probe.
- `r2_flow_checkpoint_integration.rs` is ignored by default and requires real R2 credentials.
- Storage tests validate R2 key helpers, user config builders, control-plane locks, reminder helpers and audit paging utilities.
- Web persistence tests in `persistence/r2.rs` use in-memory object store under `cfg(test)`, but still validate R2-style key layout and chunk semantics.
- `server/tests.rs` has a R2-backed app state builder test that expects missing R2 config failure.
- Snapshot tests expect `storage/r2` module/features.

Роль зоны:

- Tests encode the old architecture as expected behavior.

Что это означает для перехода:

- Tests need to move from object-key assertions to schema/query/transaction assertions.
- Integration validation should become Postgres/Supabase-compatible database validation, not bucket probe.
- Web event tests must verify append-only rows and indexed pagination.

### 4.16 Docs, env examples, deployment and CI

Затронутые файлы:

- `.env.example`
- `README.md`
- `AGENTS.md`
- `docs/deploy.md`
- `docs/wiki-memory.md`
- `docs/tips/cache-hit.md`
- `docs/goals/2026-05-27-web-console-v1.md`
- `docs/goals/2026-06-04-web-server-slice-refactor.md`
- `docs/prd/implemented/PRD_web.md`
- `.github/workflows/ci-cd.yml`
- `docker-compose.yml`
- `docker-compose.telegram*.yml`
- `docker-compose.web*.yml`
- `docker/compose*.yml`

Найдено:

- `.env.example` documents Cloudflare R2 storage and `OXIDE_R2_*` vars.
- `.env.example` says profiles with `storage/r2` automatically use R2 and `OXIDE_WEB_STORE=r2` can force it.
- `README.md` describes context management and wiki memory as S3/R2-backed.
- `AGENTS.md` says `storage-s3-r2` is the only production durable storage.
- `docs/wiki-memory.md` says Postgres was fully removed and durable memory is S3/R2-backed.
- `docs/deploy.md` lists Cloudflare R2/S3-compatible storage as production prerequisite.
- CI workflow injects dummy R2 env vars for tests, uses R2 secrets for validation and deployment, writes R2 vars to `.env` on server.
- Docker compose files mostly consume `.env`, while build args use profiles that currently include `storage-s3-r2`.

Роль зоны:

- Operator-facing and agent-facing docs currently teach the R2 architecture.
- CI/CD currently expects R2 secrets and validates R2 credentials.

Что это означает для перехода:

- All current docs/examples must be rewritten around local Postgres and Supabase Postgres.
- CI must provide Postgres service for SQL integration tests.
- Deployment secrets must replace `OXIDE_R2_*` with database connection vars.

## 5. Blast Radius

### 5.1 Core storage

Затронутые файлы/модули:

- `crates/oxide-agent-core/src/storage/mod.rs`
- `crates/oxide-agent-core/src/storage/provider.rs`
- `crates/oxide-agent-core/src/storage/modules.rs`
- `crates/oxide-agent-core/src/storage/error.rs`
- `crates/oxide-agent-core/src/storage/telemetry.rs`
- `crates/oxide-agent-core/src/storage/r2*.rs`
- `crates/oxide-agent-core/src/storage/keys.rs`
- `crates/oxide-agent-core/src/storage/{user,flows,control_plane,reminder,utils}.rs`
- `crates/oxide-agent-core/src/capabilities/compiled.rs`

Роль:

- Single durable backend factory and business-level storage trait.
- Domain record definitions for user config, memory, flows, control-plane, reminders and audit.

Что заменить:

- Replace `R2Storage` with `SqlxStorage`/`PostgresStorage` using `sqlx::PgPool`.
- Replace `R2StorageConfig` with database config.
- Replace object operations with SQL queries and transactions.
- Replace ETag-based optimistic locking with `version` columns, `WHERE version = $n`, row locks or transaction-isolated upserts.
- Replace prefix listing with indexed SQL queries.
- Replace R2 telemetry with SQL storage telemetry.
- Replace `storage/r2` capability with `storage/sqlx` or `storage/postgres`.

Risks:

- `StorageProvider` methods like wiki text currently expose `storage_key` and encode object-storage assumptions.
- Some domain records are currently stored as whole JSON documents; blindly mapping each object to JSONB would preserve the worst part of the old architecture.
- Concurrency semantics differ: ETag conflicts become SQL row lock/version conflicts.

Acceptance criteria:

- `build_primary_storage` returns SQLx/Postgres backend for durable profiles.
- No runtime import of `R2Storage`, `R2StorageConfig`, `aws_sdk_s3`, `aws_config`, `aws_credential_types` or `aws_types` remains.
- `StorageProvider::check_connection` verifies DB connectivity.
- Object-key helpers are not used in runtime durable storage paths.
- Core storage tests verify SQL inserts, updates, conflicts, transactions and indexed list queries.

### 5.2 Web transport / web UI persistence

Затронутые файлы/модули:

- `crates/oxide-agent-transport-web/src/persistence/store.rs`
- `crates/oxide-agent-transport-web/src/persistence/models.rs`
- `crates/oxide-agent-transport-web/src/persistence/r2.rs`
- New planned `crates/oxide-agent-transport-web/src/persistence/sqlx.rs`
- `crates/oxide-agent-transport-web/src/server/types.rs`
- `crates/oxide-agent-transport-web/src/bin/oxide-agent-web-console.rs`
- Web contract crates under `crates/oxide-agent-web-contracts/src/*`

Роль:

- Durable web auth/users/sessions/tasks/events/files and startup reconciliation.

Что заменить:

- Replace `R2WebUiStore` and `ObjectStoreWebUiStore` with `SqlxWebUiStore`.
- Replace login-index object with unique SQL constraints on `login_identities.normalized_login`.
- Replace browser session prefix scans with indexed `auth_sessions` queries.
- Replace session/task listing prefix scans with indexed `web_sessions` and `web_tasks` queries.
- Replace task event chunks with append-only rows in `web_task_events`.
- Replace task file object blobs with bounded Postgres `bytea` rows or a deliberately limited `web_task_file_blobs` table.
- Replace startup reconciliation prefix scan with `UPDATE ... WHERE status IN ('queued','running') RETURNING *`.

Risks:

- Existing `WebUiStore` saves whole `WebTaskRecord`; if implemented as one JSONB column, task listing/querying stays poor.
- Task event volume can grow fast and stress DB/WAL if every small progress tick is persisted as a full event plus progress update.
- Task file blobs in Postgres can make backups and WAL large.

Acceptance criteria:

- `WebUiStore` has SQLx implementation used by production web startup.
- Task events are inserted as rows with unique `(user_id, session_id, task_id, seq)`.
- Event listing uses `WHERE seq > $after ORDER BY seq LIMIT $limit + 1` and does not scan all task events.
- Restart reconciliation updates unfinished tasks through SQL and does not list object prefixes.
- Web startup does not mention R2 or `OXIDE_WEB_STORE=r2`.

### 5.3 Task execution

Затронутые файлы/модули:

- `crates/oxide-agent-transport-web/src/session.rs`
- `crates/oxide-agent-transport-web/src/server/task_routes.rs` if present in current route split, otherwise task route handlers under `server/*`
- `crates/oxide-agent-transport-web/src/server/types.rs`
- `crates/oxide-agent-core/src/agent/*` checkpoint/progress modules
- `crates/oxide-agent-core/src/storage/flows.rs`
- `crates/oxide-agent-core/src/storage/r2_memory.rs`

Роль:

- Runs agent tasks, creates/updates web task records, persists flow memory checkpoint and exposes progress/events to UI.

Что заменить:

- Ensure `StorageFlowCheckpoint` persists via SQL-backed `StorageProvider`.
- Persist execution metadata in `web_tasks`, `agent_flows` and optionally `task_execution_runs`.
- Keep live in-memory progress for low-latency UI, but persist latest progress through coalesced/debounced SQL writes.
- Avoid writing every progress tick into both event stream and task row.

Risks:

- Hot-row updates on `web_tasks.last_progress` can contend and create WAL churn.
- Memory checkpoint snapshots may still be large JSONB writes.
- Existing checkpoint coalescing test is R2-specific and must be reproduced against SQL.

Acceptance criteria:

- Flow memory survives restart through SQL.
- Task status/final response/pending input/last event seq survive restart through SQL.
- Progress persistence is bounded by debounce/coalescing policy.
- Large task smoke test does not rewrite large JSON blobs per event batch.

### 5.4 Task event stream

Затронутые файлы/модули:

- `crates/oxide-agent-transport-web/src/persistence/r2.rs`
- `crates/oxide-agent-transport-web/src/persistence/store.rs`
- `crates/oxide-agent-web-contracts/src/events.rs`
- SSE/task event routes under `crates/oxide-agent-transport-web/src/server/*`

Роль:

- Durable replayable task event log for web UI and SSE reconnect.

Что заменить:

- Replace `WebTaskEventChunkRecord` JSON chunks with `web_task_events` table.
- Keep `PersistedTaskEvent` contract, but map fields to columns: seq, kind, summary, payload JSONB, redacted, truncated, created_at.
- Insert event batches in one transaction.
- Use idempotency/unique seq to avoid duplicates on retry.
- Add separate `web_task_progress` latest snapshot table for progress-heavy updates.

Risks:

- Some callers may rely on merge/update semantics of chunk records. SQL append-only should reject duplicate seq or treat identical duplicate as idempotent.
- Event payload JSONB could become unbounded without truncation limits.

Acceptance criteria:

- `append_task_events` never reads old event rows to append new rows except optional duplicate check.
- `list_task_events` uses indexed pagination and returns `has_more` via `limit + 1`.
- Progress events do not cause object-storage operation amplification because object storage is absent.
- Event retention and cleanup jobs are documented and tested.

### 5.5 Reminders

Затронутые файлы/модули:

- `crates/oxide-agent-core/src/storage/reminder.rs`
- `crates/oxide-agent-core/src/storage/r2_reminder.rs`
- `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/reminders.rs`
- `crates/oxide-agent-transport-telegram/src/bot/reminder_scheduler.rs`

Роль:

- Durable scheduled reminders with status, lease, retries and schedule computation.

Что заменить:

- Replace object-per-reminder RMW with `reminders` table.
- Implement due claiming through SQL transaction and row locks.
- Implement status transitions with `version = version + 1` and guarded `WHERE status IN (...)` predicates.
- Implement list queries with indexes by user/context/status/due time.

Risks:

- Multi-worker behavior changes from ETag conflict loops to DB row locks.
- Incorrect due-claim query can double-run jobs or starve older jobs.

Acceptance criteria:

- Two concurrent claimers cannot claim the same reminder.
- Lease expiry makes abandoned jobs claimable again.
- Scheduled, paused, cancelled, completed and failed transitions are tested.
- Due query does not scan all user reminders in application memory.

### 5.6 Control-plane / audit

Затронутые files/modules:

- `crates/oxide-agent-core/src/storage/control_plane.rs`
- `crates/oxide-agent-core/src/storage/r2_control_plane.rs`
- `crates/oxide-agent-core/src/agent/providers/manager_control_plane/*`
- Telegram manager/control-plane handlers.

Роль:

- Durable manager state: profiles, topic context, topic AGENTS.md, infra config, topic bindings, secret values and audit.

Что заменить:

- Typed SQL tables for known records.
- JSONB only for flexible `profile` and `payload` fields.
- Transactional upserts with version checks.
- Append-only `audit_events` with per-user version allocation.
- Indexed queries for agent profile list, audit pages and topic lookups.

Risks:

- Current R2 implementation uses local locks plus ETag. SQL implementation must define exact transaction boundaries.
- Topic prompt duplicate guard currently depends on a synthetic object key; SQL version must preserve the intended conflict behavior.
- Secret values need careful logging/redaction and future encryption story.

Acceptance criteria:

- Agent profile/topic/infra/binding CRUD works without object keys.
- Audit append never rewrites prior audit events.
- Audit pagination returns stable pages by descending version.
- Concurrency tests prove version increments and conflict handling.

### 5.7 Wiki memory

Затронутые files/modules:

- `crates/oxide-agent-core/src/agent/wiki_memory/*`
- `crates/oxide-agent-core/src/storage/provider.rs`
- `crates/oxide-agent-core/src/storage/keys.rs`
- `crates/oxide-agent-core/src/storage/r2_provider.rs`
- `docs/wiki-memory.md`

Роль:

- Durable markdown pages that are injected into agent context.

Что заменить:

- Store wiki pages in `wiki_pages` SQL table with `scope_kind`, `user_id`, `context_id`, `path`, `page_kind`, `content`, size/hash metadata and timestamps.
- Replace deterministic object key as storage identity with deterministic SQL path/scope identity.
- Implement context delete as SQL scoped delete/update.
- Preserve bounded content policy and dirty flush semantics if existing writer expects them.

Risks:

- Wiki code currently describes S3-safe keys and may pass storage keys through patch/cache layers.
- If global wiki pages are not user-scoped, schema must define ownership and collision rules.
- Large markdown raw archives can grow DB quickly.

Acceptance criteria:

- Wiki read/write/delete/list flows work through SQL-backed `StorageProvider`.
- No wiki runtime code mentions S3/R2 object storage.
- Content size limits are enforced before SQL insert/update.
- `delete_session` removes related wiki context rows through SQL without prefix delete.

### 5.8 User/profile/config state

Затронутые files/modules:

- `crates/oxide-agent-core/src/storage/user.rs`
- `crates/oxide-agent-core/src/storage/control_plane.rs`
- `crates/oxide-agent-core/src/storage/r2_user.rs`
- `crates/oxide-agent-core/src/storage/r2_control_plane.rs`
- `crates/oxide-agent-transport-telegram/src/bot/context.rs`
- `crates/oxide-agent-transport-web/src/session.rs`

Роль:

- Persistent state for user context routing, current flow, profiles, model/profile defaults and control-plane config.

Что заменить:

- Normalize `UserConfig.contexts` into `user_contexts` rows.
- Store user-level state in `user_configs`.
- Store web defaults in `web_users` or `user_preferences`.
- Store profile/topic/config entities in typed tables.

Risks:

- Backwards compatibility is intentionally absent, so all defaults on fresh DB must be correct.
- Application logic that expects absent JSON object to mean default config must map cleanly to “no row yet”.

Acceptance criteria:

- Fresh user with no rows receives same logical defaults as missing R2 config.
- Updating one context does not rewrite other contexts.
- Profile/default config queries are indexed and isolated by user.

### 5.9 Deployment/configuration

Затронутые files/modules:

- `.env.example`
- `profiles/*.toml`
- `docker-compose*.yml`
- `docker/compose*.yml`
- `docker/Dockerfile.app`
- `.github/workflows/ci-cd.yml`
- `docs/deploy.md`
- `README.md`

Роль:

- Operator setup, profile selection, build args, CI env, deployment secrets.

Что заменить:

- Replace `storage/r2` profile entries with SQLx/Postgres storage module.
- Replace `OXIDE_R2_*` vars with DB connection vars.
- Add local Postgres setup docs and optional compose service.
- Add Supabase Postgres setup docs.
- Update CI to run a Postgres service and database migrations.
- Remove credential validation job that probes R2 bucket.

Risks:

- Supabase connection limits are plan-dependent and can be hit by default pool sizes.
- Local Postgres can hurt developer experience if setup is not documented or automated.
- CI might use plain Postgres while production uses Supabase pooler; compatibility needs explicit checks.

Acceptance criteria:

- Fresh local setup works with local Postgres and documented env vars.
- Fresh production setup works with Supabase Postgres connection URL.
- No `.env.example`, README, deploy docs or CI secrets require R2.
- Docker build profiles no longer include `storage-s3-r2`.

### 5.10 Tests

Затронутые files/modules:

- `crates/oxide-agent-telegram-bot/tests/integration_validation.rs`
- `crates/oxide-agent-core/tests/r2_flow_checkpoint_integration.rs`
- `crates/oxide-agent-core/src/storage/tests/*`
- `crates/oxide-agent-transport-web/src/persistence/r2.rs`
- `crates/oxide-agent-transport-web/src/server/tests.rs`
- Snapshot tests and static guards.

Роль:

- Current tests encode R2 semantics and feature topology.

Что заменить:

- Add SQL integration tests against disposable Postgres DB.
- Rewrite R2 credential validation as DB connectivity/migration validation.
- Replace object-key tests with schema/addressing tests where still needed.
- Replace web R2 persistence tests with SQLx persistence tests.
- Update static guards to forbid R2/AWS imports and ensure SQLx path is used.

Risks:

- SQL integration tests need reliable Postgres in CI.
- SQLx compile-time checked queries may require `DATABASE_URL` or offline metadata strategy.

Acceptance criteria:

- `cargo test --workspace` passes without R2 env vars.
- CI has a Postgres service and runs migrations/tests.
- Grep guard proves no runtime R2/S3/AWS references remain except historical docs/changelog if explicitly allowed.

### 5.11 Documentation

Затронутые files/modules:

- `README.md`
- `AGENTS.md`
- `.env.example`
- `docs/deploy.md`
- `docs/wiki-memory.md`
- `docs/tips/cache-hit.md`
- Historical PRDs/goals under `docs/prd/implemented` and `docs/goals`.

Роль:

- Instructions for humans and future agents.

Что заменить:

- Replace architecture text from S3/R2 to SQLx/Postgres/Supabase.
- Remove Cloudflare R2 prerequisite and env vars.
- Document fresh DB setup and intentional absence of migration.
- Update wiki memory docs to describe SQL rows and retention.
- Mark old implemented PRDs/goals as historical if they mention R2; do not edit history unless docs policy wants it.

Risks:

- Historical docs can cause grep false positives. Need define allowed historical paths if grep acceptance allows changelog/history only.
- Agents may follow stale `AGENTS.md` instructions if not updated early.

Acceptance criteria:

- Current setup docs mention Postgres/Supabase, not R2.
- `.env.example` has database vars and no `OXIDE_R2_*`.
- `AGENTS.md` says SQLx/Postgres is production durable storage.

### 5.12 CI/build features

Затронутые files/modules:

- Workspace Cargo files.
- `.github/workflows/ci-cd.yml`
- Build/check scripts if any are later found to assert `storage-s3-r2`.
- Docker build args in compose files.

Роль:

- Controls dependency graph, feature composition, build checks and deployment.

Что заменить:

- Remove `storage-s3-r2` feature and AWS SDK dependencies.
- Add SQLx feature/dependency in the crates that own SQL queries.
- Update profile features to include SQLx/Postgres storage.
- Update CI env, test jobs and deployment env propagation.
- Add cargo-tree deny guard for AWS SDK if existing scripts support dependency-boundary checks.

Risks:

- SQLx dependency increases compile time and may require TLS/runtime feature choices.
- Query macros can complicate cross-crate CI if migrations are not in a stable location.

Acceptance criteria:

- `cargo tree` for production profiles shows SQLx/Postgres and no AWS SDK/S3 crates.
- Binary required-features no longer mention `storage-s3-r2`.
- CI does not require R2 secrets.

## 6. Target Architecture

### 6.1 High-level design

The target durable storage architecture should be a single SQLx/Postgres storage layer shared by core and web transport.

Core principles:

- Postgres is the primary dialect.
- SQLx is the Rust DB access layer.
- Fresh database only. No data migration from R2.
- Runtime durable storage uses SQLx + Postgres in all production-like modes.
- Local development/self-host uses local PostgreSQL.
- Web/production uses Supabase Postgres.
- Object storage is not part of durable runtime architecture.
- Append-only data is append-only in SQL: task events and audit events are rows.
- Job queues are SQL-native: reminders use row locks/leases.
- Flexible payloads can use JSONB, but typed columns must exist for identifiers, status, timestamps, pagination and query filters.

### 6.2 Crates/modules to add or change

Recommended additions in `oxide-agent-core`:

- `crates/oxide-agent-core/src/storage/sqlx.rs`
  - Defines `SqlxStorage` / `PostgresStorage` struct holding `PgPool` and config.
- `crates/oxide-agent-core/src/storage/sqlx_config.rs`
  - Parses database config from module settings and env.
- `crates/oxide-agent-core/src/storage/sqlx_provider.rs`
  - Implements `StorageProvider` for SQLx backend.
- `crates/oxide-agent-core/src/storage/sqlx_user.rs`
  - User config/state and context rows.
- `crates/oxide-agent-core/src/storage/sqlx_memory.rs`
  - Agent memory snapshots and flow records.
- `crates/oxide-agent-core/src/storage/sqlx_control_plane.rs`
  - Profiles, topic context, topic AGENTS.md, infra config, bindings, secrets, audit.
- `crates/oxide-agent-core/src/storage/sqlx_reminder.rs`
  - Reminder queue and status transitions.
- `crates/oxide-agent-core/src/storage/sqlx_wiki.rs`
  - Wiki text/page storage.
- `crates/oxide-agent-core/src/storage/sqlx_telemetry.rs`
  - Storage-neutral/SQL telemetry.

Recommended additions in `oxide-agent-transport-web`:

- `crates/oxide-agent-transport-web/src/persistence/sqlx.rs`
  - Implements `WebUiStore` over the shared `PgPool`.
- Replace `R2WebUiStore` exports with `SqlxWebUiStore`.
- Replace web startup builder `build_r2_backed_app_state` with SQLx/Postgres builder.

Recommended schema location:

- Top-level `migrations/` directory in repository root.
- Use SQLx migrator from the application startup or deployment command.
- Keep all durable schema in one migration stream because core and web tables share the same database and foreign keys.

Alternative if future implementation wants stricter crate boundaries:

- Add a new workspace crate `oxide-agent-storage-sqlx` only if it materially reduces coupling.
- Do not split migrations across independent crates unless there is a clear migration ordering strategy.

### 6.3 Traits to preserve, extend or delete

Preserve:

- `StorageProvider` as the main business-level abstraction for core/Telegram/web session manager.
- `WebUiStore` as the web transport persistence abstraction.
- Domain records in `storage/{user,flows,control_plane,reminder}.rs` where they still match API behavior.

Extend/change:

- `StorageProvider::check_connection` should become a DB health check and optionally verify migrations.
- Wiki methods currently accept `storage_key`. Prefer a typed wiki API internally, for example scope/context/path, while keeping a compatibility shim only within the SQL implementation if needed during refactor. Do not keep R2 object-key semantics as public architecture.
- `BuiltStorageBackend` should expose enough shared DB services for web startup to avoid creating two independent pools.
- Storage errors should include SQL/database variants, conflict/version variants and migration/config variants.

Delete:

- `R2Storage`, `R2StorageConfig`, `R2WebUiStore`, `ObjectStoreWebUiStore`, `WebObjectStore` if no longer used.
- Raw object operations from durable runtime: `save_json`, `load_json`, `save_bytes`, `load_bytes`, `delete_prefix`, `list_keys_under_prefix`, `list_json_under_prefix`, conditional save by ETag.
- R2-specific key helpers in runtime durable paths.
- `storage-s3-r2` feature and `storage/r2` module.

### 6.4 Where the SQLx pool should live

Recommended:

- Build one `PgPool` during process startup through `storage::build_primary_storage` or a new `storage::build_database_services` function.
- Store the pool inside `SqlxStorage`.
- Share `PgPool` clones with:
  - `Arc<dyn StorageProvider>` implementation in core.
  - `SqlxWebUiStore` in web transport.
  - Health checks and optional migration runner.
- Do not use a global/static pool.
- Do not create separate pools for core and web in the same process unless there is a measured reason.

Possible shape:

```rust
pub struct BuiltStorageBackend {
    pub module_id: &'static str,
    pub provider: Arc<dyn StorageProvider>,
    pub database: Option<SqlxDatabaseHandle>,
}

pub struct SqlxDatabaseHandle {
    pub pool: sqlx::PgPool,
    pub config: SqlxStorageConfig,
}
```

Future implementation can simplify this shape, but the important constraint is one shared pool per process.

### 6.5 Local Postgres configuration

Local mode should use the same SQLx/Postgres code path as production.

Recommended env vars:

- `OXIDE_DATABASE_URL`
  - Canonical database URL for SQLx storage.
  - Example local shape: `postgres://oxide:oxide@localhost:5432/oxide_agent`.
- `DATABASE_URL`
  - Optional fallback for developer tooling and SQLx CLI compatibility.
- `OXIDE_DATABASE_MAX_CONNECTIONS`
  - Default should be conservative, for example 5-10 locally.
- `OXIDE_DATABASE_MIN_CONNECTIONS`
  - Default 0 or 1.
- `OXIDE_DATABASE_CONNECT_TIMEOUT_SECS`
- `OXIDE_DATABASE_ACQUIRE_TIMEOUT_SECS`
- `OXIDE_DATABASE_MIGRATE_ON_STARTUP`
  - Decide in Phase 1 whether enabled by default for local only or always disabled in production.
- `OXIDE_STORAGE_MODULE=storage/sqlx` only if module selection is not fully profile-driven.

Fresh local setup should include:

- Optional compose service for Postgres.
- Database creation instructions.
- `sqlx migrate run` instructions or app startup migration behavior.
- A clear note that old R2 data is intentionally ignored.

### 6.6 Supabase Postgres configuration

Supabase web/production mode should use the same schema and SQLx queries as local Postgres.

Recommended env vars:

- `OXIDE_DATABASE_URL`
  - Supabase Postgres connection string.
  - Include required SSL mode in the URL or documented connection settings.
- `OXIDE_DATABASE_MAX_CONNECTIONS`
  - Must default conservatively for Supabase deployments.
- `OXIDE_DATABASE_CONNECT_TIMEOUT_SECS`
- `OXIDE_DATABASE_ACQUIRE_TIMEOUT_SECS`
- `OXIDE_DATABASE_MIGRATE_ON_STARTUP=false` by default for production unless deployment explicitly opts in.

Supabase-specific notes:

- Treat connection limits as deployment-plan-specific and keep pool sizes small by default.
- Prefer running schema migrations as a deploy step rather than opportunistically from multiple app instances.
- CI should test against standard Postgres and maintain a Supabase compatibility checklist; do not require a real Supabase project in CI.
- No Supabase Storage bucket is part of this target architecture.

### 6.7 Env vars that should disappear

Remove from runtime, examples, CI and deployment docs:

- `OXIDE_R2_ACCESS_KEY_ID`
- `OXIDE_R2_SECRET_ACCESS_KEY`
- `OXIDE_R2_ENDPOINT_URL`
- `OXIDE_R2_ENDPOINT`
- `OXIDE_R2_BUCKET_NAME`
- `OXIDE_R2_BUCKET`
- `OXIDE_R2_REGION`
- `OXIDE_WEB_STORE=r2`
- Module config keys under `storage/r2`:
  - `endpoint`
  - `endpoint_url`
  - `bucket`
  - `bucket_name`
  - `access_key_id`
  - `secret_access_key`
  - `credentials.access_key_id`
  - `credentials.secret_access_key`
  - `region`

### 6.8 Test architecture

Recommended testing layers:

- Unit tests for record builders and validation remain fast and DB-free.
- SQL integration tests run against local/disposable Postgres in CI.
- Use a CI Postgres service, testcontainers, or a scripted ephemeral database. Pick one in Phase 1 and document it.
- Use either SQLx offline metadata or ensure CI provides `DATABASE_URL` for compile-time query checks. Do not make regular `cargo check` depend on a developer’s private Supabase URL.
- Web persistence tests should run the same `WebUiStore` contract against in-memory store and SQLx store where practical.
- Reminder claiming tests must use real concurrent SQL transactions.
- Supabase-specific compatibility is tested through Postgres dialect constraints and a documented manual smoke checklist, not a required Supabase CI dependency.

### 6.9 Fresh setup flow

Fresh setup must be explicit:

- Start with an empty Postgres database.
- Configure `OXIDE_DATABASE_URL`.
- Run schema migrations.
- Start Telegram or web service.
- First web admin/bootstrap flow creates fresh web auth/user records.
- Existing R2 bucket data, if any, is ignored and can be deleted out-of-band.
- There is no R2 importer and no compatibility reader.

## 7. Data Model Draft

This is a draft schema, not final production migrations. It is specific enough for implementation planning and acceptance tests. Names can change during implementation, but the model principles should not change.

General conventions:

- Use Postgres types: `bigint`, `text`, `boolean`, `jsonb`, `bytea`, `timestamptz`, `uuid`.
- Use text + `CHECK` constraints for app enums initially, unless implementation chooses Postgres enum types deliberately.
- Use `created_at` and `updated_at` consistently.
- Use `version bigint not null default 1` on mutable domain records that need optimistic concurrency.
- Use JSONB only for flexible payloads, not as the only data model.
- Use foreign keys with `ON DELETE CASCADE` where scoped cleanup is desired.
- Use retention fields for high-growth data: task events, task files, wiki raw archives and audit if needed.

### 7.1 Users and user config/state

```sql
create table users (
    user_id bigint primary key,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create table user_configs (
    user_id bigint primary key references users(user_id) on delete cascade,
    state text,
    version bigint not null default 1,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create table user_contexts (
    user_id bigint not null references users(user_id) on delete cascade,
    context_key text not null,
    state text,
    current_agent_flow_id text,
    chat_id bigint,
    thread_id bigint,
    forum_topic_name text,
    forum_topic_icon_color integer,
    forum_topic_icon_custom_emoji_id text,
    closed boolean not null default false,
    version bigint not null default 1,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    primary key (user_id, context_key)
);

create index user_contexts_user_updated_idx
    on user_contexts (user_id, updated_at desc);
```

Notes:

- Missing rows should map to existing “default config” behavior.
- Updating one context must not rewrite all user contexts.

### 7.2 Web auth users, login identities and auth sessions

```sql
create table web_users (
    user_id bigint primary key references users(user_id) on delete cascade,
    login text not null,
    normalized_login text not null unique,
    password_hash text not null,
    role text not null check (role in ('user', 'admin')),
    status text not null check (status in ('active', 'disabled')),
    default_model_selection jsonb,
    default_agent_profile_id text,
    default_effort text check (default_effort is null or default_effort in ('standard', 'extended', 'heavy')),
    last_login_at timestamptz,
    schema_version integer not null default 1,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create table login_identities (
    identity_id uuid primary key,
    user_id bigint not null references users(user_id) on delete cascade,
    provider text not null,
    provider_subject text not null,
    normalized_login text,
    password_hash text,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    unique (provider, provider_subject)
);

create unique index login_identities_password_login_uq
    on login_identities (normalized_login)
    where provider = 'password' and normalized_login is not null;

create table auth_sessions (
    session_token_hash text primary key,
    user_id bigint not null references users(user_id) on delete cascade,
    csrf_token text not null,
    created_at timestamptz not null,
    last_seen_at timestamptz not null,
    expires_at timestamptz not null,
    revoked_at timestamptz,
    schema_version integer not null default 1
);

create index auth_sessions_user_active_idx
    on auth_sessions (user_id, expires_at)
    where revoked_at is null;

create index auth_sessions_expiry_idx
    on auth_sessions (expires_at)
    where revoked_at is null;
```

Notes:

- Current `LoginIndexRecord` becomes a uniqueness constraint instead of a separate object.
- `login_identities` gives a future-safe model even if only password login is implemented now.

### 7.3 Web sessions and tasks

```sql
create table web_sessions (
    user_id bigint not null references users(user_id) on delete cascade,
    session_id text not null,
    title text not null,
    context_key text not null,
    context_keys text[] not null default '{}',
    agent_flow_id text not null,
    model_selection jsonb,
    agent_profile_id text,
    active_task_id text,
    last_task_status text check (last_task_status is null or last_task_status in (
        'queued', 'running', 'waiting_for_user_input', 'completed', 'failed', 'cancelled', 'interrupted'
    )),
    last_preview text,
    manually_renamed boolean not null default false,
    schema_version integer not null default 1,
    created_at timestamptz not null,
    updated_at timestamptz not null,
    primary key (user_id, session_id)
);

create index web_sessions_user_updated_idx
    on web_sessions (user_id, updated_at desc);

create table web_tasks (
    user_id bigint not null references users(user_id) on delete cascade,
    session_id text not null,
    task_id text not null,
    version_group_id text not null,
    version_index integer not null default 1,
    parent_task_id text,
    status text not null check (status in (
        'queued', 'running', 'waiting_for_user_input', 'completed', 'failed', 'cancelled', 'interrupted'
    )),
    input_markdown text not null,
    attachments jsonb not null default '[]'::jsonb,
    input_edited_at timestamptz,
    final_response_markdown text,
    error_message text,
    pending_user_input jsonb,
    last_event_seq bigint not null default 0,
    schema_version integer not null default 1,
    created_at timestamptz not null,
    started_at timestamptz,
    updated_at timestamptz not null,
    finished_at timestamptz,
    primary key (user_id, session_id, task_id),
    foreign key (user_id, session_id)
        references web_sessions(user_id, session_id)
        on delete cascade
);

create index web_tasks_session_created_idx
    on web_tasks (user_id, session_id, created_at asc);

create index web_tasks_unfinished_idx
    on web_tasks (status, updated_at)
    where status in ('queued', 'running', 'waiting_for_user_input');

create index web_tasks_version_lineage_idx
    on web_tasks (user_id, session_id, version_group_id, version_index);
```

Notes:

- `attachments` can remain JSONB because it is flexible and bounded metadata, not primary query structure.
- `last_progress` should not live as a frequently rewritten JSON blob in `web_tasks`; use a separate table.

### 7.4 Web task events and progress

```sql
create table web_task_events (
    id bigserial primary key,
    user_id bigint not null,
    session_id text not null,
    task_id text not null,
    seq bigint not null,
    kind text not null,
    summary text not null,
    payload jsonb not null default '{}'::jsonb,
    redacted boolean not null default false,
    truncated boolean not null default false,
    schema_version integer not null default 1,
    created_at timestamptz not null,
    retention_expires_at timestamptz,
    foreign key (user_id, session_id, task_id)
        references web_tasks(user_id, session_id, task_id)
        on delete cascade,
    unique (user_id, session_id, task_id, seq)
);

create index web_task_events_page_idx
    on web_task_events (user_id, session_id, task_id, seq asc);

create index web_task_events_retention_idx
    on web_task_events (retention_expires_at)
    where retention_expires_at is not null;

create table web_task_progress (
    user_id bigint not null,
    session_id text not null,
    task_id text not null,
    current_iteration integer not null,
    max_iterations integer not null,
    is_finished boolean not null,
    error text,
    current_thought text,
    progress_payload jsonb not null default '{}'::jsonb,
    updated_at timestamptz not null,
    primary key (user_id, session_id, task_id),
    foreign key (user_id, session_id, task_id)
        references web_tasks(user_id, session_id, task_id)
        on delete cascade
);
```

Notes:

- `web_task_events` is append-only. Updates/deletes should be limited to retention cleanup and not normal append flow.
- `web_task_progress` is mutable latest snapshot. Writes must be debounced/coalesced.
- Progress event payloads should be truncated/compacted before insert if they can grow large.

### 7.5 Web task files/artifacts

```sql
create table web_task_files (
    user_id bigint not null,
    session_id text not null,
    task_id text not null,
    file_id text not null,
    file_name text not null,
    content_type text not null,
    size_bytes bigint not null check (size_bytes >= 0),
    sha256 text,
    delivery_kind text not null,
    storage_mode text not null default 'postgres_bytea' check (storage_mode in ('postgres_bytea')),
    schema_version integer not null default 1,
    created_at timestamptz not null,
    retention_expires_at timestamptz,
    primary key (user_id, session_id, task_id, file_id),
    foreign key (user_id, session_id, task_id)
        references web_tasks(user_id, session_id, task_id)
        on delete cascade
);

create table web_task_file_blobs (
    user_id bigint not null,
    session_id text not null,
    task_id text not null,
    file_id text not null,
    content bytea not null,
    created_at timestamptz not null default now(),
    primary key (user_id, session_id, task_id, file_id),
    foreign key (user_id, session_id, task_id, file_id)
        references web_task_files(user_id, session_id, task_id, file_id)
        on delete cascade
);

create index web_task_files_retention_idx
    on web_task_files (retention_expires_at)
    where retention_expires_at is not null;
```

Notes:

- Because R2 is fully removed, blobs either live in Postgres or are rejected by size policy.
- A hard max upload/artifact size is mandatory. The current web default upload limit is 200 MB; this is risky for Postgres and should be revisited in Phase 7.
- If large binary artifacts are important, a future non-R2 architecture may need a separate blob service. That is outside this PRD unless explicitly approved later.

### 7.6 Agent flows and memory snapshots

```sql
create table agent_flows (
    user_id bigint not null references users(user_id) on delete cascade,
    context_key text not null,
    flow_id text not null,
    schema_version integer not null default 1,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    primary key (user_id, context_key, flow_id)
);

create index agent_flows_context_updated_idx
    on agent_flows (user_id, context_key, updated_at desc);

create table agent_memory_snapshots (
    user_id bigint not null references users(user_id) on delete cascade,
    scope_kind text not null check (scope_kind in ('user', 'context', 'flow')),
    context_key text not null default '',
    flow_id text not null default '',
    memory jsonb not null,
    content_hash text,
    version bigint not null default 1,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    primary key (user_id, scope_kind, context_key, flow_id)
);

create index agent_memory_context_idx
    on agent_memory_snapshots (user_id, context_key, updated_at desc);
```

Notes:

- This deliberately keeps `AgentMemory` as JSONB snapshot because the structure is complex and used as a checkpoint, not as a relational query surface.
- Snapshot writes must keep existing coalescing/skip-identical semantics.
- If `AgentMemory` grows too large, Phase 7 should add size limits and compaction/retention.

### 7.7 Wiki pages / memory pages

```sql
create table wiki_pages (
    page_id uuid primary key,
    scope_kind text not null check (scope_kind in ('global', 'context')),
    user_id bigint references users(user_id) on delete cascade,
    context_id text,
    path text not null,
    page_kind text not null check (page_kind in ('core', 'page', 'inbox', 'raw')),
    content text not null,
    content_bytes integer not null check (content_bytes >= 0),
    content_hash text,
    schema_version integer not null default 1,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    deleted_at timestamptz
);

create unique index wiki_pages_global_path_uq
    on wiki_pages (path)
    where scope_kind = 'global' and deleted_at is null;

create unique index wiki_pages_context_path_uq
    on wiki_pages (user_id, context_id, path)
    where scope_kind = 'context' and deleted_at is null;

create index wiki_pages_context_list_idx
    on wiki_pages (user_id, context_id, page_kind, updated_at desc)
    where deleted_at is null;
```

Notes:

- `path` should be deterministic and close to current wiki file names, but no longer an object key.
- `context_id` remains deterministic from `(user_id, context_key)` or equivalent wiki scope calculation.
- Global page ownership needs explicit implementation decision. The partial unique index above assumes global paths are shared globally.
- Enforce content limits before write.

### 7.8 Control-plane records

```sql
create table agent_profiles (
    user_id bigint not null references users(user_id) on delete cascade,
    agent_id text not null,
    profile jsonb not null,
    version bigint not null default 1,
    schema_version integer not null default 1,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    primary key (user_id, agent_id)
);

create table topic_contexts (
    user_id bigint not null references users(user_id) on delete cascade,
    topic_id text not null,
    context text not null,
    version bigint not null default 1,
    schema_version integer not null default 1,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    primary key (user_id, topic_id)
);

create table topic_agents_md (
    user_id bigint not null references users(user_id) on delete cascade,
    topic_id text not null,
    agents_md text not null,
    content_hash text,
    version bigint not null default 1,
    schema_version integer not null default 1,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    primary key (user_id, topic_id)
);

create table topic_infra_configs (
    user_id bigint not null references users(user_id) on delete cascade,
    topic_id text not null,
    target_name text not null,
    host text not null,
    port integer,
    remote_user text,
    auth_mode text,
    secret_ref text,
    sudo_secret_ref text,
    environment jsonb not null default '{}'::jsonb,
    tags text[] not null default '{}',
    allowed_tool_modes text[] not null default '{}',
    approval_required_modes text[] not null default '{}',
    version bigint not null default 1,
    schema_version integer not null default 1,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    primary key (user_id, topic_id)
);

create table topic_bindings (
    user_id bigint not null references users(user_id) on delete cascade,
    topic_id text not null,
    agent_id text not null,
    binding_kind text not null,
    chat_id bigint,
    thread_id bigint,
    expires_at timestamptz,
    last_activity_at timestamptz,
    version bigint not null default 1,
    schema_version integer not null default 1,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    primary key (user_id, topic_id)
);

create index topic_bindings_agent_idx
    on topic_bindings (user_id, agent_id, updated_at desc);
```

Notes:

- `profile` remains JSONB because profile shape can be flexible.
- Topic context and AGENTS.md should stay text columns to support limits and search later.
- Topic prompt duplicate guard should be implemented through a transaction and/or a unique content hash/index if the old guard’s behavior is still required.

### 7.9 Private secrets

```sql
create table private_secrets (
    user_id bigint not null references users(user_id) on delete cascade,
    secret_ref text not null,
    secret_value text not null,
    encryption_key_id text,
    version bigint not null default 1,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    primary key (user_id, secret_ref)
);
```

Notes:

- This table replaces `users/{user_id}/private/secrets/{secret_ref}` objects.
- The initial implementation can store plaintext if that matches current behavior, but logs must never include values.
- Future encryption-at-rest can use `encryption_key_id` and app-level encryption without schema churn.

### 7.10 Reminders queue

```sql
create table reminders (
    reminder_id text primary key,
    user_id bigint not null references users(user_id) on delete cascade,
    context_key text not null,
    flow_id text not null,
    chat_id bigint not null,
    thread_id bigint,
    thread_kind text not null,
    task_prompt text not null,
    schedule_kind text not null check (schedule_kind in ('once', 'interval', 'cron')),
    status text not null check (status in ('scheduled', 'leased', 'paused', 'completed', 'failed', 'cancelled')),
    next_run_at timestamptz,
    interval_secs bigint,
    cron_expression text,
    timezone text,
    lease_until timestamptz,
    last_run_at timestamptz,
    last_error text,
    run_count bigint not null default 0,
    version bigint not null default 1,
    schema_version integer not null default 2,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create index reminders_due_claim_idx
    on reminders (next_run_at, created_at)
    where status = 'scheduled';

create index reminders_user_status_idx
    on reminders (user_id, status, next_run_at);

create index reminders_lease_expiry_idx
    on reminders (lease_until)
    where status = 'leased';
```

Due claiming draft:

```sql
with due as (
    select reminder_id
    from reminders
    where status = 'scheduled'
      and next_run_at is not null
      and next_run_at <= $1
      and (lease_until is null or lease_until <= $1)
    order by next_run_at asc, created_at asc
    limit $2
    for update skip locked
)
update reminders r
set status = 'leased',
    lease_until = $3,
    version = version + 1,
    updated_at = $1
from due
where r.reminder_id = due.reminder_id
returning r.*;
```

Notes:

- The exact status naming can preserve existing `ReminderJobStatus` variants, but SQL must support safe due claiming.
- Completion/reschedule/fail transitions should be guarded by current status/version.

### 7.11 Audit events

```sql
create table audit_stream_versions (
    user_id bigint primary key references users(user_id) on delete cascade,
    next_version bigint not null default 1
);

create table audit_events (
    id bigserial primary key,
    event_id text not null unique,
    user_id bigint not null references users(user_id) on delete cascade,
    version bigint not null,
    topic_id text,
    agent_id text,
    action text not null,
    payload jsonb not null default '{}'::jsonb,
    schema_version integer not null default 1,
    created_at timestamptz not null default now(),
    unique (user_id, version)
);

create index audit_events_user_page_idx
    on audit_events (user_id, version desc);

create index audit_events_action_idx
    on audit_events (user_id, action, created_at desc);
```

Version allocation draft:

- In one transaction, upsert/select `audit_stream_versions` row for `user_id` with row lock.
- Use `next_version` as event version.
- Increment `next_version`.
- Insert audit event.

Notes:

- Do not store audit as one JSON array.
- Event payload can be JSONB, but identifiers/action/timestamps/version must be columns.

### 7.12 Task execution metadata

Current durable task execution metadata is mostly in `WebTaskRecord` and `AgentFlowRecord`. If implementation needs a separate entity for cross-transport task execution metadata, use a bounded table like:

```sql
create table task_execution_runs (
    run_id uuid primary key,
    user_id bigint not null references users(user_id) on delete cascade,
    transport text not null,
    context_key text,
    flow_id text,
    web_session_id text,
    web_task_id text,
    model_selection jsonb,
    agent_profile_id text,
    sandbox_scope jsonb,
    status text not null,
    started_at timestamptz,
    finished_at timestamptz,
    metadata jsonb not null default '{}'::jsonb,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create index task_execution_runs_user_created_idx
    on task_execution_runs (user_id, created_at desc);
```

Notes:

- This table is optional for Phase 2/3 if `web_tasks` and `agent_flows` fully cover current durable metadata.
- Do not add it just to create another JSONB catch-all; add it only if a real cross-transport execution query/use case exists.

## 8. Configuration and Fresh Setup

### 8.1 New configuration model

Recommended module config:

```toml
[modules]
"storage/sqlx" = { enabled = true }

[modules."storage/sqlx"]
database_url = "postgres://oxide:oxide@localhost:5432/oxide_agent"
max_connections = 10
connect_timeout_secs = 10
acquire_timeout_secs = 10
migrate_on_startup = false
```

Recommended env model:

- `OXIDE_DATABASE_URL`
- `DATABASE_URL` as optional SQLx/developer fallback.
- `OXIDE_DATABASE_MAX_CONNECTIONS`
- `OXIDE_DATABASE_MIN_CONNECTIONS`
- `OXIDE_DATABASE_CONNECT_TIMEOUT_SECS`
- `OXIDE_DATABASE_ACQUIRE_TIMEOUT_SECS`
- `OXIDE_DATABASE_MIGRATE_ON_STARTUP`
- `OXIDE_DATABASE_STATEMENT_TIMEOUT_MS` if implementation adds per-connection settings.

Recommended web store selection:

- Production web should use SQLx automatically when durable storage is required.
- Remove `OXIDE_WEB_STORE=r2`.
- If `OXIDE_WEB_STORE` remains, allowed production value should be `sqlx` or omitted. In-memory must remain explicit dev/test only.

### 8.2 Local PostgreSQL fresh setup

Expected local flow:

- Start local Postgres. A compose service is recommended for contributors.
- Create database/user, for example `oxide_agent` / `oxide`.
- Export `OXIDE_DATABASE_URL` or put it in `.env`.
- Run schema migrations:
  - `sqlx migrate run`, or
  - app startup migration if `OXIDE_DATABASE_MIGRATE_ON_STARTUP=true` is chosen for local dev.
- Start Telegram or web profile.
- Register/bootstrap users normally.
- Do not configure R2.

Acceptance criteria for docs:

- A new contributor can start local Postgres and run web console without R2 credentials.
- `.env.example` has DB vars and no R2 storage block.
- Fresh setup explicitly says old R2 buckets are ignored.

### 8.3 Supabase web/production fresh setup

Expected production flow:

- Create a Supabase project and obtain a Postgres connection URL.
- Set `OXIDE_DATABASE_URL` in deployment secrets.
- Set conservative pool limits.
- Run SQL migrations as deployment step.
- Start web service.
- Use web bootstrap/admin registration to create fresh app state.
- Do not create or configure Supabase Storage buckets for Oxide durable state.

Acceptance criteria for docs:

- Supabase setup explains connection URL, SSL requirement if needed by the URL, pool size caution and migration step.
- CI/deploy docs no longer mention R2 secrets.
- Production deployment does not require object-storage credentials.

## 9. Phased Plan

### Phase 0 — Recon and deletion map

Цель:

- Завершить карту R2/S3/AWS surface area и зафиксировать deletion map до реализации.

Затронутые файлы/модули:

- All files listed in `Current R2 Surface Area`.
- Workspace Cargo files.
- Profiles, docs, `.env.example`, CI.
- Tests and snapshots.

Конкретные задачи:

- Verify all references with targeted grep:
  - `R2`, `S3`, `Cloudflare`, `OXIDE_R2`, `storage-s3-r2`, `storage/r2`, `aws-sdk`, `aws_credential`, `aws_types`, `bucket`, `etag`, `list_keys_under_prefix`, `delete_prefix`.
- Classify references as runtime, test-only, docs/current, docs/historical or false positive.
- Produce final deletion list for:
  - R2 modules.
  - AWS dependencies.
  - Cargo features/profile entries.
  - Env vars/secrets.
  - Docs/examples.
  - CI/deploy env.
  - R2-specific tests.
- Produce SQL entity list matching the Data Model Draft.
- Confirm whether any durable state exists outside the scanned list.

Acceptance criteria:

- Deletion map covers core, web, Telegram, docs, tests, CI and profiles.
- Every runtime R2/S3/AWS reference has a planned removal/replacement.
- Future SQL entities are listed and mapped to old object namespaces.
- No SQLite work is added to the plan.

Risks:

- Generic words like `prefix` and `cloudflare` can produce false positives outside storage.
- Historical docs may intentionally keep R2 references; future grep acceptance must define allowed historical locations.

Заметки для реализации:

- This PRD is the initial Phase 0 artifact. Future agents should update it only with evidence-backed findings.
- Do not start code deletion until SQLx foundation has at least one passing path, unless the branch explicitly chooses big-bang replacement.

### Phase 1 — SQLx foundation

Цель:

- Add SQLx/Postgres foundation without porting business logic yet.

Затронутые файлы/модули:

- `crates/oxide-agent-core/Cargo.toml`
- `crates/oxide-agent-transport-web/Cargo.toml`
- `crates/oxide-agent-core/src/storage/mod.rs`
- `crates/oxide-agent-core/src/storage/modules.rs`
- New `sqlx_*` storage modules.
- New top-level `migrations/` directory.
- `.env.example`
- `docs/deploy.md`
- CI workflow.

Конкретные задачи:

- Add SQLx dependency with Postgres/runtime/TLS/json/chrono/uuid/migrate features.
- Create `SqlxStorageConfig` from env and module settings.
- Build a `PgPool` with configurable pool limits and timeouts.
- Add `storage/sqlx` capability module and profile entries.
- Add DB health check.
- Add migration runner strategy.
- Add first migration with base tables or a minimal `storage_health`/schema version setup if doing incremental migrations.
- Add local Postgres test strategy in CI.
- Document fresh local and Supabase setup.

Acceptance criteria:

- Code builds with SQLx dependency and without using SQLx for business records yet.
- A startup path can create/connect a `PgPool` and run/verify health check.
- CI can start a Postgres service or otherwise provide test DB.
- `storage/sqlx` appears in capability manifest and profiles under a temporary coexistence model if R2 is not removed yet.
- No SQLite dependency/feature is introduced.

Risks:

- SQLx query macros may need `DATABASE_URL` at compile time. Mitigate with offline metadata or runtime-checked queries where appropriate.
- Pool defaults can overwhelm Supabase. Start conservative.

Заметки для реализации:

- Keep this phase thin. Do not port all storage logic here.
- Decide early whether migrations run on startup or deploy step. Production should prefer deploy step.

### Phase 2 — Web persistence on SQLx

Цель:

- Move web users/auth/sessions/tasks/task events/task files from R2 object store to SQLx.

Затронутые файлы/модули:

- `crates/oxide-agent-transport-web/src/persistence/store.rs`
- `crates/oxide-agent-transport-web/src/persistence/models.rs`
- `crates/oxide-agent-transport-web/src/persistence/r2.rs`
- New `crates/oxide-agent-transport-web/src/persistence/sqlx.rs`
- `crates/oxide-agent-transport-web/src/persistence/mod.rs`
- `crates/oxide-agent-transport-web/src/server/types.rs`
- `crates/oxide-agent-transport-web/src/bin/oxide-agent-web-console.rs`
- `crates/oxide-agent-web-contracts/src/*` if minor mapping changes are required.

Конкретные задачи:

- Add migrations for `users`, `web_users`, `login_identities`, `auth_sessions`, `web_sessions`, `web_tasks`, `web_task_events`, `web_task_progress`, `web_task_files`, `web_task_file_blobs`.
- Implement `SqlxWebUiStore` for the existing `WebUiStore` trait.
- Implement login uniqueness with SQL unique constraints.
- Implement auth session load/revoke/revoke-all-except with indexed queries.
- Implement session CRUD/list/delete with SQL and cascades.
- Implement task CRUD/list and startup reconciliation with SQL.
- Implement append-only task event insert and pagination.
- Implement coalesced progress persistence separately from event append.
- Implement task file save/load with Postgres size limits.
- Update web startup to prefer SQLx durable store.
- Remove `R2WebUiStore` from production path after SQL tests pass.

Acceptance criteria:

- Web auth/users/sessions/tasks/events/files work against SQLx store.
- No R2 dependency in web persistence path.
- Task events are append-only rows and paginated by indexed seq.
- Large task event smoke does not rewrite large JSON chunks.
- `mark_unfinished_tasks_interrupted` uses SQL update/query, not object prefix listing.
- Web restart preserves sessions/tasks/events/final responses through SQL.

Risks:

- `WebUiStore` currently accepts full record structs and may encourage whole-row overwrite. Keep typed columns and use full-row mapping carefully.
- Task file blobs can create DB growth. Enforce limit and retention metadata now.

Заметки для реализации:

- Preserve API contracts while changing storage implementation.
- Keep in-memory web store for hermetic unit tests only if still valuable.
- Add a contract test suite that can run against both in-memory and SQLx implementations.

### Phase 3 — Core durable state on SQLx

Цель:

- Move user config/state/profile/topic/control-plane durable records and agent memory/flows to SQLx.

Затронутые файлы/модули:

- `crates/oxide-agent-core/src/storage/provider.rs`
- `crates/oxide-agent-core/src/storage/user.rs`
- `crates/oxide-agent-core/src/storage/flows.rs`
- `crates/oxide-agent-core/src/storage/control_plane.rs`
- `crates/oxide-agent-core/src/storage/r2_user.rs`
- `crates/oxide-agent-core/src/storage/r2_memory.rs`
- `crates/oxide-agent-core/src/storage/r2_control_plane.rs`
- New `sqlx_user.rs`, `sqlx_memory.rs`, `sqlx_control_plane.rs`.
- Telegram and web consumers that use `StorageProvider`.

Конкретные задачи:

- Add migrations for `user_configs`, `user_contexts`, `agent_flows`, `agent_memory_snapshots`, `agent_profiles`, `topic_contexts`, `topic_agents_md`, `topic_infra_configs`, `topic_bindings`, `private_secrets`.
- Implement `StorageProvider` user config methods using typed rows.
- Implement context state updates without rewriting full user config.
- Implement agent memory snapshots and flow records with coalesced writes and content hash skip.
- Implement profile/topic/infra/binding CRUD with version columns.
- Implement secrets table and redaction safeguards.
- Update control-plane concurrency from local locks/ETags to SQL transactions.
- Keep JSONB only for flexible structures: profile JSON, memory snapshot, environment metadata.

Acceptance criteria:

- Telegram and web session manager can use SQL-backed `StorageProvider`.
- User state/context/profile/topic/infra/binding operations are SQL-backed.
- Multi-record updates are transaction-safe where old behavior required consistency.
- Agent memory for flow persists and reloads through SQL.
- R2 user/memory/control-plane modules are no longer used by production paths.

Risks:

- The old `UserConfig` HashMap model may hide edge cases around missing/default context rows.
- Topic prompt duplicate guard behavior is underspecified and needs test-backed translation.
- Secret values need careful handling to avoid DB logs or tracing leaks.

Заметки для реализации:

- Start with trait-level integration tests using `Arc<dyn StorageProvider>` so Telegram/web consumers do not care about backend.
- Keep domain validation functions from existing storage modules.

### Phase 4 — Reminders and audit on SQLx

Цель:

- Replace R2 reminders and audit with SQL-native models.

Затронутые файлы/модули:

- `crates/oxide-agent-core/src/storage/reminder.rs`
- `crates/oxide-agent-core/src/storage/r2_reminder.rs`
- `crates/oxide-agent-core/src/storage/r2_control_plane.rs` audit methods.
- New `sqlx_reminder.rs` and audit portion of `sqlx_control_plane.rs`.
- `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/reminders.rs`
- `crates/oxide-agent-transport-telegram/src/bot/reminder_scheduler.rs`

Конкретные задачи:

- Add/complete migrations for `reminders`, `audit_stream_versions`, `audit_events`.
- Implement reminder CRUD/status transitions over SQL.
- Implement due claiming with `FOR UPDATE SKIP LOCKED` or equivalent transaction-safe Postgres pattern.
- Implement lease expiry and recovery.
- Implement audit append with per-user version allocation in one transaction.
- Implement audit list/page queries with indexes.
- Add concurrency tests for reminder claiming and audit version allocation.

Acceptance criteria:

- Reminder scheduler works without R2.
- Concurrent claimers do not double-claim the same job.
- Audit append is append-only and never rewrites an event array.
- Audit pagination is stable and indexed.
- Old R2 reminder/audit methods are unused.

Risks:

- Reminders can be business-critical; status transition predicates must be strict.
- Audit per-user versions need a robust transaction pattern.

Заметки для реализации:

- Implement due claim as a small, reviewed SQL transaction with tests before wiring scheduler.
- Use database timestamps consistently; convert to/from existing Unix seconds at API boundary if records still use seconds.

### Phase 5 — Wiki/memory on SQLx

Цель:

- Remove R2 from wiki/memory storage.

Затронутые файлы/модули:

- `crates/oxide-agent-core/src/agent/wiki_memory/*`
- `crates/oxide-agent-core/src/storage/provider.rs`
- `crates/oxide-agent-core/src/storage/keys.rs`
- `crates/oxide-agent-core/src/storage/r2_provider.rs`
- New `sqlx_wiki.rs`.
- `docs/wiki-memory.md`
- `docs/tips/cache-hit.md`

Конкретные задачи:

- Add/complete migration for `wiki_pages`.
- Define deterministic mapping from current wiki scope/path concepts to SQL columns.
- Implement load/save/delete wiki text through SQL.
- Implement delete wiki context through scoped SQL delete/update.
- Enforce content size and raw archive retention policies.
- Update wiki cache/patch comments and docs from object keys to SQL page paths.
- Remove any runtime dependency on `wiki_context_prefix` or object prefix deletes.

Acceptance criteria:

- Wiki pages are persisted as SQL rows.
- Reads/writes/deletes work without object storage.
- Deterministic path/scope addressing is preserved at logical level.
- Content limits are enforced.
- Dirty flush semantics remain correct if used by writer/cache.
- No R2 object storage usage remains in wiki memory runtime.

Risks:

- Existing wiki API may need object-key parsing as a temporary internal shim. Keep that shim short-lived and do not present it as architecture.
- Global vs user/context-scoped pages need explicit ownership rules.

Заметки для реализации:

- Prefer changing internal wiki store API to typed `WikiPageAddress` over preserving string object keys.
- Keep markdown content in `text`, not JSONB.

### Phase 6 — R2 removal

Цель:

- Physically delete R2/S3 code, config and dependencies after SQL paths cover durable runtime.

Затронутые файлы/модули:

- All `r2*.rs` storage modules.
- `crates/oxide-agent-transport-web/src/persistence/r2.rs`.
- Cargo features/dependencies in core, Telegram bot, Telegram transport, web transport.
- Profiles under `profiles/*.toml`.
- Capability registry and snapshots.
- CI/deploy docs/env.
- R2-specific tests.

Конкретные задачи:

- Delete `crates/oxide-agent-core/src/storage/r2.rs`.
- Delete `r2_base.rs`, `r2_config.rs`, `r2_provider.rs`, `r2_user.rs`, `r2_memory.rs`, `r2_control_plane.rs`, `r2_reminder.rs`.
- Delete or rewrite `keys.rs` if only object-key helpers remain. Keep only non-object helpers such as flow id generation if needed, likely moved elsewhere.
- Delete `R2WebUiStore` and object-store web persistence layer.
- Remove AWS SDK dependencies from all Cargo files.
- Remove `storage-s3-r2` feature from all crates and profile feature compositions.
- Remove binary `required-features` dependency on `storage-s3-r2`.
- Replace `storage/r2` with `storage/sqlx` in profiles.
- Update capability snapshots.
- Remove R2 env vars from `.env.example`, CI and deploy scripts.
- Delete or rewrite R2 integration tests as SQL tests.
- Add grep/static guard that fails on runtime R2/S3/AWS references.

Acceptance criteria:

- Project builds without R2/S3/AWS SDK runtime dependencies.
- `cargo tree` for production profiles has no AWS SDK/S3 crates.
- Runtime grep for `R2Storage`, `R2StorageConfig`, `aws_sdk_s3`, `storage-s3-r2`, `storage/r2`, `OXIDE_R2` returns no hits outside allowed historical docs, if any.
- Production web and Telegram startup paths use SQLx/Postgres.
- R2 docs/examples/secrets are removed or replaced.

Risks:

- Big-bang deletion can break many tests. Ensure SQL paths are green before deleting.
- Historical docs can make grep noisy. Define allowed paths explicitly.

Заметки для реализации:

- Do this only after Phases 2-5 have SQL-backed acceptance tests.
- Regenerate `Cargo.lock` and snapshots in one cleanup commit.

### Phase 7 — Hardening

Цель:

- Make SQL backend production-ready for local Postgres and Supabase Postgres.

Затронутые файлы/модули:

- Migrations.
- SQLx storage modules.
- Web persistence SQL module.
- CI workflow.
- Deployment docs.
- Cleanup jobs/scheduler if added.

Конкретные задачи:

- Review and add indexes for all list/page/due/lookup queries.
- Define retention policies for:
  - web task events
  - web task files/blobs
  - wiki raw archives
  - audit events, if retention is allowed
  - old auth sessions
- Add cleanup jobs with bounded batches.
- Review transaction boundaries for multi-record updates.
- Tune pool limits and timeouts for local and Supabase.
- Add failure-mode tests:
  - DB unavailable at startup
  - migration missing/out-of-date
  - transaction conflict
  - duplicate task event seq
  - concurrent reminder claim
  - DB reconnect after transient failure
- Add performance smoke tests for large task event streams.
- Add DB growth/WAL observation notes.
- Add Supabase compatibility checklist.

Acceptance criteria:

- Large task smoke test can append and page many events without O(n) scans or hot-row rewrites.
- Cleanup/retention jobs are bounded and safe to run repeatedly.
- Pool defaults are documented and conservative.
- CI covers key SQL storage flows.
- Failure modes produce actionable errors.

Risks:

- WAL/backup growth from blobs/events may be larger than expected.
- Supabase pooler/connection limits can surface only in production-like deployments.

Заметки для реализации:

- Do not defer retention decisions indefinitely. R2 removal shifts blob/event cost into Postgres.
- Add metrics before optimizing blindly.

## 10. Global Acceptance Criteria

- Project builds without R2/S3/AWS SDK runtime dependencies.
- Production durable state works through SQLx + Postgres.
- Local durable mode uses local PostgreSQL.
- Web/production mode can use Supabase Postgres.
- Fresh setup is documented and tested.
- Migration of old R2 data is intentionally absent and documented.
- SQLite is absent from scope, features, acceptance criteria and implementation plan.
- Task events are append-only rows and do not rewrite large JSON objects.
- Task progress is coalesced/debounced or stored separately from full event stream.
- Large tasks do not create object-storage operation amplification because object storage is absent.
- Reminders use SQL-native due-job claiming and leases.
- Audit is append-only SQL rows.
- Wiki/memory storage does not use R2/S3/object storage.
- R2 env vars are no longer needed.
- R2 docs are removed, replaced or explicitly marked historical.
- Tests cover key SQL storage flows.
- CI does not require R2 secrets.
- Cargo profiles no longer include `storage-s3-r2`.
- Grep/static guard confirms no runtime R2/S3/AWS durable storage references remain.

## 11. Testing Strategy

### 11.1 Unit tests

Keep and adapt DB-free tests for:

- Domain record builders and validators.
- Reminder schedule calculation.
- Topic context/AGENTS.md limits.
- Audit paging helpers if still used outside DB.
- Web API contract serialization.

Expected changes:

- Remove object-key layout assertions from unit tests unless they are historical and isolated.
- Replace control-plane local lock tests with SQL transaction/concurrency tests.

### 11.2 SQL integration tests

Add tests that run against a clean Postgres database:

- `SqlxStorage::check_connection` succeeds/fails correctly.
- Migrations apply to an empty database.
- User config/state round trips and default behavior.
- User context updates do not rewrite unrelated contexts.
- Agent memory snapshot save/load/skip-identical behavior.
- Agent flow record upsert/load.
- Agent profiles list/upsert/delete.
- Topic context and topic AGENTS.md upsert/delete and duplicate guard behavior.
- Topic infra and topic binding CRUD.
- Private secret put/get/delete with redaction guard.
- Reminder create/list/claim/reschedule/complete/fail/cancel/pause/resume/retry/delete.
- Concurrent reminder claimers do not double-claim.
- Audit append/list/page with stable versions.
- Wiki page save/load/delete/context delete.

### 11.3 Web persistence contract tests

Run `WebUiStore` behavior tests against SQLx:

- User save/load/count.
- Login uniqueness conflict.
- Auth session save/load/revoke/revoke-all-except.
- Web session save/load/list/delete.
- Task save/load/list/status update.
- Append task events and page by seq.
- Duplicate event seq handling.
- Task file save/load and size limit rejection.
- Startup reconciliation marks unfinished tasks interrupted.
- Session delete cascades tasks/events/files/wiki context rows.

### 11.4 CI strategy

Recommended CI changes:

- Add a Postgres service to GitHub Actions test job.
- Set `OXIDE_DATABASE_URL` for SQL integration tests.
- Run migrations before integration tests or let tests create isolated databases and run migrations.
- Remove dummy `OXIDE_R2_*` env vars.
- Remove R2 credential validation job.
- Add cargo-tree/static guard to reject AWS SDK/S3 crates in production profiles.

### 11.5 Supabase compatibility tests

Do not require a real Supabase project in CI. Instead:

- Use standard Postgres in CI.
- Avoid non-portable extensions unless explicitly supported and documented.
- Keep SQL inside normal Postgres features available in Supabase Postgres.
- Add a manual or scheduled smoke checklist for Supabase:
  - migrations apply
  - app connects with pool limits
  - web auth/session/task flow works
  - task event pagination works
  - reminder claiming works
  - audit append/page works

### 11.6 Performance smoke tests

Add smoke tests or benchmarks for:

- Appending many task events in batches.
- Paginating task events after high seq values.
- Updating progress snapshots at debounced intervals.
- Listing sessions/tasks for a user with many rows.
- Claiming reminders with many scheduled jobs.
- Cleanup jobs with bounded batch sizes.

## 12. Risks and Open Questions

### 12.1 Task file blobs inside Postgres

Risk:

- Current web upload default is large for DB-backed blob storage. Storing large files as `bytea` increases WAL, backups, replication load and query latency if not isolated.

Mitigation:

- Store blob content in separate `web_task_file_blobs` table, not in task metadata row.
- Enforce a strict configurable max blob size before insert.
- Set retention expiry by default for transient task files.
- Add cleanup job with bounded batch deletes.
- Revisit default upload limit during Phase 7. Do not keep R2 as fallback.

Open question:

- What is the acceptable max file size for Postgres-only storage? The current 200 MB web limit is likely too high for many Supabase/local setups.

### 12.2 Wiki/task artifact size limits

Risk:

- Wiki raw archives and task artifacts can cause unbounded DB growth.

Mitigation:

- Enforce content byte limits for wiki pages and raw archives.
- Store `content_bytes` and `retention_expires_at` where applicable.
- Add config vars for max wiki page bytes and max raw archive retention.
- Add DB growth smoke tests.

Open question:

- Should wiki raw archives be durable long-term, short-term, or disabled by default when using Postgres-only storage?

### 12.3 WAL/DB growth on large tasks

Risk:

- Append-only task events are correct, but high-volume events and progress snapshots can still grow WAL quickly.

Mitigation:

- Batch insert events.
- Truncate large payloads before persistence.
- Store progress latest snapshot separately and debounce writes.
- Add retention for old task events.
- Add indexes carefully; avoid over-indexing high-volume event rows.

Open question:

- What retention default should apply to task events: forever, per-user configurable, or time/size bounded?

### 12.4 Retention for task events

Risk:

- Keeping all events forever may be expensive; deleting events too aggressively can break replay expectations.

Mitigation:

- Separate task summary/final response from event history.
- Allow retaining final task state even after event retention cleanup.
- Document retention semantics in web UI/API.
- Implement cleanup in bounded batches with `retention_expires_at`.

Open question:

- Should completed task events be retained by age, count per task, or manual cleanup only?

### 12.5 Supabase connection limits

Risk:

- Default pool sizes that are safe for local Postgres may exhaust Supabase connection limits.

Mitigation:

- Use conservative default max connections.
- Document Supabase-specific pool settings.
- Prefer a single shared pool per process.
- Avoid background jobs opening independent pools.
- Add health metrics for pool acquire timeouts.

Open question:

- Which Supabase connection endpoint should production deployments use by default for this app’s workload? This should be verified against current Supabase project/deployment docs during implementation.

### 12.6 Local Postgres developer experience

Risk:

- R2 removal improves production cost profile, but local DB setup adds friction.

Mitigation:

- Provide compose service and one-command setup docs.
- Use clear `.env.example` values.
- Make migrations easy to run.
- Keep in-memory stores only for explicit unit tests, not ordinary local durable mode.

Open question:

- Should local dev automatically run migrations on startup by default, or require `sqlx migrate run`?

### 12.7 Transaction boundaries

Risk:

- Old R2 code uses per-key locks and ETags. SQL code needs explicit transactions to preserve consistency across multi-record updates.

Mitigation:

- Define transaction boundaries per operation:
  - user context + flow update
  - topic context + duplicate guard
  - reminder claim/status transitions
  - audit version allocation
  - session delete cascade if not handled fully by FK
- Use version columns for optimistic updates where caller needs conflict detection.
- Add concurrency tests.

Open question:

- Which current operations require user-visible conflict errors versus last-write-wins semantics?

### 12.8 JSONB overuse

Risk:

- A naive port could store every old JSON object as one JSONB column and keep poor queryability.

Mitigation:

- Require typed columns for identifiers, status, timestamps, versions, owner/scope and pagination fields.
- Limit JSONB to flexible payloads: task event payload, profile body, agent memory snapshot, progress payload, environment metadata.
- Add indexes on typed columns, not JSONB unless a real query requires it.

Open question:

- Should any profile/config JSONB fields be promoted to typed columns after observing query patterns?

### 12.9 Avoiding hot-row updates on task progress

Risk:

- Updating `web_tasks` for every progress tick creates hot rows and WAL churn.

Mitigation:

- Use separate `web_task_progress` table.
- Debounce writes by time and/or meaningful state change.
- Persist terminal progress immediately.
- Keep live SSE progress in memory for low-latency UI.
- Do not append every tiny progress tick as a full durable event unless it is semantically useful.

Open question:

- What debounce interval is acceptable for UX and restart recovery? Start conservative and tune with smoke tests.

### 12.10 Testing Supabase-compatible SQL without real Supabase in CI

Risk:

- Standard Postgres CI may miss Supabase-specific connection/pooler behavior.

Mitigation:

- Use standard Postgres for SQL correctness and migrations.
- Keep SQL portable: avoid unsupported extensions and superuser-required features.
- Add a Supabase smoke checklist for deploy validation.
- Keep pool settings configurable and conservative.

Open question:

- Should a nightly/manual job run against a real Supabase project later? Not required for this PRD’s baseline.

### 12.11 Historical docs and grep acceptance

Risk:

- Historical PRDs/goals mention R2 and can cause grep acceptance noise.

Mitigation:

- Define grep policy:
  - runtime code: zero R2/S3/AWS storage hits.
  - current docs/examples: zero R2 setup hits.
  - historical implemented PRDs/goals: allowed only if clearly historical.
- Prefer adding a static guard that checks runtime paths and current docs separately.

Open question:

- Should old implemented PRDs be edited, archived or excluded from grep? Decide in Phase 6.

### 12.12 Exact web route split uncertainty

Risk:

- Some web task/session route files may have been refactored; this PRD references route modules by likely/current paths and broader `server/*` when exact split is not material.

Mitigation:

- Before Phase 2 implementation, re-run targeted search for `append_task_events`, `save_task`, `last_progress`, `task_progress`, `TaskEventsResponse` and update touched files.

Open question:

- Whether progress persistence should be in `WebUiStore` only or a separate service owned by task executor needs implementation-level decision.

## 13. Implementation Notes for Future Agents

- Start by creating SQLx foundation and migrations. Do not delete R2 first unless instructed to do a big-bang rewrite.
- Keep the current traits as seams, but do not preserve object-storage semantics as architecture.
- Map old object namespaces to SQL entities; do not import old object data.
- Treat `Cargo.toml` feature cleanup as a major part of the work, not an afterthought.
- Do not add SQLite features, migrations, tests or docs.
- Keep Supabase support as Postgres compatibility plus conservative pool configuration.
- Use transaction tests for reminders and audit before wiring runtime schedulers.
- For task events, the invariant is: append rows, page rows, never rewrite a chunk object.
- For task progress, the invariant is: latest snapshot is coalesced/debounced and separate from full event stream.
- For blobs, the invariant is: no R2 fallback. Either bounded Postgres storage or reject/retention policy.
- For docs, update `AGENTS.md` early so later agents stop assuming R2 is the only production durable storage.
- After SQL paths are implemented, run a final deletion grep across runtime code:
  - `R2Storage`
  - `R2StorageConfig`
  - `storage-s3-r2`
  - `storage/r2`
  - `OXIDE_R2`
  - `aws_sdk_s3`
  - `aws-config`
  - `aws_credential`
  - `aws-types`
  - `S3 Get`
  - `S3 put`
  - `delete_prefix`
  - `list_keys_under_prefix`
- Final state should make the old R2 bucket irrelevant. Operators may delete old R2 data out-of-band after verifying they no longer need it, but the application must not depend on that deletion.
