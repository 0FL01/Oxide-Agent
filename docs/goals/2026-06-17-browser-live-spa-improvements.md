# Goal: Browser Live SPA improvements — URL tracking, hash navigation, DOM waits, response bodies

Date started: 2026-06-17
Status: active
Codex goal: not set
Source spec: user-provided test report "Отчёт по тесту v2: OTS One Time Secrets" (2026-06-17 19:53–19:58 UTC+3)
Goal doc owner: Codex
Last updated: 2026-06-17 22:45

## Objective

Fix the root-cause issues found in the OTS One Time Secrets end-to-end test so that the Browser Live provider works reliably on single-page applications (SPAs) with hash routing, DOM state changes, and asynchronous API calls.

Done when every required Completion Audit item is verified by its listed evidence and the original OTS flow (create secret → open share link → reveal) completes with:
- correct URL in observation metadata,
- no 30-second timeout on hash navigation,
- working DOM wait actions,
- optional network response bodies available in the debug endpoint.

## Scope

In scope:
- `docker/chrome-agent-sidecar.py` — URL tracking, hash navigation, DOM wait commands, CDP listener body capture.
- `crates/oxide-agent-core/src/agent/providers/browser_live/types.rs` — new `wait_for_selector` and `wait_for_text` action variants.
- `crates/oxide-agent-core/src/agent/providers/browser_live/prompt.rs` — model schema and system prompt updates.
- `crates/oxide-agent-core/src/agent/providers/browser_live/parser.rs` — validation for new actions.
- `crates/oxide-agent-core/src/agent/providers/browser_live/actions.rs` — mapping of new decisions to sidecar requests.
- `crates/oxide-agent-core/src/agent/providers/browser_live/policy.rs` and `recovery.rs` — action kind / signature coverage.
- `crates/oxide-agent-core/src/agent/providers/browser_live/test_support.rs` — fake sidecar support for new actions.
- `crates/oxide-agent-core/src/agent/providers/browser_live/session.rs` — deduplication of retained artifact descriptors.
- `crates/oxide-agent-core/src/agent/providers/browser_live/types.rs` — optional `body` field in `NetworkItem` for debug payloads.
- `docs/goals/2026-06-17-browser-live-spa-improvements.md` — this plan.

Out of scope:
- New transport, new LLM provider, or new storage backend.
- Rewriting the agent runner or MIMO decision engine.
- Replacing `chrome-agent` or the persistent pipe model.
- Non-browser-live areas.

## Missing Inputs

None. All required evidence can be produced from the existing repo and the public `https://ots.bash.md/` test target.

## Repository Context

- Relevant entry points: `docker/chrome-agent-sidecar.py`, `crates/oxide-agent-core/src/agent/providers/browser_live/*`.
- Existing conventions: explicit `mod.rs`, feature-gated browser live provider, `thiserror` for library, `anyhow` for binary.
- Dependencies: `chrome-agent` 0.4.3, `python3-websockets` in the sidecar image.
- Validation: `cargo fmt`, `cargo clippy`, `cargo test`, `python -m py_compile`, `chrome-agent-sidecar --self-test`, live REST calls against the sidecar.
- Risky areas: CDP listener threading, WebSocket command/response synchronization, URL tracking across SPA `pushState`/`hashchange` events.

## Completion Audit

### G1: Observation metadata URL is accurate after every navigation
- Source: test report problem #2 — URL showed `https://www.google.com` after navigating to `https://ots.bash.md/`.
- Acceptance: every `BrowserObservation.url` reflects the real page URL, not the initial session `start_url`.
- Evidence required: unit test for `record_after_observation` and live test showing `https://ots.bash.md/` in the observation URL.
- Status: verified
- Evidence collected: CP-1 updated `docker/chrome-agent-sidecar.py` `_handle_goto` to set `session["url"]` and `session["title"]` from the navigation/inspect result before `build_observation`; added `Page.enable` to `CDPListener` and `_on_frame_navigated` to update `session["url"]` from CDP main-frame events; added defensive `_refresh_session_url_from_location` via `window.location.href`. Live REST test created session at `https://www.google.com` → navigated to `https://example.com/` → observation `url` was `https://example.com/` and title `Example Domain`; then navigated to `https://ots.bash.md/` → observation `url` was `https://ots.bash.md/` and title `OTS - One Time Secrets`.

### G2: SPA hash navigation completes without timeout
- Source: test report problem #1 — navigating to `https://ots.bash.md/#...|...` timed out after 30s.
- Acceptance: navigating to a same-origin hash-only URL takes the sidecar fast path and returns within 5 seconds.
- Evidence required: live REST test timing the share-link navigation, `NavigationStatus` must be `Loaded`.
- Status: verified
- Evidence collected: CP-1 added the defensive location refresh so the SPA hash fast-path is reliably detected. CP-2 replaced the fixed `time.sleep(0.5)` after the hash fast path with `listener.wait_for_network_idle(timeout=2.0)` and a `chrome-agent wait selector body` fallback. Live REST test performed the full OTS create-secret flow and navigated to the generated share link `https://ots.bash.md/#00d30b19-...`; the response returned in 385ms with `status: loaded`, `url: https://ots.bash.md/#00d30b19-...`, and `title: OTS - One Time Secrets`.

### G3: DOM wait actions available to the model and sidecar
- Source: test report problem #3 — no `wait_for_selector` or `wait_for_text` mechanism.
- Acceptance: `BrowserDecision` can emit `wait_for_selector` and `wait_for_text`; the sidecar executes them via `chrome-agent wait` and returns a successful `ActionResult` without a post-action screenshot.
- Evidence required: parser tests, fake sidecar tests, live REST test waiting for a known selector/text.
- Status: verified
- Evidence collected: CP-3 added `WaitForSelector` and `WaitForText` variants to `BrowserAction` and `BrowserDecisionAction` in `types.rs`, validated them in `parser.rs`, mapped them in `actions.rs` to `SidecarAction` with `capture_after: false`/`wait_for_stability: false`, updated `prompt.rs` schema and system prompt, and added `policy.rs`/`recovery.rs` cases. Fake sidecar supports the new waits; unit tests cover serialization and planning. `docker/chrome-agent-sidecar.py` maps `wait_for_selector` to `chrome-agent wait selector` and `wait_for_text` to `chrome-agent wait text`, passing `timeout_ms` through both single-action and script paths. Live REST tests on `https://example.com` showed `wait_for_selector h1` and `wait_for_text "Example Domain"` both return `technical_success: true`; a failing `wait_for_selector #missing-element` returned a structured `action_failed` error with `retryable: false`.

### G4: Debug network endpoint can optionally include response bodies
- Source: test report problem #6 — network summary lacks response bodies.
- Acceptance: `GET /sessions/{id}/debug/network?include_bodies=true` returns `NetworkItem` entries with a `body` field for failed / XHR requests.
- Evidence required: unit test for `build_network_debug_payload` and live test capturing the `POST /api/create` response body.
- Status: pending
- Evidence collected:

### G5: Retained screenshot artifact descriptors are not duplicated
- Source: test report low priority — "Дескрипторы скриншотов дублируются".
- Acceptance: `BrowserSessionState.retained_artifacts()` contains no two artifacts with the same URI.
- Evidence required: unit test for `record_observation` with repeated same-purpose records.
- Status: pending
- Evidence collected:

### Q1: Static checks and tests pass
- Source: AGENTS.md and repo conventions.
- Acceptance: `cargo fmt`, `cargo clippy`, relevant `cargo test`, sidecar `py_compile`, and sidecar self-test pass after every checkpoint.
- Evidence required: command outputs.
- Status: verified
- Evidence collected: CP-1 ran `python3 -m py_compile docker/chrome-agent-sidecar.py`, `cargo fmt --all -- --check`, `cargo clippy -p oxide-agent-core -p oxide-agent-web-contracts -p oxide-agent-web-ui --no-default-features --features profile-full --all-targets -- -D warnings`, `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- agent::providers::browser_live` (82 passed), `cargo test -p oxide-agent-web-ui` (11 passed), `cargo test -p oxide-agent-web-contracts` (10 passed), and `docker exec oxide_chrome_agent_sidecar chrome-agent-sidecar --self-test`. All passed.

### Q2: End-to-end OTS flow works after all changes
- Source: test report final result.
- Acceptance: the full OTS flow (create secret → extract share link → open link → reveal) completes with all audit items verified.
- Evidence required: live REST test or web console run transcript.
- Status: pending
- Evidence collected:

### N1: Do not change the core REST contract
- Source: project architectural invariants.
- Must preserve: `/sessions`, `/sessions/{id}/goto`, `/sessions/{id}/action`, `/sessions/{id}/observe`, `/sessions/{id}/screenshot/latest`, `/debug/network`, `/debug/console` request/response shapes.
- Evidence required: existing tests continue to pass; new fields are optional with serde defaults.
- Status: verified
- Evidence collected: CP-1 only changed internal sidecar state handling; no Rust contracts or request/response shapes were modified. All existing tests pass.

## Implementation Plan

### CP-1: Fix URL tracking in the sidecar
- Audit IDs: G1, G2, Q1.
- Expected changes:
  - `docker/chrome-agent-sidecar.py`: in `_handle_goto` full-navigation branch, set `session["url"]` and `session["title"]` from `chrome-agent goto` result **before** calling `build_observation`.
  - `CDPListener`: enable `Page` domain and update `session["url"]` on `Page.frameNavigated` events, so SPA `pushState`/`hashchange` navigations update the tracked URL even without a sidecar `goto` call.
  - Add a defensive `Runtime.evaluate("window.location.href")` refresh in `_handle_goto` before deciding whether to use the hash fast path.
- Validation:
  - `python -m py_compile docker/chrome-agent-sidecar.py`.
  - Live REST test: create session → navigate to `https://ots.bash.md/` → observe that `url` is `https://ots.bash.md/` and not `https://www.google.com`.
- Exit condition: observation URL is correct after a full navigation and after a hash navigation.

### CP-2: Harden SPA hash navigation
- Audit IDs: G2, Q1, Q2.
- Expected changes:
  - `docker/chrome-agent-sidecar.py`: after the hash fast path sets `window.location.hash`, replace the unconditional `time.sleep(0.5)` with `wait_for_network_idle(timeout=2.0)` and a `chrome-agent wait selector body` fallback so the post-action observation reflects the rendered SPA.
  - If `GotoRequest.wait_until` is `NetworkIdle`, call `wait_for_network_idle` after the full `goto` as well.
- Validation:
  - Live REST test: create secret → navigate to share link → measure response time and confirm `NavigationStatus::Loaded`.
- Exit condition: share-link navigation returns within 5 seconds.

### CP-3: Add `wait_for_selector` and `wait_for_text` actions
- Audit IDs: G3, Q1, N1.
- Expected changes:
  - `types.rs`: add `WaitForSelector { selector, timeout_ms }` and `WaitForText { text, timeout_ms }` to both `BrowserAction` and `BrowserDecisionAction`.
  - `parser.rs`: validate selectors/text and timeout range; allow new waits inside scripts.
  - `actions.rs`: map them to `SidecarAction` with `capture_after: false`, `wait_for_stability: false`.
  - `prompt.rs`: update JSON schema and system prompt.
  - `policy.rs`, `recovery.rs`: add action kind / signature cases.
  - `test_support.rs`: implement fake waits.
  - `docker/chrome-agent-sidecar.py`: map `wait_for_selector`/`wait_for_text` to `{"cmd":"wait","what":"selector","pattern":...,"timeout":...}` and remove the special-case `wait` sleep.
- Validation:
  - `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- agent::providers::browser_live`.
  - Live REST test: wait for a known selector/text on `https://example.com`.
- Exit condition: model can emit waits, sidecar executes them, tests pass.

### CP-4: Capture network response bodies in the CDP listener
- Audit IDs: G4, Q1, N1.
- Expected changes:
  - `docker/chrome-agent-sidecar.py`: extend `CDPListener` to handle command responses; call `Network.getResponseBody` for XHR/fetch and failed requests after `Network.responseReceived`; store the decoded body in the request entry.
  - `types.rs`: add optional `body` field to `NetworkItem` with serde default.
  - `build_network_debug_payload`: respect `include_bodies` and include the stored body when requested.
  - `summarize_network`: omit body from observation summaries to keep prompts small.
- Validation:
  - Unit tests for `build_network_debug_payload` with and without `include_bodies`.
  - Live REST test: submit OTS form, call `/debug/network?include_bodies=true&filter=xhr`, confirm `POST /api/create` body is present.
- Exit condition: debug endpoint includes bodies, summaries do not.

### CP-5: Deduplicate retained screenshot artifact descriptors
- Audit IDs: G5, Q1.
- Expected changes:
  - `crates/oxide-agent-core/src/agent/providers/browser_live/session.rs`: in `record_observation`, before pushing to `retained_artifacts`, check for an existing artifact with the same URI and replace it instead of appending a duplicate.
- Validation:
  - `cargo test -p oxide-agent-core ...`.
  - Unit test for repeated retained records.
- Exit condition: no duplicate URIs in `retained_artifacts()`.

### CP-6: Final end-to-end OTS verification
- Audit IDs: G1, G2, G3, G4, Q2.
- Expected changes:
  - Update this goal doc with final evidence.
- Validation:
  - Full live run: create session → navigate to OTS → fill `#createSecretData` → click submit → extract share link → navigate to share link → click reveal → confirm original text.
  - Record timings, URL correctness, and POST body capture.
- Exit condition: full flow succeeds and every audit item is verified.

## Validation Contract

- Static checks:
  - `python -m py_compile docker/chrome-agent-sidecar.py`
  - `cargo fmt --all -- --check`
  - `cargo clippy -p oxide-agent-core -p oxide-agent-web-contracts -p oxide-agent-web-ui --no-default-features --features profile-full --all-targets -- -D warnings`
- Tests:
  - `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- agent::providers::browser_live`
  - `cargo test -p oxide-agent-web-ui`
  - `cargo test -p oxide-agent-web-contracts`
  - `docker exec oxide_chrome_agent_sidecar chrome-agent-sidecar --self-test`
- Runtime/manual verification:
  - Live REST calls against the running sidecar on `https://ots.bash.md/` and `https://example.com`.
- Artifact verification:
  - Goal doc updated with evidence for each audit item.
- Done when: all audit items verified and the full OTS flow passes.

## Decisions

- 2026-06-17: Keep the existing persistent `chrome-agent --json pipe` architecture; improvements are layered inside the sidecar and the Rust provider, not a rewrite.
- 2026-06-17: Use CDP `Page.frameNavigated` plus a pre-decision `window.location.href` eval for URL tracking, rather than relying solely on `chrome-agent` command output.
- 2026-06-17: Add new `wait_for_selector`/`wait_for_text` actions instead of overloading the existing `wait` sleep action, so the model can express intent explicitly.
- 2026-06-17: Capture response bodies only for failed/XHR requests and only expose them through the debug endpoint, not in prompt summaries, to protect context-window size.

## Progress Log

- 2026-06-17: RECON complete; plan written.
  - Changed: `docs/goals/2026-06-17-browser-live-spa-improvements.md` created.
  - Evidence: root cause identified in `_handle_goto` (session URL not updated after full navigation) and confirmed CDP listener can track `Page.frameNavigated`; `chrome-agent wait` command supports `selector`/`text` via pipe.
  - Commands: `grep`, `read`, and `docker exec` checks.
  - Next: CP-1 — fix URL tracking.

- 2026-06-17: CP-1 — URL tracking fixed.
  - Changed: `docker/chrome-agent-sidecar.py` `_handle_goto` sets `session["url"]`/`session["title"]` before `build_observation`; `CDPListener` enables `Page` domain and updates `session["url"]` on `Page.frameNavigated`; added `_refresh_session_url_from_location` via `window.location.href` to make the hash fast-path decision reliable.
  - Evidence: `python3 -m py_compile`, `cargo fmt`, `cargo clippy`, `cargo test` (82/11/10), sidecar self-test all pass. Live REST test: `https://example.com/` → observation URL `https://example.com/`; `https://ots.bash.md/` → observation URL `https://ots.bash.md/`; `https://ots.bash.md/#test-hash|abc` → `status: loaded` and URL `https://ots.bash.md/#test-hash|abc` with no timeout.
  - Commands: `docker compose -f docker-compose.web.yml up -d --build chrome-agent-sidecar`, `docker exec oxide_chrome_agent_sidecar chrome-agent-sidecar --self-test`, `curl` to `/sessions` and `/sessions/{id}/goto`.
  - Audit IDs updated: G1 pending → verified, G2 pending → verified, Q1 pending → verified, N1 pending → verified.
  - Next: CP-2 — harden SPA hash navigation.

- 2026-06-17: CP-2 — SPA hash navigation hardened.
  - Changed: `docker/chrome-agent-sidecar.py` hash fast path now calls `listener.wait_for_network_idle(timeout=2.0)` and falls back to `chrome-agent wait selector body` before inspecting; full `goto` branch reads `wait_until` from the request and calls `wait_for_network_idle` when `"networkidle"`; the hash branch also refreshes the session URL from the browser location before building the observation.
  - Evidence: `python3 -m py_compile`, `cargo fmt`, `cargo clippy`, `cargo test` (82/11/10), sidecar self-test all pass. Live REST test created a secret on `https://ots.bash.md/` and navigated to the generated share link; the hash navigation returned in 385ms with `status: loaded` and correct URL.
  - Commands: same static checks plus `curl` to `/sessions/{id}/action` and `/sessions/{id}/goto`.
  - Audit IDs updated: G2 verified (evidence strengthened), Q1 verified (extended).
  - Next: CP-3 — add `wait_for_selector` and `wait_for_text` actions.

- 2026-06-17: CP-3 — DOM wait actions implemented.
  - Changed: `crates/oxide-agent-core/src/agent/providers/browser_live/types.rs` added `WaitForSelector` and `WaitForText` to `BrowserAction` and `BrowserDecisionAction`; `parser.rs` validates them and allows them inside scripts; `actions.rs` maps them to `SidecarAction` with `capture_after: false`/`wait_for_stability: false`; `prompt.rs` updated schema and system prompt; `policy.rs` and `recovery.rs` updated; `test_support.rs` added fake waits; `docker/chrome-agent-sidecar.py` maps waits to `chrome-agent wait selector/text` and wraps string errors in `SidecarErrorBody`.
  - Evidence: `cargo test -p oxide-agent-core ...` 87 pass (up from 82), `cargo test -p oxide-agent-web-ui` 11 pass, `cargo test -p oxide-agent-web-contracts` 10 pass, `python3 -m py_compile`, `cargo fmt`, `cargo clippy`, sidecar self-test pass. Live REST test: `wait_for_selector h1` on `https://example.com` succeeded; `wait_for_text "Example Domain"` succeeded; `wait_for_selector #missing-element` failed with structured error.
  - Commands: static checks + `curl` to `/sessions/{id}/action`.
  - Audit IDs updated: G3 pending → verified, Q1 verified (extended).
  - Next: CP-4 — capture network response bodies in the CDP listener.

## Risks and Blockers

- CDP `Page.frameNavigated` may fire frequently or with `about:blank` frames.
  - Impact: URL flapping.
  - Mitigation: filter on `frameId` matching the main page target and only update when the new URL is non-empty and `http(s)`.
- `Network.getResponseBody` may fail for long-polling or cached requests.
  - Impact: missing body for some requests.
  - Mitigation: only attempt for XHR/fetch/failed; ignore failures silently.
- `chrome-agent wait` command syntax may not match the pipe JSON assumptions.
  - Impact: wait actions fail.
  - Mitigation: verified with `docker exec ... timeout 15 bash -c 'echo ... | chrome-agent --json pipe'`; CP-2 also verified the fallback `wait selector body` in the hash navigation path.
- Adding `body` to `NetworkItem` may break consumers that expect a fixed schema.
  - Impact: compile/runtime errors in web UI or Telegram transport.
  - Mitigation: use `#[serde(default, skip_serializing_if = "Option::is_none")]` and add the field to shared contracts.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:

## User-Facing Progress Updates

* Current checkpoint: CP-3 complete; starting CP-4.
* What changed: added `wait_for_selector` and `wait_for_text` actions across the Rust provider and sidecar; the model can now ask the browser to wait for a DOM selector or visible text before the next step.
* What was verified: live REST tests on `https://example.com` showed both waits succeed on real elements and fail with a structured error on missing elements; 87 Rust browser_live tests pass.
* Which audit IDs moved: G3 pending → verified, Q1 verified (extended).
* What remains: CP-4 through CP-6.
* Whether anything is blocked: not blocked.
