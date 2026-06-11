# Goal: Search Probe Effort Controls

Date started: 2026-06-12
Status: active
Codex goal: `/goal Implement docs/goals/2026-06-12-search-probe-effort-controls.md until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update this document after each meaningful verification, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.`
Source spec: user-approved plan in chat on 2026-06-12
Goal doc owner: Codex
Last updated: 2026-06-12 00:00 +03

## Objective

Make Search Probe cheaper and more predictable by default: keep the main agent's model-level reasoning behavior unchanged, while allowing Search Probe and its forced finalizer to use independent model reasoning effort, initially hardcoded to `medium`, and defaulting Search Probe runtime minimum effort to `standard`.

Done when every required Completion Audit item is verified by its listed evidence, the main agent still receives `high` model reasoning for `Extended`/`Heavy` execution effort, and Search Probe plus forced finalizer use `medium` model reasoning without deterministic research logic.

## Scope

In scope:
- `crates/oxide-agent-core/src/agent/executor.rs` for per-run reasoning effort override on `AgentExecutionOptions`.
- `crates/oxide-agent-core/src/agent/executor/execution.rs` and runner config propagation only as needed to preserve existing behavior.
- `crates/oxide-agent-transport-web/src/server/search_probe.rs` for Search Probe defaults and probe/finalizer execution options.
- Focused unit tests for option behavior and Search Probe execution options.
- This goal document.

Out of scope:
- Deterministic query planning, regex/entity extraction, exact/near-miss scoring, or Rust-owned research heuristics.
- Typed runtime search-budget enforcement changes.
- Hard-timeout forced-finalize behavior changes.
- Provider-wide unsupported-parameter fallback unless required by tests for this checkpoint.
- New crates, services, queues, storage tables, or transport-wide rewrites.
- Changing the main agent's existing effort mapping except to preserve it behind the new override.

## Missing Inputs

- None. User approved starting with `standard` Search Probe runtime minimum effort and `medium` model reasoning for probe/finalizer.

## Repository Context

- Current Search Probe config default still uses `Heavy` in `SearchProbeConfig::from_env()` and `Default`.
- Search Probe runtime options are built in `probe_execution_options(...)` and `forced_finalize_execution_options(...)`.
- Current model reasoning effort is derived from execution effort in `AgentExecutionOptions::reasoning_effort()`:
  - `Standard -> None`
  - `Extended`/`Heavy -> Some("high")`
- Runner config already carries `reasoning_effort` as `Option<String>` and providers receive it via LLM calls.
- `opencode-go` only sends `reasoning_effort` for known reasoning models and supports disabling with `none`/`disabled`.
- Validation conventions from `AGENTS.md`: use focused `cargo check`, final `cargo fmt --all -- --check`, and `cargo clippy --workspace --all-targets --features profile-full -- -D warnings` for completion.

## Completion Audit

- G1: Search Probe runtime minimum effort defaults to standard
  - Source: user request, approved plan.
  - Requirement: Default Search Probe `min_effort` must be `WebAgentEffort::Standard`, while env override `OXIDE_SEARCH_PROBE_MIN_EFFORT` still works.
  - Acceptance: `SearchProbeConfig::from_env()` fallback and `Default` use `Standard`; tests assert the default.
  - Evidence required: implementation diff and focused Search Probe config test.
  - Status: pending
  - Evidence collected:

- G2: Per-run model reasoning override exists independently from execution effort
  - Source: user requirement that main agent can remain high while probe/finalizer use medium.
  - Requirement: `AgentExecutionOptions` must support an explicit model reasoning effort override without changing existing effort-derived behavior when unset.
  - Acceptance: unset override preserves current mapping; explicit override returns the override for runner config.
  - Evidence required: core unit test or focused existing test update proving both paths.
  - Status: pending
  - Evidence collected:

- G3: Search Probe generation uses model reasoning effort medium
  - Source: user request: hardcode model effort to `medium` for Search Probe stage.
  - Requirement: `probe_execution_options(...)` must set explicit model reasoning effort `medium` independent of parent/main effort.
  - Acceptance: even when parent effort is `Heavy`, probe execution options resolve model reasoning effort to `Some("medium")`.
  - Evidence required: focused Search Probe options test.
  - Status: pending
  - Evidence collected:

- G4: Forced finalizer uses model reasoning effort medium
  - Source: user request and approved plan.
  - Requirement: `forced_finalize_execution_options(...)` must set explicit model reasoning effort `medium`.
  - Acceptance: forced finalizer execution options resolve model reasoning effort to `Some("medium")`.
  - Evidence required: focused Search Probe forced-finalize options test.
  - Status: pending
  - Evidence collected:

- Q1: Main agent model reasoning behavior is preserved
  - Source: architectural constraint and user question.
  - Acceptance: normal `AgentExecutionOptions::with_effort(Extended/Heavy)` still maps to `Some("high")` when no override is set; `Standard` remains `None` when no override is set.
  - Evidence required: core test and diff review showing only override path changes behavior.
  - Status: pending
  - Evidence collected:

- Q2: Simple maintainable implementation
  - Source: repository `AGENTS.md` over-engineering constraints.
  - Acceptance: no new crates, no provider abstractions, no broad fallback framework, no storage/schema changes.
  - Evidence required: `git diff --name-only` review.
  - Status: pending
  - Evidence collected:

- N1: No deterministic research logic
  - Source: explicit user constraint: regex/deterministic are forbidden.
  - Must preserve: no regex/entity/query planning/scoring logic added for Search Probe research behavior.
  - Evidence required: diff review.
  - Status: pending
  - Evidence collected:

- V1: Focused validation passes
  - Source: repository validation conventions.
  - Evidence required: `cargo test -p oxide-agent-core <focused-test> --lib` if core tests are added, `cargo test -p oxide-agent-transport-web search_probe --lib`, and `cargo check -p oxide-agent-transport-web`.
  - Status: pending
  - Evidence collected:

- V2: Final quality gates pass or blockers are documented
  - Source: repository `AGENTS.md`.
  - Evidence required: `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --features profile-full -- -D warnings`.
  - Status: pending
  - Evidence collected:

## Implementation Plan

### Checkpoint 1: Goal contract and baseline review
- Audit IDs: Q2, N1.
- Expected changes:
  - Create this goal document with scoped requirements, checkpoints, validation, and non-goals.
  - Review current Search Probe and reasoning-effort code paths.
- Validation:
  - `git status --short`
  - `git diff -- docs/goals/2026-06-12-search-probe-effort-controls.md`
- Exit condition: goal doc is committed as its own checkpoint and the first implementation step is ready for user review.

### Checkpoint 2: Core execution options reasoning override
- Audit IDs: G2, Q1, Q2, N1, V1.
- Expected changes:
  - Add a small explicit reasoning-effort override field to `AgentExecutionOptions`.
  - Add a builder such as `with_reasoning_effort("medium")`.
  - Preserve existing unset behavior exactly.
  - Add focused tests for unset and overridden behavior.
- Validation:
  - Focused core test for `AgentExecutionOptions` reasoning effort behavior.
  - `cargo check -p oxide-agent-core`
- Exit condition: core can represent per-run model reasoning independent of runtime effort without changing main-agent defaults.

### Checkpoint 3: Search Probe uses standard runtime default and medium model reasoning
- Audit IDs: G1, G3, G4, Q1, Q2, N1, V1.
- Expected changes:
  - Change Search Probe `min_effort` default from `Heavy` to `Standard`.
  - Apply `.with_reasoning_effort("medium")` to probe generation options.
  - Apply `.with_reasoning_effort("medium")` to forced-finalize options.
  - Update/add focused Search Probe tests.
- Validation:
  - `cargo test -p oxide-agent-transport-web search_probe --lib`
  - `cargo check -p oxide-agent-transport-web`
- Exit condition: Search Probe/finalizer use model reasoning `medium`; main agent effort behavior remains unchanged.

### Checkpoint 4: Final audit and quality gates
- Audit IDs: all G*, Q*, N*, V*.
- Expected changes:
  - Update this goal document with evidence and decisions.
  - Run formatting and clippy gates.
  - Commit final implementation checkpoint.
- Validation:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets --features profile-full -- -D warnings`
  - `git diff --name-only` non-goal audit.
- Exit condition: every Completion Audit item is verified or a precise blocker is documented.

## Validation Contract

- Static checks:
  - `cargo check -p oxide-agent-core`
  - `cargo check -p oxide-agent-transport-web`
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets --features profile-full -- -D warnings`
- Tests:
  - Focused core tests for `AgentExecutionOptions` reasoning override.
  - `cargo test -p oxide-agent-transport-web search_probe --lib`
- Runtime/manual verification:
  - Optional after implementation: run with current `.env` Search Probe settings and inspect logs for probe/finalizer `reasoning_effort="medium"` while main `Heavy` tasks keep high reasoning.
- Done when:
  - All Completion Audit items are verified with current evidence.

## Decisions

- 2026-06-12: Use `standard` as Search Probe default runtime minimum effort. Reason: Search Probe is a bounded sidecar and should not inherit heavy runtime budget by default.
- 2026-06-12: Hardcode Search Probe and forced finalizer model reasoning effort to `medium` for this checkpoint. Reason: smallest maintainable change that reduces reasoning overhead without introducing provider fallback abstraction.
- 2026-06-12: Defer unsupported-parameter fallback. Reason: providers already ignore or gate reasoning effort in several paths; robust typed fallback can be a separate focused change if runtime evidence shows failures.

## Progress Log

- 2026-06-12 00:00 +03: Checkpoint 1 completed
  - Changed: created goal document for Search Probe effort controls.
  - Evidence: current code paths reviewed in `AgentExecutionOptions`, Search Probe options, and opencode-go reasoning handling; new-file diff inspected before commit.
  - Commands: `git status --short`, `git branch --show-current`, `git log -3 --oneline`, `git diff --no-index -- /dev/null docs/goals/2026-06-12-search-probe-effort-controls.md || true`.
  - Audit IDs updated: Q2, N1 scoped for implementation audit.
  - Next: commit this goal doc, then start Checkpoint 2 after user review.

## Risks and Blockers

- Some providers may reject explicit `reasoning_effort="medium"`.
  - Impact: Search Probe LLM call could fail on unsupported routes.
  - Evidence: existing opencode-go tests include provider error text for unsupported `reasoning_effort`; no current runtime failure from this change yet.
  - Mitigation or requested decision: start without broad fallback; add typed fallback later only if observed.
  - Audit IDs affected: G3, G4, V1.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
