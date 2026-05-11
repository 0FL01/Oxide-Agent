# Memory reliability refactor plan

Status: active planning breadcrumb for the current Rust codebase.

Source of truth: Rust code in this repository. The previous long audit text was based on a public `main` snapshot and mixed verified risks with speculative architecture. That historical tail was removed intentionally.

## Scope

This plan covers only persistent-memory, compaction/archive, and checkpoint reliability. It does not cover the broader storage/tool/topic/Telegram crate split from `prd/pilot-refactor.prd.md`.

## Verified current risks

1. Soft-deleted memories can be resurrected by `upsert_memory`.
   - `upsert_memory` updates `deleted_at` from the incoming row: `crates/oxide-agent-memory/src/pg/repo.rs:579`, `crates/oxide-agent-memory/src/pg/repo.rs:606`, `crates/oxide-agent-memory/src/pg/repo.rs:621`.
   - First fix: block resurrection or require an explicit restore path, with a regression test.

2. Duplicate prevention is not a storage invariant.
   - `content_hash` has a non-unique index only: `crates/oxide-agent-memory/migrations/20260407130000_add_memory_consolidation_fields.sql:8`.
   - First fix: add duplicate detection/regression tests before considering a unique constraint.

3. Tool-derived memory drafts are volatile until post-run persistence.
   - Drafts are in-memory and capped/deduped locally: `crates/oxide-agent-core/src/agent/persistent_memory/behavior.rs:73`, `crates/oxide-agent-core/src/agent/persistent_memory/behavior.rs:89`.
   - First fix: document and test the current loss window; do not introduce a journal until a real failure justifies it.

4. Embedding indexing is best-effort after durable writes.
   - Indexer writes pending, then ready or failed: `crates/oxide-agent-core/src/agent/persistent_memory/embeddings.rs:81`, `crates/oxide-agent-core/src/agent/persistent_memory/embeddings.rs:123`.
   - Coordinator logs embedding/backfill failures without failing the user flow: `crates/oxide-agent-core/src/agent/persistent_memory/coordinator.rs:205`, `crates/oxide-agent-core/src/agent/persistent_memory/coordinator.rs:250`.
   - Embedding rows are one active embedding per owner because `model_id` is not part of the primary key: `crates/oxide-agent-memory/migrations/20260406120000_add_memory_embeddings.sql:16`.

5. Archive fallback refs can be non-durable.
   - Archive persistence fallback returns a local `ArchiveRef` on `None` or sink error: `crates/oxide-agent-core/src/agent/compaction/archive.rs:213`, `crates/oxide-agent-core/src/agent/compaction/archive.rs:218`, `crates/oxide-agent-core/src/agent/compaction/archive.rs:226`.
   - First fix: add tests around sink failure and decide whether production destructive compaction must require a durable receipt.

6. Background checkpoints are intentionally fire-and-forget, with a forced post-task flush in Telegram.
   - Background path: `crates/oxide-agent-core/src/agent/session.rs:455`, `crates/oxide-agent-core/src/agent/session.rs:468`.
   - Forced flush path: `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/session.rs:492`, `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/session.rs:501`.

## Implementation order

### PR-0 — documentation cleanup

Done when stale public-`main` citations, speculative phase tails, and obsolete branch notes are removed or replaced with local code breadcrumbs.

### PR-1 — minimal memory correctness hardening

1. Add a regression test proving a soft-deleted memory cannot be resurrected by a normal `upsert_memory`.
2. Change `PgMemoryRepository::upsert_memory` so normal upserts preserve existing `deleted_at` or fail with an explicit conflict when the target row is deleted.
3. Add tests documenting duplicate behavior for same `(context_key, memory_type, content_hash)` without adding a unique constraint yet.
4. Add embedding failure/backfill tests that capture current warning-only semantics.

### PR-2 — archive/compaction safety

1. Add archive sink error tests for compacted-history archive paths.
2. Add an explicit production policy decision: either keep inline content when archive is not durable, or mark compaction degraded and avoid destructive replacement.
3. Keep dev/test fallback behavior unless production policy is enabled.

### PR-3 — shadow write receipts

1. Add coordinator-local `MemoryWriteReceipt`/summary logging without changing repository traits.
2. Include episode created/skipped, memory upserted/failed, embedding ready/failed/pending, and backfill failure counts.
3. Use receipts in tests before considering a durable journal.

## Non-goals for now

- Event-sourced memory log.
- Full memory operation journal.
- Episode DAG.
- Global tool side-effect ledger.
- Full `ContextPackBuilder` redesign.
- New crates or broad storage split.
- Heavy dashboard/metrics infrastructure.

These ideas remain breadcrumbs only. Re-open them only after PR-1/PR-2 expose a concrete need.

## Validation

For documentation-only PR-0, review diff and stale references. For Rust follow-up PRs, run:

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```
