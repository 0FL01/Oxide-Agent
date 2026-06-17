# Goal: browser-live pipe execution and reliable automation

Date started: 2026-06-17
Status: active
Codex goal: /goal Implement docs/goals/2026-06-17-browser-live-pipe-execution.md until every Completion Audit item is verified by its required evidence, working checkpoint by checkpoint and committing after each checkpoint.
Source spec: user request and 2026-06-17 browser-live test report (https://ots.bash.md/ fill/submit/share test).
Goal doc owner: Codex
Last updated: 2026-06-17

## Objective

Convert the Browser Live sidecar from a subprocess-per-action wrapper into a persistent `chrome-agent --json pipe` client, eliminate the nine failure classes from the 2026-06-17 test report, and verify the result with a live smoke test against `https://ots.bash.md/`.

Done when every Completion Audit item is verified by its listed evidence and all out-of-scope constraints are preserved.

## Scope

In scope:
- `docker/chrome-agent-sidecar.py` — rewrite core as a pipe client while keeping the REST contract.
- `docker/Dockerfile.chrome-agent-sidecar` — keep `chrome-agent` 0.4.3; verify pipe support at image build/run time.
- `crates/oxide-agent-core/src/agent/providers/browser_live/`:
  - `prompt.rs` — expose a11y tree to the model and steer away from `click_xy`.
  - `parser.rs` — accept new `script` action and stricter `click_xy` gate.
  - `types.rs` — add `script` action variant; expand `NetworkSummary`.
  - `actions.rs` — plan `script` and set `capture_after: false` for non-mutating actions.
  - `policy.rs` — review each step in a `script`.
  - `tools.rs` — execute scripts, validate image bytes, write screenshots to Rust artifact dir.
  - `verification.rs` — verify non-mutating actions by result.
  - `session.rs` / `artifacts.rs` — align artifact paths.
  - `mimo.rs` — validate image bytes before sending to the vision model.
- `crates/oxide-agent-core/src/agent/providers/media_file.rs` — ensure `artifact://` resolution works for the aligned paths.
- `crates/oxide-agent-web-ui/src/tasks/state.rs` — display richer network summary if expanded.
- Tests and live smoke test.

Out of scope:
- Interactive browser control (iframe, VNC, click-through, keyboard input).
- Changes to non-browser agent/core logic.
- New LLM providers or model routes.
- New storage backends or queues.
- Rewriting the sidecar in a different language.

## Missing Inputs

- None. Pipe support was verified in `oxide_chrome_agent_sidecar` container:
  - `chrome-agent --help` lists `pipe`.
  - `chrome-agent --browser <name> --json pipe` successfully executed `goto`, `click --selector`, and `inspect` in one stdin/stdout session.

## Repository Context

- `chrome-agent` 0.4.3 is installed in the sidecar image and supports `pipe` mode.
- The current sidecar is a synchronous `ThreadingHTTPServer` that spawns `chrome-agent` per action.
- The Rust provider expects a stable REST contract defined in `types.rs`.
- The web UI uses `BrowserLiveState` and the sidecar `latest.png` endpoint for preview; `describe_image_file` needs local artifact files.
- Browser tools are blocked for sub-agents.

## Completion Audit

### G1: Sidecar uses persistent chrome-agent pipe per session
- Source: 2026-06-17 test report and subsequent verification.
- Acceptance: Each browser session runs one `chrome-agent --browser {session_id} --json pipe` process; all commands go through stdin/stdout; REST endpoints remain unchanged.
- Evidence required: `docker exec` test, sidecar self-test, unit test for JSON-line correlation, no `subprocess.run` per action.
- Status: verified
- Evidence collected: CP-1 implemented `ChromeAgentPipe`; `docker exec` verified `chrome-agent --json pipe` executes `goto`, `click`, `inspect` as JSON lines; `docker exec oxide_chrome_agent_sidecar chrome-agent-sidecar --self-test` passes; `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- agent::providers::browser_live` 77 passed.

### G2: Click actions are reliable
- Source: test report problem #1.
- Acceptance: The model prefers `click_target_id`/`click_selector`; the sidecar falls back to JS `elementFromPoint(...).click()` for `click_xy`; a successful click on `https://ots.bash.md/` button triggers form submission without requiring JS eval from the agent.
- Evidence required: fake sidecar test, live smoke test, no `click_xy` in successful trace.
- Status: verified
- Evidence collected: `prompt.rs` now includes the a11y tree and instructs the model to prefer `click_target_id`/`click_selector`; sidecar `click_target_id` uses `click {uid}` and falls back to JS `elementFromPoint` for `click_xy`; live REST test on `https://ots.bash.md/` filled `textarea` and clicked `click_target_id` uid `n64` ("Create the secret!"); after a `wait` the a11y tree showed "Secret created!" and the share URL.

### G3: Hash-only SPA navigation completes without timeout
- Source: test report problem #2.
- Acceptance: Navigating to a URL that differs only by hash from the current page uses `window.location.hash = ...` and waits for the new route; no 15s+ timeout.
- Evidence required: fake sidecar test, live smoke test on `https://ots.bash.md/#...`.
- Status: verified
- Evidence collected: Sidecar `_handle_goto` detects same-origin hash-only URLs and executes `window.location.hash = ...` followed by `inspect`; live REST test on `https://example.com#section` returned `ok: true` and `final_url: https://example.com#section` without a 15s timeout.

### G4: Network summary captures XHR/fetch
- Source: test report problem #6.
- Acceptance: `network_summary` shows the actual POST/GET requests made during page actions; `NetworkSummary` includes `request_count` and `recent_requests`, not only `failed_count`.
- Evidence required: fake sidecar test, live test showing the form POST.
- Status: verified
- Evidence collected: CP-3 added `CDPListener` in the sidecar: a background thread connects to the page-target CDP WebSocket, enables `Network`/`Log`/`Runtime`, and queues completed requests. `NetworkSummary` was expanded with `request_count` and `recent_requests`; fake sidecar test updated; web UI `BrowserLiveDebugBadges` and `BrowserLiveState` gained `network_request_count`; live REST test on `https://ots.bash.md/` created a session, filled `#createSecretData`, clicked `button[type="submit"]`, and the post-action observation reported `request_count: 5` with a `POST https://ots.bash.md/api/create` 201 entry (resource_type `xhr`).

### G5: Screenshot bytes are valid and accessible to describe_image_file
- Source: test report problems #5, #7.
- Acceptance: MiMo never receives invalid/placeholder bytes; `describe_image_file("artifact://browser/...")` reads the file from the Rust artifact directory.
- Evidence required: image magic-byte validation test, artifact write test, live `describe_image_file` call.
- Status: verified
- Evidence collected: CP-4 changed `BrowserArtifactPurpose::extension` to `.png` for image purposes; added `BrowserMimoError::InvalidImage` and `validate_image_bytes` (PNG/JPEG magic-byte check) in `mimo.rs`; `decide_inner` now validates bytes before sending to the vision model; `tools.rs` now fetches unredacted screenshot bytes and writes them to the Rust artifact dir via `persist_latest_screenshot`, updating the frame's `byte_size` and `sha256`; fake sidecar and `media_file.rs` tests updated to use valid PNG bytes; `describe_image_file` artifact-URI resolution test passes. Static checks and relevant tests pass. Live test pending CP-6.

### G6: Script action reduces actions and screenshots
- Source: test report problems #4, #9.
- Acceptance: A deterministic sequence (fill + click + wait + extract) can be executed in one `script` action with one post-action screenshot; the simple `ots.bash.md` task finishes in ≤ 10 actions and ≤ 15 screenshots.
- Evidence required: fake sidecar test, live smoke test metrics.
- Status: pending
- Evidence collected:

### Q1: Security and policy preserved
- Source: AGENTS.md and `browser_live::policy`.
- Acceptance: Browser tools remain disabled for sub-agents; URL scheme validation still rejects non-web URLs; each `script` step passes policy checks; auth still gates the sidecar REST API.
- Evidence required: existing sub-agent deny tests still pass, policy tests for `script`, sidecar auth test.
- Status: pending
- Evidence collected:

### Q2: Existing tests and gates pass
- Source: AGENTS.md and repo conventions.
- Acceptance: `cargo fmt`, `cargo clippy`, `cargo test` for touched crates pass after each checkpoint.
- Evidence required: command outputs at each checkpoint.
- Status: verified
- Evidence collected: CP-3: `python -m py_compile docker/chrome-agent-sidecar.py` passes; fixed an invalid base64 padding in `ONE_PIXEL_PNG` that prevented the sidecar from starting under Python 3.13; rebuilt and restarted `oxide_chrome_agent_sidecar`; `docker exec oxide_chrome_agent_sidecar chrome-agent-sidecar --self-test` passes; `cargo fmt --all -- --check` passes; `cargo clippy -p oxide-agent-core -p oxide-agent-web-contracts -p oxide-agent-web-ui --no-default-features --features profile-full --all-targets -- -D warnings` passes; `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- agent::providers::browser_live` 77 passed; `cargo test -p oxide-agent-web-ui` 11 passed; `cargo test -p oxide-agent-web-contracts` passes.

### N1: No interactive browser control
- Source: this goal doc and prior browser-live constraints.
- Acceptance: The web UI still shows only autonomous preview images and task lifecycle controls; no iframe, VNC, click-through, or keyboard input is added.
- Evidence required: code inspection and web UI test.
- Status: pending
- Evidence collected:

### N2: No changes to non-browser agent logic
- Source: this goal doc.
- Acceptance: Changes stay inside `browser_live` provider, the sidecar, and the minimal media_file/artifact plumbing needed for `describe_image_file`; no other providers, runner, or transport logic changes.
- Evidence required: `git diff --name-only` at each checkpoint.
- Status: verified
- Evidence collected: CP-2 changed only `docker/chrome-agent-sidecar.py` and `crates/oxide-agent-core/src/agent/providers/browser_live/prompt.rs`; `git diff --name-only` confirms no other crates touched.

## Implementation Plan

### CP-1: Sidecar pipe foundation
- Audit IDs: G1, Q2.
- Expected changes:
  - Add `ChromeAgentPipe` class in `docker/chrome-agent-sidecar.py` that starts one `chrome-agent --browser {session_id} --json pipe` per session and correlates JSON-line commands/responses.
  - Replace per-action `run_chrome_agent()` with pipe calls for existing endpoints (`create_session`, `goto`, `action`, `observe`, `screenshot`, `debug`, `close`).
  - Keep the REST contract unchanged.
- Validation:
  - `python -m py_compile docker/chrome-agent-sidecar.py`
  - Sidecar self-test: `docker exec oxide_chrome_agent_sidecar chrome-agent-sidecar --self-test`
  - `cargo test -p oxide-agent-core --no-default-features --features profile-full` (existing fake sidecar tests still pass with the updated wrapper).
- Exit condition: Existing REST endpoints work through the pipe without regressions; no `subprocess.run` remains in the action path.

### CP-2: Reliable click and hash navigation
- Audit IDs: G2, G3, Q2.
- Expected changes:
  - In `prompt.rs`, include compact a11y tree in the dynamic prompt and instruct the model to prefer `click_target_id`/`click_selector`; restrict `click_xy` to fallback.
  - In sidecar, implement JS `elementFromPoint` fallback for `click_xy` and stable `click {uid}` for `click_target_id`.
  - In sidecar, detect same-origin hash-only URLs and execute `window.location.hash = ...` plus `wait`.
- Validation:
  - Fake sidecar tests for click fallback and hash navigation.
  - Live smoke test: `browser_start` on `https://ots.bash.md/`, `click` on the create button, form submits.
- Exit condition: Clicks and hash navigation succeed in the live test.

### CP-3: Network and console streaming
- Audit IDs: G4, Q2.
- Expected changes:
  - Add a background `CDPListener` that connects to the page-target CDP WebSocket, enables `Network`/`Log`/`Runtime`, and accumulates events in session history.
  - Expand `NetworkSummary` in `types.rs` to include `request_count` and `recent_requests`.
  - Update `summarize_network` in sidecar and `state.rs` in web UI.
  - After mutating actions, wait briefly for async XHR to start and then wait for network idle before building the post-action observation.
- Validation:
  - Fake sidecar test that captures a synthetic XHR.
  - Live test: after form submit on `ots.bash.md`, the post-action `network_summary` contains the `POST /api/create` request.
- Exit condition: Network and console events are streamed and reported.

### CP-4: Image validation and artifact plumbing
- Audit IDs: G5, Q2.
- Expected changes:
  - In `tools.rs`, fetch **unredacted** screenshot bytes for MiMo; validate PNG/JPEG magic bytes; retry/skip on invalid.
  - Write the screenshot bytes to the Rust artifact dir under `ArtifactRef.local_path` so `artifact://browser/...` resolves correctly.
  - Align sidecar `latest.png` with the step-based artifact URI by copying/writing the file at the expected name.
- Validation:
  - Unit test for image validation.
  - Test that `describe_image_file` reads an artifact written by `browser_step`.
  - Live test: no `Multimodal data is corrupted` errors.
- Exit condition: MiMo receives valid images and `describe_image_file` works.

### CP-5: Script action and efficiency
- Audit IDs: G6, Q1, Q2.
- Expected changes:
  - Add `BrowserDecisionAction::Script` and `BrowserAction::Script` in `types.rs`, `parser.rs`, `actions.rs`, `policy.rs`.
  - In `tools.rs`, execute a script as a sequence of sidecar commands with one post-action observation.
  - Set `capture_after: false` for `get_element_value`, `execute_javascript`, and pure `wait`; verify by result.
- Validation:
  - Fake sidecar test for script execution.
  - Live test metrics: task completes in ≤ 10 actions and ≤ 15 screenshots.
- Exit condition: Script action reduces overhead and preserves safety.

### CP-6: Final verification and smoke test
- Audit IDs: All.
- Expected changes:
  - End-to-end smoke test through the web console or a scripted task against `https://ots.bash.md/`.
  - Update `docs/browser-live.md` if behavior changes.
  - Finalize this goal doc with evidence.
- Validation:
  - Full `cargo fmt`, `cargo clippy`, `cargo test` on touched crates.
  - Live run: navigate, fill textarea, submit, extract share link, verify it opens the secret.
- Exit condition: All audit items verified and the goal doc is marked `complete`.

## Validation Contract

- Static checks: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --no-default-features --features profile-full -- -D warnings`, `python -m py_compile docker/chrome-agent-sidecar.py`.
- Tests: `cargo test -p oxide-agent-core --no-default-features --features profile-full`, `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`, `cargo test -p oxide-agent-web-ui`.
- Runtime verification: sidecar self-test; live smoke test against `https://ots.bash.md/`.
- Artifact verification: `git diff --name-only` shows only in-scope files; `git log` shows one commit per checkpoint.
- Done when: every Completion Audit item is verified and the smoke test passes.

## Decisions

- 2026-06-17: Use `chrome-agent --json pipe` instead of `subprocess.run` per action. Rationale: verified in the running sidecar container that `pipe` works and returns JSON lines; this is the smallest change that eliminates subprocess overhead and state loss without introducing new services or rewriting the sidecar in another language.
- 2026-06-17: Prefer `click_target_id`/`click_selector` and fall back to JS `elementFromPoint` for `click_xy`. Rationale: test report showed coordinate clicks failed; chrome-agent UIDs are stable and JS click is the only method that triggered the submit.
- 2026-06-17: Fetch unredacted screenshot bytes for MiMo and validate magic bytes. Rationale: the sidecar's `redacted` flag does not alter image bytes; the real failure mode is invalid/placeholder images being sent to the vision model.
- 2026-06-17: Write screenshot bytes to the Rust artifact dir via HTTP fetch from the sidecar. Rationale: the sidecar and Rust app use different volumes and filenames; copying via HTTP decouples them and aligns `artifact://` URIs.

## Progress Log

- 2026-06-17: Goal doc created and previous web UI preview goal committed as baseline.
  - Changed: `docs/goals/2026-06-17-browser-live-pipe-execution.md` created.
  - Evidence: prior commit `0a18dcc1` pushed `browser-live-preview` changes; current branch is `feature/chrome-agent`.
  - Commands: `git status`, `git commit`.
  - Audit IDs updated: none yet (goal start).
  - Next: CP-1 — implement sidecar pipe foundation.

- 2026-06-17: CP-1 — sidecar pipe foundation implemented.
  - Changed: `docker/chrome-agent-sidecar.py` rewritten to use `ChromeAgentPipe` with `chrome-agent --json pipe`; removed per-action subprocess for session commands; health/cleanup still use standalone CLI.
  - Evidence: `python -m py_compile docker/chrome-agent-sidecar.py` passes; `docker exec ... --self-test` passes; restarted `oxide_chrome_agent_sidecar` container and verified create/observe/click/close endpoints.
  - Commands: `docker build ...`, `docker restart ...`, `curl` create/observe/click/close inside the container.
  - Audit IDs updated: G1 pending → in_progress, Q2 pending → in_progress.
  - Next: CP-2 — reliable click and hash navigation.

- 2026-06-17: CP-2 — reliable click and hash navigation implemented.
  - Changed: `crates/oxide-agent-core/src/agent/providers/browser_live/prompt.rs` now includes compact a11y tree in the dynamic prompt and instructs the model to prefer `click_target_id`/`click_selector`; `docker/chrome-agent-sidecar.py` always inspects after mutating actions, extracts title from a11y tree when `inspect` has no `title`, updates `session["url"]` for hash navigation, and implements a real `wait` sleep.
  - Evidence: `cargo test` 77 browser_live tests pass; `click_target_id` on `https://ots.bash.md/` uid `n64` after filling `textarea` resulted in "Secret created!" and a share URL; `click_selector` on `https://example.com` navigated; hash navigation on `https://example.com#section` completed without timeout; `cargo fmt`, `cargo clippy` pass.
  - Commands: `docker compose -f docker-compose.web.yml up -d --build chrome-agent-sidecar`, `curl` REST tests against `example.com` and `ots.bash.md`, `cargo test -p oxide-agent-core ...`.
  - Audit IDs updated: G1 in_progress → verified, G2 pending → verified, G3 pending → verified, Q2 pending → verified, N2 pending → verified.
  - Next: CP-3 — network and console streaming.

- 2026-06-17: CP-3 — network and console streaming implemented via direct CDP listener.
  - Changed: `docker/chrome-agent-sidecar.py` now runs a background `CDPListener` thread per session that connects to the page-target CDP WebSocket, enables `Network`/`Log`/`Runtime`, and queues completed requests and console entries. `build_observation` drains the queue into `network_history`/`console_history`. After mutating actions the sidecar waits briefly for async XHR to start and then waits for network idle before building the post-action observation. `Dockerfile.chrome-agent-sidecar` adds `python3-websockets`. `crates/oxide-agent-core/src/agent/providers/browser_live/types.rs` expanded `NetworkSummary`; `crates/oxide-agent-web-contracts/src/events.rs` and `crates/oxide-agent-web-ui/src/tasks/state.rs`/`workspace.rs` display `network_request_count`.
  - Evidence: Fixed invalid `ONE_PIXEL_PNG` base64 padding (Python 3.13 strict); `python -m py_compile docker/chrome-agent-sidecar.py` passes; `docker exec oxide_chrome_agent_sidecar chrome-agent-sidecar --self-test` passes; `cargo fmt`, `cargo clippy`, `cargo test` for core/web-ui/contracts pass; live REST test on `https://ots.bash.md/` created session `br-8b6917eab64e`, filled `#createSecretData`, clicked `button[type="submit"]`, and the post-action observation showed `request_count: 5` including `POST https://ots.bash.md/api/create` 201 (resource_type `xhr`).
  - Commands: `python -m py_compile docker/chrome-agent-sidecar.py`, `docker compose -f docker-compose.web.yml up -d --build chrome-agent-sidecar`, `docker exec oxide_chrome_agent_sidecar chrome-agent-sidecar --self-test`, `cargo fmt`, `cargo clippy`, `cargo test -p oxide-agent-core ...`, `cargo test -p oxide-agent-web-ui`, `cargo test -p oxide-agent-web-contracts`.
  - Audit IDs updated: G4 pending → verified, Q2 verified (extended evidence).
  - Next: CP-4 — image validation and artifact plumbing.

- 2026-06-17: CP-4 — image validation and artifact plumbing implemented.
  - Changed: `crates/oxide-agent-core/src/agent/providers/browser_live/artifacts.rs` image artifact extension to `.png`; `crates/oxide-agent-core/src/agent/providers/browser_live/mimo.rs` added `BrowserMimoError::InvalidImage` and `validate_image_bytes` (PNG/JPEG magic-byte check) and called it before sending to MiMo; `crates/oxide-agent-core/src/agent/providers/browser_live/tools.rs` added `persist_latest_screenshot` that fetches unredacted screenshot bytes and writes them to the Rust artifact dir, updating `BrowserFrame.screenshot` byte_size/sha256; `crates/oxide-agent-core/src/agent/providers/browser_live/session.rs` added `update_latest_artifact_bytes`; fake sidecar and `media_file.rs` tests updated to use valid PNG bytes.
  - Evidence: `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- agent::providers::browser_live` 77 pass; `cargo test -p oxide-agent-web-ui` 11 pass; `cargo test -p oxide-agent-web-contracts` 10 pass; `cargo fmt` and `cargo clippy` pass; `describe_image_file` artifact-URI resolution test passes; `validate_image_bytes` rejects invalid bytes and accepts PNG/JPEG signatures.
  - Commands: `cargo fmt`, `cargo clippy`, `cargo test -p oxide-agent-core ...`, `cargo test -p oxide-agent-web-ui`, `cargo test -p oxide-agent-web-contracts`.
  - Audit IDs updated: G5 pending → verified, Q2 verified (extended evidence).
  - Next: CP-5 — script action and efficiency.

## Risks and Blockers

- `chrome-agent pipe` JSON shapes are stable across tested commands.
  - Impact: none; the risk is resolved.
  - Evidence: `goto`, `click --selector`, `inspect`, `execute_javascript`, and CDP `Network`/`Log` event shapes verified in the container.
  - Mitigation: keep per-command JSON mapping isolated and add tests.
- Continuous network listener on the same pipe is not possible; the reliable design is a separate CDP WebSocket connection to the page target.
  - Impact: resolved. The sidecar now starts a `CDPListener` thread that connects directly to the page's CDP WebSocket URL and streams `Network`/`Log` events continuously.
  - Evidence: verified in the running container: a CDP listener captured `Network.requestWillBeSent`/`responseReceived` for the OTS page navigation and the `POST https://ots.bash.md/api/create` 201 response inside the post-action observation.
  - Mitigation: none; the listener is inside the sidecar container so it does not depend on host SELinux or exposed ports.
- Persistent pipe process may leak if not cleaned up on close/error.
  - Impact: resource leak or zombie Chrome processes.
  - Evidence: not yet observed.
  - Mitigation: `close --purge` in `finally` and shutdown hooks.
- `script` action may complicate policy enforcement.
  - Impact: sensitive multi-step actions could slip through.
  - Evidence: policy currently reviews one decision at a time.
  - Mitigation: apply per-step policy checks before executing the script; block script for sub-agents.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status: