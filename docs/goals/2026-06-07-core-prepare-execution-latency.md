# Goal: Core Prepare Execution Latency

Date started: 2026-06-07
Status: active
Codex goal: `/goal Implement docs/goals/2026-06-07-core-prepare-execution-latency.md until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update this document after each meaningful verification, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.`
Source spec: User request after web first-message latency was reduced from `1482ms` to `~0.55ms`; remaining first-LLM delay is now inside core `prepare_execution` and pre-LLM runner work.
Goal doc owner: Codex
Last updated: 2026-06-07 15:09

## Objective

Find and reduce the remaining core-side delay between web task executor entry and the first LLM call, starting with precise INFO-level latency observability inside `prepare_execution` and wiki context assembly.

Done when current runtime logs identify the dominant core sub-phase, the selected optimization is implemented with the smallest maintainable change, validation passes, and all Completion Audit items are verified by evidence.

## Scope

In scope:
- `crates/oxide-agent-core/src/agent/executor/execution.rs` execution preparation timing.
- `crates/oxide-agent-core/src/agent/wiki_memory/context.rs` and related wiki read-through cache timing.
- `crates/oxide-agent-core/src/agent/runner/execution.rs` pre-LLM runner timing if needed to separate prepare from loop overhead.
- `crates/oxide-agent-core/src/agent/runner/llm_calls.rs` only for final pre-provider boundary timing if needed.
- Focused docs updates and validation commands.

Out of scope:
- Web transport hot-path caching already completed in `docs/goals/2026-06-07-web-first-message-write-behind.md`.
- Telegram transport behavior.
- LLM provider API behavior or model routing semantics.
- New services, queues, storage backends, distributed cache, HA, sharding, or broad abstractions.
- Direct Google Gemini provider work.

## Missing Inputs

- No hard target exists for core prepare latency.
  - Impact: optimization success starts as relative improvement and bottleneck removal, not a fixed SLO.
  - Low-risk assumption or fallback: target first implementation to reduce `prepare_execution` from `~601ms` to under `~250ms` if wiki reads dominate, while preserving current behavior.
  - User/external action needed: provide a stricter target if `~250ms` prepare is insufficient.

## Repository Context

- Web first-message hot path is no longer the bottleneck: runtime evidence showed `create_task completed ~0.55ms` and `core_executor_call_started ~0.6ms` after checkpoint 5 in the web goal.
- Remaining measured path from the same runtime sample:
  - `core_executor_call_started -> Starting agent task`: `~113ms`.
  - `Starting agent task -> first LLM call`: `~617ms`.
  - Existing aggregate core log: `prepare_ms=601`.
- Current aggregate prepare log is emitted in `crates/oxide-agent-core/src/agent/executor/execution.rs:227`.
- `prepare_execution` starts at `crates/oxide-agent-core/src/agent/executor/execution.rs:393` and includes tool registry/spec collection, wiki context, prompt assembly, memory conversion, and runner config.
- Wiki context assembly starts at `crates/oxide-agent-core/src/agent/executor/execution.rs:466`, calls `WikiContextAssembler::assemble_for_context` in `crates/oxide-agent-core/src/agent/wiki_memory/context.rs:61`, and creates a fresh `WikiSessionCache` per prepare.
- Validation convention for this work: `cargo fmt`, `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`, and `cargo clippy --workspace --no-default-features --features profile-web-embedded-opencode-local` when Rust code changes.

## Completion Audit

- G1: Core prepare phase observability is present
  - Source: User asked for RECON and a plan because `prepare_execution: 601ms` is now the dominant delay before first LLM call.
  - Acceptance: INFO logs under `oxide_agent_core::agent_latency` show phase-level timings for model route resolution, tool runtime registry build, tool specs collection, wiki context rendering, prompt assembly, memory conversion, and final prepare completion.
  - Evidence required: code diff, successful Rust validation, and runtime log showing the new phase logs with `phase_ms` and `elapsed_ms`.
  - Status: in_progress
  - Evidence collected: Core-1 implementation adds `oxide_agent_core::agent_latency` INFO phase logs inside `prepare_execution` for `prepare_started`, `todos_snapshot_created`, `model_routes_resolved`, `tool_runtime_registry_built`, `tool_specs_collected`, `structured_output_resolved`, `wiki_context_rendered`, `prompt_instructions_resolved`, `system_prompt_assembled`, `memory_messages_converted`, `runner_limits_resolved`, and `prepare_assembled`. Validation passed: `cargo fmt`; `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`; `cargo clippy --workspace --no-default-features --features profile-web-embedded-opencode-local`. Runtime log evidence is still pending after rebuild.

- G2: Wiki context latency is isolated
  - Source: RECON found `render_wiki_context_for_task` is the most likely source of `~601ms`, because it performs multiple storage reads and currently has no INFO sub-phase logs.
  - Acceptance: Runtime logs show whether wiki is skipped, empty, successful, or failed, plus duration, rendered byte count, and candidate/page counts or enough sub-phase markers to identify storage read cost.
  - Evidence required: code diff, runtime logs from a first-message run, and updated analysis in this document.
  - Status: pending
  - Evidence collected:

- G3: Dominant core bottleneck is identified from real logs
  - Source: User wants the next action based on truth, not assumptions.
  - Acceptance: This document records runtime timings after Core-1/Core-2 logs and names the dominant sub-phase by measured evidence.
  - Evidence required: user or local runtime log excerpt with phase timings and before/after comparison against the `prepare_ms=601` sample.
  - Status: pending
  - Evidence collected:

- G4: Selected core optimization is implemented only after measurement
  - Source: Repository guidance forbids over-engineering; current evidence supports logging first, then the smallest fix.
  - Acceptance: The fix targets the measured bottleneck only, preserves agent semantics, and avoids new dependencies/services. Candidate fixes include reusing/caching wiki session context, caching stable tool specs/prompt blocks, or moving safe wiki work out of the hot path if evidence supports it.
  - Evidence required: implementation diff, validation commands, runtime before/after measurement, and updated decision record.
  - Status: pending
  - Evidence collected:

- Q1: Existing architecture and cache-hit invariants are preserved
  - Source: `AGENTS.md` architecture and prompt-cache guidance.
  - Acceptance: Core/runtime remain transport-agnostic; prompt dynamic blocks stay at the end; no direct Google Gemini provider is introduced; no new external infrastructure is added.
  - Evidence required: diff review and validation commands.
  - Status: in_progress
  - Evidence collected: Core-1 only adds INFO latency logging in `crates/oxide-agent-core/src/agent/executor/execution.rs`; it does not change prompt assembly order, tool registration, wiki behavior, provider routing, transport boundaries, or add dependencies. Validation passed: `cargo fmt`; `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`; `cargo clippy --workspace --no-default-features --features profile-web-embedded-opencode-local`.

- N1: Web transport checkpoint remains closed
  - Source: CP5 runtime evidence proved web task creation is no longer bottleneck.
  - Must preserve: Avoid reopening web task create persistence/caching unless new core logs prove a web interaction is still on the hot path.
  - Evidence required: diff scope review.
  - Status: in_progress
  - Evidence collected: Core-1 diff scope is limited to core executor logging and this goal doc; no web transport files are touched.

## Implementation Plan

1. Core-1: Add `prepare_execution` phase logs
   - Audit IDs: G1, Q1, N1
   - Expected changes: Add small timing helper or explicit `Instant` checkpoints in `crates/oxide-agent-core/src/agent/executor/execution.rs` around model route resolution, tool registry build, `specs()`, wiki rendering, prompt instructions, system prompt, memory conversion, and prepare completion.
   - Validation: `cargo fmt`; `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`; `cargo clippy --workspace --no-default-features --features profile-web-embedded-opencode-local`.
   - Exit condition: Runtime logs can split aggregate `prepare_ms` into named sub-phases.

2. Core-2: Add wiki context sub-phase logs
   - Audit IDs: G2, Q1
   - Expected changes: Add INFO timing around wiki store availability, global index load, context index load, candidate selection, page loads, and render result in `wiki_memory/context.rs` or a narrow wrapper around it.
   - Validation: Same Rust validation commands.
   - Exit condition: Runtime logs prove whether wiki storage reads dominate `prepare_execution`.

3. Core-3: Analyze runtime evidence and choose one fix
   - Audit IDs: G3
   - Expected changes: Update this document with measured phase table and decision.
   - Validation: Runtime log review.
   - Exit condition: One dominant bottleneck is named with numbers, or the document records that latency is distributed and proposes the smallest combined fix.

4. Core-4: Implement the measured optimization
   - Audit IDs: G4, Q1, N1
   - Expected changes: Depends on Core-3 evidence. Likely options: reuse/carry `WikiSessionCache` across prepares, cache rendered wiki context for the same session/context/task keywords, cache stable tool specs/prompt blocks, or make non-critical wiki context asynchronous/background.
   - Validation: Rust validation commands plus runtime before/after logs.
   - Exit condition: Measured `prepare_execution` latency improves materially while behavior and architectural constraints remain intact.

## Validation Contract

- Static checks:
  - `cargo fmt`
  - `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`
  - `cargo clippy --workspace --no-default-features --features profile-web-embedded-opencode-local`
  - `git diff --check`
- Runtime/manual verification:
  - Rebuild/run web profile and capture `docker logs oxide_agent_web -f` for a first-message task.
  - Confirm logs include `oxide_agent_core::agent_latency` prepare sub-phases and first LLM boundary.
- Done when:
  - Every Completion Audit item is verified by current evidence, or a documented blocker names the exact missing runtime evidence.

## Decisions

- 2026-06-07: Treat web first-message optimization as complete for this goal. CP5 reduced `create_task` from `1482ms` to `~0.55ms`; remaining delay is core-side.
- 2026-06-07: Start with observability, not optimization. `prepare_ms=601` is aggregate and several plausible contributors exist; optimizing without sub-phase logs risks changing the wrong subsystem.
- 2026-06-07: Prefer the smallest measured fix. No new crates/services/queues are justified.

## Progress Log

- 2026-06-07 14:50: Goal created after RECON
  - Changed: Added this goal document with scope, audit IDs, checkpoints, validation contract, and first-step recommendation.
  - Evidence: RECON traced `prepare_execution` and likely contributors: wiki context reads, tool registry/specs, prompt assembly, memory conversion, and runner pre-LLM work.
  - Commands: `git diff --check` passed. Rust validation not run because this checkpoint is docs-only.
  - Audit IDs updated: G1-G4/Q1/N1 pending.
  - Next: Core-1 implementation: add INFO phase timing inside `prepare_execution`.

- 2026-06-07 15:06: Core-1 prepare phase logging implemented
  - Changed: Added INFO phase timing in `crates/oxide-agent-core/src/agent/executor/execution.rs` across the serial `prepare_execution` pipeline.
  - Evidence: Logs now split aggregate prepare latency into model route resolution, tool registry/spec collection, wiki context render, prompt assembly, memory conversion, runner limit resolution, and final prepare assembly.
  - Commands: `cargo fmt`; `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`; `cargo clippy --workspace --no-default-features --features profile-web-embedded-opencode-local`.
  - Audit IDs updated: G1 in progress, Q1 in progress, N1 in progress.
  - Next: Collect runtime logs after rebuild to verify G1 and decide whether Core-2 wiki sub-phase logging is still needed.

## Risks and Blockers

- Runtime evidence depends on user rebuild/run logs.
  - Impact: Code-level observability can be validated statically, but bottleneck attribution needs actual runtime logs against the same remote Postgres/storage setup.
  - Evidence: Prior CP logs were user-provided from Docker.
  - Mitigation or requested decision: If local runtime is unavailable, stop after validated logging changes and ask user for logs.
  - Audit IDs affected: G2, G3, G4.

- Wiki optimization may trade memory for latency.
  - Impact: In-process caching could lose data/state on container restart, but user has explicitly accepted crash-loss for latency work.
  - Evidence: User stated container crash data loss is acceptable during web write-behind work.
  - Mitigation or requested decision: Keep caches bounded and document semantics before implementing.
  - Audit IDs affected: G4, Q1.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
