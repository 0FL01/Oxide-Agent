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

## Removed Persistent-Memory Data

The old Postgres persistent-memory tables and R2 objects under `persistent_memory/` are no longer runtime inputs. Postgres has been fully removed from the stack (`docker-compose.yml` postgres service deleted, `crates/oxide-agent-memory` removed).

Oxide Agent does not provide a migration, compatibility reader, startup cleanup routine, or transformation path for these records. Clean deployments recover only from `.env` plus the S3/R2-backed wiki memory object layout described above.

If obsolete `persistent_memory/` objects or old Postgres tables still exist outside the current stack, treat them as orphaned deployment leftovers and delete them out-of-band after separate operator verification. The Oxide runtime must not depend on that deletion to start or to assemble durable context.
