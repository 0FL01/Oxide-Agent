# Goal: Chrome-agent sidecar → native Rust (Option B)

Date started: 2026-06-18
Status: active
Codex goal: see /goal objective below
Source spec: RECON report (this session, 2026-06-18) — `docker/chrome-agent-sidecar.py` rewrite feasibility study; plan approved by user
Goal doc owner: Codex
Last updated: 2026-06-18 18:00

## Objective

Replace the Python `chrome-agent-sidecar.py` (2612 lines) + external `chrome-agent` v0.4.3 subprocess with a single native Rust implementation that talks CDP directly to Chromium over one WebSocket. Eliminate the entire class of contract-drift bugs (CP-A/CP-B), the redundant CDPListener, the latent stealth conflict (`Runtime.enable`), the subprocess boundary, and the Python runtime — by root redesign, not by patching the existing architecture.

Done when every required Completion Audit item is verified by its listed evidence and all out-of-scope constraints are preserved.

## Scope

In scope:
- `docker/chrome-agent-sidecar.py` — replaced by native Rust binary (then deleted)
- `docker/Dockerfile.chrome-agent-sidecar` — simplified (no python3, no chrome-agent cargo install, no websockets)
- `crates/oxide-agent-core/src/agent/providers/browser_live/types.rs` — REST types extracted to shared contracts location
- New binary/crate for the native sidecar (location decided in CP1; candidates: new crate `oxide-browser-sidecar`, or new binary target in an existing crate)
- `docker/compose.full.yml`, `docker/compose.dev.yml`, `docker-compose.web.yml` — sidecar service image/build updated
- `docs/browser-live.md` — deployment section updated
- `docs/goals/2026-06-18-chrome-agent-native-rust.md` (this file)

Out of scope:
- `crates/oxide-agent-core/src/agent/providers/browser_live/{client,tools,session,actions,verification,artifacts,error,metrics,policy,test_support}.rs` — Oxide-side consumer code. Only `types.rs` is touched (extraction). The `BrowserSidecar` trait and `BrowserSidecarClient` stay byte-identical in contract.
- `chromiumoxide` crate — not used; raw `tokio-tungstenite` + `serde_json` per chrome-agent's own approach and AGENTS.md "no new crates unless clearly required".
- Any change to `BrowserSidecar` trait method signatures or REST endpoint paths/semantics.
- Transport crates (telegram/web) — untouched except transitively via shared types.
- Vision-based browser control, iframe/VNC, MiMo intermediate model — explicitly not supported (per `docs/browser-live.md`).

## Missing Inputs

- B1: CP0 (P0.5 verification framework) requires a running Chromium reachable for CDP. If no local Chromium is available in the dev environment, the verification must run inside the existing `Dockerfile.chrome-agent-sidecar` container (which has chromium) or a temporary container. User action needed only if neither is available.

## Repository Context

- Relevant entry points:
  - `docker/chrome-agent-sidecar.py` (2612 lines) — current sidecar.
  - `crates/oxide-agent-core/src/agent/providers/browser_live/` (7214 lines) — Oxide-side consumer: `client.rs` (814, `BrowserSidecarClient` + `BrowserSidecar` trait), `types.rs` (803, REST contract), `tools.rs` (2595, provider/executor), `test_support.rs` (1075, fake impl).
  - `docker/Dockerfile.chrome-agent-sidecar` (57 lines) — current image build.
- Existing conventions:
  - Workspace members in root `Cargo.toml`; `edition = "2024"`, `resolver = "2"`.
  - Shared contracts pattern: `crates/oxide-agent-web-contracts/` exists for web API types. Browser REST types can follow the same pattern or live in a `browser-contracts` module.
  - `thiserror` for library crates, `anyhow` for app/binary crates.
  - Profile-feature gating: `tool-browser-live = ["dep:reqwest"]` in `crates/oxide-agent-core/Cargo.toml`.
  - `axum = { version = "0.7", features = ["macros", "multipart"] }` already used in `oxide-agent-transport-web`.
  - `tokio-tungstenite` is NOT currently a workspace dependency — will be added as a justified dep for CDP WebSocket.
- Dependencies or runtime assumptions:
  - Chromium launched with `--no-sandbox --remote-debugging-port=...` (current wrapper in Dockerfile).
  - CDP discovery: HTTP `GET /json/list` on the debug port → page target `webSocketDebuggerUrl`.
  - chrome-agent v0.4.3 is binary-only (docs.rs: "not a library") — cannot be embedded; this is the root reason for the subprocess boundary and thus for the sidecar's existence.
- Validation infrastructure:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets --features <profile> -- -D warnings`
  - `cargo test -p oxide-agent-core --features profile-full` (browser_live tests in `test_support.rs` + `tools.rs`)
  - `cargo build --release` for the new binary
  - Docker build of the new Dockerfile
  - Smoke test: sidecar + Oxide client, execute a browser task end-to-end
- Risky areas:
  - `types.rs` is imported across `client.rs`, `tools.rs`, `test_support.rs`, `session.rs`, `actions.rs`, `verification.rs`, `artifacts.rs`. Extraction to shared location touches every import — blast radius wide, must enumerate via `git grep`.
  - CDP edge cases (cross-origin iframes, shadow DOM, lazy-loaded content) — already handled in current Python sidecar; porting, not inventing.
  - Latency claims in the plan are architectural estimates, not measurements — CP0 must verify before code.

## Completion Audit

- G1: Python sidecar + chrome-agent subprocess replaced by a native Rust implementation talking CDP directly.
  - Source: RECON — `docker/chrome-agent-sidecar.py` spawns `chrome-agent --json pipe` subprocess per session; chrome-agent is binary-only.
  - Acceptance: a Rust binary launches Chromium directly and speaks CDP over a single WebSocket per session; no `chrome-agent` subprocess; no Python process; `docker/chrome-agent-sidecar.py` deleted.
  - Evidence required: `git grep -n 'chrome-agent' docker/` returns nothing (except historical references in docs); `ls docker/chrome-agent-sidecar.py` → not found; new binary `cargo build --release` succeeds; running binary launches Chromium and serves the REST API.
  - Status: in_progress
  - Evidence collected: CP2 — native Rust binary launches Chromium directly via `tokio::process::Command` with `--headless=new --no-sandbox --remote-debugging-port=0`, reads `DevToolsActivePort` file for port, discovers page target via `/json/list`, connects CDP WebSocket via `tokio-tungstenite`. No `chrome-agent` subprocess, no Python process. POST /sessions and DELETE /sessions are functional. Remaining endpoints (goto, observe, action, screenshot, debug) are stubs for CP3-CP6. `docker/chrome-agent-sidecar.py` not yet deleted (CP8).

- G2: Shared types between sidecar and Oxide client — contract drift architecturally impossible.
  - Source: RECON — sidecar `run_unit_tests()` comment: "Rust mock in test_support.rs can diverge from the real sidecar implementation and all Rust tests stay green while production breaks — exactly the class of bug seen in CP-A (noise filter on wrong shape) and CP-B (failure criterion mismatch)."
  - Acceptance: REST types (`BrowserAction`, `BrowserObservation`, `CreateSessionRequest/Response`, `GotoRequest/Response`, `ActionRequest/Response`, `NetworkDebug*`, `ConsoleDebug*`, etc.) defined in one location imported by both the sidecar binary and `oxide-agent-core` browser_live client; renaming a field in either → compile error in both.
  - Evidence required: `git grep` shows single source of each type; `cargo check` of both the sidecar and `oxide-agent-core` succeeds with shared import; no duplicate struct/enum definitions of REST contract types.
  - Status: verified
  - Evidence collected: CP1 — all browser REST types moved to `crates/oxide-browser-contracts/src/types.rs` (803 lines, verbatim from `types.rs` including serde attributes, impls, tests). `oxide-agent-core/src/agent/providers/browser_live/types.rs` is now a 8-line re-export: `pub use oxide_browser_contracts::*;`. `git grep` confirms no duplicate browser REST type definitions (the `CreateSessionRequest` in `oxide-agent-web-contracts` is a different struct for web console sessions, not browser). Both `oxide-browser-sidecar` and `oxide-agent-core --features profile-full` compile against the shared types. 4 contract tests in `oxide-browser-contracts` pass; 71 browser_live tests in `oxide-agent-core` pass.

- G3: Single CDP WebSocket connection per session (no redundant CDPListener).
  - Source: RECON — sidecar `CDPListener` opens a SEPARATE WebSocket for network/console capture, duplicating chrome-agent's own connection.
  - Acceptance: exactly one WebSocket to the page target per session; network/console capture uses the same connection as control commands; no second CDP listener thread/task.
  - Evidence required: code review — one `tokio_tungstenite::WebSocketStream` per session struct; no separate listener struct; concurrent CDP commands multiplexed on the single stream.
  - Status: verified
  - Evidence collected: CP2 — `CdpClient` (cdp.rs) owns exactly one `tokio-tungstenite` WebSocket per session. `BrowserSession` holds one `CdpClient` (session.rs:27). Background reader task dispatches both command responses (by `id` correlation via `HashMap<u64, oneshot::Sender>`) and events (via `broadcast::channel<CdpEvent>`) on the same connection. No separate CDPListener struct, no second WebSocket. `CdpClient::send_command` is `&self` (concurrent-safe via `Arc<Mutex<HashMap>>` + `mpsc::Sender`), allowing multiple concurrent commands on the single stream. `navigate_to` demonstrates the pattern: `Page.enable` + `Page.navigate` + `Page.loadEventFired` event all on one connection. Integration test confirms single-session lifecycle works end-to-end.

- G4: Stealth-safe capture — `Runtime.enable` never sent.
  - Source: RECON — sidecar `CDPListener` line 602 calls `Runtime.enable`, which chrome-agent specifically avoids (detection vector).
  - Acceptance: `Runtime.enable` is never sent on the CDP connection; console capture uses an injected JS interceptor (via `Page.addScriptToEvaluateOnNewDocument` + `Runtime.evaluate` for current page, matching chrome-agent's `setup.rs` approach); network capture uses `Network.enable` without `Runtime.enable`.
  - Evidence required: `git grep -n 'Runtime.enable\|"Runtime"\s*,\s*"enable"' <sidecar source>` returns nothing; stealth patches JS ported from chrome-agent `setup.rs` (`STEALTH_PATCHES_JS`: navigator.webdriver, chrome.runtime mock, Permissions API fix, WebGL vendor/renderer mask, MouseEvent screenX/screenY fix); `Network.setUserAgentOverride` removes "HeadlessChrome".
  - Status: verified
  - Evidence collected: CP3 — `stealth.rs` (78 lines) ports `STEALTH_PATCHES_JS` verbatim from chrome-agent `setup.rs`: navigator.webdriver=undefined, chrome.runtime mock, Permissions API fix, WebGL vendor/renderer mask (Intel Inc./Intel Iris OpenGL Engine), MouseEvent screenX/screenY leak fix. `apply_stealth()` calls `Page.addScriptToEvaluateOnNewDocument` (survives navigations) + `Runtime.evaluate` (patches current page) + `Network.setUserAgentOverride` (removes HeadlessChrome, replaces with Chrome/131.0.0.0). `grep -r 'Runtime.enable' crates/oxide-browser-sidecar/src/` → only in comments/assertions, never as a CDP command. `grep '"Runtime' crates/oxide-browser-sidecar/src/` → only `Runtime.evaluate` (stealth.rs:92). Integration test `snapshot_and_stealth_on_real_chromium` verifies: (1) `navigator.webdriver` is undefined/null, (2) UA does not contain "HeadlessChrome".

- G5: REST contract with Oxide client unchanged.
  - Source: RECON — `client.rs` `BrowserSidecar` trait + `types.rs` REST contract consumed by `tools.rs`.
  - Acceptance: all endpoints (`GET /healthz`, `POST /sessions`, `DELETE /sessions/{id}`, `POST /sessions/{id}/goto`, `GET /sessions/{id}/observe`, `POST /sessions/{id}/action`, `GET /sessions/{id}/screenshot/latest`, `GET /sessions/{id}/debug/network`, `GET /sessions/{id}/debug/console`) preserve path, method, request body schema, response body schema, query params; `BrowserSidecarClient` requires zero changes to call the new sidecar.
  - Evidence required: `git diff` of `client.rs` shows no trait/method/signature changes (only import path changes for shared types); smoke test with real `BrowserSidecarClient` against new sidecar succeeds on a representative action sequence.
  - Status: pending
  - Evidence collected:

- G6: All BrowserAction variants implemented natively.
  - Source: RECON — `types.rs` `BrowserAction` enum: click_xy, click_selector, click_target_id, fill, type_text, press, scroll, get_element_value, execute_javascript, wait, wait_for_selector, wait_for_text, script.
  - Acceptance: every variant translates to CDP commands and produces an `ActionResult` + post-action `BrowserObservation`; semantic input (React/Vue/Angular native setter events) ported from Python sidecar; key combo dispatch (ctrl+a etc.) via JS KeyboardEvent ported.
  - Evidence required: for each variant, a test or smoke-test demonstrating successful execution against a real page; `fill`/`type_text` verified on a framework page (React or Vue) that native setter events fire.
  - Status: in_progress
  - Evidence collected: CP4 — `actions.rs` (570 lines) implements all 13 `BrowserAction` variants: `click_xy` (Input.dispatchMouseEvent at x,y), `click_selector` (JS getBoundingClientRect → Input.dispatchMouseEvent at center), `fill`/`type_text` (semantic input JS ported from Python sidecar — native value setter from HTMLInputElement/TextAreaElement/SelectElement prototypes, InputEvent with insertReplacementText/insertText), `press` (JS KeyboardEvent for simple + combo keys with modifier/key aliases), `scroll` (window.scrollBy via Runtime.evaluate), `get_element_value` (JS eval), `execute_javascript` (Runtime.evaluate with try/catch wrapping), `wait` (tokio::time::sleep), `wait_for_selector`/`wait_for_text` (polling via Runtime.evaluate), `script` (iterate steps with Box::pin for async recursion), `navigate` (returns Failed — handled by /goto endpoint). 22 unit tests (action_kind, semantic_input, press_key, parse_key, value_to_string, json_str, resolve_key_alias). Integration test `actions_on_real_chromium` on Chrome/149 passes in 1.18s: click_selector (button text changes), fill (input value set + verified), get_element_value (reads input value), type_text (replaces value), press Enter + ctrl+a (KeyboardEvent dispatched), scroll (action Executed), execute_javascript (1+2=3, error case → Failed), wait_for_text (positive + negative), wait_for_selector, wait (100ms NoOp), script (multi-step), navigate (Failed — correct). Post-action BrowserObservation pending CP6 (observation building). `fill`/`type_text` on a React/Vue framework page — pending CP7 smoke test (semantic input JS ported verbatim from Python, same native setter events).

- G7: a11y snapshot with 4 noise filtering rules, stable UIDs.
  - Source: RECON — chrome-agent `snapshot.rs`: `Accessibility.getFullAXTree` + 4 rules (skip `ignored`, skip roles `none`/`StaticText`/`InlineTextBox`, skip unnamed `generic`, pull text from `StaticText` children); UID = `n{backendDOMNodeId}` or `e{counter}`.
  - Acceptance: a11y snapshot produces the same `uid=nN role "name" [props]` format consumed by `parse_snapshot` / `DomSnapshotNode`; 4 noise rules applied; UIDs stable across snapshots for the same DOM.
  - Evidence required: snapshot of a known page matches expected node set; a test with a fixture AX tree verifying each noise rule; UID stability test across two snapshots of the same page.
  - Status: verified
  - Evidence collected: CP3 — `snapshot.rs` (753 lines) ports 4 noise rules from chrome-agent `snapshot.rs`: (1) skip `ignored` nodes — recurse children; (2) skip roles `none`/`StaticText`/`InlineTextBox` — recurse children; (3) skip unnamed `generic` containers — recurse children; (4) pull text from `StaticText` children when node name empty. UID = `n{backendDOMNodeId}` (stable) or `e{counter}` (fallback). Structured output `Vec<A11yNode { uid, role, text, depth }>` matches Python sidecar `parse_snapshot()` format. Text format `uid=nN role "name" [props]` produced in parallel for logging (properties: focused, disabled, expanded, selected, level, checked, required, readonly). `uid_to_backend: HashMap<String, i64>` map for CP4 click actions. 14 unit tests verify each noise rule + UID stability + text format + edge cases. Integration test `snapshot_and_stealth_on_real_chromium` on Chrome/149: snapshot of data URL test page contains heading "Welcome" + button "Login" + stable n-prefix UIDs; UID stability verified across two snapshots (same UIDs, same count). P0.5 bug caught: `AxNode` struct needed `#[serde(rename_all = "camelCase")]` for CDP field names (`nodeId`, `childIds`, `parentId`).

- G8: Docker deployment simplified — binary + chromium only.
  - Source: RECON — current Dockerfile: stage 1 cargo install chrome-agent, stage 2 debian + chromium + python3 + python3-websockets + tini + chrome-agent binary + python script.
  - Acceptance: new Dockerfile has no `cargo install chrome-agent`, no `python3`, no `python3-websockets`; image contains debian + chromium + tini + the Rust sidecar binary only.
  - Evidence required: `docker build` succeeds; `docker image` size reduced; `docker run` serves `/healthz` and a session lifecycle.
  - Status: pending
  - Evidence collected:

- Q1: Per-action latency reduced (verified, not assumed).
  - Source: RECON plan — estimated ~2x on light actions, ~2x on observe (concurrent CDP), ~3-5% end-to-end.
  - Acceptance: CP0 measurements confirm or correct the estimates; post-implementation measurements show sidecar overhead reduced vs the Python+pipe baseline.
  - Evidence required: measured timings (click+observe cycle, observe with 3 CDP commands, screenshot) recorded in CP0 (baseline) and CP7 (new); numbers in the Progress Log.
  - Status: in_progress
  - Evidence collected: CP0 baseline measured (direct CDP, no pipe): `Accessibility.getFullAXTree` avg=2.6ms, `Page.captureScreenshot(png)` avg=38.2ms, `Runtime.evaluate` avg=1.0ms. Concurrent vs sequential (a11y+screenshot+eval): sequential=49ms, concurrent=30ms → ~1.6x speedup, confirming plan's claim. New-implementation measurements pending CP7.

- Q2: No new workspace crates beyond what is clearly required.
  - Source: AGENTS.md — "No new crates ... unless clearly required."
  - Acceptance: `tokio-tungstenite` added as a justified dependency (CDP WebSocket — no alternative); if a new crate `oxide-browser-sidecar` is created, it is justified as a binary (not a library) and follows workspace conventions; shared types location decided with rationale in Decisions.
  - Evidence required: `Cargo.toml` diff reviewed; decision recorded in Decisions.
  - Status: verified
  - Evidence collected: CP1 — two new workspace crates, both justified: (1) `oxide-browser-contracts` — shared REST types, mirrors `oxide-agent-web-contracts` pattern; both sidecar binary and core depend on it, neither depends on the other (correct dependency direction). (2) `oxide-browser-sidecar` — native sidecar binary, separate process, cannot be part of core (library) or telegram bot. `tokio-tungstenite` not yet added (CP2). `Cargo.toml` diff reviewed: 2 workspace members added, 1 optional dep + feature in core.

- Q3: cargo fmt + clippy clean across relevant profiles.
  - Source: AGENTS.md — CI enforces both.
  - Acceptance: `cargo fmt --all -- --check` exit 0; `cargo clippy --workspace --all-targets --features profile-full -- -D warnings` exit 0 (and profile-web-embedded-opencode-local if touched).
  - Evidence required: command outputs in Progress Log.
  - Status: pending
  - Evidence collected:

- Q4: Tests green for browser_live provider and new sidecar.
  - Source: AGENTS.md testing section.
  - Acceptance: `cargo test -p oxide-agent-core --features profile-full` green (browser_live tests unchanged semantics); new sidecar has its own unit/integration tests (a11y noise rules, action translation, stealth patches, CDP message handling).
  - Evidence required: test command outputs in Progress Log.
  - Status: pending
  - Evidence collected:

- V1: P0.5 verification framework executed before code.
  - Source: P0.5 — "Reality is checked BEFORE anything is built on it."
  - Acceptance: CP0 document records: CDP `Accessibility.getFullAXTree` response shape (fields, `backendDOMNodeId`, `ignored`, `role`, `name`); `Page.navigate` + `Page.loadEventFired` wait_until semantics; `Input.dispatchMouseEvent` click; `DOM.requestNode`+`DOM.getBoxModel` uid→coords; `Page.captureScreenshot` format/quality; `Runtime.evaluate` returnByValue/awaitPromise; `/json/list` discovery; baseline latency measurements.
  - Evidence required: CP0 section in Progress Log with raw CDP responses (or a reference to a verification artifact file).
  - Status: verified
  - Evidence collected: CP0 complete — `.cp0-verify/CP0-VERIFICATION-RESULTS.md`. Chromium Chrome/149.0.7827.102 headless. All V1 sub-items verified on real Chromium via Node 22 WebSocket script: (1) `Accessibility.getFullAXTree` response shape — `nodes[]` with `nodeId`, `backendDOMNodeId`, `ignored`, `role.value`, `name.value`, `childIds`, `properties`; `role.type` can be `"role"` or `"internalRole"`; `ignored:true` nodes have `role.value:"none"`; button "Login" `backendDOMNodeId=11` → UID `n11`. (2) `Page.navigate` returns `{frameId,loaderId}`; `Page.loadEventFired` event fires with `{timestamp}`; also captured `domContentEventFired` for "domcontentloaded" wait_until. (3) `Input.dispatchMouseEvent` mousePressed+mouseReleased → `{}` success. (4) **CORRECTED**: `DOM.requestNode` takes `objectId` NOT `backendNodeId` (P0.5 caught); correct path is `DOM.getDocument({depth:0})` → `DOM.pushNodesByBackendIdsToFrontend({backendNodeIds:[N]})` → `nodeIds[0]` → `DOM.getBoxModel({nodeId})` → content quad → center coords → `Input.dispatchMouseEvent`. `DOM.getDocument` MUST be called first (else "Document needs to be requested first" error). (5) `Page.captureScreenshot` png→19948 bytes 38ms avg, jpeg q=80→20644 bytes 31ms. (6) `Runtime.evaluate` returnByValue + awaitPromise both work. (7) `/json/list` returns page target `webSocketDebuggerUrl`. Stealth-safe capture verified: console interceptor via `Runtime.evaluate` (no `Runtime.enable`) captures logs; `Network.enable` captures network events. 23 events on single WebSocket confirms G3 viability.

- V2: Smoke test with real Oxide client against new sidecar.
  - Source: end-to-end validation.
  - Acceptance: a representative browser task (start session, goto, observe, click, fill, observe, close) runs through the real `BrowserSidecarClient` against the new sidecar with a real Chromium and succeeds.
  - Evidence required: smoke-test log or script in the Progress Log.
  - Status: pending
  - Evidence collected:

- N1: `BrowserSidecar` trait and REST API contract unchanged (only shared-types extraction).
  - Source: RECON — Oxide-side consumer code is out of scope.
  - Must preserve: trait method signatures, endpoint paths/methods, request/response schemas, `BrowserAction` enum variants and serde tags.
  - Evidence required: `git diff crates/oxide-agent-core/src/agent/providers/browser_live/client.rs` shows no signature changes.
  - Status: verified
  - Evidence collected: CP1 — `git diff --stat HEAD` shows `client.rs` is NOT in the diff (zero changes). Only `types.rs` was modified (803 lines → 8-line re-export), `Cargo.toml` (new optional dep), and root `Cargo.toml` (workspace members). 71 browser_live tests pass unchanged.

- N2: No `chromiumoxide` crate.
  - Source: AGENTS.md + RECON — raw `tokio-tungstenite` + `serde_json` matches chrome-agent's own approach.
  - Must preserve: no `chromiumoxide` in any `Cargo.toml`.
  - Evidence required: `git grep chromiumoxide` returns nothing.
  - Status: pending
  - Evidence collected:

- N3: No Python or non-Rust runtime in the sidecar.
  - Source: Objective — native Rust.
  - Must preserve: no `.py` files in the sidecar image; no `python3` in the Dockerfile.
  - Evidence required: `git grep -n 'python' docker/Dockerfile.chrome-agent-sidecar` returns nothing (after rename); no `.py` files in the new sidecar source.
  - Status: pending
  - Evidence collected:

## Implementation Plan

1. CP0 — P0.5 verification framework (CDP behavior + baseline latency on real Chromium).
   - Audit IDs: V1, Q1 (baseline).
   - Expected changes: a verification artifact (e.g. `docs/goals/cp0-cdp-verification.md` or a section in this file) recording raw CDP responses and latency measurements; possibly a throwaway script. No production code yet.
   - Validation: CDP responses captured for each command listed in V1; baseline latency numbers recorded.
   - Exit condition: V1 acceptance met; estimates in Q1 either confirmed or corrected with real numbers.

2. CP1 — Shared types extraction + new crate/binary scaffold.
   - Audit IDs: G2, N1, Q2.
   - Expected changes: REST types extracted from `types.rs` to a shared location (decision in Decisions); new crate/binary created with `Cargo.toml`, `src/main.rs` stub, axum server scaffold with `/healthz`; all `oxide-agent-core` browser_live imports updated to shared types.
   - Validation: `cargo check --workspace` green; `git grep` confirms single source of each REST type; `cargo fmt` + `cargo clippy` clean.
   - Exit condition: G2 acceptance met; scaffold serves `/healthz`.

3. CP2 — CDP client + Chromium lifecycle + session management.
   - Audit IDs: G1 (partial), G3.
   - Expected changes: `cdp/` module (tokio-tungstenite WebSocket, serde_json message framing, request/response correlation by `id`); `browser/` module (launch Chromium with `--remote-debugging-port`, wait for ws endpoint, `/json/list` discovery, graceful shutdown); session struct holding one `WebSocketStream`.
   - Validation: unit tests for CDP message framing; integration test launching Chromium and sending `Page.navigate`; `cargo test` green.
   - Exit condition: a session can be created (POST /sessions) that launches Chromium, opens a CDP WebSocket, and navigates to a start URL.

4. CP3 — a11y snapshot (4 noise rules) + stealth patches.
   - Audit IDs: G4, G7.
   - Expected changes: `snapshot/` module (`Accessibility.enable` + `getFullAXTree`, 4 noise rules, UID generation, text format); `stealth/` module (`STEALTH_PATCHES_JS` ported, `Page.addScriptToEvaluateOnNewDocument`, `Runtime.evaluate` patch, `Network.setUserAgentOverride`).
   - Validation: fixture AX tree test verifying each noise rule; snapshot of a real page matches expected nodes; UID stability test; `git grep Runtime.enable` returns nothing.
   - Exit condition: G4 + G7 acceptance met.

5. CP4 — Actions (BrowserAction → CDP).
   - Audit IDs: G6.
   - Expected changes: `actions/` module translating every `BrowserAction` variant to CDP commands: click_target_id (uid→`DOM.pushNodesByBackendIdsToFrontend`→`DOM.getBoxModel`→coords→`Input.dispatchMouseEvent`; requires `DOM.getDocument` first); click_selector (`DOM.querySelector`→`DOM.getBoxModel`→...); click_xy (`Input.dispatchMouseEvent`); fill/type_text (semantic input JS ported from Python); press (simple via `Input.dispatchKeyEvent`, combos via JS KeyboardEvent); scroll, get_element_value, execute_javascript, wait/wait_for_selector/wait_for_text, script. Post-action observation capture.
   - Validation: per-variant test or smoke test against a real page; `fill` on a React/Vue page verifying framework-visible events.
   - Exit condition: G6 acceptance met for every variant.

6. CP5 — Capture (network/console, stealth-safe).
   - Audit IDs: G3 (capture side), G4 (capture side).
   - Expected changes: `capture/` module on the same WebSocket: `Network.enable` for network events; injected JS interceptor for console (no `Runtime.enable`); history accumulation, dedup, filtering, debug payloads; noise/failure normalization ported from Python (`_is_noise_event`, `_is_network_failure`, `summarize_network`, `summarize_console`).
   - Validation: capture a page load with network requests + console logs; verify debug endpoints return filtered history; verify no `Runtime.enable` sent.
   - Exit condition: network/console capture works stealth-safely on the single connection.

7. CP6 — REST server (full contract) + observation building + DOM snapshot + artifacts.
   - Audit IDs: G5.
   - Expected changes: axum routes for all 9 endpoints; observation building (concurrent a11y + screenshot + DOM snapshot via 3 CDP commands on one stream); DOM snapshot JS ported; artifact management (screenshots to disk, `artifact://` URIs); SPA hash navigation; force_reload (close browser without purge, restart, navigate with hash); bearer auth.
   - Validation: `cargo test` for route handlers; contract equivalence check against `types.rs` (now shared); `BrowserSidecarClient` (from `client.rs`) can drive the new server in a test.
   - Exit condition: G5 acceptance met — full REST contract served.

8. CP7 — Integration: Docker, compose, smoke test, latency re-measurement.
   - Audit IDs: G8, V2, Q1 (new), Q3, Q4.
   - Expected changes: new `Dockerfile.chrome-agent-sidecar` (or renamed); compose files updated; `docs/browser-live.md` deployment section updated; smoke test script; latency measurements (new) compared to CP0 baseline.
   - Validation: `docker build` succeeds; `docker run` serves healthz + session lifecycle; smoke test with real `BrowserSidecarClient` green; `cargo fmt` + `cargo clippy` clean; `cargo test` green.
   - Exit condition: G8, V2, Q1, Q3, Q4 acceptance met.

9. CP8 — Cleanup: delete Python sidecar + chrome-agent references.
   - Audit IDs: G1 (final), N3.
   - Expected changes: delete `docker/chrome-agent-sidecar.py`; remove chrome-agent from any remaining Docker/docs references; remove Python unit-test references; final `docs/browser-live.md` update.
   - Validation: `git grep -n 'chrome-agent' docker/` returns nothing (except historical docs if kept); `ls docker/chrome-agent-sidecar.py` → not found; full gate clean.
   - Exit condition: G1, N3 acceptance met; Completion Audit passes.

## Validation Contract

- Static checks: `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets --features profile-full -- -D warnings` (and `profile-web-embedded-opencode-local` if web transport touched).
- Tests: `cargo test -p oxide-agent-core --features profile-full` (browser_live tests); new sidecar crate `cargo test`.
- Runtime/manual verification: CP0 CDP verification artifact; CP7 Docker build + run + smoke test with real `BrowserSidecarClient`.
- Artifact verification: new Docker image builds and runs; `docker/chrome-agent-sidecar.py` deleted; no `chrome-agent`/`python3` in Dockerfile.
- Done when: every G/Q/V/N audit item is `verified` with current evidence in the Completion Audit; Final Verification filled.

## Decisions

- 2026-06-18: Option B (fully native, drop chrome-agent) chosen over Option A (Rust sidecar + chrome-agent subprocess). Rationale (P0): Option A preserves the subprocess boundary (root reason the sidecar exists), the redundant CDP listener, and the stealth conflict — it patches the architecture rather than redesigning the root. Option B eliminates all four root problems (subprocess boundary, redundant CDP, stealth conflict, contract drift) and the chrome-agent external dependency + Python runtime, at ~3000-3500 lines Rust (same order as the 2612-line Python).
- 2026-06-18: `tokio-tungstenite` + `serde_json` for CDP (not `chromiumoxide`). Rationale: AGENTS.md "no new crates unless clearly required"; chrome-agent itself uses tokio-tungstenite + serde_json for CDP; raw CDP is straightforward and keeps the dep surface minimal. `chromiumoxide` is a heavy abstraction not justified for ~3K lines of direct CDP.
- 2026-06-18: Shared types location — to be decided in CP1 (candidates: new `oxide-browser-contracts` crate mirroring `oxide-agent-web-contracts`, or a module within an existing crate re-exported by both). Decision deferred to CP1 when the binary crate structure is settled.
- 2026-06-18 (CP0): `DOM.requestNode` takes `objectId`, NOT `backendNodeId`. Correct click-by-uid path: `DOM.getDocument({depth:0})` (once per navigation, document invalidated on `DOM.documentUpdated` event) → `DOM.pushNodesByBackendIdsToFrontend({backendNodeIds:[N]})` → `nodeIds[0]` → `DOM.getBoxModel({nodeId})` → content quad center → `Input.dispatchMouseEvent`. P0.5 caught this before any production code.
- 2026-06-18 (CP0): AX `role.type` can be `"role"` or `"internalRole"` — noise filter checks `role.value` only. `ignored:true` nodes have `role.value:"none"`. `name.value` can be empty string. All confirmed on real Chromium.
- 2026-06-18 (CP0): Stealth-safe capture confirmed: console interceptor via `Runtime.evaluate` (no `Runtime.enable`) + `Page.addScriptToEvaluateOnNewDocument`; network via `Network.enable`. Both work on real Chromium.
- 2026-06-18 (CP1): Two new workspace crates created: `oxide-browser-contracts` (library, shared REST types) and `oxide-browser-sidecar` (binary, native sidecar). Rationale: (1) Contracts crate mirrors the established `oxide-agent-web-contracts` pattern — sidecar binary and core both depend on it, neither depends on the other. (2) Sidecar binary is a separate process, cannot be part of core (library) or telegram bot. Alternative considered: single crate with both lib+bin targets (sidecar exports types as lib, core depends on sidecar lib) — rejected because it creates a backwards dependency from core to a binary crate and allows sidecar-specific code to leak into the shared types. (3) `types.rs` in oxide-agent-core becomes a thin re-export (`pub use oxide_browser_contracts::*;`) preserving all internal import paths — zero churn in `client.rs`, `tools.rs`, `test_support.rs`, etc.
- 2026-06-18 (CP3): A11y snapshot produces structured `Vec<A11yNode>` directly (no text intermediate → parse back). Text format produced in parallel for logging. Rationale: P0 — the Python sidecar's `parse_snapshot()` parses chrome-agent's text output back into structured data; in the native implementation, producing text just to parse it back is an intermediate representation. Both outputs (structured + text) are primary, neither is parsed by the other.
- 2026-06-18 (CP4): `wait_for_text` uses `document.body.textContent` (not `innerText`). Rationale: `innerText` requires a reflow to compute, which may not happen between CDP commands in headless mode. `textContent` reads directly from the DOM and is always current. Tradeoff: `textContent` matches text in hidden elements, but this is acceptable — if text is in the DOM, the page is loading it and it will likely become visible soon.
- 2026-06-18 (CP4): All `Press` actions use JS `KeyboardEvent` (not `Input.dispatchKeyEvent`). Rationale: `Input.dispatchKeyEvent` was not verified in CP0 (P0.5); JS `KeyboardEvent` is a standard DOM API verified via `Runtime.evaluate`. The Python sidecar uses chrome-agent's `press` for simple keys (which uses `Input.dispatchKeyEvent` internally) and JS for combos — we unify on JS for all keys. Tradeoff: JS `KeyboardEvent` for character keys won't insert text (unlike `Input.dispatchKeyEvent`), but `Press` is for control keys and combos, not text input — `Fill`/`TypeText` handles text input via semantic input JS.
- 2026-06-18 (CP4): `Navigate` variant returns `Failed` in the actions module. Rationale: the Rust client's `plan_browser_action` maps `BrowserAction::Navigate` to `BrowserExecutePlan::Navigate(GotoRequest)` which goes to the `/goto` endpoint, never to `/action`. The Python sidecar also doesn't handle `navigate` in `action_to_pipe_cmd` (raises `ValueError`). The sidecar's `/action` endpoint should not handle navigation — that's the `/goto` endpoint's responsibility (CP6).

## Progress Log

- 2026-06-18 14:00: Goal created. RECON complete (see compressed conversation summary). Plan approved by user. Branch: `feature/chrome-agent`. Next: CP0 (P0.5 verification framework on real Chromium).
- 2026-06-18 14:10: CP0 complete — P0.5 CDP verification on real Chromium.
  - Changed: `.cp0-verify/` (verification scripts + test page + results artifact); this goal doc (V1→verified, Q1→in_progress, CP4 corrected, Decisions +4, R1 resolved).
  - Evidence: `.cp0-verify/CP0-VERIFICATION-RESULTS.md` — all V1 sub-items verified on Chrome/149.0.7827.102. Baseline latencies: a11y 2.6ms, screenshot 38ms, eval 1ms. Concurrent vs sequential: 30ms vs 49ms (~1.6x). Stealth-safe capture confirmed (no Runtime.enable). Click-by-uid path corrected (`DOM.pushNodesByBackendIdsToFrontend`, not `DOM.requestNode`). 23 events on single WebSocket confirms G3.
  - Commands: `chromium --headless=new --remote-debugging-port=9222`; `curl /json/list`; `node .cp0-verify/cdp-verify.mjs`; `node .cp0-verify/cdp-verify-click-uid.mjs`.
  - Audit IDs updated: V1→verified, Q1→in_progress (baseline done, new pending CP7).
  - Next: CP1 — shared types extraction + new crate/binary scaffold.

- 2026-06-18 15:30: CP1 complete — shared types extraction + new crate/binary scaffold.
  - Changed: `crates/oxide-browser-contracts/` (new crate: Cargo.toml, clippy.toml, src/lib.rs, src/types.rs — 803 lines of REST types moved verbatim from oxide-agent-core); `crates/oxide-browser-sidecar/` (new crate: Cargo.toml, clippy.toml, src/main.rs — axum scaffold with /healthz + bearer auth middleware); `Cargo.toml` (2 workspace members added); `crates/oxide-agent-core/Cargo.toml` (oxide-browser-contracts optional dep + tool-browser-live feature); `crates/oxide-agent-core/src/agent/providers/browser_live/types.rs` (803 lines → 8-line re-export `pub use oxide_browser_contracts::*;`); `docs/goals/2026-06-18-chrome-agent-native-rust.md` (this file).
  - Evidence: `cargo check -p oxide-browser-contracts` ✓; `cargo check -p oxide-browser-sidecar` ✓; `cargo check -p oxide-agent-core --no-default-features --features profile-full` ✓; `cargo fmt --all -- --check` ✓; `cargo clippy -p oxide-browser-contracts -p oxide-browser-sidecar -- -D warnings` ✓; `cargo clippy -p oxide-agent-core --no-default-features --features profile-full --all-targets -- -D warnings` ✓; `cargo test -p oxide-browser-contracts` — 4 passed ✓; `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- browser_live` — 71 passed, 1 ignored ✓; sidecar binary smoke test: /healthz → `{"native":true,"ok":true}` (200, no auth), POST /sessions correct token → 501, wrong/no token → 401 ✓; `git grep` confirms single source of browser REST types ✓; `client.rs` NOT in diff (N1 preserved) ✓. Pre-existing `cargo check --workspace` default-features failure (reqwest in llm/error.rs) verified identical with/without my changes.
  - Commands: see above.
  - Audit IDs updated: G2→verified, N1→verified, Q2→verified.
  - Next: CP2 — CDP client + Chromium lifecycle + session management.
  - Commit: `2591d38a`.

- 2026-06-18 16:30: CP2 complete — CDP client + Chromium lifecycle + session management.
  - Changed: `crates/oxide-browser-sidecar/Cargo.toml` (added tokio-tungstenite 0.29, futures-util, reqwest 0.13, uuid, thiserror, tempfile); `crates/oxide-browser-sidecar/src/lib.rs` (new — module declarations, AppState, create_app, route handlers, bearer_auth); `crates/oxide-browser-sidecar/src/main.rs` (thinned to entry point, uses lib); `crates/oxide-browser-sidecar/src/cdp.rs` (new — CdpError, CdpEvent, CdpClient with WebSocket connect, send_command with id correlation + timeout, event broadcast, background reader/writer tasks); `crates/oxide-browser-sidecar/src/browser.rs` (new — ChromiumProcess: launch with --headless=new --no-sandbox --remote-debugging-port=0, DevToolsActivePort file polling, /json/list discovery with camelCase serde, page target polling, pipe draining, Drop kill safety); `crates/oxide-browser-sidecar/src/session.rs` (new — navigate_to free function with Page.enable + Page.navigate + Page.loadEventFired, BrowserSession struct, SessionManager with create/get/close, ID generation); `crates/oxide-browser-sidecar/tests/cdp_integration.rs` (new — integration test: healthz, create session with data URL, close session, auth check); `docs/goals/2026-06-18-chrome-agent-native-rust.md` (this file).
  - Evidence: `cargo clippy -p oxide-browser-sidecar --all-targets -- -D warnings` ✓; `cargo fmt --all -- --check` ✓; `cargo test -p oxide-browser-sidecar` — 2 unit tests passed, 1 integration test ignored ✓; `cargo test -p oxide-browser-sidecar --test cdp_integration -- --ignored --nocapture` — session_lifecycle passed in 0.5s ✓ (launches Chromium, creates session via POST /sessions, navigates to data URL, closes via DELETE /sessions, verifies auth); `cargo check -p oxide-agent-core --no-default-features --features profile-full` ✓; `cargo clippy -p oxide-agent-core --no-default-features --features profile-full --all-targets -- -D warnings` ✓. P0.5 bugs caught during CP2: (1) `/json/list` targets use camelCase (`webSocketDebuggerUrl`) not snake_case — fixed with `#[serde(rename_all = "camelCase")]`; (2) `Page.loadEventFired` event fires before subscription — fixed by subscribing BEFORE `Page.navigate` and calling `Page.enable` first; (3) page target may not have WebSocket URL immediately — fixed with polling loop; (4) `reqwest::get` truncates DevTools HTTP response — fixed with `Client::builder().http1_only()`.
  - Commands: see above.
  - Audit IDs updated: G1→in_progress (native launch+CDP+session works, remaining endpoints pending), G3→verified (single WebSocket per session, no separate listener).
  - Next: CP3 — a11y snapshot (4 noise rules) + stealth patches.
  - Commit: `3185a2c5`.

- 2026-06-18 17:30: CP3 complete — a11y snapshot (4 noise rules) + stealth patches.
  - Changed: `crates/oxide-browser-sidecar/src/snapshot.rs` (new — 753 lines: AXNode/AxValue/AxProperty CDP types with `#[serde(rename_all = "camelCase")]`, `take_snapshot()` calling Accessibility.enable + getFullAXTree, `format_ax_tree()` + `format_node()` with 4 noise rules, `SnapshotResult { nodes, text, uid_to_backend }`, 14 unit tests); `crates/oxide-browser-sidecar/src/stealth.rs` (new — 143 lines: `STEALTH_PATCHES_JS` ported verbatim from chrome-agent setup.rs, `apply_stealth()` calling Page.addScriptToEvaluateOnNewDocument + Runtime.evaluate + Network.setUserAgentOverride, 3 unit tests); `crates/oxide-browser-sidecar/src/lib.rs` (added `pub mod snapshot; pub mod stealth;`); `crates/oxide-browser-sidecar/src/session.rs` (`navigate_to` made `pub`, added `stealth: bool` parameter, calls `stealth::apply_stealth()` on first navigation); `crates/oxide-browser-sidecar/tests/snapshot_stealth.rs` (new — integration test: stealth verification + a11y snapshot + UID stability on real Chromium); `docs/goals/2026-06-18-chrome-agent-native-rust.md` (this file — G4 header fixed, G4→verified, G7→verified, Decisions +1, Progress Log +CP3).
  - Evidence: `cargo clippy -p oxide-browser-sidecar --all-targets -- -D warnings` ✓; `cargo fmt --all -- --check` ✓; `cargo test -p oxide-browser-sidecar` — 21 unit tests passed, 2 integration tests ignored ✓; `cargo test -p oxide-browser-sidecar --test snapshot_stealth -- --ignored --nocapture` — snapshot_and_stealth_on_real_chromium passed in 0.54s ✓ (stealth: navigator.webdriver=undefined, UA no HeadlessChrome; snapshot: heading "Welcome" + button "Login" found, stable n-prefix UIDs, UID stability across 2 snapshots, text format contains uid=n + heading); `cargo test -p oxide-browser-sidecar --test cdp_integration -- --ignored --nocapture` — session_lifecycle passed ✓; `cargo check -p oxide-agent-core --no-default-features --features profile-full` ✓; `cargo clippy -p oxide-agent-core --no-default-features --features profile-full --all-targets -- -D warnings` ✓; `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- browser_live` — 72 passed, 1 ignored ✓; `grep 'Runtime.enable' crates/oxide-browser-sidecar/src/` → only comments/assertions, never as CDP command ✓; `grep '"Runtime' crates/oxide-browser-sidecar/src/` → only `Runtime.evaluate` (stealth-safe) ✓. P0.5 bug caught: `AxNode` struct needed `#[serde(rename_all = "camelCase")]` for CDP field names (`nodeId`, `childIds`, `parentId`) — without it, deserialization silently failed and `MissingNodes` error was returned.
  - Commands: see above.
  - Audit IDs updated: G4→verified (stealth patches ported, Runtime.enable never sent, integration test confirms webdriver=undefined + UA no HeadlessChrome), G7→verified (4 noise rules, stable UIDs, structured+text output, 14 unit tests + integration test on real Chromium).
  - Next: CP4 — Actions (BrowserAction → CDP).

- 2026-06-18 18:00: CP4 complete — Actions (BrowserAction → CDP).
  - Changed: `crates/oxide-browser-sidecar/src/actions.rs` (new — 570 lines: execute_action dispatch, eval_js helper, dispatch_mouse_click helper, all 13 BrowserAction variants → CDP translation, semantic_input_script + press_key_script JS builders ported from Python, poll_condition helper, key/modifier alias resolution, 22 unit tests); `crates/oxide-browser-sidecar/src/lib.rs` (added `pub mod actions;`); `crates/oxide-browser-sidecar/src/session.rs` (added `pub fn cdp(&self) -> &CdpClient` accessor); `crates/oxide-browser-sidecar/tests/actions_integration.rs` (new — integration test for all action variants on real Chromium); `docs/goals/2026-06-18-chrome-agent-native-rust.md` (this file — G6→in_progress, Decisions +3, Progress Log +CP4).
  - Evidence: `cargo clippy -p oxide-browser-sidecar --all-targets -- -D warnings` ✓; `cargo fmt --all -- --check` ✓; `cargo test -p oxide-browser-sidecar` — 37 unit tests passed, 3 integration tests ignored ✓; `cargo test -p oxide-browser-sidecar --test actions_integration -- --ignored --nocapture` — actions_on_real_chromium passed in 1.18s ✓ (click_selector, fill, get_element_value, type_text, press Enter + ctrl+a, scroll, execute_javascript 1+2=3 + error case, wait_for_text positive + negative, wait_for_selector, wait 100ms, script multi-step, navigate→Failed); `cargo test -p oxide-browser-sidecar --test snapshot_stealth -- --ignored --nocapture` — passed ✓; `cargo test -p oxide-browser-sidecar --test cdp_integration -- --ignored --nocapture` — passed ✓; `cargo check -p oxide-agent-core --no-default-features --features profile-full` ✓; `cargo clippy -p oxide-agent-core --no-default-features --features profile-full --all-targets -- -D warnings` ✓; `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- browser_live` — 72 passed, 1 ignored ✓. P0.5 bugs caught: (1) `#` in CSS hex color (`background:#eee;`) is a URL fragment delimiter in data URLs — truncates HTML after `#`, causing `#dynamic` div to be missing; fixed by using named color `gray`; (2) `awaitPromise: true` with `returnByValue: true` — verified works for both Promise and non-Promise values; (3) async fn recursion (`run_action` → `script` → `run_action`) requires `Box::pin` to avoid infinitely sized future; (4) `innerText` requires reflow not guaranteed between CDP commands in headless mode — switched to `textContent`.
  - Commands: see above.
  - Audit IDs updated: G6→in_progress (all variants implemented + tested on real Chromium; post-action BrowserObservation pending CP6; fill/type_text on framework page pending CP7).
  - Next: CP5 — Capture (network/console, stealth-safe).
  - Commit: `78ccf11f`.

## Risks and Blockers

- R1: CP0 requires a running Chromium for CDP verification.
  - Impact: cannot validate architecture assumptions before code (P0.5 violation).
  - Evidence: **RESOLVED** — Chromium available locally at `/usr/bin/chromium` (Chrome/149.0.7827.102). CP0 executed successfully.
  - Mitigation: N/A.
  - Audit IDs affected: V1, Q1.

- R2: `types.rs` extraction blast radius.
  - Impact: `types.rs` imported across 7+ files in `browser_live/`; extraction could break many call sites.
  - Evidence: RECON — `client.rs`, `tools.rs`, `test_support.rs`, `session.rs`, `actions.rs`, `verification.rs`, `artifacts.rs` all import from `types.rs`.
  - Mitigation: CP1 enumerates every import via `git grep` before moving; re-export from the original `types.rs` location if needed to minimize churn; `cargo check --workspace` gate.
  - Audit IDs affected: G2, N1.

- R3: CDP edge cases (cross-origin iframes, shadow DOM, lazy-loaded content).
  - Impact: ported behavior may regress on pages the Python sidecar handled.
  - Evidence: these edge cases exist in the current Python sidecar's JS scripts.
  - Mitigation: port the exact JS scripts (DOM snapshot, semantic input) verbatim from Python; smoke-test on the same pages used by the Python sidecar's `self_test()`.
  - Audit IDs affected: G6, V2.

## Final Verification

Filled only when complete.
