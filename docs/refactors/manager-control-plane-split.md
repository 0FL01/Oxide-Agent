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
- `mod.rs`: module root, public types, provider struct, tool definitions, execute methods, dispatch wiring.
- `shared.rs`: validation, normalization, topic-id resolution, JSON serialization helpers.
- `tests/mod.rs`: manager control-plane test suite moved out of production code.

## Attention Budget Rules
- One iteration changes one class of things: move code, or deduplicate code, or change behavior.
- One iteration should focus on one domain area and the directly related tests only.
- Do not introduce generic CRUD traits or macros until the file split is complete.
- Do not change `ToolProvider` dispatch names while moving code.
- Update this document after each split step so future LLM agents do not need to rescan the monolith.

## Planned Next Splits
1. `audit.rs` for audit and rollback helpers.
2. `bindings.rs` for topic binding args, definitions, and execute methods.
3. `contexts.rs` for topic context CRUD.
4. `agents_md.rs` for topic AGENTS.md CRUD.
5. `infra.rs` for topic infra CRUD and preflight helpers.
6. `profiles.rs` for agent profile CRUD.
7. `agent_controls.rs` for topic agent tools/hooks control.
8. `forum_topics.rs` for forum lifecycle and SSH provisioning.
9. `sandboxes.rs` for sandbox inventory and mutation flows.

## Verification Rule Per Iteration
- Run `cargo check` after every move.
- Run targeted tests for the touched area before moving to the next slice.
- Delay cross-cutting deduplication until after the file graph is stable.
