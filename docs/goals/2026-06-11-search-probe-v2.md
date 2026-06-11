# Goal: Search Probe v2 Agentic Research Sidecar

Date started: 2026-06-11
Status: active
Codex goal: `/goal Implement docs/goals/2026-06-11-search-probe-v2.md until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update this document after each meaningful verification, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.`
Source spec: `docs/prd/plan-search-probe.md`
Goal doc owner: Codex
Last updated: 2026-06-11 19:22 +03

## Objective

Implement Search Probe v2 for the web transport: before a normal web `Execute` task reaches the main runtime, run 1-3 fresh agentic probe generations on the selected model route, allow only web-research tools, publish short user-visible progress updates, compile probe handoffs into a compact `SearchProbeDossier`, then start the main agent with clean attention using only the dossier plus the original user prompt.

Done when every required Completion Audit item is verified by its listed evidence, the feature is disabled by default, and the main runtime prompt/cacheable prefix remains unchanged by Search Probe.

## Scope

In scope:
- `docs/prd/plan-search-probe.md` as the source product/implementation plan.
- `docs/goals/2026-06-11-search-probe-v2.md` as the implementation contract.
- Web-only MVP under `crates/oxide-agent-transport-web/src/server/` and `crates/oxide-agent-transport-web/src/session.rs`.
- Minimal core API use through existing `AgentExecutor`, `AgentUserInput`, `AgentExecutionProfile`, and `ToolAccessPolicy`.
- Focused tests for web task execution, probe orchestration, tool policy, dossier injection, failure, cancellation, and model-route inheritance.

Out of scope:
- Telegram transport integration.
- Core-level reusable Search Probe abstraction before MVP validation.
- Deterministic query planning, entity extraction, exact/near-miss scoring, or Rust-driven research heuristics.
- New search HTTP clients outside existing agent tools.
- New crates, services, queues, storage backends, caches, or observability systems.
- Probe transcript persistence or long-term probe memory.
- Direct Google Gemini provider work.

## Missing Inputs

- None for the current checkpoint. User approved starting Checkpoint 1 on `feature/search-probe`.

## Repository Context

- Web task execution entry point: `crates/oxide-agent-transport-web/src/server/task_executor.rs`.
- Web task creation routes produce `TaskRunRequest::Execute` in `crates/oxide-agent-transport-web/src/server/task_routes.rs`.
- Resume route produces `TaskRunRequest::ResumeUserInput` and must not run probe in MVP.
- Event collector already starts before `spawn_executor_task`, so probe events can use the existing task stream.
- Main executor write-lock is acquired inside `spawn_executor_task`; probe must run before that lock.
- `WebSessionManager` owns `LlmClient`, `AgentSettings`, web session metadata, model selection handling, and runtime session creation logic.
- Existing `ToolAccessPolicy` supports allowlists, and runtime tool registration already filters tools by policy.
- Existing project validation conventions: `cargo check` for focused verification, `cargo fmt --all -- --check`, and `cargo clippy --workspace --all-targets -- -D warnings` before final completion.

## Completion Audit

- G1: Web-only Search Probe orchestrator exists
  - Source: `docs/prd/plan-search-probe.md` sections 3-5 and 20.
  - Requirement: Add web transport orchestration that can run Search Probe before main `Execute` tasks and leave `ResumeUserInput` unchanged.
  - Acceptance: `TaskRunRequest::Execute` goes through `maybe_run_search_probe` when enabled; `ResumeUserInput` bypasses probe; disabled config leaves requests unchanged.
  - Evidence required: implementation diff, focused unit tests for enabled/disabled/Resume behavior, and `cargo check -p oxide-agent-transport-web`.
  - Status: in_progress
  - Evidence collected: Checkpoint 1 added `crates/oxide-agent-transport-web/src/server/search_probe.rs`, wired `TaskRunRequest::Execute` through `maybe_run_search_probe`, left `ResumeUserInput` as a no-op skip, and verified disabled/enabled shell behavior with `cargo test -p oxide-agent-transport-web search_probe --lib`.

- G2: Probe runs before parent executor write-lock
  - Source: `docs/prd/plan-search-probe.md` section 4.
  - Requirement: Probe pipeline must not hold the main session executor write-lock while performing search/research.
  - Acceptance: code path invokes probe before `executor_arc.write().await`; test or instrumentation proves probe-start event precedes lock-acquired marker.
  - Evidence required: diff review and focused test or event-order assertion.
  - Status: in_progress
  - Evidence collected: Checkpoint 1 calls `maybe_run_search_probe` after event collector creation and before `spawn_executor_task`; the main execution write-lock remains inside `spawn_executor_task`.

- G3: Probe generations use fresh ephemeral agent runtimes
  - Source: `docs/prd/plan-search-probe.md` sections 2, 8, and 9.
  - Requirement: Each generation uses a fresh `AgentSession`/`AgentExecutor`, does not hydrate durable memory, does not install a memory checkpoint, and does not persist probe transcript.
  - Acceptance: generation 1..N do not share hot memory with each other or the main runtime; only handoffs flow forward.
  - Evidence required: implementation diff and tests or assertions for no registry insertion/checkpoint/hydration path.
  - Status: in_progress
  - Evidence collected: Checkpoint 2 added `WebSessionManager::create_search_probe_executor`, which builds a fresh unregistered `AgentExecutor` with empty memory; focused tests verify no registry insertion and missing parent sessions return `None`.

- G4: Probe inherits selected model route and effort policy
  - Source: `docs/prd/plan-search-probe.md` section 9.
  - Requirement: Probe uses the same selected model route as the web session, with configurable minimum effort defaulting to heavy.
  - Acceptance: selected web model route override is applied to probe executor; standard requests can be elevated to configured probe minimum effort.
  - Evidence required: focused tests for selected route inheritance and effort mapping.
  - Status: in_progress
  - Evidence collected: Checkpoint 2 reuses existing web model-route selection logic for probe executors and verifies selected route inheritance. Checkpoint 3 maps parent effort through the configured probe minimum and covers that mapping with a focused test.

- G5: Probe tool policy is web-research-only
  - Source: `docs/prd/plan-search-probe.md` section 10.
  - Requirement: Probe exposes only `searxng_search`, `crawl4ai_markdown`, and fallback `web_markdown` in MVP.
  - Acceptance: mutable/high-blast-radius tools are unavailable to probe; normal main runtime tool policy remains unchanged.
  - Evidence required: tool registry/policy test showing only allowed probe tools are exposed.
  - Status: in_progress
  - Evidence collected: Checkpoint 2 applies a probe `AgentExecutionProfile` with a deny-by-default tool allowlist; focused tests verify the allowlist can hide all runtime tools.

- G6: Probe final contract and fallback parser work
  - Source: `docs/prd/plan-search-probe.md` sections 6 and 7.
  - Requirement: Parse `search_probe_public_update`, `search_probe_handoff`, and `search_probe_decision`; safely fall back when sections are missing.
  - Acceptance: valid contract extracts all fields; invalid/missing contract produces a raw-response handoff and safe decision behavior.
  - Evidence required: parser unit tests.
  - Status: in_progress
  - Evidence collected: Checkpoint 3 added the XML-like contract parser for `search_probe_public_update`, `search_probe_handoff`, and `search_probe_decision`; focused tests cover valid extraction and raw-text fallback.

- G7: User-visible progress updates are emitted through existing events
  - Source: `docs/prd/plan-search-probe.md` section 12.
  - Requirement: Send started/completed/failure milestones and short public TL;DR updates without introducing new `AgentEvent` variants in MVP.
  - Acceptance: web task stream shows probe generation progress and public updates; optional tool-event forwarding works when enabled.
  - Evidence required: event collector test or persisted event inspection in focused web transport tests.
  - Status: in_progress
  - Evidence collected: Checkpoint 3 emits `Milestone` events for probe/generation lifecycle and `Reasoning` events for public TL;DR updates; focused tests assert public updates use existing `Reasoning` events.

- G8: Main runtime receives compact dossier plus original prompt only
  - Source: `docs/prd/plan-search-probe.md` sections 13 and 14.
  - Requirement: Render handoffs into `SearchProbeDossier`, inject it into `AgentUserInput.content`, preserve attachments, and avoid passing raw probe transcript to main runtime.
  - Acceptance: main input contains original task first and an appended XML-like `<search_probe_dossier>` only when handoffs exist; attachments unchanged; no probe internal message history, raw tool outputs, reasoning, or event stream is injected.
  - Evidence required: unit tests for renderer/injection and attachment preservation.
  - Status: in_progress
  - Evidence collected: Checkpoint 4 added deterministic XML-like dossier rendering and injection into `AgentUserInput.content` after the original prompt. Focused tests cover no-op empty handoffs, XML-like rendering/escaping, attachment preservation, and newest-handoff truncation.

- G9: Failure and cancellation behavior are safe
  - Source: `docs/prd/plan-search-probe.md` sections 17 and 18.
  - Requirement: Probe errors/timeouts do not fail the task; user cancellation during probe prevents main runtime start.
  - Acceptance: partial/failure dossier or unchanged input allows main runtime to start after probe failure; cancellation stops the pipeline and marks task cancelled via existing flow.
  - Evidence required: focused async tests for failure and cancellation paths.
  - Status: in_progress
  - Evidence collected: Checkpoint 3 treats probe errors/timeouts as non-fatal fallback, adds per-generation/total timeout budgeting, and stops before main runtime when the parent cancellation token is cancelled; focused tests cover cancellation and timeout budget behavior.

- Q1: Main runtime prompt cache friendliness is preserved
  - Source: `docs/prd/plan-search-probe.md` section 15.
  - Acceptance: Search Probe does not modify core prompt composer or inject volatile probe data into system prompt/stable prefix; dossier is user input/runtime content only and is appended after the original prompt.
  - Evidence required: diff audit and a focused assertion/test if prompt path is touched.
  - Status: in_progress
  - Evidence collected: Checkpoint 1 did not touch core prompt composition; probe shell returns the original `TaskRunRequest` unchanged. Checkpoint 4 injects probe data only into web `TaskRunRequest::Execute` user input after the original prompt and does not touch core prompt composer/system prompt paths.

- Q2: Simple MVP architecture
  - Source: repository `AGENTS.md` over-engineering constraints and `docs/prd/plan-search-probe.md` section 22.
  - Acceptance: no new crates, services, queues, storage tables, custom search clients, broad abstractions, or transport-wide rewrites.
  - Evidence required: dependency diff and file-scope diff review.
  - Status: in_progress
  - Evidence collected: Checkpoint 1 added one web-transport module. Checkpoint 2 reused `WebSessionManager`, existing route selection, `AgentExecutor`, and `ToolAccessPolicy`. Checkpoint 3 stayed inside web transport and reused existing `AgentEvent` variants; it added a direct `tokio-util` dependency to the web crate only for the already-used workspace `CancellationToken` type.

- N1: No deterministic research logic
  - Source: `docs/prd/plan-search-probe.md` sections 1 and 22.
  - Must preserve: no `should_probe`, entity extractor, exact/near-miss scorer, query template planner, or Rust-owned research heuristics.
  - Evidence required: diff review and test names/content review.
  - Status: in_progress
  - Evidence collected: Checkpoints 1-4 added lifecycle config, an executor factory, generation prompts, contract parsing, event/cancellation handling, and deterministic dossier formatting only; no query planner, entity extraction, scorers, or `should_probe` logic were added.

- N2: Non-web transports remain untouched
  - Source: `docs/prd/plan-search-probe.md` section 3.
  - Must preserve: Telegram transport and transport-agnostic core/runtime behavior are not changed for MVP except using existing public APIs.
  - Evidence required: `git diff --name-only` review.
  - Status: in_progress
  - Evidence collected: Checkpoints 1-4 diff is limited to `crates/oxide-agent-transport-web/` and this goal document.

- V1: Focused web transport validation passes
  - Source: repository validation conventions.
  - Evidence required: `cargo check -p oxide-agent-transport-web` and focused web transport tests for Search Probe.
  - Status: in_progress
  - Evidence collected: Checkpoints 1-4 passed `cargo check -p oxide-agent-transport-web` and `cargo test -p oxide-agent-transport-web search_probe --lib`; Checkpoints 3-4 also passed `cargo fmt --all -- --check` and scoped `cargo clippy -p oxide-agent-transport-web --all-targets -- -D warnings`.

- V2: Final workspace quality gates pass
  - Source: repository `AGENTS.md`.
  - Evidence required: `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets -- -D warnings`.
  - Status: pending
  - Evidence collected:

## Implementation Plan

### Checkpoint 1: Web orchestrator skeleton and config
- Audit IDs: G1, G2, Q1, Q2, N1, N2, V1.
- Expected changes:
  - Add `crates/oxide-agent-transport-web/src/server/search_probe.rs` and wire it from `server/mod.rs`.
  - Add env-backed `SearchProbeConfig` with enabled flag, 1-3 generation clamp, timeouts, min effort, event forwarding flag, tool allowlist, and dossier char cap.
  - Add `maybe_run_search_probe` shell that handles enabled/disabled behavior and skips `ResumeUserInput`.
  - Integrate the shell before the parent executor write-lock in `task_executor.rs`.
  - No actual probe LLM execution yet; this checkpoint establishes the safe lifecycle hook and no-op behavior.
- Validation:
  - `cargo check -p oxide-agent-transport-web`
  - Focused tests for disabled behavior, enabled shell behavior, `ResumeUserInput` skip, and pre-lock ordering if practical at this stage.
- Exit condition: Search Probe module exists, compiles, is disabled by default, does not alter runtime behavior when disabled, and cannot hold the parent executor lock.

### Checkpoint 2: Ephemeral probe executor factory
- Audit IDs: G3, G4, G5, Q2, N2, V1.
- Expected changes:
  - Add a `WebSessionManager` helper that creates an unregistered ephemeral probe `AgentExecutor`.
  - Reuse existing model route selection logic from `session.rs`.
  - Apply probe `AgentExecutionProfile` with stable prompt instructions and tool allowlist.
  - Avoid durable memory hydration, memory checkpoint installation, registry insertion, and transcript persistence.
- Validation:
  - `cargo check -p oxide-agent-transport-web`
  - Focused tests for model route inheritance, tool allowlist, and no registry/checkpoint side effects.
- Exit condition: a fresh probe executor can be created per generation with the selected model and restricted tools.

### Checkpoint 3: Generation runner, final contract parser, and event updates
- Audit IDs: G6, G7, G9, V1.
- Expected changes:
  - Run 1-3 generations with per-generation and total timeouts.
  - Build stable generation prompts from original prompt plus previous handoffs.
  - Parse XML-like final contract and fallback safely.
  - Emit existing `Milestone` and `Reasoning` events for probe progress and public TL;DR updates.
  - Respect cancellation before starting main runtime.
- Validation:
  - `cargo check -p oxide-agent-transport-web`
  - Parser tests, timeout/failure tests, cancellation test, and event stream assertions.
- Exit condition: generations execute agentically and produce structured handoffs without deterministic research logic.

### Checkpoint 4: Dossier render and main input injection
- Audit IDs: G8, Q1, N1, V1.
- Expected changes:
  - Render an XML-like `<search_probe_dossier>` from compact generation handoffs, optional useful public updates, final synthesis, final decision, and truncation marker.
  - Inject dossier after the original `AgentUserInput.content` while preserving attachments.
  - No-op when Search Probe produced no handoffs.
  - Preserve newest handoffs first and set an explicit truncated marker when `dossier_max_chars` is exceeded.
  - Ensure full probe transcript, raw tool outputs, internal reasoning, and internal event stream are not passed to main runtime.
  - Keep probe data out of the main system prompt/cacheable prefix.
- Validation:
  - `cargo check -p oxide-agent-transport-web`
  - Renderer/injection tests, attachment preservation test, prompt-path diff audit.
- Exit condition: main runtime receives original task plus compact dossier only, or unchanged input when no handoffs exist, and starts clean.

### Checkpoint 5: End-to-end web validation and final audit
- Audit IDs: all G*, Q*, N*, V*.
- Expected changes:
  - Add or extend web E2E/focused integration coverage for enabled probe happy path, failure path, and disabled path.
  - Update this goal document with evidence and final decisions.
  - Perform final diff audit against non-goals.
- Validation:
  - `cargo check -p oxide-agent-transport-web`
  - Focused Search Probe tests.
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
- Exit condition: all Completion Audit items are verified with current evidence and no non-goal is violated.

## Validation Contract

- Static checks:
  - `cargo check -p oxide-agent-transport-web`
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
- Tests:
  - Focused Search Probe tests added under web transport.
  - Existing relevant web transport tests where task execution/event behavior is touched.
- Runtime/manual verification:
  - With `OXIDE_SEARCH_PROBE_ENABLED=true`, start a web task and verify visible probe updates precede main runtime activity.
  - With `OXIDE_SEARCH_PROBE_ENABLED=false`, verify behavior matches the current web task path.
- Artifact verification:
  - `git diff --name-only` remains limited to web transport/docs/tests unless explicitly justified.
  - No dependency or lockfile changes unless a blocker proves they are necessary.
- Done when: every Completion Audit item is verified by current evidence and Final Verification is filled.

## Decisions

- 2026-06-11: MVP is web-only. Reason: user explicitly requested only web transport for MVP, and it minimizes blast radius.
- 2026-06-11: Remove deterministic research logic from the plan. Reason: user wants agentic probe behavior, not Rust heuristics or query templates.
- 2026-06-11: Preserve main runtime cache-hit by injecting probe output as user/runtime input only, not into stable system prompt.
- 2026-06-11: Checkpoint 1 required user review before implementation; user approved it and requested work on `feature/search-probe` from `dev`.
- 2026-06-11: User approved Checkpoint 4 dossier policy: append an XML-like dossier after the original prompt; include only compact handoffs, optional useful public updates, final synthesis, decision, and truncation marker; skip injection when no handoffs exist; preserve attachments unchanged; keep probe transcript, raw tool outputs, internal reasoning, and events out of main runtime.

## Progress Log

- 2026-06-11 Goal creation
  - Changed: created `docs/goals/2026-06-11-search-probe-v2.md` from `docs/prd/plan-search-probe.md`.
  - Evidence: source plan reviewed; existing `docs/goals/` convention followed.
  - Commands: not run; documentation-only conversion.
  - Audit IDs updated: none; implementation not started.
  - Next: user review before Checkpoint 1.

- 2026-06-11 18:22 +03 Checkpoint 1: Web orchestrator skeleton and config
  - Changed: created `server/search_probe.rs`, wired `server/mod.rs`, and inserted `maybe_run_search_probe` before `spawn_executor_task` in `task_executor.rs`.
  - Evidence: config defaults disabled, env parsing/clamping, enabled no-op shell, disabled no-op shell, and `ResumeUserInput` skip covered by focused tests.
  - Commands: `cargo fmt --all`; `cargo check -p oxide-agent-transport-web`; `cargo test -p oxide-agent-transport-web search_probe --lib`.
  - Audit IDs updated: G1, G2, Q1, Q2, N1, N2, V1 moved to `in_progress` with Checkpoint 1 evidence.
  - Next: Checkpoint 2, ephemeral probe executor factory.

- 2026-06-11 18:33 +03 Checkpoint 2: Ephemeral probe executor factory
  - Changed: added `SearchProbeRuntimeOptions` and `WebSessionManager::create_search_probe_executor`; Search Probe shell now creates and drops an unregistered probe executor when enabled.
  - Evidence: focused tests cover selected model route inheritance, deny-by-default tool policy, no registry insertion, empty fresh memory, and missing parent session handling.
  - Commands: `cargo fmt --all`; `cargo fmt --all -- --check`; `cargo check -p oxide-agent-transport-web`; `cargo test -p oxide-agent-transport-web search_probe --lib`; `cargo clippy -p oxide-agent-transport-web --all-targets -- -D warnings`.
  - Audit IDs updated: G3, G4, G5 moved to `in_progress`; Q2, N1, N2, V1 evidence extended.
  - Next: Checkpoint 3, generation runner, final contract parser, and event updates.

- 2026-06-11 18:52 +03 Checkpoint 3: Generation runner, final contract parser, and event updates
  - Changed: Search Probe now runs up to 1-3 fresh probe generations, builds stable generation prompts from the original prompt plus previous handoffs, parses the XML-like final contract, emits existing milestone/reasoning events, forwards only optional tool events, and stops before main runtime on cancellation.
  - Evidence: focused tests cover parser extraction/fallback, configured minimum effort mapping, public update event shape, cancellation before generation, and timeout budget calculation.
  - Commands: `cargo fmt --all`; `cargo fmt --all -- --check`; `cargo check -p oxide-agent-transport-web`; `cargo test -p oxide-agent-transport-web search_probe --lib`.
  - Audit IDs updated: G4 evidence extended; G6, G7, G9 moved to `in_progress`; Q2, N1, N2, V1 evidence extended.
  - Next: Checkpoint 4, dossier render and main input injection.

- 2026-06-11 19:22 +03 Checkpoint 4: Dossier render and main input injection
  - Changed: added XML-like `SearchProbeDossier` rendering, no-op empty handoff behavior, newest-handoff truncation with explicit marker, and injection after original `AgentUserInput.content` while preserving attachments.
  - Evidence: focused tests cover empty dossier no-op, XML-like rendering/escaping, original-prompt-first injection, attachment preservation, and truncation preserving newest handoff.
  - Commands: `cargo fmt --all`; `cargo fmt --all -- --check`; `cargo check -p oxide-agent-transport-web`; `cargo test -p oxide-agent-transport-web search_probe --lib`; `cargo clippy -p oxide-agent-transport-web --all-targets -- -D warnings`; `git diff --check`.
  - Audit IDs updated: G8 moved to `in_progress`; Q1, N1, V1 evidence extended.
  - Next: Checkpoint 5, end-to-end web validation and final audit.

## Risks and Blockers

- Existing search budget hook may constrain probe too much
  - Impact: probe may stop before useful research if default search budget applies too aggressively.
  - Evidence: plan allows search-budget relaxation only if needed.
  - Mitigation or requested decision: first try existing effort/min-effort controls; relax or disable `search_budget` for probe only if tests/manual runs prove it blocks the intended behavior.
  - Audit IDs affected: G3, G5, Q2.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
