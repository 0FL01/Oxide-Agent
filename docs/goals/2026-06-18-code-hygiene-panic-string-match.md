# Goal: Code hygiene — close foreign-input panic and string-match classes

Date started: 2026-06-18
Status: active
Codex goal: see /goal objective below
Source spec: RECON report (this session, 2026-06-18) — static scan of warnings/errors/coffee-smells across the workspace
Goal doc owner: Codex
Last updated: 2026-06-18 13:20

## Objective

Close the architectural classes of defects surfaced by the RECON static scan:
(A) foreign input reaching `unreachable!`/`panic!` panic paths,
(B) string-match heuristics over provider error messages used to reconstruct HTTP status / retryability,
(D) `too_many_arguments` anti-pattern in transport handlers,
and (low) silent audit-event error discard in reminders.

Done when every required Completion Audit item is verified by its listed evidence and all out-of-scope constraints are preserved.

## Scope

In scope:
- `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/controls.rs` (Class A1)
- `crates/oxide-agent-transport-telegram/src/bot/progress_render.rs` (Class A2)
- `crates/oxide-agent-transport-web/src/web_transport.rs` (Class A3)
- `crates/oxide-agent-core/src/llm/support/backoff.rs` (Class B)
- `crates/oxide-agent-core/src/llm/types.rs` and `LlmError` definition + all call sites (Class B)
- `crates/oxide-agent-core/src/llm/providers/**` (all providers constructing `LlmError::ApiError` / `NetworkError`)
- `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/{controls,lifecycle}.rs`, `handlers.rs`, `runner.rs` (Class D)
- `crates/oxide-agent-transport-telegram/src/runner.rs` (`handle_agent_confirmation` delegate)
- `crates/oxide-agent-core/src/llm/providers/openai_base/mod.rs` (`build_tool_chat_body`)
- `crates/oxide-agent-core/src/agent/providers/webfetch_md/fetch.rs` (Habr fetchers)
- `crates/oxide-agent-core/src/agent/providers/reminder.rs` (Stage 4 audit logging)
- `docs/goals/2026-06-18-code-hygiene-panic-string-match.md` (this file)

Out of scope:
- Class C (`#[allow(dead_code)]` 62 sites) — feature gating, not dead code; cosmetic, not nuclear.
- Narrowing casts / `from_utf8_lossy` flagged as low-risk in RECON — practically safe, leave as-is.
- `unimplemented!()` in test trait mocks — standard pattern.
- `composer.rs` `expect()` calls — all inside `#[cfg(test)] mod tests`.
- `tts/client.rs:24` `expect` on `reqwest::Client::build()` — practically never fails.

## Missing Inputs

None. RECON provided enough evidence to design all fixes.

## Repository Context

- Relevant entry points: `crates/oxide-agent-core/src/llm/types.rs` (LlmError), `crates/oxide-agent-core/src/llm/support/backoff.rs` (retry policy), transport handler entry points.
- Existing conventions: thiserror for library crates, anyhow for app/binary; `#![forbid(unsafe)]` in core/web lib.rs; explicit `mod.rs`; profile-feature gating in Cargo.toml.
- Dependencies or runtime assumptions: providers surface HTTP errors as `LlmError::ApiError(String)` today; `backoff.rs` reconstructs status from the string. No external API changes needed — fix is internal type enrichment.
- Validation infrastructure: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --features <profile> -- -D warnings`, `cargo check/test` per profile.
- Risky areas: `LlmError` is a shared type touched by every provider; blast radius is wide for Stage 2. Must enumerate every call site via `git grep`.

## Completion Audit

- G1: No `unreachable!()`/`panic!()` reachable from foreign input in production code.
  - Source: RECON Class A — `controls.rs:786`, `progress_render.rs:180`, `web_transport.rs:55`.
  - Acceptance: each site either replaced with a safe fallback (warn + no-op / default view) or refactored so the invariant is enforced by types (exhaustive enum match), making panic architecturally impossible.
  - Evidence required: `git grep -n 'unreachable!' crates/oxide-agent-transport-telegram/src/bot/agent_handlers/controls.rs crates/oxide-agent-transport-telegram/src/bot/progress_render.rs crates/oxide-agent-transport-web/src/web_transport.rs` shows no reachable-from-foreign-input `unreachable!`; `cargo clippy --workspace --all-targets --features profile-full -- -D warnings` clean; targeted tests pass.
  - Status: verified
  - Evidence collected: CP1 — A1 `controls.rs`: introduced `ConfirmationReply` enum with `parse`, replaced guard+`_ => unreachable!()` with `let-else` + exhaustive enum match (foreign input → `None` → UX fallback). A2 `progress_render.rs`: introduced `BrowserMilestoneKind` enum, `parse` returns `Option<BrowserMilestoneKind>`, `summary()`/`blocked_reason()` match exhaustive (adding a variant → compile error, not panic). A3 `web_transport.rs:55`: `unreachable!()` replaced with `"sub_agent".to_string()` safe fallback (SubAgent is our type, `effective_agent_event` already called in same function — defense-in-depth). Remaining `unreachable!` in `web_transport.rs` (`_event_parts` functions) are internal invariant markers, not reachable from foreign input — outside CP1 scope. Remaining `panic!` in `web_transport.rs:2272+` are in `#[cfg(test)] mod tests` (N2). Gates: `cargo fmt --all -- --check` exit 0; `cargo clippy --workspace --all-targets --features profile-full -- -D warnings` exit 0; profile-embedded-opencode-local exit 0; profile-web-embedded-opencode-local exit 0; profile-search-only exit 0. Tests: `cargo test -p oxide-agent-transport-telegram --no-default-features --features profile-embedded-opencode-local` green; `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local` green (23 ignored require local TCP listener — pre-existing).

- G2: No string-match heuristic over provider error messages to reconstruct HTTP status or retryability.
  - Source: RECON Class B — `backoff.rs:57` (`contains("builder")`), `:84` (`contains("429")`), `:41-54` (`contains("500"/"502"/...)`).
  - Acceptance: `LlmError` carries typed status/retryability information; `backoff.rs` matches on enum variants, not substrings; `git grep -nE 'msg.*contains\("(429|500|502|503|504|builder|internal server error)' crates/oxide-agent-core/src/llm/` returns nothing.
  - Evidence required: clippy clean across all profiles; `cargo test -p oxide-agent-core` green; every provider constructing the enriched `LlmError` variants verified via `git grep 'LlmError::ApiError\|LlmError::NetworkError\|LlmError::RateLimit'`.
  - Status: verified
  - Evidence collected: CP2 — `LlmError::ApiError(String)` enriched to `ApiError { status: Option<u16>, message: String }` (struct variant); added `RequestBuilder(String)` variant for deterministic reqwest builder errors (NOT retryable); added `LlmError::api_error(msg)`, `api_error_status(status, msg)`, `from_reqwest_error(e)` helper constructors. `backoff.rs`: all `contains("429"/"500"/"builder"/...)` replaced with typed variant matching. `transcription.rs`: same typed patterns in local `get_retry_delay`. `opencode_go.rs`: `opencode_go_should_throttle` uses typed status checks. `llm_calls.rs`: `error_class` uses typed status. All provider construction sites updated across 17 files. `git grep -nE 'contains\("(429|500|502|503|504|builder|gateway|unavailable|overloaded)"\)'` returns 0 matches across all .rs files. `git grep 'LlmError::ApiError\('` returns 0 (no tuple construction remaining). `git grep 'ApiError\(msg\)|ApiError\(message\)'` returns 0.

- G3: `too_many_arguments` anti-pattern collapsed into context structs where it reduces call-site fragility.
  - Source: RECON Class D — 8 `#[allow(clippy::too_many_arguments)]` sites across 6 files.
  - Acceptance: `git grep -n 'clippy::too_many_arguments' crates/` count drops; no new clippy warnings; behavior unchanged (tests green).
  - Evidence required: clippy clean; targeted tests green; diff reviewed for behavior preservation.
  - Status: verified
  - Evidence collected: CP3 — Removed all 8 `#[allow(clippy::too_many_arguments)]`. 7 were stale (already under threshold after analysis: 4 transport-telegram free functions at 7 args with default threshold 7; 3 fetch.rs methods at or under threshold 9). 1 `build_tool_chat_body` in `openai_base/mod.rs` refactored: merged `profile: &OpenAICompatibleProfile` + `reasoning_effort: Option<&str>` into `chat_completions_request::ChatRequestOptions<'_>` (10→9 args), 17 call sites updated. Introduced `FetchOptions<'a>` struct in `webfetch_md/fetch.rs` (private, `#[derive(Clone, Copy)]`) collapsing `timeout_secs, output_window, cancellation_token` into one arg for 4 internal Habr fetchers (article_json 10→9, article_html_fallback 9→8, comments_json 10→9, comments_html_fallback 9→8). `git grep "too_many_arguments" -- '*.rs'` → 0 matches. Gates: `cargo fmt --all -- --check` exit 0; `cargo clippy --workspace --all-targets --features <profile> -- -D warnings` exit 0 for all 4 profiles; `cargo check --workspace --all-targets --features profile-full` exit 0. Tests: `cargo test -p oxide-agent-core --features profile-full` 1370 passed 0 failed 10 ignored; `cargo test -p oxide-agent-transport-telegram --features profile-embedded-opencode-local` 168 passed 0 failed 4 ignored.

- G4: Reminder audit-event errors no longer silently discarded.
  - Source: RECON Stage 4 — `reminder.rs:247/328/495`.
  - Acceptance: `let _ = ... append_audit_event(...)` replaced with `if let Err(e) = ... { tracing::warn!(...) }` (or equivalent logging); `git grep -n 'let _ = .*append_audit_event' crates/oxide-agent-core/src/agent/providers/reminder.rs` returns nothing.
  - Evidence required: clippy clean; `cargo test -p oxide-agent-core` green.
  - Status: verified
  - Evidence collected: CP4 — 3 sites in `reminder.rs` fixed: inline `reminder_job_scheduled` (line 247→248), inline `reminder_job_cancelled` (line 328→329), `append_audit` helper (line 494→496, covers 3 more call sites at 373/419/466). All `let _ = ... .await;` → `if let Err(e) = ... .await { warn!(error = %e, ...) }`. Added `use tracing::warn;` import. `git grep -n 'let _ = .*append_audit_event' reminder.rs` → 0 matches. Gates: fmt exit 0; clippy ×4 exit 0; `cargo test -p oxide-agent-core --features profile-full` 1370 passed 0 failed 10 ignored.

- Q1: Workspace clippy clean with `-D warnings` across `profile-full`, `profile-embedded-opencode-local`, `profile-web-embedded-opencode-local`, `profile-search-only`.
  - Source: AGENTS.md lint requirement.
  - Acceptance: all four clippy invocations exit 0.
  - Evidence required: command outputs logged.
  - Status: verified
  - Evidence collected: CP2 — `cargo clippy --workspace --all-targets --features <profile> -- -D warnings` exit 0 for all 4 profiles.

- Q2: `cargo fmt --all -- --check` clean.
  - Source: AGENTS.md.
  - Acceptance: exit 0.
  - Evidence required: command output.
  - Status: verified
  - Evidence collected: CP2 — `cargo fmt --all -- --check` exit 0.

- Q3: Workspace `cargo check` clean on `profile-full`.
  - Source: AGENTS.md.
  - Acceptance: exit 0.
  - Evidence required: command output.
  - Status: verified
  - Evidence collected: CP2 — `cargo check --workspace --all-targets --features profile-full` exit 0.

- N1: Do not touch Class C `#[allow(dead_code)]` sites (feature gating, not in scope).
  - Source: RECON.
  - Must preserve: no edits to `compiled.rs`, `modules.rs`, `builders.rs`, `schema.rs`, `capabilities.rs` `allow(dead_code)` lines.
  - Evidence required: `git diff` shows no edits to those lines.
  - Status: verified
  - Evidence collected: CP1+CP2 — `git diff` shows no edits to Class C `#[allow(dead_code)]` lines; no edits to `compiled.rs`, `modules.rs`, `builders.rs`, `schema.rs`, `capabilities.rs` allow lines.

- N2: Do not touch test-only `expect()`/`panic()` (in `#[cfg(test)]` modules).
  - Source: RECON.
  - Must preserve: `composer.rs` tests, `tts/client.rs:24`, test mock `unimplemented!()`.
  - Evidence required: `git diff` shows no edits to those lines.
  - Status: verified
  - Evidence collected: CP1+CP2 — `git diff` shows no edits to `composer.rs` test `expect()`, `tts/client.rs:24`, or test mock `unimplemented!()`.

- N3: Do not introduce new crates, services, queues, caches, storage backends, or abstraction layers.
  - Source: AGENTS.md scale principles.
  - Must preserve: `Cargo.toml` dependency list unchanged (no new crates); no new modules beyond what's needed for the typed error / context structs.
  - Evidence required: `git diff Cargo.toml crates/*/Cargo.toml` shows no new dependencies.
  - Status: verified
  - Evidence collected: CP2 — no new crates, services, or modules. `LlmError` enrichment is internal to existing `llm/error.rs`; `is_transient_server_status` re-exported from existing `llm/mod.rs`. No Cargo.toml changes.

## Implementation Plan

### CP1 — Stage 1: close foreign-input panic class (Class A)

- Audit IDs: G1, Q1, Q2, Q3, N1, N2
- Expected changes:
  - `controls.rs:786`: replace `_ => unreachable!()` with `tracing::warn!` + no-op fallback (Telegram callback data is foreign input; match must have safe catch-all).
  - `progress_render.rs:180`: refactor string-match on `self.kind` to an exhaustive enum match (introduce `enum BrowserMilestoneKind` if not present; parse into the enum; match exhaustively so adding a variant is a compile error, not a runtime panic).
  - `web_transport.rs:55`: collapse `event_kind` + `effective_agent_event` so the "unwrap sub-agent first" invariant is enforced by the type/signature, not by caller discipline.
- Validation:
  - `cargo clippy --workspace --all-targets --features profile-full -- -D warnings`
  - `cargo clippy --workspace --all-targets --features profile-embedded-opencode-local -- -D warnings`
  - `cargo clippy --workspace --all-targets --features profile-web-embedded-opencode-local -- -D warnings`
  - `cargo clippy --workspace --all-targets --features profile-search-only -- -D warnings`
  - `cargo fmt --all -- --check`
  - `cargo test -p oxide-agent-transport-telegram --no-default-features --features profile-embedded-opencode-local` (scoped — this profile activates telegram features)
  - `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`
- Exit condition: no reachable-from-foreign-input `unreachable!` in the three target files; all gates green.

### CP2 — Stage 2: typify LlmError, remove string-match (Class B)

- Audit IDs: G2, Q1, Q2, Q3, N3
- Expected changes:
  - `llm/types.rs`: enrich `LlmError` — `ApiError { status: Option<u16>, message: String }` (or split into `RateLimit`, `ServerError`, `ClientError`); split `NetworkError` into `NetworkTransient` (retryable) and `RequestBuilder` (deterministic).
  - `llm/support/backoff.rs`: remove all `contains("429"/"500"/"builder"/...)`; match on typed variants.
  - All providers (`anthropic`, `openai_base`, `openrouter`, `opencode_go`, `chatgpt`, `mistral`, `minimax`): update `LlmError::ApiError` / `NetworkError` construction sites to pass typed status. Enumerate via `git grep -n 'LlmError::ApiError\|LlmError::NetworkError' crates/`.
- Validation:
  - all four profile clippy gates
  - `cargo fmt --all -- --check`
  - `cargo test -p oxide-agent-core`
  - `git grep -nE 'msg.*contains\("(429|500|502|503|504|builder|internal server error)' crates/oxide-agent-core/src/llm/` empty
- Exit condition: no string-match over provider error messages in `backoff.rs`; all providers construct typed errors; all gates green.

### CP3 — Stage 3: collapse too_many_arguments (Class D)

- Audit IDs: G3, Q1, Q2, N3
- Expected changes:
  - Introduce `AgentConfirmationCtx` (or similar) in `controls.rs` / `runner.rs`; refactor `handle_agent_confirmation` signature.
  - Refactor `activate_agent_mode` (`lifecycle.rs`), `handle_menu_commands` (`handlers.rs`), `build_tool_chat_body` (`openai_base/mod.rs`), `fetch_habr_article_{json,html_fallback,comments_json}` (`webfetch_md/fetch.rs`) — group related args into context structs where grouping is natural; do not force grouping where it would hurt locality.
- Validation:
  - all four profile clippy gates
  - `cargo fmt --all -- --check`
  - targeted tests green
- Exit condition: `too_many_arguments` `allow` count reduced; behavior unchanged; all gates green.

### CP4 — Stage 4: reminder audit logging

- Audit IDs: G4, Q1, Q2
- Expected changes:
  - `reminder.rs:247/328/495`: `let _ = ... append_audit_event(...)` → `if let Err(e) = ... { tracing::warn!(error = %e, action = %action, "audit append failed") }`.
- Validation:
  - clippy + fmt
  - `cargo test -p oxide-agent-core`
- Exit condition: no silent `let _ =` discard of `append_audit_event` in `reminder.rs`; gates green.

## Validation Contract

- Static checks: `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets --features <profile> -- -D warnings` for all four profiles.
- Tests: `cargo test -p oxide-agent-core`; `cargo test -p oxide-agent-transport-telegram --no-default-features --features profile-embedded-opencode-local`; `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`.
- Runtime/manual verification: none required — fixes are static and covered by unit tests.
- Artifact verification: `git grep` commands listed per audit item return empty / expected results.
- Done when: every audit item `G*`, `Q*`, `N*` is `verified` with evidence; Final Verification filled.

## Decisions

- 2026-06-18: Scope is the four RECON stages; Class C (`#[allow(dead_code)]`) explicitly out of scope as it's feature gating, not dead code. N1 enforces.
- 2026-06-18: Use `tracing::warn!` (not `eprintln!`) for audit-failure logging — consistent with the rest of the codebase's structured logging.
- 2026-06-18: For Stage 2, prefer enriching `ApiError` with `Option<u16>` status over splitting into many variants — smaller blast radius, preserves `Display`/`From` impls, still closes the string-match class. Re-evaluate at CP2 if call-site enumeration reveals a cleaner split.

## Progress Log

- 2026-06-18 11:10: Goal doc created from RECON report. OpenCode goal set. Starting CP1.
  - Changed: `docs/goals/2026-06-18-code-hygiene-panic-string-match.md`
  - Evidence: RECON report (this session) — fmt/clippy clean across 4 profiles; static grep findings documented above.
  - Commands: (RECON only — see report)
  - Audit IDs updated: none yet
  - Next: CP1 — Stage 1 foreign-input panic class.

- 2026-06-18 11:25: CP1 complete — Stage 1 foreign-input panic class closed.
  - Changed: `controls.rs` (A1: `ConfirmationReply` enum + `let-else`), `progress_render.rs` (A2: `BrowserMilestoneKind` enum + exhaustive match), `web_transport.rs` (A3: safe fallback for SubAgent arm).
  - Evidence: G1 verified — `git grep` confirms no reachable-from-foreign-input `unreachable!` in the three target files; `cargo fmt` exit 0; `cargo clippy` exit 0 on all four profiles; targeted tests green.
  - Commands: `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets --features <profile> -- -D warnings` ×4; `cargo test -p oxide-agent-transport-telegram --no-default-features --features profile-embedded-opencode-local`; `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`.
  - Audit IDs updated: G1 → verified; Q1 → verified (clippy ×4); Q2 → verified (fmt); Q3 → pending (cargo check not re-run, covered by clippy); N1 → preserved (no Class C edits); N2 → preserved (test panics untouched).
  - Next: CP2 — Stage 2 typify LlmError, remove string-match in backoff.rs.

- 2026-06-18 12:30: CP2 complete — Stage 2 typify LlmError, remove string-match class closed.
  - Changed: `llm/error.rs` (ApiError struct variant + RequestBuilder + helper constructors), `llm/support/backoff.rs` (typed matching, `is_transient_server_status` pub(crate)), `llm/mod.rs` (re-export), `llm/support/http.rs` (5 sites), `llm/client.rs` (4 sites), `llm/providers/messages/response.rs` (3 sites), `llm/providers/chat_completions/response.rs` (4 sites), `llm/providers/anthropic/client.rs` (1 site), `llm/providers/openai_base/mod.rs` (2 sites), `llm/providers/openai_base/transcription.rs` (11 sites), `llm/providers/chatgpt/mod.rs` (5 sites), `llm/providers/chatgpt/auth.rs` (2 sites), `llm/providers/chat_completions/streaming.rs` (1 site), `llm/providers/opencode_go.rs` (8 sites), `agent/runner/llm_calls.rs` (5 sites), `agent/runner/test_support.rs` (1 site), `tests/hermetic_agent.rs` (2 sites), `tests/json_decode_error.rs` (1 site), `transport-web/src/server/search_probe.rs` (1 site), `transport-web/tests/e2e/providers.rs` (1 site).
  - Evidence: G2 verified — 0 string-match patterns remaining (`git grep` confirms); all providers construct typed errors; `cargo test -p oxide-agent-core` 1370 passed 0 failed; `cargo test -p oxide-agent-transport-web` 151 passed 0 failed. Q1 verified (clippy ×4 exit 0). Q2 verified (fmt exit 0). Q3 verified (cargo check exit 0). N3 verified (no new deps/crates).
  - Commands: `cargo check --workspace --all-targets --features profile-full`; `cargo clippy --workspace --all-targets --features <profile> -- -D warnings` ×4; `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features profile-full`; `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`.
  - Audit IDs updated: G2 → verified; Q1 → verified; Q2 → verified; Q3 → verified; N1 → verified; N2 → verified; N3 → verified.
  - Next: CP3 — Stage 3 collapse too_many_arguments.

- 2026-06-18 13:05: CP3 complete — Stage 3 too_many_arguments class closed. Commit `5a2a28d3`.
  - Changed: `agent/providers/webfetch_md/fetch.rs` (FetchOptions struct + 4 function refactors + 4 call sites), `llm/providers/openai_base/mod.rs` (build_tool_chat_body refactor + 17 call sites + test import fix), `transport-telegram/src/bot/agent_handlers/controls.rs`, `transport-telegram/src/bot/agent_handlers/lifecycle.rs`, `transport-telegram/src/bot/handlers.rs`, `transport-telegram/src/runner.rs` (stale `#[allow]` removal).
  - Evidence: G3 verified — `git grep "too_many_arguments" -- '*.rs'` → 0 matches; all gates green; tests green (1370 + 168). N1/N2/N3 preserved (no Class C / test panic / new deps edits).
  - Commands: `git grep "too_many_arguments" -- '*.rs'`; `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets --features <profile> -- -D warnings` ×4; `cargo check --workspace --all-targets --features profile-full`; `cargo test -p oxide-agent-core --features profile-full`; `cargo test -p oxide-agent-transport-telegram --features profile-embedded-opencode-local`.
  - Audit IDs updated: G3 → verified.
  - Next: CP4 — Stage 4 reminder.rs audit logging.

- 2026-06-18 13:20: CP4 complete — Stage 4 reminder audit logging class closed.
  - Changed: `agent/providers/reminder.rs` (3 `let _ = append_audit_event` → `if let Err(e) = ... { warn!(...) }` + `use tracing::warn;` import). `append_audit` helper fix covers 3 more call sites (373/419/466).
  - Evidence: G4 verified — `git grep -n 'let _ = .*append_audit_event' reminder.rs` → 0 matches; clippy ×4 exit 0; fmt exit 0; `cargo test -p oxide-agent-core` 1370 passed 0 failed 10 ignored.
  - Commands: `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets --features <profile> -- -D warnings` ×4; `cargo test -p oxide-agent-core --no-default-features --features profile-full`.
  - Audit IDs updated: G4 → verified.
  - Next: Final Verification — completion audit, all gates green.

## Risks and Blockers

- Risk: Stage 2 `LlmError` enrichment has wide blast radius (every provider). Mitigation: enumerate call sites with `git grep` before editing; run all four profile clippy gates; run core tests after each provider update.
- Risk: Stage 3 refactor could change behavior if context-struct field order / defaults drift. Mitigation: keep structs as pure aggregation (no logic), preserve field types exactly, run targeted tests.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
