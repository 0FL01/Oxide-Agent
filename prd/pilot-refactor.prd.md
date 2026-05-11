# Pilot refactor program — historical breadcrumb

Status: historical planning note, not the active refactor schedule.

This file used to contain a broad multi-phase crate/storage/tools/topic/Telegram refactor plan for an old branch-local effort. The detailed phase table and GitHub footnote citations were removed because they were stale and mixed target architecture with old observations.

## Why this document was reduced

- The old text claimed the runtime tool-call oracle was sequential. Current code executes approved tool calls concurrently and then processes results in deterministic order: `crates/oxide-agent-core/src/agent/runner/tools.rs:91`, `crates/oxide-agent-core/src/agent/runner/tools.rs:175`, `crates/oxide-agent-core/src/agent/runner/tools.rs:218`.
- The old citation markers used broken or external `[GitHub][N]` references instead of local code breadcrumbs.
- The old phase table marked everything `pending`, while characterization tests already exist in `oxide-agent-transport-web`.
- The old target crate extraction plan is still not implemented and should not be presented as an in-flight schedule.

## Current code breadcrumbs

- Workspace members are still the existing seven crates: `Cargo.toml:2`.
- `oxide-agent-core` still owns `agent`, `config`, `llm`, `sandbox`, `storage`, and `utils`: `crates/oxide-agent-core/src/lib.rs:7`, `crates/oxide-agent-core/src/lib.rs:11`, `crates/oxide-agent-core/src/lib.rs:13`, `crates/oxide-agent-core/src/lib.rs:15`.
- `StorageProvider` remains broad: `crates/oxide-agent-core/src/storage/provider.rs:21`.
- Tool identity and policy are still string-based: `crates/oxide-agent-core/src/agent/provider.rs:22`, `crates/oxide-agent-core/src/agent/profile.rs:110`, `crates/oxide-agent-core/src/agent/profile.rs:139`.
- `ToolRegistry` still routes by scanning providers: `crates/oxide-agent-core/src/agent/registry.rs:51`.
- Execution has request/transition wrappers, not a full state machine: `crates/oxide-agent-core/src/agent/executor/types.rs:95`, `crates/oxide-agent-core/src/agent/executor/types.rs:109`.
- Approval remains disabled at the SSH provider source while outcome types still exist: `crates/oxide-agent-core/src/agent/providers/ssh_mcp.rs:1186`, `crates/oxide-agent-core/src/agent/executor.rs:53`.
- Telegram still owns bootstrap/reminder/startup orchestration: `crates/oxide-agent-transport-telegram/src/runner.rs:21`, `crates/oxide-agent-transport-telegram/src/runner.rs:27`, `crates/oxide-agent-transport-telegram/src/runner.rs:41`.

## How to revive this plan

Do not resurrect the removed phase table as-is. If this refactor becomes active again, start with a new current-code characterization PR and use local `file_path:line_number` references only.

Suggested order if revived:

1. Update characterization coverage in `oxide-agent-transport-web`.
2. Split storage capabilities behind compatibility adapters.
3. Introduce stable tool identity and alias migration.
4. Formalize execution state transitions.
5. Extract topic control-plane service boundaries.
6. Only then consider crate extraction and Telegram thinning.

Memory-specific reliability work is tracked separately in `prd/memento.md`.
