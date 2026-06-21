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

Full evidence with reasoning, traces, and design assessments: `docs/goals/2026-06-21-core-fit-audit-evidence.md` (referred to as "evidence doc" below). Condensed tables here; full verdicts there.

### A1 — Architectural invariants

| # | Invariant | Verdict | Evidence |
|---|---|---|---|
| A1.1 | No transport dependency leak | PASS | `Cargo.toml:19-61` no teloxide/transport; grep 0 hits |
| A1.2 | Explicit `mod.rs` convention | PARTIAL (low) | 3 dirs use modern `foo.rs+foo/` style (`agent/executor.rs`, `llm/providers/opencode_go.rs`, `openrouter.rs`) |
| A1.3 | cfg-gating on `oxide_module_<id>` | FIXED | mechanical migration: 491 simple + 13 compound + 10 cfg_attr + 4 cfg!() gates → `oxide_module_*` aliases; 0 remaining non-profile non-http-client raw gates |
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
| A2.4 | `validate_detection` English keyword gate over scout reasoning | HIGH (live) → FIXED | `llm_detector.rs:212-218` — removed; `validate_detection` now trusts `is_stuck=true` + `confidence >= threshold` directly |
| A2.5 | `extract_reasoning_summary` regex strip English filler | MEDIUM (live) | `thoughts.rs:146-151` |
| A2.6 | `should_salvage_structured_output_failure` accepts prose as final | HIGH (live) | `responses.rs:288-313`, `:31-42` |
| A2.7 | `>=3` fail-fast cap accepts raw after 3 JSON failures | MEDIUM (live) | `responses.rs:44-69` |
| A2.8 | Divergent `looks_like_prose` vs `should_salvage` duplicates | MEDIUM (latent bug) | `structured_output.rs:223-240` vs `responses.rs:288-313` — same logic, different edge cases |
| A2.9 | 3+ JSON-extractor variants with behavior gaps | MEDIUM | `recovery.rs:574` (brace+serde), `llm_detector.rs:338` (brace, no serde), `executor/execution.rs:1084` (naive find/rfind) |
| A2.10 | `is_recovered=true` bypasses tool loop detector | MEDIUM → FIXED | `runner/loop_detection.rs:93-99` — bypass removed; all tool calls feed into detector |
| A2.11 | 0 `TODO`/`FIXME`/`HACK`/`unimplemented!` markers | POSITIVE | grep clean |
| A2.12 | Compaction is class-closing | POSITIVE | typed `AgentMessageKind`, deterministic budget, externalized payloads, atomic replacement |

### A3 — Contracts and error handling

| # | Area | Verdict | Evidence |
|---|---|---|---|
| A3.1 | thiserror/anyhow in sandbox | FIXED | `SandboxError` enum introduced; anyhow removed from all sandbox files |
| A3.2 | Provider contracts (sender knows all it supplies) | SOUND | `claim_reminder_job` atomic UPDATE with precondition inside receiver (`sqlx/mod.rs:1711-1746`) |
| A3.3 | Tool runtime correlation | SMELL | `ToolCallCorrelation` typed; call↔output pairing runtime-verified (`runtime.rs:267-302`), not type-invariant |
| A3.4 | Schema versioning | SOUND | all 8 records carry `schema_version`; 2 bumped (binding v2, reminder v2); migrations runtime-path not embedded |
| A3.5 | Race/concurrency | SOUND | atomic claim + `FOR UPDATE`; 2 `tokio::Mutex`-across-await serialize but correct; no `await_holding_lock` |
| A3.6 | `LlmError` context-poor | SMELL → FIXED | `ApiError` now has `provider`/`model` fields; `Unknown` changed to struct variant with `provider`/`model`; `with_provider()`/`with_model()` methods; `LlmClient` wraps errors with context at all return sites |
| A3.7 | `StorageProvider::check_connection` stringly-typed | SMELL (low) | `provider.rs:214` returns `Result<(), String>` |
| A3.8 | Secret handling | SOUND | `SecretProbeReport` metadata-only by type; no central redaction net at tool-output boundary (caveat) |

### A4 — Testing discipline

| # | Area | Verdict | Evidence |
|---|---|---|---|
| A4.1 | cfg-gating hygiene in tests | FIXED | 26 raw module-level gates in test contexts migrated to `oxide_module_*` aliases along with all src gates |
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
| A5.2 | Structured output parsing | FIXED | `should_use_native_json_mode` gate removed; `json_object` enforced provider-side with tools; prose-wrap removed; salvage removed; hard-error after 3 retries |
| A5.3 | Recovery from malformed responses | FIXED | history repair SOUND (class-closing, kept); content sanitization removed (`sanitize_xml_tags`, `sanitize_tool_call`, all dead-code extractors) |
| A5.4 | Loop detection class-closing | SMELL → FIXED | cycle detection (A-B-A-B detected via periodicity check); re-prompt remediation (inject context + continue, not halt-only); LLM keyword gate removed |
| A5.5 | Route failover & 429 quarantine | SOUND | typed time-based quarantine (`model_routes.rs:126`), count-then-quarantine |
| A5.6 | Prompt cache hit architecture | SOUND | static `base` + volatile `date_suffix`; fold pipeline (`history.rs:56-80`); minor: wiki_context in base |
| A5.7 | Compaction design | SOUND | typed classes, deterministic budget, externalized payloads, atomic replacement |
| A5.8 | Provider capability negotiation | SOUND | default-deny static allowlist, model-level verified policy |
| A5.9 | Hot context health hook | SOUND | typed `HookResult`, deterministic thresholds |

Full audit evidence with reasoning, traces, and design assessments: `docs/goals/2026-06-21-core-fit-audit-evidence.md`

## Current-State Baselines (measured 2026-06-21)

### File line counts

| File | Current lines | Target after remediation | Phase |
|---|---|---|---|
| `src/agent/recovery.rs` | 1544 → 805 | ~600-900 (history-repair stays, content-sanitization removed) | Phase 1 ✅ |
| `src/agent/structured_output.rs` | 558 → 475 | ~350 (recovery paths removed, typed parse stays) | Phase 1 ✅ |
| `src/agent/runner/responses.rs` | 724 → 665 | ~600 (salvage/give-up removed, re-request stays) | Phase 1 ✅ |
| `src/sandbox/manager.rs` | 3118 | ~3120 (typing changes, not deletion) | Phase 2 |
| `src/sandbox/broker.rs` | 1399 | ~1400 (typing changes, not deletion) | Phase 2 |
| `src/sandbox/traits.rs` | 373 | ~390 (SandboxError added) | Phase 2 |
| `src/llm/error.rs` | 82 | ~110 (provider/model fields on ApiError/Unknown) | Phase 5 |

### cfg gate counts

| Metric | Current | Target |
|---|---|---|
| `#[cfg(feature = "<module-feature>")]` (raw, attribute form) | ~~490 simple + 13 compound~~ → 0 (all migrated) | 0 module-level ✓ |
| `#[cfg(oxide_module_<id>)]` (aliased) | ~~107~~ → ~600+ (all module gates migrated) | ~610 ✓ |
| `#[cfg(feature = "profile-*")]` (allowed raw) | 0 attribute, 7 `cfg!()` macro | unchanged ✓ |
| `#[cfg(feature = "http-client")]` (non-module utility) | 1 | 1 (not a module, stays raw) ✓ |

### anyhow usage in sandbox

| Metric | Current | Target |
|---|---|---|
| `anyhow` in `src/sandbox/**/*.rs` (non-test) | ~~70 uses across 3 files~~ → 0 | 0 ✓ |
| `SandboxError` enum | ~~does not exist~~ → introduced with 13 typed variants | ✓ |

## Completion Audit

### Functional requirements (G*)

- G1: Structured output enforced by provider-side mode, not prompt text
  - Source: A2.1, A2.2, A2.6, A2.7, A2.8, A5.2
  - Acceptance: when tools present and provider supports structured-output mode, `response_format`/tool-forced-schema is set; non-JSON response → hard-error + re-request (not prose-wrap, not salvage, not `>=3` accept)
  - Evidence required: `cargo test -p oxide-agent-core --no-default-features --features profile-full` green; `structured_output.rs` has no `looks_like_prose`; `responses.rs` has no `should_salvage_structured_output_failure`; `recovery.rs` has no `sanitize_xml_tags`/`sanitize_tool_call` (history-repair half stays)
  - Status: verified
  - Evidence collected: P0.5 probes confirmed `json_object` + `tools` accepted by OpenCode Go (200), ZAI (200), OpenRouter (200), Mistral (docs). `should_use_native_json_mode` gate `!has_tools` removed (`request.rs:361`). OpenRouter `JsonModePolicy::None`→`Standard` (`profile.rs:358`). `looks_like_prose` removed from `structured_output.rs`. `should_salvage_structured_output_failure` and `>=3` accept-raw cap removed from `responses.rs`, replaced with `MAX_STRUCTURED_OUTPUT_RETRIES` hard error. `sanitize_xml_tags`, `sanitize_tool_call`, `sanitize_tool_calls`, `sanitize_leaked_xml`, `try_parse_malformed_tool_call`, `looks_like_tool_call_text` + 12 dead-code helpers removed from `recovery.rs`. `thoughts.rs` regex strip removed. `execution.rs` naive `extract_json_object` replaced with shared `extract_first_json`. 921 tests pass, clippy clean, fmt clean, grep 0. `recovery.rs` 1544→805 lines.

- G2: `SandboxError` typed enum introduced
  - Source: A1.4, A3.1
  - Acceptance: `SandboxError` enum exists with variants (NotRunning, ContainerNotFound, ExecTimeout, Cancelled, FileNotFound, BackendNotCompiled, Broker, Protocol, InvalidEdit, ReadGuardMismatch, Docker(#[from]), Io(#[from]), Other); `SandboxBackend` trait methods return `Result<_, SandboxError>`; `anyhow` removed from `sandbox/manager.rs`, `sandbox/broker.rs`, `sandbox/traits.rs`, `sandbox/admin.rs`, `sandbox/diagnostics.rs`, `sandbox/manager_stub.rs`, `sandbox/mod.rs`
  - Evidence required: `cargo check` green; grep `anyhow` in `src/sandbox/**/*.rs` returns 0; `cargo clippy` clean
  - Status: verified
  - Evidence collected: `src/sandbox/error.rs` created with 13-variant `SandboxError` enum (thiserror). All sandbox trait methods (`SandboxExec`, `SandboxFileOps`, `SandboxLifecycle`, `SandboxAdmin`, `SandboxDiagnostics`) return `Result<_, SandboxError>`. `manager.rs`: 30+ `anyhow!`/`.context()` converted to typed variants. `broker.rs`: 40+ `anyhow!` converted; `SandboxBrokerResponse::Error` → `SandboxError::Broker`; encoding errors → `SandboxError::Protocol`. `manager_stub.rs`: `BackendNotCompiled` variant. `admin.rs`, `diagnostics.rs`: trait impls updated. Callers updated: `agent/providers/sandbox.rs` (trait impls + `.map_err(anyhow::Error::from)` at tool-output boundary), `agent/providers/tts/provider.rs`, `silero_tts/provider.rs`, `manager_control_plane/mod.rs`, `agent/runner/llm_calls.rs`, `agent/preprocessor.rs`, `agent/providers/ytdlp.rs` (FakeSandbox test), `agent/providers/stack_logs.rs` (FakeDiagnostics test), `transport-web/src/server/types.rs`, `transport-web/tests/e2e/setup.rs`. Also fixed Phase 1 bug: OpenRouter `chat_with_tools` was ignoring `json_mode` parameter (hardcoded `false`), now passes it through. 1295 tests pass, clippy clean, fmt clean, grep `anyhow` in sandbox = 0.

- G3: cfg-alias migration complete
  - Source: A1.3, A4.1
  - Acceptance: all module-level `#[cfg(feature = "<module-feature>")]` in `src/**/*.rs` replaced with `#[cfg(oxide_module_<id>)]`; only profile-level raw gates and `http-client` (non-module utility feature) remain
  - Evidence required: `cargo run -p xtask -- module-registry check` green; grep `feature = "` in `src/**/*.rs` excluding `profile-` and `http-client` returns 0; `cargo check` green on `profile-full` and `profile-embedded-opencode-local`
  - Status: verified
  - Evidence collected: Mechanical sed-based migration applied to all `src/**/*.rs` files. 491 simple `#[cfg(feature="...")]` + 13 compound `#[cfg(any/all(... feature="..."))]` + 10 `cfg_attr(not(feature="..."))` + 4 `cfg!(feature="...")` gates migrated to `oxide_module_*` aliases. `http-client` (non-module utility feature for `dep:reqwest`) preserved as raw. Profile-level gates (`cfg!(feature = "profile-*")` in `compiled.rs`) preserved as raw per AGENTS.md. Post-migration: 0 remaining non-profile non-http-client raw feature gates; `xtask module-registry check` passes (40 modules, 45 Cargo features, 40 compiled declarations); `cargo check` green on both `profile-full` and `profile-embedded-opencode-local`; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --all -- --check` clean; 1295 tests pass. Also fixed pre-existing clippy `needless_return` in `manager.rs:2449`.

- G4: Loop detection catches cycles, not just consecutive repeats
  - Source: A2.10, A5.4
  - Acceptance: tool-call sequence analyzed as cycle (A-B-A-B detected); `is_recovered=true` calls no longer bypass tool detector; cycle detected → re-prompt with "you are looping, change approach" + context injection, not halt-only
  - Evidence required: new test `detects_abab_cycle` passes; `is_recovered` bypass removed; re-prompt remediation path asserted
  - Status: verified
  - Evidence collected: `tool_detector.rs` rewritten with cycle detection: bounded `Vec<String>` history of SHA-256 hashes, `detect_cycle()` checks if last `threshold` entries are periodic with any period `p` from 1 to `threshold/2` via `is_periodic()` — catches A-A-A-A-A (p=1), A-B-A-B-A (p=2), A-B-C-A-B-C (p=3). `is_recovered=true` bypass removed from `runner/loop_detection.rs:tool_loop_outcome` (grep confirms no `is_recovered` in code, only doc comment). `llm_detector.rs:validate_detection` simplified to `parsed.is_stuck && parsed.confidence >= self.confidence_threshold` — English keyword gate removed. `LoopDetectionOutcome` enum (NoLoop/RePrompt/Halt) added to `types.rs`. `LoopDetectionService` tracks `re_prompt_count`/`max_re_prompts=2`: first detection → RePrompt (inject "you are looping" system context + reset detectors + continue iterating), second detection → RePrompt again, third → Halt (cancel + error). `handle_loop_outcome` in runner injects re-prompt as `AgentMessage::system_context` (persists to memory, visible on next LLM call). `response_dispatch.rs` and `execution.rs` all 6 call sites updated. Tests: `detects_abab_cycle`, `detects_abc_abc_cycle`, `recovered_calls_detected`, `tool_call_detection_re_prompts_then_halts`, `re_prompt_includes_loop_type` — all pass. 1302 tests pass, clippy clean, fmt clean.

- G5: `LlmError` carries provider/model context
  - Source: A3.6
  - Acceptance: `LlmError::ApiError` and `LlmError::Unknown` have `provider: Option<String>` and `model: Option<String>` fields; `LlmClient::chat_with_tools` wraps errors at `client.rs:696` with provider/model; retry-exhaustion error includes provider/model
  - Evidence required: `cargo test` green; new test asserts `ApiError.provider`/`.model` populated on wrapped error
  - Status: verified
  - Evidence collected: `ApiError` variant now has `provider: Option<String>` and `model: Option<String>` fields. `Unknown` variant changed from tuple `Unknown(String)` to struct `Unknown { message, provider, model }`. `with_provider()`/`with_model()` methods added — mutate only `ApiError`/`Unknown`, noop on other variants. `LlmClient::chat_with_tools` wraps errors at line 706 (`return Err(e.with_provider(&model_info.provider).with_model(&model_info.id))`) and retry-exhaustion at line 715. `chat_with_tools_single_attempt_for_model_info` wraps at line 544. `complete_internal_text` wraps at line 483. Capability-check errors wrapped at lines 544 and 633. `LlmError::unknown()` helper added for ergonomic construction. All `LlmError::Unknown(...)` construction sites across workspace migrated to `LlmError::unknown(...)`. All `ApiError` match sites already used `..` pattern — no changes needed. `llm_detector.rs:313` match pattern updated to `LlmError::Unknown { message: msg, .. }`. Tests: `api_error_carries_provider_model`, `unknown_carries_provider_model`, `with_provider_model_noop_on_other_variants`, `api_error_defaults_to_none` — all pass. 1306 tests pass, clippy clean, fmt clean.

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
- П0.5 Каркас (must run BEFORE design):
  - **Question:** does each provider accept `response_format` + `tools` simultaneously? Current code says no (`!has_tools` gate is universal). Verify per-provider with live probe.
  - **Full provider matrix:** see `docs/goals/2026-06-21-core-fit-audit-evidence.md` § "Provider Profile Matrix" (8 providers, `JsonModePolicy` / `StructuredOutputPolicy` / `supports_structured_output` / `response_format` set-when / source file:line)
  - **Live-probe endpoints (new info, not in evidence doc):**

    | Provider | Live-probe endpoint |
    |---|---|
    | Mistral | `https://api.mistral.ai/v1/chat/completions` |
    | ZAI/Zhipu | `https://api.z.ai/api/coding/paas/v4/chat/completions` |
    | OpenRouter | `https://openrouter.ai/api/v1/chat/completions` |
    | OpenCode Go | `https://opencode.ai/zen/go/v1/chat/completions` |
    | OpenCode Zen | `https://opencode.ai/zen/v1/chat/completions` |
    | Generic | (configured) |
    | ChatGPT/Codex | `https://api.openai.com/v1/responses` |
    | Anthropic | `https://api.anthropic.com/v1/messages` |

  - **Live-probe plan (П0.5):** for each provider, send minimal request with both `response_format: {type: "json_object"}` (or `json_schema`) AND `tools: [{type: "function", function: {...}}]`. Record: HTTP status, response body, whether response is valid JSON. Gate env: `RUN_LLM_E2E_CHECKS=1` + valid API key per provider. Fixtures in `tests/phase1_provider_probe.rs` (new). Expected outcomes to verify:
    1. Mistral: docs claim `response_format` + `tools` supported. Verify.
    2. ZAI: `zai_supports_structured_output` (`profile.rs:504`) is per-model. Verify which GLM models accept both.
    3. OpenRouter: `JsonModePolicy::None` — verify whether per-model structured-output is available with tools.
    4. OpenCode Go/Zen: `supports_structured_output: false` — verify if this is a real API limitation or conservative default.
    5. ChatGPT: Responses API — verify `text.format` parameter with tools present.
    6. Anthropic: no json_mode — verify tool-forced-schema alternative (tool with `input_schema` enforcing JSON structure).
  - **Design decision after probes:** if provider accepts both → force structured output mode when tools present. If provider rejects both → hard-error + re-request is class-closing fallback (task fails loudly > silently accepts prose). If provider accepts tool-forced-schema only → use that.
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
- Mapping table (`cargo_feature` → `oxide_module_<id>` cfg alias, from `module_registry.toml`):

  | `cargo_feature` | `oxide_module_<id>` cfg alias | Raw gates to migrate |
  |---|---|---|
  | `transport-telegram` | `oxide_module_transport_telegram` | (check) |
  | `transport-web` | `oxide_module_transport_web` | (check) |
  | `storage-sqlx` | `oxide_module_storage_sqlx` | (check) |
  | `llm-chatgpt` | `oxide_module_llm_provider_openai_chatgpt` | (check) |
  | `llm-mistral` | `oxide_module_llm_provider_mistral` | (check) |
  | `llm-minimax` | `oxide_module_llm_provider_anthropic` | (check) |
  | `llm-openai-base` | `oxide_module_llm_provider_openai_base` | (check) |
  | `llm-opencode-go` | `oxide_module_llm_provider_opencode_go` + `oxide_module_llm_provider_opencode_zen` | 33 |
  | `llm-openrouter` | `oxide_module_llm_provider_openrouter` | 14 |
  | `tool-todos` | `oxide_module_tool_todos` | (check) |
  | `tool-compression` | `oxide_module_tool_compression` | (check) |
  | `tool-delegation` | `oxide_module_tool_delegation` | (check) |
  | `tool-agents-md` | `oxide_module_tool_agents_md` | (check) |
  | `tool-reminder` | `oxide_module_tool_reminder` | (check) |
  | `tool-wiki-memory` | `oxide_module_tool_wiki_memory` | (check) |
  | `tool-webfetch-md` | `oxide_module_tool_webfetch_md` + `oxide_module_tool_web_crawler` | 44 |
  | `tool-tavily` | `oxide_module_tool_tavily` | (check) |
  | `tool-brave-search` | `oxide_module_tool_brave_search` | (check) |
  | `tool-crw` | `oxide_module_tool_crw` | 19 |
  | `tool-browser-live` | `oxide_module_tool_browser_live` | (check) |
  | `tool-sandbox-fileops` | `oxide_module_tool_sandbox_fileops` | (check) |
  | `tool-sandbox-exec` | `oxide_module_tool_sandbox_exec` | (check) |
  | `tool-sandbox-recreate` | `oxide_module_tool_sandbox_recreate` | (check) |
  | `tool-file-delivery` | `oxide_module_tool_file_delivery` | (check) |
  | `tool-media-audio` | `oxide_module_tool_media_audio` | (check) |
  | `tool-media-image` | `oxide_module_tool_media_image` | (check) |
  | `tool-media-video` | `oxide_module_tool_media_video` | (check) |
  | `tool-ytdlp` | `oxide_module_tool_ytdlp` | (check) |
  | `tool-tts-kokoro` | `oxide_module_tool_tts_kokoro` | (check) |
  | `tool-tts-silero` | `oxide_module_tool_tts_silero` | (check) |
  | `tool-stack-logs` | `oxide_module_tool_stack_logs` | 15 |
  | `sandbox-backend-docker-direct` | `oxide_module_sandbox_backend_docker_direct` | 94 |
  | `sandbox-backend-sandboxd-client` | `oxide_module_sandbox_backend_sandboxd_client` | 29 |
  | `sandbox-daemon` | `oxide_module_sandbox_daemon_sandboxd` | (check) |
  | `integration-mcp-jira` | `oxide_module_integration_mcp_jira` | (check) |
  | `integration-mcp-mattermost` | `oxide_module_integration_mcp_mattermost` | (check) |
  | `integration-ssh-mcp` | `oxide_module_integration_ssh_mcp` | 22 |
  | `manager-control-plane` | `oxide_module_manager_control_plane` | (check) |

  Note: `llm-opencode-go` and `tool-webfetch-md` map to TWO module IDs each (one Cargo feature → multiple modules). Both aliases must be used when migrating gates for these features. The exact alias name is derived from the module `id` field in `module_registry.toml` with `/` → `_`.

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

- 2026-06-21: Phase 0 — evidence doc + baselines + П0.5 каркас
  - Changed: `docs/goals/2026-06-21-core-fit-audit-evidence.md` created (full A1–A5 evidence with reasoning, traces, design assessments, provider profile matrix)
  - Changed: goal doc supplemented with Current-State Baselines (file line counts, cfg gate counts, anyhow usage counts), Phase 1 П0.5 Каркас (provider matrix + live-probe plan), Phase 3 mapping table (38 feature→module-ID rows from `module_registry.toml`)
  - Evidence: provider profiles read from `llm/providers/chat_completions/profile.rs:208-449`, `chatgpt/mod.rs:295`, `anthropic/client.rs:95`; `module_registry.toml` parsed for mapping
  - Audit IDs updated: none (baseline supplement)
  - Next: commit + user review

- 2026-06-21: Phase 1 — structured output → provider-side mode (G1 verified)
  - Changed: `llm/providers/chat_completions/request.rs` — `should_use_native_json_mode` gate `!has_tools` removed; `json_object` now set when `json_mode=true` even with tools
  - Changed: `llm/providers/chat_completions/profile.rs` — OpenRouter `JsonModePolicy::None`→`Standard` (probe confirmed support)
  - Changed: `agent/structured_output.rs` — `looks_like_prose` and prose-wrapper branch removed; deterministic lexer fixes (fence-strip, control-strip, JSON extraction) kept
  - Changed: `agent/runner/responses.rs` — `should_salvage_structured_output_failure` removed; `>=3` accept-raw cap replaced with `MAX_STRUCTURED_OUTPUT_RETRIES` hard error
  - Changed: `agent/recovery.rs` — 22 functions removed (sanitize_xml_tags, sanitize_tool_call, sanitize_tool_calls, sanitize_leaked_xml, contains_xml_tags, control_xml_tag_pattern, normalize_tool_name, try_parse_malformed_tool_call, extract_malformed_tool_arguments, is_valid_argument, build_recovered_tool_call, extract_tag_value, extract_token_after_tool_name, 8 extract_*_arguments, looks_like_tool_call_text); 1544→805 lines
  - Changed: `agent/runner/tools.rs` — sanitize_xml_tags calls removed from progress events
  - Changed: `agent/runner/response_dispatch.rs` — sanitize_tool_calls call removed from tool dispatch
  - Changed: `agent/providers/todos.rs` — sanitize_xml_tags call removed
  - Changed: `agent/mod.rs` — `pub use sanitize_xml_tags` removed
  - Changed: `agent/thoughts.rs` — regex strip of English filler prefixes removed
  - Changed: `agent/executor/execution.rs` — naive `extract_json_object` replaced with shared `extract_first_json`
  - Changed: `tests/proptest_recovery.rs` — deleted (tested removed function)
  - Changed: `transport-telegram/tests/agent_xml_leak_prevention.rs` — `bugfix_agent_2026_001_tests` module removed (tested removed function)
  - Evidence: P0.5 probes — OpenCode Go `json_object`+`tools`→200, ZAI→200, OpenRouter→200; `json_schema`+`tools`→400 for OpenCode Go, 200 for ZAI/OpenRouter. Mistral docs confirm support. 921 tests pass, clippy clean, fmt clean, grep 0. `recovery.rs` 1544→805.
  - Commands: `cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local` (921 pass, 0 fail); `cargo clippy --all-targets -- -D warnings` clean; `cargo fmt --all -- --check` clean
  - Audit IDs updated: G1 verified, A5.2 FIXED, A5.3 FIXED
  - Next: Phase 2 (SandboxError typed enum)

- 2026-06-21: Phase 2 — SandboxError typed enum (G2 verified)
  - Changed: `src/sandbox/error.rs` — new `SandboxError` enum (13 variants: NotRunning, ContainerNotFound, ExecTimeout, Cancelled, FileNotFound, BackendNotCompiled, Broker, Protocol, InvalidEdit, ReadGuardMismatch, Docker(#[from] bollard), Io(#[from] std::io), Other)
  - Changed: `src/sandbox/mod.rs` — `pub mod error`, re-export `SandboxError`, `preflight_sandbox_backend` returns `Result<(), SandboxError>`
  - Changed: `src/sandbox/traits.rs` — all trait methods return `Result<_, SandboxError>`; `apply_sandbox_file_edit`/`validate_edit_read_guard`/`apply_exact_text_edit` use typed variants
  - Changed: `src/sandbox/manager.rs` — 30+ `anyhow!`/`.context()` converted to typed variants; `#[from]` for bollard; `.parse()` errors → `SandboxError::Other`
  - Changed: `src/sandbox/broker.rs` — 40+ `anyhow!` converted; `SandboxBrokerResponse::Error` → `SandboxError::Broker`; encoding → `SandboxError::Protocol`; `UnixListener::bind` sync fix; test code updated
  - Changed: `src/sandbox/manager_stub.rs` — `BackendNotCompiled` variant; all methods return `Result<_, SandboxError>`
  - Changed: `src/sandbox/admin.rs`, `diagnostics.rs` — trait impls updated
  - Changed: `agent/providers/sandbox.rs` — `SandboxRuntime` trait impls return `Result<_, SandboxError>`; `.map_err(anyhow::Error::from)` at tool-output boundary; `apply_exact_text_edit` test helper uses typed variants
  - Changed: `agent/providers/tts/provider.rs`, `silero_tts/provider.rs` — `.map_err(anyhow::Error::from)` on `write_file`
  - Changed: `agent/providers/manager_control_plane/mod.rs` — `.map_err(anyhow::Error::from)` on 7 `SandboxAdmin` delegations
  - Changed: `agent/runner/llm_calls.rs` — `NativeImageFileReader` impl `.map_err(anyhow::Error::from)`
  - Changed: `agent/preprocessor.rs` — `RecordingSandboxFileOps`/`RecordingSandboxExec` test impls updated
  - Changed: `agent/providers/ytdlp.rs` — `FakeSandbox` test impl updated
  - Changed: `agent/providers/stack_logs.rs` — `FakeDiagnostics` test impl updated
  - Changed: `transport-web/src/server/types.rs` — `.map_err(anyhow::Error::from)` on 3 `SandboxAdmin` delegations
  - Changed: `transport-web/tests/e2e/setup.rs` — `.map_err(anyhow::Error::from)` on `delete_sandbox_by_name`
  - Fixed: OpenRouter `chat_with_tools` was ignoring `json_mode` (hardcoded `false`); now passes it through (Phase 1 bug)
  - Evidence: 1295 tests pass, clippy clean, fmt clean, grep `anyhow` in `src/sandbox/**/*.rs` = 0
  - Commands: `cargo test -p oxide-agent-core --no-default-features --features profile-full` (1295 pass, 0 fail); `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --all -- --check` clean
  - Audit IDs updated: G2 verified, A3.1 FIXED
  - Next: Phase 3 (cfg-alias migration)

- 2026-06-21: Phase 3 — cfg-alias migration (G3 verified)
  - Changed: all `src/**/*.rs` files — mechanical sed-based migration of 491 simple `#[cfg(feature="...")]` + 13 compound `#[cfg(any/all(... feature="..."))]` + 10 `cfg_attr(not(feature="..."))` + 4 `cfg!(feature="...")` gates to `#[cfg(oxide_module_*)]` / `cfg!(oxide_module_*)` aliases
  - Preserved: `#[cfg(feature = "http-client")]` in `llm/error.rs:73` (non-module utility feature for `dep:reqwest`); `cfg!(feature = "profile-*")` in `capabilities/compiled.rs:272-285` (profile-level, allowed raw per AGENTS.md)
  - Fixed: pre-existing clippy `needless_return` in `sandbox/manager.rs:2449` (return Err(...) → Err(...))
  - Fixed: `cargo fmt` reformatted 4 `cfg_attr` lines that exceeded line width after alias migration (longer `oxide_module_*` names)
  - Evidence: `xtask module-registry check` passes (40 modules, 45 features, 40 declarations); 0 remaining non-profile non-http-client raw feature gates; `cargo check` green on `profile-full` and `profile-embedded-opencode-local`; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --all -- --check` clean; 1295 tests pass
  - Commands: `cargo run -p xtask -- module-registry check`; `cargo check -p oxide-agent-core --no-default-features --features profile-full`; `cargo check -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features profile-full` (1295 pass, 0 fail)
  - Audit IDs updated: G3 verified, A1.3 FIXED, A4.1 FIXED
  - Next: Phase 4 (Loop detection class-closing)

- <2026-06-21>: Phase 4 — Loop detection cycle-DAG + re-prompt (G4)
  - Changed: `loop_detection/tool_detector.rs` (rewritten: cycle detection via periodicity check on bounded hash history); `loop_detection/service.rs` (rewritten: `LoopDetectionOutcome` enum, re-prompt remediation with `re_prompt_count`/`max_re_prompts=2`, `handle_detection` resets detectors on re-prompt); `loop_detection/types.rs` (added `LoopDetectionOutcome`); `loop_detection/llm_detector.rs` (removed English keyword gate in `validate_detection`); `loop_detection/mod.rs` (export `LoopDetectionOutcome`); `runner/loop_detection.rs` (rewritten: removed `is_recovered` bypass, methods return `LoopDetectionOutcome`, added `handle_loop_outcome`); `runner/response_dispatch.rs` (6 call sites updated); `runner/execution.rs` (LLM loop check updated)
  - Evidence: `detects_abab_cycle` test passes (A-B-A-B-A detected as p=2 cycle); `detects_abc_abc_cycle` test passes (A-B-C-A-B-C detected as p=3 cycle); `recovered_calls_detected` test passes (is_recovered bypass removed); `tool_call_detection_re_prompts_then_halts` test passes (RePrompt → RePrompt → Halt); `re_prompt_includes_loop_type` test passes; grep confirms no `is_recovered` in runner code (only doc comment); grep confirms no English keyword `contains` in `validate_detection`; 1302 tests pass, 0 fail; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --all -- --check` clean
  - Commands: `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib` (1302 pass, 0 fail); `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all -- --check`
  - Audit IDs updated: G4 verified, A2.10 FIXED, A5.4 FIXED, A2.4 FIXED
  - Next: Phase 5 (LlmError provider/model context)

- <2026-06-21>: Phase 5 — LlmError provider/model context (G5)
  - Changed: `llm/error.rs` (added `provider`/`model` fields to `ApiError` and `Unknown`; `Unknown` changed from tuple to struct variant; added `unknown()` helper, `with_provider()`, `with_model()` methods; 4 new tests); `llm/client.rs` (wrapped errors at 5 sites: `chat_with_tools` retry-exhaustion + provider error, `chat_with_tools_single_attempt_for_model_info` capability check, `chat_with_tools` capability check, `complete_internal_text` error; retry-exhaustion in `chat_with_tools`); all `LlmError::Unknown(...)` construction sites migrated to `LlmError::unknown(...)` across workspace (oxide-agent-core, oxide-agent-transport-web, oxide-agent-transport-telegram); `llm_detector.rs` match pattern updated
  - Evidence: `api_error_carries_provider_model` test asserts `ApiError.provider=Some("openrouter")`, `.model=Some("deepseek-v3.1")`; `unknown_carries_provider_model` test asserts `Unknown.provider`/`.model` populated; `with_provider_model_noop_on_other_variants` test asserts noop on `NetworkError`; `api_error_defaults_to_none` test asserts defaults; 1306 tests pass, 0 fail; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --all -- --check` clean
  - Commands: `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib` (1306 pass, 0 fail); `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all -- --check`
  - Audit IDs updated: G5 verified, A3.6 FIXED
  - Next: Phase 6 (Tests for remediated subsystems)

## Risks and Blockers

- R1: Provider structured-output-with-tools support unknown
  - Impact: G1 design depends on whether providers (OpenRouter, ChatGPT, Anthropic, OpenCode Go) accept `response_format` + `tools` together or require tool-forced-schema
  - Evidence: RESOLVED — P0.5 probes confirmed `json_object`+`tools` accepted by OpenCode Go (200), ZAI (200), OpenRouter (200); Mistral docs confirm support; Anthropic uses Messages API (no response_format, tool input_schema instead); ChatGPT uses Responses API (separate path)
  - Mitigation: applied — `!has_tools` gate removed, `json_object` forced provider-side; hard-error after 3 retries for non-JSON
  - Audit IDs affected: G1 (verified)

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
