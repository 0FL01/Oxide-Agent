# Goal: Deterministic Research Runtime

Date started: 2026-06-10
Status: active
Codex goal: `/goal Implement docs/goals/2026-06-10-deterministic-research-runtime.md until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update this document after each meaningful verification, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.`
Source spec: `docs/prd/plan.md`
Goal doc owner: Codex
Last updated: 2026-06-10 16:56 +03

## Objective

Implement a deterministic research runtime that separates “the model wrote this” from “the system checked this” for agent answers. The runtime must observe typed tool outputs, normalize search/fetch provider payloads, track passive research state, and eventually gate high-impact final claims with evidence-aware policy.

Done when every required Completion Audit item is verified by its listed evidence and all out-of-scope constraints are preserved.

## Scope

In scope:
- Agent lifecycle and hook execution in `crates/oxide-agent-core/src/agent/runner/`, `agent/hooks/`, `agent/tool_runtime/`, and `agent/executor/`.
- Passive deterministic research state in `crates/oxide-agent-core/src/agent/research/`.
- Provider structured payload normalization for the primary stack: `searxng_search` discovery and `crawl4ai_markdown` fetched evidence.
- Fallback/compatibility normalization for `web_markdown`, `web_search` / `web_extract`, `brave_search`, and `duckduckgo_*` only after the primary boundary is working.
- Runtime tests, provider tests, and focused validation commands for touched areas.
- Goal document updates after each checkpoint commit.

Out of scope:
- New storage backends, queues, services, crates, or generalized observability frameworks.
- Direct Google Gemini provider code.
- A full query planner, fetch planner, claim/evidence linker, or adversarial verifier before typed evidence capture exists.
- Treating Markdown or sub-agent prose as the primary source of truth for verified evidence.
- Browser-side search UX. Web transport should continue streaming backend agent events over the existing task/SSE model.

## Missing Inputs

- Exact final config flag names for rollout.
  - Impact: implementation needs stable env/config names for passive runtime, guard, and audit behavior.
  - Low-risk assumption or fallback: use the PRD names `RESEARCH_RUNTIME_ENABLED`, `RESEARCH_GUARD_ENABLED`, `RESEARCH_AUDIT_ENABLED`, `RESEARCH_DEBUG_TRACE` unless existing config conventions suggest a better `OXIDE_*` prefix.
  - User/external action needed: none before checkpoint 1; decide before config checkpoint if naming matters.

## Repository Context

- Relevant entry points:
  - `crates/oxide-agent-core/src/agent/runner/execution.rs`
  - `crates/oxide-agent-core/src/agent/runner/tools.rs`
  - `crates/oxide-agent-core/src/agent/runner/responses.rs`
  - `crates/oxide-agent-core/src/agent/runner/hooks.rs`
  - `crates/oxide-agent-core/src/agent/hooks/types.rs`
  - `crates/oxide-agent-core/src/agent/tool_runtime/runtime.rs`
  - `crates/oxide-agent-core/src/agent/providers/searxng/`
  - `crates/oxide-agent-core/src/agent/providers/crawl4ai_markdown/`
- Existing conventions:
  - Keep `oxide-agent-core` transport-agnostic.
  - Preserve parallel tool-call history repair and `tool_call_id` integrity.
  - Use explicit modules and predictable exports.
  - Prefer small, boring, locally understandable changes.
- Dependencies or runtime assumptions:
  - Rust 1.94 workspace.
  - `ToolOutput` already contains `structured_payload` and serializes it to model-facing content.
  - Primary research stack is self-hosted/local `searxng_search` + `crawl4ai_markdown`.
- Validation infrastructure:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
  - Focused `cargo test -p oxide-agent-core ...` tests for changed provider/runtime modules.
- Risky areas:
  - Runtime hook enforcement can affect all tool execution.
  - Search provider failures currently reported as successful stdout in some paths.
  - Final-answer guard can cause continuation loops if enabled too early.

## Completion Audit

- G1: Real `BeforeTool` policy boundary is enforced in typed runtime
  - Source: `docs/prd/plan.md` lines 72, 158-162, 558-575, 1393-1398.
  - Requirement: typed tool execution must dispatch or otherwise apply pre-tool policy after arguments are parsed and before provider execution.
  - Acceptance: `SearchBudgetHook`, `ToolAccessPolicyHook`, and sub-agent blocked-tool policy have runtime-path tests proving a blocked tool returns a pairable failure `ToolOutput` instead of silently executing or corrupting tool-call history.
  - Evidence required: focused runtime/hook tests plus `cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local --lib <test-filter>`.
  - Status: verified
  - Evidence collected: `crates/oxide-agent-core/src/agent/runner/tools.rs:213` collects parsed pre-tool policy blocks; `crates/oxide-agent-core/src/agent/runner/hooks.rs:113` dispatches `HookEvent::BeforeTool`; `crates/oxide-agent-core/src/agent/tool_runtime/runtime.rs:119` applies blocks as pairable `ToolOutput` before executor dispatch; tests `typed_runtime_before_tool_applies_tool_access_policy_without_dispatch`, `typed_runtime_before_tool_applies_search_budget_without_dispatch`, `typed_runtime_before_tool_applies_sub_agent_safety_without_dispatch`, and `pre_tool_block_returns_paired_failure_without_executor_dispatch` passed.

- G2: `AfterAgent` hook results are applied consistently
  - Source: `docs/prd/plan.md` lines 118-126 and 1447-1452.
  - Requirement: `AfterAgent -> ForceIteration` must keep existing behavior, and `AfterAgent -> Finish` / `Block` must no longer be silently ignored.
  - Acceptance: final-response runner tests cover `ForceIteration`, `Finish`, and `Block` outcomes.
  - Evidence required: focused runner tests and review of `crates/oxide-agent-core/src/agent/runner/responses.rs` diff.
  - Status: verified
  - Evidence collected: `crates/oxide-agent-core/src/agent/runner/responses.rs:210` returns errors for `AfterAgent -> Block`; `crates/oxide-agent-core/src/agent/runner/responses.rs:213` saves and returns `AfterAgent -> Finish` reports; existing ForceIteration test plus new tests `after_agent_finish_overrides_final_response` and `after_agent_block_returns_error_without_saving_final_response` passed.

- G3: Passive `ResearchRuntime` records typed research observations
  - Source: `docs/prd/plan.md` lines 107-116, 142-152, 1399-1403, 1451-1454.
  - Requirement: add passive state for queries, search leads, fetched sources, source priority, snippet-only flag, failures, truncation, and anti-bot signals without requiring a full evidence ledger.
  - Acceptance: `PreparedExecution`, `AgentRunnerContext`, and `HookContext` can carry the runtime; `runner/tools.rs` records full `ToolOutput` before string-only `AfterTool` hooks; runtime is safe to disable.
  - Evidence required: unit tests for `ResearchRuntime::record_tool_output`, runner integration test or focused state test, and `cargo check` for the profile touched.
  - Status: verified
  - Evidence collected: `crates/oxide-agent-core/src/agent/research/mod.rs:125` adds the passive `ResearchRuntime`; `crates/oxide-agent-core/src/agent/research/mod.rs:137` records structured search leads, fetched sources, failures, truncation, source priority, snippet-only, and anti-bot signals from full `ToolOutput`; `crates/oxide-agent-core/src/agent/executor/types.rs:50`, `crates/oxide-agent-core/src/agent/runner/types.rs:158`, and `crates/oxide-agent-core/src/agent/hooks/types.rs:114` thread optional runtime state through prepared execution, runner context, and hook context; `crates/oxide-agent-core/src/agent/runner/tools.rs:379` records the full `ToolOutput` before string-only `AfterTool` hooks. Tests `records_search_payload_leads_and_query`, `records_fetch_payload_truncation_and_failure_signals`, `ignores_non_research_tools`, and `typed_runtime_records_research_output_before_after_tool_hooks` passed.

- G4: Primary provider payloads are structured and status-correct
  - Source: `docs/prd/plan.md` lines 128-140, 577-629, 1423-1427, 1455.
  - Requirement: `searxng_search` and `crawl4ai_markdown` success/failure outputs must expose canonical `structured_payload` and provider failures must use `ToolOutputStatus::Failure`.
  - Acceptance: SearXNG payload has `provider`, `kind`, `query`, ranked `results`, and `fetched_at`; Crawl4AI payload has `provider`, `kind: "fetch"`, URL/final URL/status/markdown/truncation/freshness metadata; failures preserve human-readable stdout but are not marked success.
  - Evidence required: provider tests for success payload and failure status for both tools.
  - Status: pending
  - Evidence collected:

- G5: Fallback provider normalization does not drive the architecture
  - Source: `docs/prd/plan.md` lines 631-674 and 1456.
  - Requirement: normalize `web_markdown`, Tavily, Brave, and DuckDuckGo only for compatibility/fallback after the primary stack is working.
  - Acceptance: fallback payloads are canonical enough for UI/audit/ledger, but no core research policy depends on them as mandatory providers.
  - Evidence required: provider tests or explicit deferred decision recorded if not implemented in the first milestone.
  - Status: pending
  - Evidence collected:

- G6: Final answer guard is soft, evidence-aware, and config-gated
  - Source: `docs/prd/plan.md` lines 736-815, 1405-1409, 1447-1457.
  - Requirement: implement `FinalAnswerGuardHook` behind config; it must use `AfterAgent -> ForceIteration` for unsupported high-impact claims and remain disabled or observe-only by default at first.
  - Acceptance: conceptual stable answers pass; volatile/current/price/version/legal/high-impact claims without adequate evidence force continuation only when guard is enabled for research modes.
  - Evidence required: focused hook tests for allow/block/caveat decisions, config-default test, and review of hook registration.
  - Status: pending
  - Evidence collected:

- G7: Research audit artifacts are available without becoming policy
  - Source: `docs/prd/plan.md` lines 816-882.
  - Requirement: emit optional JSON/Markdown audit artifacts or equivalent structured debug state when enabled.
  - Acceptance: audit includes task kind/mode, providers used, queries, fetched URLs, evidence observations, unsupported claims, and final guard decision when available.
  - Evidence required: unit test or fixture output plus documentation of artifact location/config.
  - Status: pending
  - Evidence collected:

- Q1: Preserve project architecture and simplicity
  - Source: `AGENTS.md` lines 11-23 and 50-64.
  - Acceptance: no new crates/services/storage layers; core/runtime remain transport-agnostic; Gemini direct provider remains absent.
  - Evidence required: `git diff` review and `cargo check`.
  - Status: in_progress
  - Evidence collected: Checkpoint 1 added no new crates/services/storage layers; lifecycle changes stayed in `oxide-agent-core`; Checkpoint 2 added only in-process passive state in `oxide-agent-core`; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` passed.

- Q2: Preserve prompt-cache and tool-call invariants
  - Source: `AGENTS.md` lines 68-72 and 86-94.
  - Acceptance: tool calls still run in parallel; history repair and `tool_call_id` matching remain intact; no large volatile blocks are added to the stable prompt prefix.
  - Evidence required: focused tests for tool history plus diff review of prompt changes.
  - Status: in_progress
  - Evidence collected: Tool calls still execute through the existing parallel typed runtime; blocked calls are emitted as ordered pairable `ToolOutput` values; focused tests verify tool-call IDs and provider tool-call IDs are preserved for blocked policy paths. Checkpoint 2 records `ToolOutput` passively before `AfterTool` without changing tool-call content or prompt assembly.

- Q3: Rollout is backward-compatible
  - Source: `docs/prd/plan.md` lines 1290-1317 and 1405-1409.
  - Acceptance: passive runtime and guard can be disabled; guard is not strict-by-default; provider stdout remains model-readable after payload changes.
  - Evidence required: config-default tests and provider output tests.
  - Status: in_progress
  - Evidence collected: `PreparedExecution.research_runtime` is `None` by default in `crates/oxide-agent-core/src/agent/executor/execution.rs`, so passive research observation remains disabled unless explicitly supplied; no final-answer guard behavior changed in Checkpoint 2.

- N1: No premature full research planner or full evidence graph
  - Source: `docs/prd/plan.md` lines 1399-1421.
  - Must preserve: first milestone stops at typed boundary, passive ledger, provider payloads, and soft guard; query/fetch planners and full claim/evidence linking are later work.
  - Evidence required: `git diff` review and updated Decisions if scope changes.
  - Status: in_progress
  - Evidence collected: Checkpoint 2 added passive observation types and no query planner, fetch planner, claim/evidence graph, or final-answer guard.

- N2: Fetch tools are not reduced to a flat hard search counter
  - Source: `docs/prd/plan.md` lines 676-735 and 1429-1433.
  - Must preserve: `crawl4ai_markdown` / `web_markdown` control should use dedupe, failed-host quarantine, anti-bot quarantine, truncation, and progress guards rather than a simple global search counter.
  - Evidence required: policy design diff and tests when fetch loop guards are implemented.
  - Status: in_progress
  - Evidence collected: `cargo fmt --all -- --check` passed for Checkpoint 2; full final clippy remains pending for rollout readiness.

- V1: Formatting and lint validation
  - Source: `AGENTS.md` lines 145-153.
  - Evidence required: `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings` before final completion, or documented narrower checkpoint validation when appropriate.
  - Status: pending
  - Evidence collected:

- V2: Build and focused test validation
  - Source: `AGENTS.md` lines 132-153.
  - Evidence required: `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`; focused `cargo test -p oxide-agent-core` filters for touched runtime/provider modules.
  - Status: in_progress
  - Evidence collected: Checkpoint 1 focused tests and embedded profile check passed on 2026-06-10. Checkpoint 2 tests `research::` and `typed_runtime_records_research_output_before_after_tool_hooks` passed, and `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` passed; full final validation remains pending for later checkpoints.

## Implementation Plan

### Checkpoint 1: Lifecycle correctness boundary
- Audit IDs: G1, G2, Q1, Q2, V2
- Expected changes:
  - Add real pre-tool policy dispatch in the typed runtime path after parsed arguments are available and before provider execution.
  - Convert policy blocks into pairable failure `ToolOutput` values rather than aborting whole batches.
  - Add runtime-path tests proving blocked tools do not execute and tool-call history remains valid.
  - Fix `AfterAgent` handling for `Finish` and `Block` while preserving `ForceIteration` behavior.
- Validation:
  - `cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local --lib <focused hook/runtime filters>`
  - `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
- Exit condition: real runtime enforcement is proven by tests, and final-response hook results are no longer silently ignored.

### Checkpoint 2: Passive research runtime and full `ToolOutput` observer
- Audit IDs: G3, Q1, Q2, Q3, N1, V2
- Expected changes:
  - Add `agent/research` with minimal passive types and runtime.
  - Thread optional runtime through `PreparedExecution`, `AgentRunnerContext`, and `HookContext`.
  - Record full `ToolOutput` in `runner/tools.rs` before string-only hooks.
  - Keep runtime disabled or observe-only by default.
- Validation:
  - Focused `ResearchRuntime` unit tests.
  - `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`.
- Exit condition: search/fetch/failure observations are captured from typed payloads without changing final-answer behavior.

### Checkpoint 3: Primary provider payload contract
- Audit IDs: G4, Q3, V2
- Expected changes:
  - Add SearXNG success/failure structured payloads and correct failure status.
  - Move Crawl4AI success JSON into `structured_payload` while preserving model-readable stdout.
  - Prefer SearXNG as discovery and Crawl4AI as fetched-source evidence in runtime classification.
- Validation:
  - Provider tests for `searxng_search` and `crawl4ai_markdown` payload/status.
  - `cargo test -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local --lib -- searxng`
  - `cargo test -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local --lib -- crawl4ai`
- Exit condition: primary research stack produces typed evidence inputs without Markdown parsing as the source of truth.

### Checkpoint 4: Soft final answer guard
- Audit IDs: G6, Q3, N1, V2
- Expected changes:
  - Add deterministic high-impact claim heuristic for volatile/current/legal/pricing/version/current-status claims.
  - Add `FinalAnswerGuardHook` using `AfterAgent -> ForceIteration` and concrete next-action context.
  - Register guard only behind config and only for research/deep/paranoid behavior at first.
- Validation:
  - Hook tests: conceptual pass, unsupported high-impact claim blocks, fetched non-snippet evidence allows, snippet-only evidence does not allow high-impact claim.
  - Config-default test proving guard is not strict-by-default.
- Exit condition: guard can safely run in soft/observe mode without surprising standard conceptual tasks.

### Checkpoint 5: Fallback normalization and audit artifact
- Audit IDs: G5, G7, Q1, Q3, V1, V2
- Expected changes:
  - Normalize fallback provider payloads enough for audit/UI/ledger compatibility.
  - Add optional audit artifact or equivalent structured debug output.
  - Update docs/config examples only after behavior is validated.
- Validation:
  - Provider tests for touched fallback tools.
  - Audit fixture or snapshot.
  - `cargo fmt --all -- --check` and focused `cargo clippy` or full workspace clippy before final completion.
- Exit condition: fallback providers degrade gracefully and audit output can explain research decisions.

### Checkpoint 6: Completion audit and rollout readiness
- Audit IDs: all
- Expected changes:
  - Run full completion audit.
  - Fill `Final Verification` only if every required item has current evidence.
  - Commit final documentation and evidence updates.
- Validation:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
  - Focused provider/runtime tests listed above.
- Exit condition: every Completion Audit item is `verified` or explicitly `dropped_by_user` with user evidence.

## Validation Contract

- Static checks:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
- Build checks:
  - `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
- Tests:
  - Focused `cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local --lib <runtime/hook filter>` for lifecycle changes.
  - `cargo test -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local --lib -- crawl4ai` for Crawl4AI provider changes.
  - Focused SearXNG/provider tests under the feature profile that enables SearXNG.
- Runtime/manual verification:
  - Optional after core tests: run a web/telegram research task with SearXNG + Crawl4AI enabled and inspect streamed tool events/audit output.
- Artifact verification:
  - Goal doc progress log updated after each checkpoint.
  - Audit artifact inspected when G7 is implemented.
- Done when: all Completion Audit items are verified with current evidence and final status is marked complete.

## Decisions

- 2026-06-10: Use `docs/prd/plan.md` as the source spec because the user explicitly requested goal creation from that file.
- 2026-06-10: Existing repository convention is `docs/goals/<YYYY-MM-DD>-<short-slug>.md`; this goal uses `docs/goals/2026-06-10-deterministic-research-runtime.md`.
- 2026-06-10: The first implementation checkpoint is lifecycle correctness (`BeforeTool` runtime enforcement plus `AfterAgent` `Finish`/`Block`) before `ResearchRuntime`, matching the fixed RECON decisions.
- 2026-06-10: Primary research architecture is `searxng_search` for discovery and `crawl4ai_markdown` for fetched evidence; other search/fetch providers are fallback compatibility.
- 2026-06-10: Guard starts disabled or observe-only by default. Runtime policy, not prompt guidance, is the acceptance boundary.
- 2026-06-10: Checkpoint 2 keeps passive research runtime disabled by default (`PreparedExecution.research_runtime = None`) and only records observations when a runtime is explicitly supplied.

## Progress Log

- 2026-06-10 16:13 +03: Goal contract created from `docs/prd/plan.md`.
  - Changed: added this repo-local goal document with objective, scope, Completion Audit, checkpoints, validation contract, decisions, risks, and first implementation step.
  - Evidence: source spec inspected through line 1457; existing goal-doc convention inspected in `docs/goals/2026-06-08-crawl4ai-markdown-module-slice.md`; branch/status checked on `feature/research-madness`.
  - Commands: `git status --short --branch`; file reads for `AGENTS.md`, `README.md`, existing goal doc, and `docs/prd/plan.md`.
  - Audit IDs updated: none verified; goal is active.
  - Next: Checkpoint 1 -- lifecycle correctness boundary.

- 2026-06-10 16:36 +03: Checkpoint 1 lifecycle correctness boundary implemented.
  - Changed: added runtime-path `BeforeTool` dispatch from parsed tool calls, precomputed policy blocks, pairable policy failure outputs, and consistent `AfterAgent` `Finish`/`Block` handling.
  - Evidence: `crates/oxide-agent-core/src/agent/runner/tools.rs:213`, `crates/oxide-agent-core/src/agent/runner/hooks.rs:113`, `crates/oxide-agent-core/src/agent/tool_runtime/runtime.rs:119`, `crates/oxide-agent-core/src/agent/runner/responses.rs:210`.
  - Commands: `cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local --lib pre_tool_block_returns_paired_failure_without_executor_dispatch`; `cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local --lib typed_runtime_before_tool_applies`; `cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local --lib after_agent_`; `cargo fmt --all -- --check`; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`.
  - Audit IDs updated: G1 verified, G2 verified, Q1/Q2/V2 in progress with checkpoint evidence.
  - Next: Checkpoint 2 -- passive research runtime and full `ToolOutput` observer.

- 2026-06-10 16:56 +03: Checkpoint 2 passive research runtime and full `ToolOutput` observer implemented.
  - Changed: added `agent/research` passive runtime, threaded optional runtime through prepared execution, runner context, and hook context, and recorded full typed `ToolOutput` before `AfterTool` string hooks.
  - Evidence: `crates/oxide-agent-core/src/agent/research/mod.rs:125`, `crates/oxide-agent-core/src/agent/research/mod.rs:137`, `crates/oxide-agent-core/src/agent/runner/tools.rs:379`, `crates/oxide-agent-core/src/agent/hooks/types.rs:114`.
  - Commands: `cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local --lib research::`; `cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local --lib typed_runtime_records_research_output_before_after_tool_hooks`; `cargo fmt --all -- --check`; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`.
  - Audit IDs updated: G3 verified; Q1/Q2/Q3/N1/V1/V2 in progress with checkpoint evidence.
  - Next: Checkpoint 3 -- primary provider payload contract for SearXNG + Crawl4AI.

## Risks and Blockers

- Runtime policy dispatch can affect all tool calls.
  - Impact: a wrong block path can corrupt tool-call pairing or break parallel batch execution.
  - Evidence: PRD identifies `BeforeTool` as currently absent from the real typed path.
  - Mitigation or requested decision: implement checkpoint 1 with pairable failure `ToolOutput` and focused history/policy tests before research state.
  - Audit IDs affected: G1, Q2.

- Provider failure status changes can alter model behavior.
  - Impact: SearXNG/Tavily-like failures currently look like success text in some paths; fixing status may change fallback behavior.
  - Evidence: PRD requires failures to become `ToolOutputStatus::Failure` while preserving human-readable stdout.
  - Mitigation or requested decision: preserve model-facing failure message and add provider tests for fallback semantics.
  - Audit IDs affected: G4, G5, Q3.

- Final-answer guard can loop.
  - Impact: guard may force repeated iterations if the model refuses to fetch evidence or downgrade wording.
  - Evidence: PRD calls out continuation-limit and false-positive risks.
  - Mitigation or requested decision: keep guard disabled/observe-only first; when enabled, produce concrete next actions and preserve continuation limit behavior.
  - Audit IDs affected: G6, Q3.

- Payload bloat from fetched Markdown.
  - Impact: storing full Markdown in `structured_payload` could increase prompt/tool payload size.
  - Evidence: PRD warns about Crawl4AI/WebMarkdown payload bloat.
  - Mitigation or requested decision: cap payload markdown or store compact metadata/artifact refs if bloat is observed.
  - Audit IDs affected: G3, G4, G7.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
