# Goal: oxide-agent-core П0-fit remediation

Date started: 2026-06-21
Status: active
Codex goal: not set
Source spec: 5-axis audit of `crates/oxide-agent-core` against development protocol (П0/П0.5/П0.6)
Goal doc owner: Codex
Last updated: 2026-06-21

## Objective

Close the П0-fit gaps in `crates/oxide-agent-core` identified by the 5-axis audit (A1–A5). The crate is bimodal: ~90% fit in the transport/history/routing spine, ~30% fit in the LLM-output recovery crust. This goal eliminates the crust by root-cause redesign, not symptom patching, and closes the verification gaps that allowed the crust to persist.

Done when every Completion Audit item is verified by its listed evidence, all out-of-scope constraints are preserved, and the crate's П0-fit rises from ~65% to ≥90%.

## Scope

In scope:
- `crates/oxide-agent-core/src/agent/recovery.rs` — content sanitization removal (history repair stays)
- `crates/oxide-agent-core/src/agent/structured_output.rs` — provider-side enforcement + dead recovery removal
- `crates/oxide-agent-core/src/agent/runner/responses.rs` — salvage/give-up removal
- `crates/oxide-agent-core/src/agent/loop_detection/` — cycle-DAG + re-prompt remediation
- `crates/oxide-agent-core/src/sandbox/` — `SandboxError` typed enum introduction
- `crates/oxide-agent-core/src/llm/error.rs` — `provider`/`model` context fields
- `crates/oxide-agent-core/src/llm/client.rs` — error wrapping at `chat_with_tools` boundary
- `crates/oxide-agent-core/src/**/*.rs` — cfg-alias migration (raw `feature=` → `oxide_module_*`)
- `crates/oxide-agent-core/tests/` — property tests + live-contract tests for uncovered providers

Out of scope:
- Sound subsystems: compaction (`agent/compaction/`), tool_call_id integrity (`llm/support/history.rs`, `recovery.rs` history-repair half), route failover (`runner/model_routes.rs`), claim/lease reminders, secret probe-by-type, prompt cache composition
- Transport crates (`oxide-agent-transport-telegram`, `oxide-agent-transport-web`)
- New LLM providers or new tool providers
- Behavioural changes to `StorageProvider` or `LlmProvider` trait surfaces
- Web UI, Leptos frontend
- Any new crates, services, queues, caches, or storage backends

## Missing Inputs

None. All findings are evidence-backed with file:line citations from the audit.

## Repository Context

- Relevant entry points: `crates/oxide-agent-core/src/lib.rs`, `src/agent/mod.rs`, `src/llm/mod.rs`, `src/storage/mod.rs`
- Existing conventions: `module_registry.toml` is single source of truth; `build.rs` emits `oxide_module_<id>` cfg aliases; `thiserror` for lib, `anyhow` for binaries; `#[cfg(oxide_module_<id>)]` for module gating, raw `feature=` only for profile-level
- Dependencies: Rust 1.94, `teloxide` (transport-only), SQLx/Postgres, `mockall`, `insta`, `proptest`
- Validation infrastructure:
  - `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` (lite)
  - `cargo build --release --no-default-features --features profile-full` (full)
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo fmt --all -- --check`
  - `cargo run -p xtask -- module-registry check`
  - `cargo test -p oxide-agent-core --no-default-features --features <profile>`
- Risky areas:
  - `recovery.rs` has live callers in `runner/tools.rs:294,359` and `response_dispatch.rs:129` — removal requires rewiring callers to hard-error path
  - `structured_output.rs` recovery paths are used by `runner/responses.rs` — removal requires provider-side mode negotiation first
  - cfg-alias migration is mechanical but high-volume (~503 sites) — must be batched by feature name to avoid noise
  - Loop detection `is_recovered=true` bypass (`runner/loop_detection.rs:93-99`) interacts with recovery removal — must be addressed together

## Audit Baseline (evidence locked, 2026-06-21)

### A1 — Architectural invariants

| # | Invariant | Verdict | Evidence |
|---|---|---|---|
| A1.1 | No transport dependency leak | PASS | `Cargo.toml:19-61` no teloxide/transport; grep 0 hits |
| A1.2 | Explicit `mod.rs` convention | PARTIAL (low) | 3 dirs use modern `foo.rs+foo/` style (`agent/executor.rs`, `llm/providers/opencode_go.rs`, `openrouter.rs`) |
| A1.3 | cfg-gating on `oxide_module_<id>` | VIOLATION (high) | ~503 raw `#[cfg(feature="...")]` vs 108 `#[cfg(oxide_module_*)]`; top: `delegation.rs:44-71`, `manager_control_plane/mod.rs:203-256` |
| A1.4 | thiserror for lib | VIOLATION (medium) | 272 `anyhow!` in non-test lib; `SandboxError` does not exist; `SandboxBackend` trait returns `anyhow::Result` (`sandbox/traits.rs:50`) |
| A1.5 | Context-scoped storage | PASS | `storage/provider.rs` three-tier API, legacy fallback marked |
| A1.6 | Typed provider boundaries | PARTIAL (low) | `check_connection() -> Result<(), String>` (`provider.rs:214`); 2 manager traits on `anyhow::Result` |
| A1.7 | No premature abstractions | PARTIAL (low) | `ManagerTopicSandboxCleanup`/`Control` single-implementor traits (`mod.rs:372,383`) |

### A2 — П0 crutch signals

| # | Signal | Severity | Evidence |
|---|---|---|---|
| A2.1 | `sanitize_xml_tags` regex over LLM output | HIGH (live) | `recovery.rs:455-458`; called from `runner/tools.rs:294,359`, `response_dispatch.rs:129` |
| A2.2 | `sanitize_tool_call` PATTERN 1/2 `contains` over LLM tool-name | HIGH (live) | `recovery.rs:469,496`; special-cased to `write_todos` |
| A2.3 | `try_parse_malformed_tool_call` + 12 hardcoded tool names | MEDIUM (dead) | `recovery.rs:673-703`; 0 callers outside tests |
| A2.4 | `validate_detection` English keyword gate over scout reasoning | HIGH (live) | `llm_detector.rs:212-218` — overrides `is_stuck=true`+high-confidence unless `reasoning` contains one of 5 English words |
| A2.5 | `extract_reasoning_summary` regex strip English filler | MEDIUM (live) | `thoughts.rs:146-151` |
| A2.6 | `should_salvage_structured_output_failure` accepts prose as final | HIGH (live) | `responses.rs:288-313`, `:31-42` |
| A2.7 | `>=3` fail-fast cap accepts raw after 3 JSON failures | MEDIUM (live) | `responses.rs:44-69` |
| A2.8 | Divergent `looks_like_prose` vs `should_salvage` duplicates | MEDIUM (latent bug) | `structured_output.rs:223-240` vs `responses.rs:288-313` — same logic, different edge cases |
| A2.9 | 3+ JSON-extractor variants with behavior gaps | MEDIUM | `recovery.rs:574` (brace+serde), `llm_detector.rs:338` (brace, no serde), `executor/execution.rs:1084` (naive find/rfind) |
| A2.10 | `is_recovered=true` bypasses tool loop detector | MEDIUM | `runner/loop_detection.rs:93-99` |
| A2.11 | 0 `TODO`/`FIXME`/`HACK`/`unimplemented!` markers | POSITIVE | grep clean |
| A2.12 | Compaction is class-closing | POSITIVE | typed `AgentMessageKind`, deterministic budget, externalized payloads, atomic replacement |

### A3 — Contracts and error handling

| # | Area | Verdict | Evidence |
|---|---|---|---|
| A3.1 | thiserror/anyhow in sandbox | VIOLATION | 69 non-test lib files use anyhow; no `SandboxError` enum |
| A3.2 | Provider contracts (sender knows all it supplies) | SOUND | `claim_reminder_job` atomic UPDATE with precondition inside receiver (`sqlx/mod.rs:1711-1746`) |
| A3.3 | Tool runtime correlation | SMELL | `ToolCallCorrelation` typed; call↔output pairing runtime-verified (`runtime.rs:267-302`), not type-invariant |
| A3.4 | Schema versioning | SOUND | all 8 records carry `schema_version`; 2 bumped (binding v2, reminder v2); migrations runtime-path not embedded |
| A3.5 | Race/concurrency | SOUND | atomic claim + `FOR UPDATE`; 2 `tokio::Mutex`-across-await serialize but correct; no `await_holding_lock` |
| A3.6 | `LlmError` context-poor | SMELL | `ApiError{status,message}` no `provider`/`model` (`llm/error.rs:8`); retry-exhaustion bare string (`client.rs:701`) |
| A3.7 | `StorageProvider::check_connection` stringly-typed | SMELL (low) | `provider.rs:214` returns `Result<(), String>` |
| A3.8 | Secret handling | SOUND | `SecretProbeReport` metadata-only by type; no central redaction net at tool-output boundary (caveat) |

### A4 — Testing discipline

| # | Area | Verdict | Evidence |
|---|---|---|---|
| A4.1 | cfg-gating hygiene in tests | PARTIAL | 26 raw module-level `#[cfg(feature="...")]` in test contexts (`runner/llm_calls.rs:1115,1165,1429`, `manager_control_plane/tests/*.rs`) |
| A4.2 | Test category coverage | SOUND | hermetic/integration/snapshot/property present; 1394 test fns |
| A4.3 | Hermetic vs integration gating | SOUND | Postgres + live LLM env-gated skip-cleanly |
| A4.4 | `mock_storage_noop` masks contract bugs | PARTIAL | `testing.rs:100` blanket `Ok(None)`/`Ok(())`; only 2 call-sites, mitigated |
| A4.5 | П0.5 live-contract coverage | PARTIAL | live-shape tests for 2 of ~7 providers (Anthropic, Mistral); OpenRouter/ChatGPT/OpenCode Go/ZAI/MiniMax mocked-only |
| A4.6 | Mock setup duplication | PARTIAL | 99 raw `MockStorageProvider::new()` outside `testing.rs` |
| A4.7 | Snapshot discipline | SOUND | git-locked, per-profile isolation |
| A4.8 | Property/fuzz coverage | WEAK | proptest only for `sanitize_xml_tags` (1 function); `canonicalize_tool_call_args`, `parse_structured_output` uncovered |
| A4.9 | Loop detection test strength | PARTIAL | 11 enumerated tests; reordered-args canonicalization integration + LLM-scout escalation unverified |

### A5 — LLM integration correctness

| # | Area | Verdict | Evidence |
|---|---|---|---|
| A5.1 | tool_call_id integrity | SOUND | typed `ToolCallCorrelation`, pre-request `validate_tool_history` (`history.rs:202-243`), typed repair before retry |
| A5.2 | Structured output parsing | SMELL | `should_use_native_json_mode = json_mode && !has_tools` (`request.rs:356`); JSON enforced by prompt text only; `parse_structured_output` cascades fence-strip→control-strip→prose-wrap→brace-extract |
| A5.3 | Recovery from malformed responses | SPLIT | history repair SOUND (class-closing); content sanitization symptom-patching (`sanitize_xml_tags`, `sanitize_tool_call`) |
| A5.4 | Loop detection class-closing | SMELL | deterministic layers catch consecutive-identical + lexical-chunks; A-B-A-B evades; all layers halt-only; LLM layer is unreliable-judging-unreliable gated by keyword allowlist |
| A5.5 | Route failover & 429 quarantine | SOUND | typed time-based quarantine (`model_routes.rs:126`), count-then-quarantine |
| A5.6 | Prompt cache hit architecture | SOUND | static `base` + volatile `date_suffix`; fold pipeline (`history.rs:56-80`); minor: wiki_context in base |
| A5.7 | Compaction design | SOUND | typed classes, deterministic budget, externalized payloads, atomic replacement |
| A5.8 | Provider capability negotiation | SOUND | default-deny static allowlist, model-level verified policy |
| A5.9 | Hot context health hook | SOUND | typed `HookResult`, deterministic thresholds |

## Completion Audit

### Functional requirements (G*)

- G1: Structured output enforced by provider-side mode, not prompt text
  - Source: A2.1, A2.2, A2.6, A2.7, A2.8, A5.2
  - Acceptance: when tools present and provider supports structured-output mode, `response_format`/tool-forced-schema is set; non-JSON response → hard-error + re-request (not prose-wrap, not salvage, not `>=3` accept)
  - Evidence required: `cargo test -p oxide-agent-core --no-default-features --features profile-full` green; `structured_output.rs` has no `looks_like_prose`; `responses.rs` has no `should_salvage_structured_output_failure`; `recovery.rs` has no `sanitize_xml_tags`/`sanitize_tool_call` (history-repair half stays)
  - Status: pending
  - Evidence collected:

- G2: `SandboxError` typed enum introduced
  - Source: A1.4, A3.1
  - Acceptance: `SandboxError` enum exists with variants (NotRunning, ContainerNotFound, ExecTimeout, ImagePull, BrokerUnavailable, Docker(_), Io(_)); `SandboxBackend` trait methods return `Result<_, SandboxError>`; `anyhow` removed from `sandbox/manager.rs`, `sandbox/broker.rs`, `sandbox/traits.rs`
  - Evidence required: `cargo check` green; grep `anyhow` in `src/sandbox/**/*.rs` returns 0; `cargo clippy` clean
  - Status: pending
  - Evidence collected:

- G3: cfg-alias migration complete
  - Source: A1.3, A4.1
  - Acceptance: all module-level `#[cfg(feature = "<module-feature>")]` in `src/**/*.rs` and `tests/**/*.rs` replaced with `#[cfg(oxide_module_<id>)]`; only profile-level raw gates remain
  - Evidence required: `cargo run -p xtask -- module-registry check` green; grep `#[cfg(feature = "(?!profile-)` returns 0 (or only profile-level)
  - Status: pending
  - Evidence collected:

- G4: Loop detection catches cycles, not just consecutive repeats
  - Source: A2.10, A5.4
  - Acceptance: tool-call sequence analyzed as cycle (A-B-A-B detected); `is_recovered=true` calls no longer bypass tool detector; cycle detected → re-prompt with "you are looping, change approach" + context injection, not halt-only
  - Evidence required: new test `detects_abab_cycle` passes; `is_recovered` bypass removed; re-prompt remediation path asserted
  - Status: pending
  - Evidence collected:

- G5: `LlmError` carries provider/model context
  - Source: A3.6
  - Acceptance: `LlmError::ApiError` and `LlmError::Unknown` have `provider: Option<String>` and `model: Option<String>` fields; `LlmClient::chat_with_tools` wraps errors at `client.rs:696` with provider/model; retry-exhaustion error includes provider/model
  - Evidence required: `cargo test` green; new test asserts `ApiError.provider`/`.model` populated on wrapped error
  - Status: pending
  - Evidence collected:

### Quality requirements (Q*)

- Q1: No new crates, services, queues, caches, or abstraction layers
  - Source: AGENTS.md "Implementation bias"
  - Acceptance: `Cargo.toml` `[dependencies]` unchanged except possibly removing `anyhow` if sandbox is fully migrated
  - Evidence required: `git diff Cargo.toml` shows no new deps
  - Status: pending
  - Evidence collected:

- Q2: clippy + fmt clean across workspace
  - Source: AGENTS.md "Format and lint"
  - Acceptance: `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all -- --check` both pass
  - Evidence required: commands green
  - Status: pending
  - Evidence collected:

- Q3: No transport dependency leak introduced
  - Source: AGENTS.md "Architectural invariants"
  - Acceptance: grep for `teloxide|oxide_agent_transport_*|leptos` in `src/**/*.rs` returns 0
  - Evidence required: grep output
  - Status: pending
  - Evidence collected:

### Validation requirements (V*)

- V1: Property test for `canonicalize_tool_call_args` roundtrip-equivalence
  - Source: A4.8
  - Acceptance: proptest asserts `canonicalize(canonicalize(x)) == canonicalize(x)` for arbitrary JSON values; reordered object keys produce identical canonical form
  - Evidence required: `cargo test -p oxide-agent-core -- proptest` green with new property
  - Status: pending
  - Evidence collected:

- V2: Property test for `parse_structured_output` malformed-input class
  - Source: A4.8, A5.2
  - Acceptance: proptest asserts any non-JSON input → `Err` (after G1, no prose-wrap path); any valid `StructuredOutput` JSON → parsed correctly
  - Evidence required: `cargo test` green with new property
  - Status: pending
  - Evidence collected:

- V3: Live-contract tests for 5 uncovered providers
  - Source: A4.5
  - Acceptance: live-shape-asserting tests for OpenRouter, ChatGPT/Codex OAuth, OpenCode Go, ZAI/Zhipu, MiniMax — each gated on env var + valid API key, skip-cleanly, asserts real response struct shape
  - Evidence required: test files exist; `RUN_LLM_E2E_CHECKS=1` with valid keys → tests pass; without → tests skip
  - Status: pending
  - Evidence collected:

### Non-goals (N*)

- N1: No behaviour change in sound subsystems
  - Source: A5.1, A5.5, A5.7, A5.8, A5.9
  - Must preserve: tool_call_id integrity, route failover/quarantine, compaction, capability negotiation, hot-context health
  - Evidence required: `git diff` shows no changes in `llm/support/history.rs`, `runner/model_routes.rs`, `agent/compaction/`, `llm/capabilities.rs`, `agent/hooks/hot_context.rs` (except import adjustments)
  - Status: pending
  - Evidence collected:

- N2: No secret handling regression
  - Source: A3.8
  - Must preserve: `SecretProbeReport` metadata-only by type; resolved secrets never serialized to `ToolOutput`/prompt/memory
  - Evidence required: grep `ssh_mcp.rs` `SecretProbeReport` unchanged; no new field carrying secret material
  - Status: pending
  - Evidence collected:

- N3: No prompt cache prefix regression
  - Source: A5.6
  - Must preserve: `ComposedPrompt.base` has no timestamp/per-request user data; `date_suffix` isolated; fold pipeline order unchanged
  - Evidence required: `git diff prompt/composer.rs` shows no cache-busting leak in base; `git diff llm/support/history.rs` fold order unchanged
  - Status: pending
  - Evidence collected:

## Implementation Plan

### Phase 0 — Audit baseline lock
- Audit IDs: all A* (evidence locked above)
- Expected changes: this document only
- Validation: document exists, file:line citations present
- Exit condition: document committed

### Phase 1 — Structured output → provider-side mode (G1)
- Audit IDs: G1, A2.1, A2.2, A2.6, A2.7, A2.8, A2.9, A5.2, A5.3
- Expected changes:
  - `llm/providers/chat_completions/request.rs:356` — replace `should_use_native_json_mode = json_mode && !has_tools` with mode negotiation that forces structured output when provider supports it, even with tools
  - `llm/providers/chat_completions/profile.rs` — extend `StructuredOutputPolicy` to cover tools-present case
  - `agent/structured_output.rs` — remove `looks_like_prose`, prose-wrap branch, `extract_fenced_json` fallback; keep typed `serde_json::from_str` + `validate_structured_output`
  - `agent/runner/responses.rs` — remove `should_salvage_structured_output_failure`, `>=3` fail-fast cap; replace with hard-error + re-request with stricter mode
  - `agent/recovery.rs` — remove `sanitize_xml_tags`, `sanitize_tool_call` (PATTERN 1/2), `try_parse_malformed_tool_call`, `extract_*_arguments`, `looks_like_tool_call_text`; keep history-repair half (`repair_agent_message_history*`, `prune_tool_history_by_availability`)
  - `agent/runner/tools.rs:294,359` and `response_dispatch.rs:129` — rewire callers from `sanitize_xml_tags` to hard-error path
  - `agent/thoughts.rs:146-151` — remove regex strip of English filler (display-only, but П0 violation)
  - `agent/executor/execution.rs:1084` — remove naive `find/rfind` JSON extractor; use shared `recovery.rs:extract_first_json` (the one with serde validation) if any caller remains
- Validation:
  - `cargo test -p oxide-agent-core --no-default-features --features profile-full` green
  - `cargo clippy --workspace --all-targets -- -D warnings` green
  - grep `sanitize_xml_tags|sanitize_tool_call|looks_like_prose|should_salvage` in `src/**/*.rs` returns 0
  - line count of `recovery.rs` reduced by ~400-600 lines
- Exit condition: all sub-items done, tests green, grep clean
- Risk: providers that reject `response_format` + `tools` together — must verify per-provider capability (П0.5: test with live call before committing design). Check OpenRouter/ChatGPT/Anthropic docs + live probe.
- Blocker if: a provider used in a profile has no structured-output-with-tools mode AND no tool-forced-schema alternative — then hard-error + re-request is the only class-closing option (acceptable: task fails loudly instead of silently accepting prose)

### Phase 2 — `SandboxError` typed enum (G2)
- Audit IDs: G2, A1.4, A3.1
- Expected changes:
  - new `src/sandbox/error.rs` — `SandboxError` enum with `thiserror::Error`, variants: `NotRunning`, `ContainerNotFound`, `ExecTimeout`, `ImagePull`, `BrokerUnavailable`, `Docker(#[from] bollard::errors::Error)`, `Io(#[from] std::io::Error)`, `Protocol(String)`, `Other(String)`
  - `src/sandbox/traits.rs:50` — `SandboxBackend` methods return `Result<_, SandboxError>`
  - `src/sandbox/manager.rs` — convert ~30 `anyhow!`/`.context()` to typed `SandboxError` variants
  - `src/sandbox/broker.rs` — convert ~40 `anyhow!` to typed variants
  - `src/sandbox/mod.rs` — re-export `SandboxError`
  - callers in `agent/providers/` that match on sandbox error strings — convert to typed matches
- Validation:
  - `cargo check` green
  - grep `anyhow` in `src/sandbox/**/*.rs` returns 0
  - `cargo clippy` clean
- Exit condition: `SandboxError` introduced, anyhow removed from sandbox, all callers compile

### Phase 3 — cfg-alias migration (G3)
- Audit IDs: G3, A1.3, A4.1
- Expected changes:
  - mechanical find-replace: `#[cfg(feature = "tool-todos")]` → `#[cfg(oxide_module_tool_todos)]` etc., guided by `module_registry.toml` mapping
  - batch by feature name to keep diffs reviewable (one commit per feature group)
  - profile-level gates (`#![cfg(any(feature = "profile-..."))]`) stay raw
- Validation:
  - `cargo run -p xtask -- module-registry check` green
  - grep `#[cfg(feature = "(?!profile-)` in `src/**/*.rs` returns only profile-level
  - `cargo check --workspace --no-default-features --features profile-full` green
  - `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` green
- Exit condition: all module-level raw gates migrated, both profiles check, registry check green

### Phase 4 — Loop detection cycle-DAG + re-prompt (G4)
- Audit IDs: G4, A2.10, A5.4, A2.4
- Expected changes:
  - `agent/loop_detection/tool_detector.rs` — replace consecutive-identical hash with cycle detection over tool-call sequence (Floyd's or visited-set over last N hashes)
  - `agent/runner/loop_detection.rs:93-99` — remove `is_recovered=true` bypass; recovered calls feed into detector like any other
  - `agent/loop_detection/llm_detector.rs:212-218` — remove English keyword gate; trust `is_stuck=true` + `confidence >= threshold` (the structured fields the prompt already requests at `:33-41`)
  - `agent/loop_detection/service.rs` — add re-prompt remediation: on detection, instead of halt-only, inject "you are looping, change approach" context + ForceIteration (not cancel)
  - `agent/hooks/` or `runner/` — wire re-prompt to `HookResult::ForceIteration` or new `HookResult::InjectContextAndForce`
- Validation:
  - new test `detects_abab_cycle` (A-B-A-B with varied args) passes
  - new test `recovered_calls_detected` (`is_recovered=true` loop caught) passes
  - new test `re_prompt_remediation_continues` (detection → ForceIteration, not cancel) passes
  - existing loop detection tests still pass
- Exit condition: cycle detection live, bypass removed, re-prompt remediation asserted

### Phase 5 — `LlmError` provider/model context (G5)
- Audit IDs: G5, A3.6
- Expected changes:
  - `src/llm/error.rs:8` — add `provider: Option<String>` and `model: Option<String>` to `ApiError` and `Unknown` variants (or wrap in outer struct)
  - `src/llm/client.rs:696` — wrap provider error with `e.with_provider(model_info.provider).with_model(model_info.id)` before returning
  - `src/llm/client.rs:701` — retry-exhaustion error includes provider/model
  - callers that match on `LlmError` — adjust for new fields (add `..` or populate)
- Validation:
  - `cargo test` green
  - new test `api_error_carries_provider_model` asserts wrapped error has both fields populated
- Exit condition: fields added, wrapping live, test green

### Phase 6 — Verification gap closure (V1, V2, V3)
- Audit IDs: V1, V2, V3, A4.8, A4.5
- Expected changes:
  - `tests/proptest_canonicalize.rs` — property: `canonicalize(canonicalize(x)) == canonicalize(x)`; reordered keys → identical canonical
  - `tests/proptest_structured_output.rs` — property: any non-JSON → Err; valid StructuredOutput → parsed (after G1)
  - `tests/openrouter_e2e.rs`, `tests/chatgpt_e2e.rs`, `tests/opencode_go_e2e.rs`, `tests/zai_e2e.rs`, `tests/minimax_e2e.rs` — live-shape-asserting, env-gated, skip-cleanly (mirror `anthropic_e2e.rs` pattern)
- Validation:
  - `cargo test -p oxide-agent-core -- proptest` green
  - `RUN_LLM_E2E_CHECKS=1` with valid keys → new e2e tests pass; without → skip
- Exit condition: properties + 5 new e2e files exist and pass/skip

## Validation Contract

- Static checks:
  - `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
  - `cargo check --workspace --no-default-features --features profile-full`
  - `cargo run -p xtask -- module-registry check`
- Lint:
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo fmt --all -- --check`
- Tests:
  - `cargo test -p oxide-agent-core --no-default-features --features profile-full`
  - `cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local`
  - `cargo test -p oxide-agent-core -- proptest` (after V1/V2)
- Live-contract (env-gated):
  - `RUN_LLM_E2E_CHECKS=1 cargo test -p oxide-agent-core --no-default-features --features profile-full --test '*_e2e'` (after V3)
- Greps (class-closing verification):
  - `grep -rn 'sanitize_xml_tags|sanitize_tool_call|looks_like_prose|should_salvage' src/` → 0
  - `grep -rn 'anyhow' src/sandbox/` → 0
  - `grep -rn '#\[cfg\(feature = "(?!profile-)' src/` → 0
- Done when: all G*, Q*, V* items verified, all N* preserved, both profiles check + test green, clippy + fmt clean, registry check green

## Decisions

- 2026-06-21: Phases ordered by ROI — Phase 1 (structured output) first because it is 10x ROI (closes class + deletes ~600 lines + raises A2/A5 from 30%→90% in one fix). Other phases each raise fit 3-5 points.
- 2026-06-21: History-repair half of `recovery.rs` is SOUND (A5.3) and stays. Only content-sanitization half is removed. This preserves the tool_call_id integrity invariant (A5.1).
- 2026-06-21: Loop detection re-prompt remediation uses `HookResult::ForceIteration` or equivalent, not a new enum variant, unless the existing variants cannot express the semantics. Avoid abstraction sprawl (Q1).
- 2026-06-21: cfg-alias migration is mechanical but batched by feature name to keep diffs reviewable and commits meaningful (per skill commit guidance).

## Progress Log

- 2026-06-21: Phase 0 — audit baseline lock
  - Changed: `docs/goals/2026-06-21-core-fit-remediation.md` created
  - Evidence: 5-axis audit complete with file:line citations (A1–A5 above)
  - Commands: 5 parallel `general` subagents, 285 .rs files inspected
  - Audit IDs updated: all A* locked as baseline
  - Next: Phase 1 (structured output → provider-side mode) after user review

## Risks and Blockers

- R1: Provider structured-output-with-tools support unknown
  - Impact: G1 design depends on whether providers (OpenRouter, ChatGPT, Anthropic, OpenCode Go) accept `response_format` + `tools` together or require tool-forced-schema
  - Evidence: `should_use_native_json_mode = json_mode && !has_tools` (`request.rs:356`) suggests historical incompatibility, but not verified per-provider
  - Mitigation: П0.5 — live probe each provider's structured-output-with-tools behavior before committing Phase 1 design. If a provider lacks both modes, hard-error + re-request is the class-closing fallback (task fails loudly > silently accepts prose).
  - Audit IDs affected: G1

- R2: `recovery.rs` removal may break callers not found by audit
  - Impact: `sanitize_xml_tags` is exported (`agent/mod.rs:92`) and may have callers outside `runner/tools.rs` and `response_dispatch.rs`
  - Evidence: audit found 3 call sites, but `git grep sanitize_xml_tags` must be re-run before removal
  - Mitigation: `git grep` before deletion; rewire each caller to hard-error path
  - Audit IDs affected: G1

- R3: Loop detection re-prompt may cause unbounded iteration
  - Impact: replacing halt with re-prompt could loop the re-prompt itself
  - Evidence: current design halts on detection (`runner/loop_detection.rs:24-56`); re-prompt needs a bounded retry count
  - Mitigation: re-prompt at most N times, then halt; N is a typed config field, not a magic number
  - Audit IDs affected: G4

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
