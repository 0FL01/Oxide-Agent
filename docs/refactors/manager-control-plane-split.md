# Manager Control Plane Split Map

## Goal
- Reduce attention pressure for LLM-driven edits in `crates/oxide-agent-core/src/agent/providers/manager_control_plane/mod.rs`.
- Keep the external module API stable while splitting the implementation into smaller files.
- Separate pure moves from behavioral refactors.

## Frozen Public Surface
- Keep `ManagerControlPlaneProvider` public.
- Keep public forum topic request/result types and traits public from the module root.
- Keep tool names and `manager_control_plane_tool_names()` behavior unchanged.
- Keep `crates/oxide-agent-core/src/agent/providers/mod.rs` exports unchanged during the split.

## Current High-Level Layout
- `mod.rs`: module root, public types, provider struct, remaining tool definitions, dispatch wiring.
- `shared.rs`: validation, normalization, topic-id resolution, JSON serialization helpers.
- `audit.rs`: audit status, audit writes, applied-mutation lookup, rollback snapshot lookup.
- `bindings.rs`: topic binding args, tool schema, execute methods, rollback flow.
- `contexts.rs`: topic context args, tool schema, execute methods, rollback flow.
- `agents_md.rs`: topic AGENTS.md args, tool schema, execute methods, rollback flow.
- `infra.rs`: topic infra args, tool schema, preflight helpers, execute methods, rollback flow.
- `profiles.rs`: agent profile args, tool schema, execute methods, rollback flow.
- `agent_controls.rs`: topic agent tool/hook args, catalogs, snapshots, execute methods.
- `forum_topics.rs`: forum lifecycle CRUD, topic cleanup flows, SSH provisioning flow.
- `sandboxes.rs`: sandbox inventory, lookup, create/recreate/delete/prune flows.
- `tests/mod.rs`: manager control-plane test suite moved out of production code.

## Attention Budget Rules
- One iteration changes one class of things: move code, or deduplicate code, or change behavior.
- One iteration should focus on one domain area and the directly related tests only.
- Do not introduce generic CRUD traits or macros until the file split is complete.
- Do not change `ToolProvider` dispatch names while moving code.
- Update this document after each split step so future LLM agents do not need to rescan the monolith.

## Remaining Root Contents
- Public forum topic request/result types and sandbox traits that must stay easy to discover.
- Provider construction, tool-name constants, base dispatch wiring, and shared provider state.
- Cross-domain helpers already extracted into `shared.rs` and `audit.rs`.

## Verification Rule Per Iteration
- Run `cargo check` after every move.
- Run targeted tests for the touched area before moving to the next slice.
- Delay cross-cutting deduplication until after the file graph is stable.
