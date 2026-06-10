# Goal: Strict Answer Verifier

Date started: 2026-06-10
Status: active
Codex goal: `/goal Implement docs/goals/2026-06-10-strict-answer-verifier.md until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update this document after each meaningful verification, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.`
Source spec: `docs/prd/plan.md` section `Strict zero-trust LLM verifier update`
Goal doc owner: Codex
Last updated: 2026-06-10 18:32 +03

## Objective

Replace the soft regex/metadata final-answer guard with a strict zero-trust LLM verifier that checks final drafts against bounded fetched evidence documents and refuses delivery unless the verifier returns `allow` or a verified constrained `proof_not_found` report.

Done when every required Completion Audit item is verified by its listed evidence and all out-of-scope constraints are preserved.

## Scope

In scope:
- `crates/oxide-agent-core/src/agent/research/` evidence document capture and audit payload updates.
- `crates/oxide-agent-core/src/agent/runner/` final-response verifier integration.
- `crates/oxide-agent-core/src/llm/client.rs` internal purpose support for answer verification.
- `crates/oxide-agent-core/src/config.rs` verifier configuration.
- Removal or active retirement of the old `FinalAnswerGuardHook` regex/metadata gate.
- Focused runtime/verifier/provider tests and final workspace validation.
- Optional web/agent progress event for visible verifier/audit trace if needed for the audit requirement.

Out of scope:
- New crates, services, queues, databases, storage backends, embedding systems, or generalized evidence graphs.
- Async hook trait migration.
- Query/fetch planner implementation beyond verifier-provided concrete next actions.
- Direct Google Gemini provider code.
- Treating search snippets, sub-agent prose, memory, or reasoning as proof.
- Browser-side search logic; web transport should only display backend events.

## Missing Inputs

- Verifier route selection for production.
  - Impact: strict mode needs a configured verifier model/provider; implementation can add config and tests without choosing the production model.
  - Low-risk assumption: `RESEARCH_VERIFIER_MODEL_ID` and `RESEARCH_VERIFIER_MODEL_PROVIDER` are required when `RESEARCH_VERIFIER_ENABLED=true`.
  - User/external action needed: choose verifier route before production rollout.

## Repository Context

- Relevant entry points:
  - `crates/oxide-agent-core/src/agent/research/mod.rs`
  - `crates/oxide-agent-core/src/agent/hooks/` legacy guard registration surface
  - `crates/oxide-agent-core/src/agent/executor/config.rs`
  - `crates/oxide-agent-core/src/agent/executor/execution.rs`
  - `crates/oxide-agent-core/src/agent/runner/responses.rs`
  - `crates/oxide-agent-core/src/agent/runner/mod.rs`
  - `crates/oxide-agent-core/src/llm/client.rs`
  - `crates/oxide-agent-core/src/config.rs`
  - `crates/oxide-agent-core/src/agent/progress.rs`
  - `crates/oxide-agent-web-contracts/src/events.rs`
- Existing conventions:
  - Keep `oxide-agent-core` transport-agnostic.
  - Use existing `LlmClient::complete_internal_text` for sidecar LLM calls.
  - Preserve final-response draft save / `ForceIteration` mechanics.
  - Keep provider stdout model-readable; typed payloads and evidence docs are the verifier source of truth.
- Dependencies or runtime assumptions:
  - Prior deterministic research runtime goal is complete.
  - `ResearchRuntime` already observes full typed `ToolOutput`.
  - `crawl4ai_markdown` success payload contains fetched Markdown in `structured_payload`.
- Validation infrastructure:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
  - Focused `cargo test -p oxide-agent-core ...` tests for research/runtime/verifier/config modules.
- Risky areas:
  - Final-answer path can block delivery for all research tasks when verifier config is wrong.
  - Verifier prompt/JSON parsing must fail closed without creating infinite loops.
  - Evidence excerpts can grow large; keep bounded by config.

## Completion Audit

- G1: Old regex/metadata guard is removed from the critical path
  - Source: `docs/prd/plan.md:1461` and `docs/prd/plan.md:1481`.
  - Requirement: final delivery must not depend on marker regex, “some primary fetch exists”, snippets, or continuation-limit pass-through.
  - Acceptance: `FinalAnswerGuardHook` is deleted or no longer registered as the active final-answer gate; tests prove unsupported factual drafts are not allowed by marker absence or any-fetch presence.
  - Evidence required: diff review plus focused tests for marker-free unsupported claims and irrelevant fetch evidence.
  - Status: verified
  - Evidence collected: Checkpoint 1 deleted `crates/oxide-agent-core/src/agent/hooks/final_answer_guard.rs`, removed its export from `crates/oxide-agent-core/src/agent/hooks/mod.rs`, removed registration from `crates/oxide-agent-core/src/agent/executor/config.rs`, and removed `RESEARCH_GUARD_ENABLED` config/env docs. Test `executor_does_not_register_legacy_final_answer_guard` passed, proving the old regex/any-fetch gate is not active.

- G2: `ResearchRuntime` stores bounded proof `EvidenceDocument`s
  - Source: `docs/prd/plan.md:1501` and `docs/prd/plan.md:1523`.
  - Requirement: fetched source text must be captured as bounded evidence documents with URL/final URL/source metadata/excerpt/hash/truncation fields.
  - Acceptance: `crawl4ai_markdown` produces proof documents; `searxng_search` remains discovery-only; snippets, sub-agent prose, memory, and reasoning do not become proof.
  - Evidence required: unit tests for Crawl4AI evidence capture, snippet-only exclusion, bounds/hash/truncation handling.
  - Status: verified
  - Evidence collected: `crates/oxide-agent-core/src/agent/research/mod.rs:88` adds `EvidenceDocument`; `crates/oxide-agent-core/src/agent/research/mod.rs:549` records only successful `crawl4ai_markdown` payload Markdown as bounded proof documents with URL/final URL/status/source metadata, excerpt/content SHA-256, char counts, and truncation state; `crates/oxide-agent-core/src/agent/research/mod.rs:12` caps excerpts at 12,000 chars. Tests `records_crawl4ai_evidence_document_with_hash_and_bounds` and `search_snippets_and_fallback_fetches_do_not_become_proof_documents` passed.

- G3: Strict verifier config is explicit and fail-closed
  - Source: `docs/prd/plan.md:1533` and `docs/prd/plan.md:1547`.
  - Requirement: add `RESEARCH_VERIFIER_*` config; enabled verifier requires explicit model/provider and never silently falls back to the main agent route.
  - Acceptance: missing verifier route, provider error, timeout, and invalid JSON block/fail instead of delivering a draft.
  - Evidence required: config tests and runner/verifier tests for fail-closed cases.
  - Status: pending
  - Evidence collected:

- G4: LLM answer verifier sidecar validates strict JSON verdicts
  - Source: `docs/prd/plan.md:1554` and `docs/prd/plan.md:1564`.
  - Requirement: add `InternalTextPurpose::AnswerVerification` and a verifier module that calls the configured LLM route, parses strict JSON, and returns typed verdicts.
  - Acceptance: verdicts `allow`, `revise`, `need_more_evidence`, `proof_not_found`, and `block` are parsed; malformed output fails closed.
  - Evidence required: verifier unit tests with mocked `complete_internal_text` responses.
  - Status: pending
  - Evidence collected:

- G5: Final response path gates delivery through verifier
  - Source: `docs/prd/plan.md:1558` and `docs/prd/plan.md:1650`.
  - Requirement: `handle_final_response` must run strict verification before saving/delivering final responses.
  - Acceptance: `allow` delivers; `revise`/`need_more_evidence` save an undelivered draft and force iteration with exact claims/actions; `block`/errors do not deliver.
  - Evidence required: runner tests for each verdict and draft/continuation behavior.
  - Status: pending
  - Evidence collected:

- G6: Proof exhaustion produces verified no-proof report, not hallucinated final
  - Source: `docs/prd/plan.md:1610` and `docs/prd/plan.md:1622`.
  - Requirement: after `RESEARCH_VERIFIER_MAX_ROUNDS` (default 10), the original unsupported draft must not pass; the agent gets one constrained proof-not-found report opportunity that is also verified.
  - Acceptance: exhausted `need_more_evidence` forces no-proof report; verified `proof_not_found` report can deliver; no-proof report with unsupported recommendations blocks.
  - Evidence required: runner/verifier tests for exhausted rounds and proof-not-found delivery/block cases.
  - Status: pending
  - Evidence collected:

- G7: Verifier trace is visible enough for audit/debug
  - Source: `docs/prd/plan.md:1664` and user requirement to see what happens after missing proofs.
  - Requirement: expose verifier verdict, unsupported claims count/list, evidence document count, and next actions through audit payload and/or progress event.
  - Acceptance: tests or contract snapshots show structured verifier decisions are available to transports/logs without adding storage backends.
  - Evidence required: unit/contract test for audit/event payload; web contract update if an event is added.
  - Status: pending
  - Evidence collected:

- Q1: Preserve architecture and simplicity
  - Source: `AGENTS.md` architecture/over-engineering rules and `docs/prd/plan.md:1493`.
  - Acceptance: no new crates/services/storage layers; no async hook migration; no direct Gemini provider; core remains transport-agnostic.
  - Evidence required: diff review plus workspace checks.
  - Status: in_progress
  - Evidence collected: Checkpoint 1 removed the old regex/metadata hook without adding crates, services, storage, async hook migration, or transport coupling. Full final workspace validation remains for checkpoint 6.

- Q2: Preserve tool-call and final-response invariants
  - Source: `AGENTS.md` runner/tool-call invariants.
  - Acceptance: existing `ForceIteration`, undelivered draft, `Finish`/`Block`, and tool-call pairing behavior still pass tests.
  - Evidence required: focused lifecycle tests plus final workspace check.
  - Status: pending
  - Evidence collected:

- Q3: Evidence is bounded and prompt-cache aware
  - Source: `docs/prd/plan.md:1541` and `AGENTS.md` prompt-cache invariants.
  - Acceptance: evidence excerpts are capped; no large volatile verifier blocks are added to the stable system prompt prefix.
  - Evidence required: tests for excerpt limits and diff review of prompt assembly.
  - Status: in_progress
  - Evidence collected: Checkpoint 1 added bounded evidence capture in `ResearchRuntime`; tests prove the 12,000-character excerpt cap and no verifier blocks were added to prompt assembly or stable system prompt prefix.

- N1: No fallback trust path
  - Source: `docs/prd/plan.md:1481`.
  - Must preserve: no fallback to old regex, snippets, any-fetch, continuation-limit pass-through, or same-model verifier unless explicitly configured as verifier route.
  - Evidence required: tests for fail-closed/missing config and marker-free unsupported claims.
  - Status: in_progress
  - Evidence collected: Checkpoint 1 deleted the old regex/metadata guard and removed `RESEARCH_GUARD_ENABLED`; tests prove search snippets plus fallback fetches do not become proof documents. Missing verifier-route fail-closed tests remain for checkpoint 2.

- N2: No premature planner/evidence graph
  - Source: `docs/prd/plan.md:1529` and `docs/prd/plan.md:1664`.
  - Must preserve: verifier supplies next actions, but this goal does not build a query planner, fetch planner, embeddings, or full evidence graph.
  - Evidence required: diff review.
  - Status: in_progress
  - Evidence collected: Checkpoint 1 added only passive evidence documents and legacy guard retirement; no planner, embeddings, or evidence graph were introduced.

- V1: Formatting/lint validation
  - Source: `AGENTS.md` validation rules.
  - Evidence required: `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings` before final completion.
  - Status: in_progress
  - Evidence collected: `cargo fmt --all -- --check` passed for checkpoint 1. Full workspace clippy is reserved for final completion.

- V2: Build and focused test validation
  - Source: `AGENTS.md` build/test rules.
  - Evidence required: `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`; focused `cargo test -p oxide-agent-core` filters for verifier/research/runtime/config modules.
  - Status: in_progress
  - Evidence collected: Checkpoint 1 focused tests passed: `cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local --lib research::`; `cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local --lib executor_does_not_register_legacy_final_answer_guard`; `cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local --lib prepare_execution_uses_executor_model_routes_override`. `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` passed.

## Implementation Plan

### Checkpoint 1: Evidence documents and old guard retirement
- Audit IDs: G1, G2, Q1, Q3, N1, V2
- Expected changes:
  - Add bounded `EvidenceDocument` capture to `ResearchRuntime`.
  - Capture Crawl4AI fetched Markdown excerpts/hash/source metadata.
  - Keep search snippets as leads only.
  - Remove or unregister the old regex/metadata `FinalAnswerGuardHook` from active final-answer gating.
- Validation:
  - Focused `ResearchRuntime` evidence tests.
  - Focused guard-retirement/config tests.
- Exit condition: proof documents exist for fetched source text and no active final gate can pass a draft via regex/any-fetch logic.

### Checkpoint 2: Strict verifier sidecar and config
- Audit IDs: G3, G4, Q1, N1, V2
- Expected changes:
  - Add `RESEARCH_VERIFIER_*` config and explicit route validation.
  - Add `InternalTextPurpose::AnswerVerification`.
  - Add typed verifier request/response parsing with strict JSON verdicts.
  - Fail closed on missing route, provider failure, timeout, or invalid JSON.
- Validation:
  - Mocked LLM verifier tests for all verdicts and failure cases.
  - Config tests for required model/provider behavior.
- Exit condition: verifier module can make a sidecar LLM call and produce typed verdicts without any fallback trust path.

### Checkpoint 3: Final-response integration
- Audit IDs: G5, Q2, N1, V2
- Expected changes:
  - Run verifier in `handle_final_response` before save/delivery.
  - Convert `revise` / `need_more_evidence` to `ForceIteration` with exact unsupported claims and next actions.
  - Preserve undelivered draft behavior.
  - Convert `block` / verifier errors to no-delivery failure.
- Validation:
  - Runner tests for `allow`, `revise`, `need_more_evidence`, `block`, invalid JSON/error.
  - Existing lifecycle tests for `AfterAgent` / drafts still pass.
- Exit condition: no final answer is delivered without verifier approval or verified proof-not-found report.

### Checkpoint 4: Proof-not-found exhaustion flow
- Audit IDs: G6, Q2, N1, V2
- Expected changes:
  - Track verifier rounds per run.
  - At max rounds, force one constrained proof-not-found report instruction.
  - Deliver only if verifier returns `proof_not_found` or `allow` for that constrained report.
  - Block if the constrained report still contains unsupported recommendations/claims.
- Validation:
  - Tests for 10-round exhaustion, no-proof report delivery, and no-proof report block.
- Exit condition: missing proofs produce a transparent no-proof report or no final answer, never the unsupported original draft.

### Checkpoint 5: Visible verifier audit trace
- Audit IDs: G7, Q1, Q3, V2
- Expected changes:
  - Extend research audit payload and/or add progress event with verifier verdict, evidence doc count, unsupported claims, and next actions.
  - Keep transport storage unchanged; web may display existing persisted event payloads if event is added.
- Validation:
  - Audit/event payload tests and web contract test if applicable.
- Exit condition: operator/user can see why a final draft was rejected or why no proofs were found.

### Checkpoint 6: Completion audit and rollout readiness
- Audit IDs: all
- Expected changes:
  - Run full Completion Audit.
  - Fill final verification only when every item has current evidence.
  - Update `.env.example` / docs for verifier config after behavior is tested.
- Validation:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
  - Focused runtime/research/verifier/config tests.
- Exit condition: goal is complete, working tree is clean, and strict verifier rollout evidence is recorded.

## Validation Contract

- Static checks:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
- Build checks:
  - `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
- Tests:
  - Focused `cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local --lib <verifier/research/runtime/config filters>`.
  - Web contract tests if a new event kind is introduced.
- Runtime/manual verification:
  - After implementation, run a Heavy HuggingFace-style research scenario and verify unsupported claims are rejected or converted to a proof-not-found report.
- Done when:
  - All Completion Audit items are `verified`; final validation commands pass; final behavior is fail-closed for verifier failures and missing proof.

## Decisions

- 2026-06-10: Use strict LLM verifier instead of improving regex guard. Reason: marker heuristics cannot prove factual claims and can pass marker-free hallucinations.
- 2026-06-10: No fallback trust path. Reason: user explicitly prefers spending more tokens over delivering unsupported claims.
- 2026-06-10: Exhausted proof search must produce a verified `proof_not_found` report or no final answer. Reason: transparent uncertainty is acceptable; hallucinated certainty is not.
- 2026-06-10: Do not make hooks async. Reason: `handle_final_response` is already async and has the required LLM/context access with a smaller change surface.
- 2026-06-10: Delete the legacy `FinalAnswerGuardHook` instead of leaving it disabled behind env. Reason: the strict verifier goal must have no regex/any-fetch fallback trust path.

## Progress Log

- 2026-06-10 18:10 +03: Goal contract created
  - Changed: added this goal doc from `docs/prd/plan.md` strict verifier section.
  - Evidence: source spec lines `docs/prd/plan.md:1459-1678` inspected.
  - Commands: pending commit validation.
  - Audit IDs updated: all initialized as pending.
  - Next: Checkpoint 1 — evidence documents and old guard retirement.

- 2026-06-10 18:32 +03: Checkpoint 1 — evidence documents and old guard retirement
  - Changed: added bounded `EvidenceDocument` capture to `ResearchRuntime`, records successful `crawl4ai_markdown` Markdown payloads as proof documents, keeps search snippets/fallback fetches out of proof docs, removed legacy `FinalAnswerGuardHook` code/registration/config/env docs, and provisions `ResearchRuntime` for prepared executions.
  - Evidence: `crates/oxide-agent-core/src/agent/research/mod.rs:88`, `crates/oxide-agent-core/src/agent/research/mod.rs:549`, `crates/oxide-agent-core/src/agent/executor/config.rs:42`, `crates/oxide-agent-core/src/agent/executor/execution.rs:569`, `crates/oxide-agent-core/src/agent/executor/tests/basics.rs:93`.
  - Commands: `cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local --lib research::`; `cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local --lib executor_does_not_register_legacy_final_answer_guard`; `cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local --lib prepare_execution_uses_executor_model_routes_override`; `cargo fmt --all -- --check`; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`.
  - Audit IDs updated: G1 verified, G2 verified, Q1/Q3/N1/V1/V2 in progress.
  - Next: Checkpoint 2 — strict verifier sidecar and config.

## Risks and Blockers

- Missing production verifier route
  - Impact: strict verifier cannot run in production until `RESEARCH_VERIFIER_MODEL_ID` and `RESEARCH_VERIFIER_MODEL_PROVIDER` are chosen.
  - Evidence: source spec requires explicit verifier route.
  - Mitigation or requested decision: implement config/tests now; choose route before rollout.
  - Audit IDs affected: G3, V2.

- False blocking due weak evidence excerpts
  - Impact: verifier may ask for more evidence even when full source contains support outside the excerpt.
  - Evidence: evidence is intentionally bounded.
  - Mitigation or requested decision: make excerpt caps configurable and include source hashes/URLs for debugging.
  - Audit IDs affected: G2, G4, G6.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
