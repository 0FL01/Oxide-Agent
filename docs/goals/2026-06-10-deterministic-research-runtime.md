# Goal: Deterministic Research Runtime

Date started: 2026-06-10
Status: active
Codex goal: `/goal Implement docs/goals/2026-06-10-deterministic-research-runtime.md until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update this document after each meaningful verification, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.`
Source spec: `docs/prd/plan.md`
Goal doc owner: Codex
Last updated: 2026-06-10 17:50 +03

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

- None currently. Checkpoint 5 fixed `RESEARCH_AUDIT_ENABLED` as the audit/debug output flag; `RESEARCH_GUARD_ENABLED` remains the final-answer guard flag.

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
  - Status: verified
  - Evidence collected: `crates/oxide-agent-core/src/agent/providers/searxng/format.rs:9` now returns Markdown plus structured search payload with provider/kind/query/ranked results/fetched_at; `crates/oxide-agent-core/src/agent/providers/searxng/error.rs:46` builds structured failure payloads; `crates/oxide-agent-core/src/agent/providers/searxng/provider.rs:153` and `crates/oxide-agent-core/src/agent/providers/searxng/provider.rs:169` attach success/failure payloads and mark provider failures as `ToolOutputStatus::Failure`. `crates/oxide-agent-core/src/agent/providers/crawl4ai_markdown/crawl.rs:222` returns model-readable Markdown stdout plus structured fetch payload with `kind: "fetch"`, URL/final URL/status/markdown/truncation/freshness metadata; `crates/oxide-agent-core/src/agent/providers/crawl4ai_markdown/executor.rs:63` attaches it to `ToolOutput.structured_payload`. Tests `formats_results_with_ranked_structured_payload`, `empty_query_returns_structured_failure_status`, `typed_runtime_executor_posts_expected_crawl_contract`, `health_unavailable_returns_structured_failure`, and `reddit_rss_fallback_output_respects_max_chars` passed.

- G5: Fallback provider normalization does not drive the architecture
  - Source: `docs/prd/plan.md` lines 631-674 and 1456.
  - Requirement: normalize `web_markdown`, Tavily, Brave, and DuckDuckGo only for compatibility/fallback after the primary stack is working.
  - Acceptance: fallback payloads are canonical enough for UI/audit/ledger, but no core research policy depends on them as mandatory providers.
  - Evidence required: provider tests or explicit deferred decision recorded if not implemented in the first milestone.
  - Status: verified
  - Evidence collected: `crates/oxide-agent-core/src/agent/providers/tavily.rs:258` and `crates/oxide-agent-core/src/agent/providers/tavily.rs:327` add Tavily search/fetch success and failure payloads with status-correct structured failures; `crates/oxide-agent-core/src/agent/providers/webfetch_md/fetch.rs:29` and `crates/oxide-agent-core/src/agent/providers/webfetch_md/mod.rs:160` attach WebMarkdown success fetch payloads; `crates/oxide-agent-core/src/agent/providers/brave_search/format.rs:11` and `crates/oxide-agent-core/src/agent/providers/duckduckgo/format.rs:11` add fallback search audit fields; `crates/oxide-agent-core/src/agent/providers/duckduckgo/provider.rs:264` covers failure retry/fallback hints. Tests `web_search_empty_query_returns_structured_failure_without_network`, `web_extract_empty_urls_returns_structured_failure_without_network`, `typed_runtime_executor_fetches_web_markdown`, `formats_success_markdown_and_structured_payload`, `formats_search_results_and_structured_payload`, and `failure_payload_includes_retryable_and_fallback_hints` passed. Core guard policy still only accepts primary fetched evidence for high-impact claims.

- G6: Final answer guard is soft, evidence-aware, and config-gated
  - Source: `docs/prd/plan.md` lines 736-815, 1405-1409, 1447-1457.
  - Requirement: implement `FinalAnswerGuardHook` behind config; it must use `AfterAgent -> ForceIteration` for unsupported high-impact claims and be disableable through env.
  - Acceptance: conceptual stable answers pass; volatile/current/price/version/legal/high-impact claims without adequate fetched evidence force continuation while the default-on guard is enabled; `RESEARCH_GUARD_ENABLED=false` disables registration.
  - Evidence required: focused hook tests for allow/block/caveat decisions, config-default test, and review of hook registration.
  - Status: verified
  - Evidence collected: `crates/oxide-agent-core/src/agent/hooks/final_answer_guard.rs:59` adds `FinalAnswerGuardHook`; `crates/oxide-agent-core/src/agent/hooks/final_answer_guard.rs:164` detects deterministic current/pricing/version/legal/status markers; `crates/oxide-agent-core/src/agent/hooks/final_answer_guard.rs:179` requires successful primary fetched non-snippet evidence; `crates/oxide-agent-core/src/agent/executor/config.rs:55` registers the hook by default; `crates/oxide-agent-core/src/config.rs:2284` exposes default-on `RESEARCH_GUARD_ENABLED`; `crates/oxide-agent-core/src/agent/executor/execution.rs:570` supplies passive `ResearchRuntime` when guard is enabled. Tests `conceptual_final_answer_passes`, `conceptual_supported_wording_without_freshness_passes`, `unsupported_high_impact_claim_forces_iteration`, `fetched_primary_evidence_allows_high_impact_claim`, `snippet_only_search_evidence_is_not_sufficient`, `research_guard_is_enabled_by_default_and_env_disables_it`, `executor_registers_final_answer_guard_by_default`, and `executor_skips_final_answer_guard_when_env_disables_it` passed.

- G7: Research audit artifacts are available without becoming policy
  - Source: `docs/prd/plan.md` lines 816-882.
  - Requirement: emit optional JSON/Markdown audit artifacts or equivalent structured debug state when enabled.
  - Acceptance: audit includes task kind/mode, providers used, queries, fetched URLs, evidence observations, unsupported claims, and final guard decision when available.
  - Evidence required: unit test or fixture output plus documentation of artifact location/config.
  - Status: verified
  - Evidence collected: `crates/oxide-agent-core/src/agent/research/mod.rs:108` adds `ResearchGuardDecision`; `crates/oxide-agent-core/src/agent/research/mod.rs:262` exposes `ResearchRuntime::audit_payload()` with task kind/mode, providers, queries, fetched URLs, observations, failures, unsupported claims, and final guard decision; `crates/oxide-agent-core/src/agent/hooks/final_answer_guard.rs:152` records guard decisions when audit output is enabled; `crates/oxide-agent-core/src/config.rs:2315` adds default-on `RESEARCH_AUDIT_ENABLED`; `.env.example:150` documents env disable. Tests `audit_payload_summarizes_research_state_and_guard_decision`, `unsupported_high_impact_claim_forces_iteration`, and `research_audit_is_enabled_by_default_and_env_disables_it` passed.

- Q1: Preserve project architecture and simplicity
  - Source: `AGENTS.md` lines 11-23 and 50-64.
  - Acceptance: no new crates/services/storage layers; core/runtime remain transport-agnostic; Gemini direct provider remains absent.
  - Evidence required: `git diff` review and `cargo check`.
  - Status: in_progress
  - Evidence collected: Checkpoint 1 added no new crates/services/storage layers; lifecycle changes stayed in `oxide-agent-core`; Checkpoint 2 added only in-process passive state in `oxide-agent-core`; Checkpoint 5 kept audit output as in-memory JSON state and fallback provider normalization inside existing provider modules. `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` passed.

- Q2: Preserve prompt-cache and tool-call invariants
  - Source: `AGENTS.md` lines 68-72 and 86-94.
  - Acceptance: tool calls still run in parallel; history repair and `tool_call_id` matching remain intact; no large volatile blocks are added to the stable prompt prefix.
  - Evidence required: focused tests for tool history plus diff review of prompt changes.
  - Status: in_progress
  - Evidence collected: Tool calls still execute through the existing parallel typed runtime; blocked calls are emitted as ordered pairable `ToolOutput` values; focused tests verify tool-call IDs and provider tool-call IDs are preserved for blocked policy paths. Checkpoint 2 records `ToolOutput` passively before `AfterTool` without changing tool-call content or prompt assembly.

- Q3: Rollout is backward-compatible
  - Source: `docs/prd/plan.md` lines 1290-1317 and 1405-1409.
  - Acceptance: passive runtime and guard can be disabled; default-on guard stays soft for conceptual answers and respects continuation limits; provider stdout remains model-readable after payload changes.
  - Evidence required: config-default tests and provider output tests.
  - Status: in_progress
  - Evidence collected: Checkpoint 2 kept passive research optional until supplied by execution context. Checkpoint 3 preserves model-readable provider stdout: SearXNG still returns Markdown summaries, and Crawl4AI returns fetched Markdown as stdout while moving the typed JSON contract into `structured_payload`. Checkpoint 4 makes the guard default-on per user direction, keeps conceptual answers as `Continue`, respects continuation limits, and supports env disable with `RESEARCH_GUARD_ENABLED=false`. Checkpoint 5 preserves model-readable fallback stdout while adding typed payloads, and audit/debug state can be disabled with `RESEARCH_AUDIT_ENABLED=false`.

- N1: No premature full research planner or full evidence graph
  - Source: `docs/prd/plan.md` lines 1399-1421.
  - Must preserve: first milestone stops at typed boundary, passive ledger, provider payloads, and soft guard; query/fetch planners and full claim/evidence linking are later work.
  - Evidence required: `git diff` review and updated Decisions if scope changes.
  - Status: in_progress
  - Evidence collected: Checkpoint 2 added passive observation types and no query planner, fetch planner, claim/evidence graph, or final-answer guard. Checkpoint 4 added only a deterministic marker heuristic and coarse fetched-evidence check. Checkpoint 5 added compact audit/debug state and fallback compatibility payloads, not a query planner, fetch planner, or full claim/evidence linker.

- N2: Fetch tools are not reduced to a flat hard search counter
  - Source: `docs/prd/plan.md` lines 676-735 and 1429-1433.
  - Must preserve: `crawl4ai_markdown` / `web_markdown` control should use dedupe, failed-host quarantine, anti-bot quarantine, truncation, and progress guards rather than a simple global search counter.
  - Evidence required: policy design diff and tests when fetch loop guards are implemented.
  - Status: in_progress
  - Evidence collected: `cargo fmt --all -- --check` passed for Checkpoint 2; full final clippy remains pending for rollout readiness.

- V1: Formatting and lint validation
  - Source: `AGENTS.md` lines 145-153.
  - Evidence required: `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings` before final completion, or documented narrower checkpoint validation when appropriate.
  - Status: in_progress
  - Evidence collected: `cargo fmt --all -- --check` passed for Checkpoint 5; focused `cargo clippy -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local --lib -- -D warnings` passed. Full final clippy remains pending for rollout readiness.

- V2: Build and focused test validation
  - Source: `AGENTS.md` lines 132-153.
  - Evidence required: `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`; focused `cargo test -p oxide-agent-core` filters for touched runtime/provider modules.
  - Status: in_progress
  - Evidence collected: Checkpoint 1 focused tests and embedded profile check passed on 2026-06-10. Checkpoint 2 tests `research::` and `typed_runtime_records_research_output_before_after_tool_hooks` passed, and `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` passed. Checkpoint 3 tests `cargo test -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local --lib -- searxng` and `cargo test -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local --lib -- crawl4ai` passed, plus `cargo check -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local` and embedded workspace check. Checkpoint 4 focused `final_answer_guard`, config default, and executor registration tests passed, plus embedded workspace check. Checkpoint 5 focused fallback/audit tests passed for Tavily, WebMarkdown, Brave, DuckDuckGo, audit payload, audit config, and guard decision recording; `cargo check -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local` and embedded workspace check passed. Full final validation remains pending for Checkpoint 6.

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
  - Register guard by default and allow env disable with `RESEARCH_GUARD_ENABLED=false`.
- Validation:
  - Hook tests: conceptual pass, unsupported high-impact claim blocks, fetched non-snippet evidence allows, snippet-only evidence does not allow high-impact claim.
  - Config-default test proving guard is default-on, env-disableable, and not strict for conceptual answers.
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
- 2026-06-10: User changed rollout direction for Checkpoint 4: `FinalAnswerGuardHook` is registered by default and can be disabled with `RESEARCH_GUARD_ENABLED=false`; runtime policy, not prompt guidance, remains the acceptance boundary.
- 2026-06-10: Checkpoint 2 initially kept passive research runtime disabled by default (`PreparedExecution.research_runtime = None`) and only recorded observations when a runtime was explicitly supplied; Checkpoint 4 now supplies it for default-on guard evaluation.
- 2026-06-10: Checkpoint 3 keeps provider stdout model-readable while making typed payloads authoritative: SearXNG stdout remains Markdown discovery text, and Crawl4AI stdout becomes fetched Markdown instead of JSON.
- 2026-06-10: Checkpoint 5 uses `RESEARCH_AUDIT_ENABLED` for structured in-memory research audit/debug output. It intentionally does not add storage, file artifacts, transport events, or policy dependencies on fallback providers.

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

- 2026-06-10 17:15 +03: Checkpoint 3 primary provider payload contract implemented.
  - Changed: added SearXNG success/failure `structured_payload` values with correct failure status, moved Crawl4AI success JSON into `structured_payload`, and kept model-facing stdout readable as Markdown.
  - Evidence: `crates/oxide-agent-core/src/agent/providers/searxng/format.rs:9`, `crates/oxide-agent-core/src/agent/providers/searxng/error.rs:46`, `crates/oxide-agent-core/src/agent/providers/searxng/provider.rs:153`, `crates/oxide-agent-core/src/agent/providers/crawl4ai_markdown/crawl.rs:222`, `crates/oxide-agent-core/src/agent/providers/crawl4ai_markdown/executor.rs:63`.
  - Commands: `cargo test -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local --lib -- searxng`; `cargo test -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local --lib -- crawl4ai`; `cargo fmt --all -- --check`; `cargo check -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local`; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`.
  - Audit IDs updated: G4 verified; Q3/V2 in progress with checkpoint evidence.
  - Next: Checkpoint 4 -- soft final answer guard.

- 2026-06-10 17:34 +03: Checkpoint 4 soft final answer guard implemented.
  - Changed: added `FinalAnswerGuardHook`, deterministic high-impact/current claim marker detection, fetched primary evidence allowance, snippet-only rejection, default-on env config, executor registration, and passive runtime provisioning while the guard is enabled.
  - Evidence: `crates/oxide-agent-core/src/agent/hooks/final_answer_guard.rs:52`, `crates/oxide-agent-core/src/agent/hooks/final_answer_guard.rs:104`, `crates/oxide-agent-core/src/agent/hooks/final_answer_guard.rs:119`, `crates/oxide-agent-core/src/agent/executor/config.rs:54`, `crates/oxide-agent-core/src/agent/executor/execution.rs:569`, `crates/oxide-agent-core/src/config.rs:2265`, `.env.example:147`.
  - Commands: `cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local --lib final_answer_guard`; `cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local --lib research_guard_is_enabled_by_default_and_env_disables_it`; `cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local --lib executor_registers_final_answer_guard`; `cargo fmt --all -- --check`; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`.
  - Audit IDs updated: G6 verified; Q3/N1/V1/V2 in progress with checkpoint evidence.
  - Next: Checkpoint 5 -- fallback normalization and audit artifact.

- 2026-06-10 17:50 +03: Checkpoint 5 fallback normalization and audit/debug output implemented.
  - Changed: normalized Tavily/WebMarkdown fallback payloads, added audit fields for Brave/DuckDuckGo fallback payloads, added structured in-memory research audit payloads, recorded final guard decisions behind `RESEARCH_AUDIT_ENABLED`, and documented the env flag.
  - Evidence: `crates/oxide-agent-core/src/agent/providers/tavily.rs:258`, `crates/oxide-agent-core/src/agent/providers/webfetch_md/fetch.rs:29`, `crates/oxide-agent-core/src/agent/providers/brave_search/format.rs:11`, `crates/oxide-agent-core/src/agent/providers/duckduckgo/provider.rs:264`, `crates/oxide-agent-core/src/agent/research/mod.rs:262`, `crates/oxide-agent-core/src/agent/hooks/final_answer_guard.rs:152`, `crates/oxide-agent-core/src/config.rs:2315`.
  - Commands: `cargo test -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local --lib -- tavily`; `cargo test -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local --lib -- webfetch_md`; `cargo test -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local --lib -- brave_search`; `cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local --lib failure_payload_includes_retryable_and_fallback_hints`; `cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local --lib audit_payload_summarizes_research_state_and_guard_decision`; `cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local --lib research_audit_is_enabled_by_default_and_env_disables_it`; `cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local --lib unsupported_high_impact_claim_forces_iteration`; `cargo clippy -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local --lib -- -D warnings`; `cargo fmt --all -- --check`; `cargo check -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local`; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`.
  - Audit IDs updated: G5 verified, G7 verified; Q1/Q3/V1/V2 in progress with checkpoint evidence.
  - Next: Checkpoint 6 -- completion audit and rollout readiness.

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
  - Mitigation or requested decision: guard is default-on per user direction but remains env-disableable, allows conceptual answers, emits concrete next actions, and preserves continuation-limit escape behavior.
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
