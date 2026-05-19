# Goal: LLM Wiki Memory Migration

Date started: 2026-05-19
Status: completed
Codex goal: Implement `prd/PRD.md` by replacing typed/vector persistent memory with bounded Markdown LLM Wiki memory in S3/R2; keep hot/session memory separate; remove or disable legacy durable memory paths; validate with workspace checks and document the breaking reset.

## Objective

Replace Oxide Agent's current typed/vector-oriented persistent memory system with the LLM Wiki durable memory architecture defined in `prd/PRD.md`.

The stopping condition is a working MVP where durable memory is stored as bounded Markdown pages under `{prefix}/wiki/v1/`, read through deterministic `index.md` plus selected pages, updated only through validated patch flow, and no runtime path reads or writes the old `persistent_memory/` typed durable memory model.

## Scope

In scope:
- Add wiki memory config, deterministic S3/R2 key builders, store/cache/service modules, context assembler, signal buffer, patch planner, patch validator, flush/reset behavior, metrics, and tests.
- Wire bounded wiki context into agent prompt assembly while keeping `AgentMemory`, compaction, todos, runtime injections, topic `AGENTS.md`, and flow/session state as hot/session context.
- Disable and then remove legacy durable memory components: `oxide-agent-memory`, `agent/persistent_memory`, typed memory providers/tools, R2 `persistent_memory/` storage code, Postgres memory startup/migrations, memory classifier, typed post-run writer, and memory embedding backfill/indexing.
- Update Telegram transport wiring so it no longer constructs or passes `PersistentMemoryStore`, except where a transitional phase explicitly disables it.
- Add docs for wiki memory, breaking reset, S3 layout, and cleanup/reset of old persistent memory data.

Out of scope:
- Migrating old `ThreadRecord`, `EpisodeRecord`, `MemoryRecord`, `EmbeddingRecord`, Postgres rows, or old R2 `persistent_memory/` objects into wiki pages.
- Implementing mandatory embeddings/vector search for durable memory.
- Per-message durable writes, raw transcript archive by default, distributed locking, transactional WAL, enterprise ACLs, or a manual wiki editing UI.
- Removing embeddings used by the skills system unless they are only coupled through legacy persistent memory config.

## Repository Context

- PRD source: `prd/PRD.md`.
- Current conflicting legacy plan: `backlog/persistent-memory-plan.md` describes the old typed/hybrid RAG direction and should be superseded or archived by this goal.
- Current branch observed: `dev...origin-ssh/dev [ahead 2]`.
- Legacy durable memory originally lived in `crates/oxide-agent-memory`, `crates/oxide-agent-core/src/agent/persistent_memory/`, `crates/oxide-agent-core/src/storage/persistent_memory.rs`, and `crates/oxide-agent-core/src/storage/r2_persistent_memory.rs`.
- Legacy storage provider APIs originally exposed typed thread, episode, memory, session-state, lexical search, embedding, backfill, and vector search methods.
- Legacy executor/transport wiring originally configured `PersistentMemoryCoordinator`, `LlmMemoryTaskClassifier`, `LlmPostRunMemoryWriter`, `PersistentMemoryEmbeddingIndexer`, and `PersistentMemoryStore`.
- `crates/oxide-agent-core/src/agent/memory.rs` and `crates/oxide-agent-core/src/agent/compaction/` are hot/session context mechanisms and must not be treated as durable wiki memory.
- Topic-scoped `AGENTS.md` remains a separate control/context record and must not become wiki memory.

## Implementation Plan

1. Establish wiki primitives: add `WikiMemoryConfig`, deterministic `wiki/v1` key builders, `WikiStore`, page/path validation basics, content hashing, and storage tests proving no S3 LIST is needed for normal wiki reads.
2. Add read path: implement `WikiSessionCache` and `WikiContextAssembler`, bootstrap missing global/context indexes in cache without immediate writes, select pages from `index.md`, render bounded prompt context, and test missing/empty wiki behavior.
3. Add write path: implement `WikiSignalBuffer`, `WikiPatchPlanner`, `WikiPatchValidator`, dirty-page application, index/log reconciliation, coalesced flush, unchanged hash skip, inbox handling, and flush failure behavior.
4. Wire runtime: call wiki context assembly before prompt creation, run patch planning only after meaningful successful runs or explicit remember requests, flush at run end, and expose only constrained wiki read/search/patch/reset operations where needed.
5. Disable legacy durable memory: stop instantiating `PersistentMemoryCoordinator`, `DurableMemoryRetriever`, `PersistentMemoryEmbeddingIndexer`, memory classifier, Postgres memory startup/migrations, and old typed memory provider tools in normal runtime.
6. Remove legacy durable memory code: delete or unwind `oxide-agent-memory`, `agent/persistent_memory`, R2 `persistent_memory` implementation, old storage provider methods, old key builders, unused config/env fields, old tests, and stale docs.
7. Add reset/cleanup path: provide an admin/dev operation or documented manual cleanup for old R2 `persistent_memory/` prefix and old Postgres memory tables; runtime must ignore old data before cleanup.
8. Update docs and README: document wiki memory architecture, S3 layout, config, breaking reset, no dual-write migration, and validation expectations.

## Validation Contract

- Formatting: `cargo fmt --all -- --check`
- Fast compile check: `cargo check --workspace`
- Tests: `cargo test --workspace`
- Lint before completion: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Targeted tests to add or update: wiki key builders, wiki path validation, wiki store no-LIST read path, cache dirty tracking, context assembler budget behavior, patch validator rejection cases, flush coalescing/hash skip, legacy runtime disabled, Telegram wiring without `PersistentMemoryStore`.
- Done when all required commands pass or any remaining failure is documented with an explicit blocker accepted by the user.

## Decisions

- 2026-05-19: Treat `prd/PRD.md` as the source of truth and supersede `backlog/persistent-memory-plan.md` for durable memory direction.
- 2026-05-19: Make this a breaking reset with no dual-write and no old durable memory migration.
- 2026-05-19: Preserve hot/session context and compaction; only durable semantic memory moves to LLM Wiki.
- 2026-05-19: Do not introduce vector search or embeddings as an MVP dependency for durable wiki memory.
- 2026-05-19: Prefer simple modules over a new broad repository trait hierarchy because target load is personal/small-group scale.
- 2026-05-19: Use a conservative deterministic post-run patch planner for MVP instead of a second LLM planner call: explicit remember and confident procedure/preference candidates become scoped pages, low-confidence facts go to inbox, and protected `index.md`/`log.md` are reconciled by runtime after validation.

## Progress Log

- 2026-05-19 16:51 +03: Read `prd/PRD.md`, repository state, active goal state, existing docs, README, workspace membership, executor persistent-memory wiring, storage provider shape, and legacy backlog plan. Active Codex goal was initially absent, then set to this LLM Wiki Memory migration objective. Next checkpoint is Phase 1 implementation: wiki config, key builders, store, and no-LIST storage tests.
- 2026-05-19 16:59 +03: Started Phase 1. Added versioned wiki key builders for `{prefix}/wiki/v1/`, `agent::wiki_memory` primitives (`WikiMemoryConfig`, deterministic context id/slug helpers, `WikiStore`, `WikiObjectBackend`, content hashes), R2-backed text object implementation, and store-side path validation that rejects traversal/unsafe page identifiers before backend access. Added targeted tests for context ids, deterministic keys, no-discovery missing reads, deterministic writes, and validation rejection. Verified with `cargo fmt --all -- --check`, `cargo test -p oxide-agent-core wiki_memory --lib`, `cargo test -p oxide-agent-core storage::tests::keys_and_user::wiki --lib`, and `cargo check -p oxide-agent-core`.
- 2026-05-19 17:08 +03: Started Phase 2 read path. Added `WikiSessionCache` with per-run read-through caching, deterministic backend GET accounting, in-memory bootstrap for missing global/context `index.md`, original content hash tracking, and cache metrics. Added `WikiContextAssembler` that derives context id from `user_id + context_key`, reads `global/index.md` and `contexts/{context_id}/index.md` first, always considers `overview.md`, lazy-loads matching core/topic pages from `index.md`, and renders a bounded durable wiki prompt block. Added tests for missing-index bootstrap without writes, overview/topic page selection, repeated cache reuse, render budget behavior, and storage key layout. Verified with `cargo fmt --all -- --check`, `cargo test -p oxide-agent-core wiki_memory --lib`, `cargo test -p oxide-agent-core storage::tests::keys_and_user::wiki --lib`, `cargo check -p oxide-agent-core`, and `cargo clippy -p oxide-agent-core --lib -- -D warnings`.
- 2026-05-19 17:15 +03: Started Phase 3 write-path primitives. Added `WikiSignalBuffer` with candidate/byte limits and deduplication, structured `WikiPatchSet`/`WikiPatchOperation`, `WikiPatchValidator` with scoped path allowlist, operation/page/byte limits, protected `index.md`/`log.md` rejection, required frontmatter checks, source-ref requirement, simple secret detection, and raw transcript dump rejection. Extended `WikiSessionCache` with validated patch application, expected-hash conflict checks, dirty-page tracking, coalesced `flush_dirty_pages`, unchanged hash skip, deterministic PUT metrics, and write ordering that keeps `index.md`/`log.md` last for future reconciliation. Added tests for signal limits, patch acceptance/rejection, dirty apply, expected-hash mismatch, flush write, and unchanged hash skip. Verified with `cargo fmt --all -- --check`, `cargo test -p oxide-agent-core wiki_memory --lib`, `cargo test -p oxide-agent-core storage::tests::keys_and_user::wiki --lib`, `cargo check -p oxide-agent-core`, and `cargo clippy -p oxide-agent-core --lib -- -D warnings`.
- 2026-05-19 17:19 +03: Started Phase 4 prompt integration prep. Extended `create_agent_system_prompt` with an explicit `wiki_context: Option<&str>` parameter, appending bounded durable wiki context before structured-output/tool guidance while preserving role instructions and existing behavior when absent. Updated executor and manual compaction call sites to pass `None` until assembler wiring is added. Added prompt test proving the durable wiki block is included before structured-output instructions. Verified with `cargo fmt --all -- --check`, `cargo test -p oxide-agent-core prompt::composer --lib`, `cargo test -p oxide-agent-core wiki_memory --lib`, `cargo check -p oxide-agent-core`, and `cargo clippy -p oxide-agent-core --lib -- -D warnings`.
- 2026-05-19 17:25 +03: Wired the wiki read path into core executor when a `WikiStore` is explicitly attached. Added `AgentExecutor::with_wiki_memory_store`, executor-owned optional `WikiStore`, and `prepare_execution` assembly through `WikiSessionCache` + `WikiContextAssembler` using the session `AgentMemoryScope`. Assembly failures are logged and skipped so user task execution is not broken by wiki storage issues. Added an executor-level test proving the LLM request system prompt includes rendered durable wiki memory when configured. Existing production call sites still need transport/storage wiring to pass an R2-backed `WikiStore`. Verified with `cargo fmt --all -- --check`, `cargo test -p oxide-agent-core executor::tests::basics::executor_injects_configured_wiki_memory_context --lib`, `cargo test -p oxide-agent-core prompt::composer --lib`, `cargo test -p oxide-agent-core wiki_memory --lib`, `cargo check -p oxide-agent-core`, and `cargo clippy -p oxide-agent-core --lib -- -D warnings`.
- 2026-05-19 17:31 +03: Wired storage-backed wiki memory into production executor construction. Added `StorageProvider::load_wiki_text` / `save_wiki_text` with R2 implementation using existing text object GET/PUT, and `WikiStore::from_storage_provider` adapter so transports can pass `Arc<dyn StorageProvider>` without depending on concrete `R2Storage`. Updated Telegram flow executor, startup maintenance executor, and web transport executor to attach storage-backed `WikiStore`. Verified with `cargo fmt --all -- --check`, `cargo test -p oxide-agent-core wiki_memory --lib`, `cargo test -p oxide-agent-core executor::tests::basics::executor_injects_configured_wiki_memory_context --lib`, `cargo check -p oxide-agent-core`, `cargo check -p oxide-agent-transport-web`, `cargo check -p oxide-agent-transport-telegram`, and `cargo clippy -p oxide-agent-core --lib -- -D warnings`.
- 2026-05-19 17:38 +03: Disabled legacy typed persistent-memory runtime paths in the executor. The old `with_persistent_memory_*` configuration methods are now compatibility no-ops, `has_persistent_memory()` reports false, runner context receives no `PersistentMemoryCoordinator`, typed durable retrieval/classification injection was removed from `prepare_execution`, and `MemoryProvider`/`memory_*` tools are no longer registered by the executor. Removed old executor persistent-memory tests from the active test module and added a regression proving legacy store configuration does not expose memory tools. Kept `agent::persistent_memory` compiled with an explicit legacy allowance while later removal is staged. Verified with `cargo fmt --all -- --check`, `cargo test -p oxide-agent-core executor::tests::basics --lib`, `cargo test -p oxide-agent-core wiki_memory --lib`, `cargo test -p oxide-agent-core executor::tests::basics::legacy_persistent_memory_configuration_is_disabled --lib`, `cargo check -p oxide-agent-core`, `cargo check -p oxide-agent-transport-web`, `cargo check -p oxide-agent-transport-telegram`, and `cargo clippy -p oxide-agent-core --lib -- -D warnings`.
- 2026-05-19 17:47 +03: Removed normal transport wiring for legacy typed persistent memory. Telegram startup no longer initializes Postgres memory or injects `PersistentMemoryStore` into dispatcher dependencies, reminder scheduler, menu handlers, agent lifecycle, callbacks, controls, startup maintenance, or session construction. Web transport session manager no longer stores or accepts a `PersistentMemoryStore`. Removed obsolete Web live ZAI Postgres memory E2E setup/tests, deleted the inactive executor persistent-memory test file, and removed public `agent::connect_postgres_memory_store` / `agent::PersistentMemoryStore` re-exports. Verified that Telegram/Web source and tests no longer reference `persistent_memory_store`, `PersistentMemoryStore`, `with_persistent_memory`, `connect_postgres_memory_store`, `MEMORY_DATABASE_URL`, or Postgres memory strings. Verified with `cargo fmt --all -- --check`, `cargo check -p oxide-agent-core`, `cargo check -p oxide-agent-transport-telegram --tests`, `cargo check -p oxide-agent-transport-web --tests`, and `cargo clippy -p oxide-agent-core --lib -- -D warnings`.
- 2026-05-19 18:12 +03: Removed the remaining compiled legacy durable-memory surface. Moved task-local memory behavior signals into `agent::memory_behavior`, removed `agent/persistent_memory`, removed old storage repository/R2 typed memory modules and key builders, deleted `crates/oxide-agent-memory` from the workspace, removed persistent-memory Postgres/classifier config and `.env.example` entries, rewrote memory hook advice away from old `memory_*` tools, and added `docs/wiki-memory.md` with the breaking reset and cleanup notes. In-memory web storage now persists wiki objects instead of typed memory fixtures. Verified mid-change with `cargo check -p oxide-agent-core --tests`, `cargo check -p oxide-agent-transport-web --tests`, and `cargo check -p oxide-agent-transport-telegram --tests`; final fmt/check/test/clippy pass is still pending.
- 2026-05-19 18:58 +03: Closed the runtime write-path gap. Added `WikiPatchPlanner` as a conservative deterministic post-run planner, wired `AgentExecutor` to plan/apply/flush wiki updates only after completed runs, and added runtime reconciliation for protected `index.md` and `log.md` so newly written pages are discoverable without S3 LIST. Explicit remember requests and confident procedure/preference drafts write scoped `pages/*.md`; low-confidence facts remain in `inbox/*.md`. Added regression coverage for planner routing, index/log reconciliation, and executor flush of explicit remember. Full validation passed.

## Risks and Follow-ups

- MVP patch planning is deterministic and conservative, not a second LLM synthesis pass. This is intentional for the current personal/small-group scale; a later refinement can promote/merge pages more intelligently.
- Socket-backed web E2E tests are ignored by default in sandboxed validation because they require a local TCP listener; run with the `socket_e2e` feature in an environment that allows binding.
- Legacy durable-memory data is ignored but not automatically deleted; operators should use the documented cleanup after deployment and rollback window.

## Final Verification

- `cargo fmt --all -- --check` passed.
- `cargo check --workspace` passed.
- `cargo test -p oxide-agent-core wiki_memory --lib` passed: 29 passed.
- `cargo test -p oxide-agent-core executor::tests::basics::executor_flushes_explicit_remember_to_wiki_after_completed_run --lib` passed.
- `cargo test --workspace` passed. Core lib: 853 passed, 8 ignored. Telegram lib: 151 passed. Web E2E default: 7 passed, 21 ignored due local TCP listener gating. Doctests passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` passed.
