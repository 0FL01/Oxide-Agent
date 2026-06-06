# LLM Wiki Memory

Oxide Agent durable memory is a bounded Markdown wiki stored through the durable storage facade. Current SQLx/Postgres deployments persist wiki text as rows keyed by deterministic scope/path metadata. It replaced the old typed/vector persistent-memory subsystem (Postgres + pgvector). The old `ThreadRecord`, `EpisodeRecord`, `MemoryRecord`, embedding records, old Postgres memory tables, and R2 objects under `persistent_memory/` are not read or migrated.

## Runtime Model

- Hot/session context remains in `AgentMemory`, runtime injections, todos, compaction summaries, topic `AGENTS.md`, and flow state.
- Durable context is assembled from deterministic wiki keys before the agent prompt is built.
- Normal wiki reads use deterministic key lookups only; list/prefix scans are not required in the hot path.
- Wiki writes are staged as validated patches in the session cache and flushed as bounded Markdown rows after successful runs.
- Explicit remember requests and confident procedure/preference candidates create scoped `pages/*.md`; low-confidence facts go to `inbox/*.md`.
- `index.md` and `log.md` are protected from planner edits and reconciled by runtime after patch validation, so new pages are discoverable without storage listing.
- The legacy skills/embeddings subsystem has been removed; durable context now comes from wiki memory, topic `AGENTS.md`, runtime injections, and enabled tools.

## Logical Key Layout

With an optional storage prefix, wiki rows are addressed by logical keys shaped as:

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

The old pgvector persistent-memory tables and R2 objects under `persistent_memory/` are no longer runtime inputs. They are unrelated to the current SQLx/Postgres durable storage tables.

Oxide Agent does not provide a migration, compatibility reader, startup cleanup routine, or transformation path for these records. Clean deployments recover only from current configuration plus SQLx/Postgres wiki rows created from the logical key layout above.

If obsolete `persistent_memory/` objects or old Postgres tables still exist outside the current stack, treat them as orphaned deployment leftovers and delete them out-of-band after separate operator verification. The Oxide runtime must not depend on that deletion to start or to assemble durable context.
