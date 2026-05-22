# Goal: Async Parallel Tool Runtime

Date started: 2026-05-22
Status: active
Codex goal: Implement prd/PRD.md async parallel tool runtime for Oxide Agent in phased commits: document the goal in docs/goals/2026-05-22-async-parallel-tool-runtime.md, add typed runtime/config/output/parser/registry/scheduler/process pieces, migrate the active opencode-go + deepseek-v4-flash tool execution path, remove legacy tool execution paths, and validate paired history, parallelism, timeout/cancel/hung cleanup, truncation/artifacts, and focused cargo checks/tests.

## Objective

Replace Oxide Agent's active tool execution paths with one async parallel runtime for the v1 scope defined in `prd/PRD.md`.

Done when the active Agent Mode tool path for `opencode-go` + `deepseek-v4-flash` records every assistant tool call before execution, executes each batch in parallel, writes exactly one paired tool output per `tool_call_id` in deterministic order, enforces per-tool timeout/cancel/hung cleanup and output limits, removes legacy/fallback tool execution from the active path, and passes the focused validation contract below.

## Scope

In scope:
- Implement typed tool runtime modules under `crates/oxide-agent-core/src/agent/tool_runtime/`.
- Preserve the existing opencode-go chat-like function-calling request shape, including `parallel_tool_calls: true`.
- Add strict opencode-go tool-call parsing and tool-output encoding for paired history.
- Add deterministic registry/executor interfaces and migrate the active v1 tool execution path.
- Enforce per-tool hard timeout, cancellation propagation, hung normalization, cleanup metadata, output truncation, and artifact references.
- Port sandbox and SSH process-like tools only as needed for the v1 active runtime contract.
- Remove or disconnect active legacy tool paths, approval gates, unstructured tool-call fallbacks, and old bridge replay.
- Add unit, integration, and static grep tests that prove paired history, parallel execution, no legacy fallback, and output bounds.

Out of scope:
- Generic multi-provider runtime compatibility.
- GLM, MiniMax, Gemini, OpenRouter, Mistral, or ChatGPT tool-runtime support in v1.
- Approval gates, per-tool allow/deny policy, command safety classifier, or resource-aware scheduler.
- Background job system or "model thinks while tools still running".
- Enterprise-scale orchestration, sharding, queues, HA, or heavy observability.
- Public artifact upload by default; user-downloadable links remain explicit delivery/upload tool behavior.

## Repository Context

- PRD: `prd/PRD.md`.
- Existing goal convention: `docs/goals/2026-05-21-opencode-go-provider.md`.
- Runner entry points: `crates/oxide-agent-core/src/agent/runner/execution.rs`, `crates/oxide-agent-core/src/agent/runner/tools.rs`, `crates/oxide-agent-core/src/agent/runner/mod.rs`.
- Legacy bridge path: `crates/oxide-agent-core/src/agent/tool_bridge.rs`, plus replay in `crates/oxide-agent-core/src/agent/executor/execution.rs`.
- Current registry/provider contracts: `crates/oxide-agent-core/src/agent/registry.rs`, `crates/oxide-agent-core/src/agent/provider.rs`, `crates/oxide-agent-core/src/agent/executor/registry.rs`.
- Existing model-route task-local file: `crates/oxide-agent-core/src/agent/tool_runtime.rs`; this must be renamed or moved before creating `agent/tool_runtime/`.
- Provider protocol files: `crates/oxide-agent-core/src/llm/providers/opencode_go.rs`, `crates/oxide-agent-core/src/llm/providers/protocol_profiles.rs`, `crates/oxide-agent-core/src/llm/providers/tool_call_adapter.rs`, `crates/oxide-agent-core/src/llm/providers/tool_result_encoder.rs`.
- History and repair: `crates/oxide-agent-core/src/agent/memory.rs`, `crates/oxide-agent-core/src/agent/recovery.rs`.
- Process/sandbox/SSH targets: `crates/oxide-agent-core/src/sandbox/manager.rs`, `crates/oxide-agent-core/src/agent/providers/sandbox.rs`, `crates/oxide-agent-core/src/agent/providers/ssh_mcp.rs`.
- Current branch observed at start: `dev`; repository instructions say default branch is `testing`, but this run keeps the current branch unless instructed otherwise.

## Implementation Plan

1. Phase 0: Create this goal document from `prd/PRD.md` and commit it.
2. Phase 1: Add runtime namespace and typed foundations: config, IDs, invocation, output, cleanup/truncation/artifact metadata, output JSON encoding, and normalizer tests.
3. Phase 2: Add strict opencode-go provider parser/encoder fixtures for chat-like tool calls, duplicate/missing id repair, invalid argument normalization, and paired output content.
4. Phase 3: Introduce deterministic `ToolExecutor` and `ToolRegistry` interfaces with duplicate detection and unknown-tool normalization, then add focused tests.
5. Phase 4: Implement `ToolCallRuntime` batch scheduler with task spawn/join, deterministic output order, cancellation propagation, timeout/hung terminal states, invariant checks, and history writer tests.
6. Phase 5: Implement minimal Unix `ProcessManager` semantics for process group execution, stdout/stderr caps, SIGTERM/SIGKILL cleanup, and no-orphan tests.
7. Phase 6: Port active sandbox and SSH process-like tools into the typed executor/runtime model, preserving v1 YOLO parallelism and cleanup metadata.
8. Phase 7: Wire the new runtime into the active runner path for opencode-go + deepseek-v4-flash, fail fast for unsupported v1 provider/model combinations, and remove active legacy bridge/fallback execution.
9. Phase 8: Add integration and static tests for paired history, 10+ parallel calls, timeout/cancel/hung, truncation/artifacts, no legacy fallback, and final cargo validation.

## Validation Contract

- Formatting: `cargo fmt --all --check`.
- Focused core check: `cargo check -p oxide-agent-core`.
- Focused lint: `cargo clippy -p oxide-agent-core --all-targets --all-features`.
- Runtime unit tests: `cargo test -p oxide-agent-core tool_runtime -- --nocapture`.
- Provider/parser tests: `cargo test -p oxide-agent-core opencode_go -- --nocapture`.
- Runner/history tests: named focused tests added during phases for paired history and barrier semantics.
- Process tests: named focused tests added during Phase 5 for timeout cleanup, child cleanup, SIGKILL fallback, truncation, and artifacts.
- Static legacy check: grep-based test or documented command proving no active call path uses `tool_bridge`, bridge replay, approval-pending string detection, or old unstructured tool-call fallback.

Done when:
- The active v1 tool execution path runs through `ToolCallRuntime`.
- Every assistant tool-call batch gets exactly one paired tool output per call before the next provider request.
- Batch execution is parallel and output write order is deterministic by original batch index.
- Unknown tool, invalid args, timeout, cancellation, hung detection, executor panic/join error, and cleanup failure all normalize into provider-valid tool outputs.
- stdout/stderr/model-facing content are bounded, with large or binary content represented by artifacts.
- Legacy/fallback execution cannot be re-enabled by feature flag or hidden branch.
- Required validation commands pass, or any remaining gap is documented with evidence and accepted explicitly before goal completion.

## Decisions

- 2026-05-22: Treat the PRD as a large migration, not a single patch; every phase must be independently committable and validated.
- 2026-05-22: Keep v1 strict to `opencode-go` + `deepseek-v4-flash`; broad provider compatibility is intentionally out of scope.
- 2026-05-22: Keep the implementation pragmatic for this personal-scale project: typed runtime contracts and tests are necessary, but no distributed scheduler, HA queue, or policy engine.
- 2026-05-22: Commit after each completed phase as requested by the user.

## Progress Log

- 2026-05-22 20:21: Read `prd/PRD.md`, `AGENTS.md`, `README.md`, workspace `Cargo.toml`, existing goal convention, and confirmed working tree was clean. Active Codex goal created for the async parallel tool runtime migration. Next: commit Phase 0, then start runtime typed foundations.
- 2026-05-22 20:30: Phase 1 implemented typed runtime foundations under `agent/tool_runtime/`, renamed task-local route metadata to `agent/tool_model_route.rs`, and added normalizer tests for success JSON, unknown tool pairing, truncation, and executor error mapping. Validation passed: `cargo test -p oxide-agent-core tool_runtime -- --nocapture`, `cargo check -p oxide-agent-core`, `cargo fmt --all --check`, `cargo clippy -p oxide-agent-core --all-targets --all-features`. Next: Phase 2 strict opencode-go parser/encoder fixtures.
- 2026-05-22 20:36: Phase 2 added strict opencode-go parser/encoder fixtures in `agent/tool_runtime/provider_opencode_go.rs`, covering valid wire ids, object arguments, missing/duplicate id repair, unsupported argument protocol errors, unpairable missing function/name, and exact tool output message encoding. Validation passed: `cargo test -p oxide-agent-core tool_runtime -- --nocapture`, `cargo check -p oxide-agent-core`, `cargo fmt --all --check`, `cargo clippy -p oxide-agent-core --all-targets --all-features`. Next: Phase 3 deterministic executor registry.
- 2026-05-22 20:39: Phase 3 added `ToolExecutor` and deterministic `tool_runtime::ToolRegistry` using exact `ToolName` lookup, duplicate registration failure, sorted specs, unknown-tool output normalization, executor error normalization, and focused registry tests. Validation passed: `cargo test -p oxide-agent-core tool_runtime -- --nocapture`, `cargo check -p oxide-agent-core`, `cargo fmt --all --check`, `cargo clippy -p oxide-agent-core --all-targets --all-features`. Next: Phase 4 batch scheduler and history invariants.
- 2026-05-22 20:45: Phase 4 added isolated `ToolCallRuntime`, `ToolTurnContext`, async `ToolHistoryWriter`, batch invariant verification, per-tool timeout/cancellation select, invalid-args normalization, provider-protocol output handling, join-error normalization, and ordered history writes. Focused tests cover parallel batch execution, deterministic output order, timeout, cancellation, invalid args, provider protocol issue, executor panic, and assistant-before-output history order. Validation passed: `cargo test -p oxide-agent-core tool_runtime -- --nocapture`, `cargo check -p oxide-agent-core`, `cargo fmt --all --check`, `cargo clippy -p oxide-agent-core --all-targets --all-features`. Next: Phase 5 minimal process manager.
- 2026-05-22 20:53: Phase 5 added minimal local `ProcessManager` with shell execution, process group setup, concurrent stdout/stderr draining with caps, per-tool timeout, cancellation cleanup, SIGTERM/SIGKILL process-group cleanup, exit-code normalization, and focused tests for success, non-zero failure, timeout cleanup, SIGKILL fallback, child cleanup, and stdout truncation. Validation passed: `cargo test -p oxide-agent-core tool_runtime -- --nocapture`, `cargo test -p oxide-agent-core tool_runtime::process -- --nocapture`, `cargo check -p oxide-agent-core`, `cargo fmt --all --check`, `cargo clippy -p oxide-agent-core --all-targets --all-features`. Next: Phase 6 sandbox/SSH executor ports.
- 2026-05-22 21:08: Phase 6 added typed runtime executors for sandbox tools and SSH process-like tools. Sandbox runtime executors now emit structured `ToolOutput` for execute/read/write/list/send/recreate helpers, preserve shared parallel read-lock behavior for normal tools, and mark binary file output as non-inline. SSH runtime executors cover `ssh_exec`, `ssh_sudo_exec`, and `ssh_check_process`, expose v1 schemas without approval fields, and create a fresh upstream backend/session per invocation instead of using the provider-level shared backend/call lock. Validation passed: `cargo test -p oxide-agent-core sandbox -- --nocapture`, `cargo test -p oxide-agent-core ssh_mcp -- --nocapture`, `cargo test -p oxide-agent-core tool_runtime -- --nocapture`, `cargo check -p oxide-agent-core`, `cargo fmt --all --check`, `cargo clippy -p oxide-agent-core --all-targets --all-features`. Next: Phase 7 active runner wiring for opencode-go + deepseek-v4-flash.
- 2026-05-22 21:18: Phase 7a prepared active v1 tool exposure by adding executor-side typed runtime registry construction, strict `opencode-go` + `deepseek-v4-flash` route detection, and runtime-spec selection for `current_tool_definitions` and execution prompt preparation. Non-v1 routes keep the existing registry and policy filtering. Validation passed: `cargo test -p oxide-agent-core agent::executor::tests::registry -- --nocapture`, `cargo fmt --all --check`, `cargo check -p oxide-agent-core`, `cargo clippy -p oxide-agent-core --all-targets --all-features`. Next: wire the runner dispatch path through `ToolCallRuntime` for the same v1 route.
- 2026-05-22 21:39: Phase 7b wired the active runner tool-call path through `ToolCallRuntime` when a v1 typed registry is present. Prepared execution now carries the typed registry into `AgentRunnerContext`; the runner converts normalized opencode-go calls into runtime batches, writes assistant/output history through a `ToolHistoryWriter`, emits tool progress, and fails fast if an active route is not the supported `opencode-go` + `deepseek-v4-flash` v1 pair. Validation passed: `cargo test -p oxide-agent-core typed_runtime_path_records_paired_assistant_and_tool_history -- --nocapture`, `cargo test -p oxide-agent-core batch_from_llm_tool_calls_preserves_correlation_ids -- --nocapture`, `cargo test -p oxide-agent-core agent::executor::tests::registry -- --nocapture`, `cargo test -p oxide-agent-core tool_runtime -- --nocapture`, `cargo check -p oxide-agent-core`, `cargo fmt --all --check`, `cargo clippy -p oxide-agent-core --all-targets --all-features`. Next: Phase 8 integration/static cleanup for legacy fallback removal and broad paired-history/parallelism assertions.
- 2026-05-22 21:47: Phase 8a added active-runner regression coverage for multi-call v1 batches. The test uses an empty legacy registry and a typed executor with atomic concurrency counters to prove that the active v1 path uses `ToolCallRuntime`, runs multiple calls concurrently, and writes provider history in original batch order despite out-of-order completion. Validation passed: `cargo test -p oxide-agent-core typed_runtime_path_runs_batch_in_parallel_and_preserves_output_order -- --nocapture`, `cargo check -p oxide-agent-core`, `cargo fmt --all --check`, `cargo clippy -p oxide-agent-core --all-targets --all-features`. Next: static cleanup/guards for active legacy bridge removal and unsupported fallback paths.

## Risks and Blockers

- The PRD intentionally removes several existing compatibility paths; migrations must avoid breaking unrelated transports until the v1 path is clearly gated/fail-fast.
- Current providers return `String` through `ToolProvider`; migrating all active tools may touch many files and should be split by runtime layer, runner wiring, then tool ports.
- Process cleanup in Docker/broker and SSH may require protocol changes; if an upstream cannot guarantee cleanup, output must report best-effort or failed cleanup instead of hiding it.
- Full live provider validation requires an `OPENCODE_GO_API_KEY`; no secrets should be added to docs, tests, or commits.

## Final Verification

- Pending.
