# LLM Wiki Memory

Oxide Agent durable memory is a bounded Markdown wiki stored in the existing S3/R2 object store. It replaced the old typed/vector persistent-memory subsystem (Postgres + pgvector). The old `ThreadRecord`, `EpisodeRecord`, `MemoryRecord`, embedding records, Postgres memory tables, and R2 objects under `persistent_memory/` are not read or migrated. Postgres has been fully removed from the stack; no Postgres service or dependency exists.

## Runtime Model

- Hot/session context remains in `AgentMemory`, runtime injections, todos, compaction summaries, topic `AGENTS.md`, and flow state.
- Durable context is assembled from deterministic wiki keys before the agent prompt is built.
- Normal wiki reads use deterministic `GET` operations only; S3 `LIST` is not required in the hot path.
- Wiki writes are staged as validated patches in the session cache and flushed as bounded Markdown objects after successful runs.
- Explicit remember requests and confident procedure/preference candidates create scoped `pages/*.md`; low-confidence facts go to `inbox/*.md`.
- `index.md` and `log.md` are protected from planner edits and reconciled by runtime after patch validation, so new pages are discoverable without S3 `LIST`.
- The legacy skills/embeddings subsystem has been removed; durable context now comes from wiki memory, topic `AGENTS.md`, runtime injections, and enabled tools.

## Object Layout

With an optional storage prefix, wiki objects live under:

```text
{prefix}/wiki/v1/global/index.md
{prefix}/wiki/v1/contexts/{context_id}/index.md
{prefix}/wiki/v1/contexts/{context_id}/overview.md
{prefix}/wiki/v1/contexts/{context_id}/pages/{slug}.md
{prefix}/wiki/v1/contexts/{context_id}/inbox/{slug}.md
{prefix}/wiki/v1/contexts/{context_id}/raw/{yyyy-mm}/{run_id}.md
```

`context_id` is derived deterministically from the transport memory scope. It is intentionally not split by `flow_id`, so topic/project memory can survive individual agent flows.

## Legacy Data Cleanup

The old Postgres persistent-memory tables and R2 objects under `persistent_memory/` are no longer read. Postgres has been fully removed from the stack (`docker-compose.yml` postgres service deleted, `crates/oxide-agent-memory` removed).

If you still have legacy data, clean up manually:

```bash
# R2/S3: remove old typed durable-memory objects after verifying the bucket/prefix.
aws s3 rm s3://<bucket>/<optional-prefix>/persistent_memory/ --recursive --endpoint-url <r2-endpoint>

# Postgres: only if you still have a Postgres instance with old tables — the service is no longer in compose.
# DROP TABLE IF EXISTS memory_embeddings, memory_episodes, memory_threads, memory_session_states, memories;
```

The runtime does not require cleanup to be correct; cleanup only removes orphaned legacy data.
