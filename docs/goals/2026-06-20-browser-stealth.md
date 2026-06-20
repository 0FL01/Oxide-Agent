# Goal: Browser Live Stealth Hardening

Date started: 2026-06-20
Status: active
Codex goal: not set
Source spec: RECON of `.donor/patchright/` vs `crates/oxide-browser-sidecar/`; 2026 benchmark (ianlpaterson.com)
Goal doc owner: Codex
Last updated: 2026-06-20 00:00

## Objective

Significantly improve the anti-detection capability of the browser-live sidecar so it passes JS-layer, protocol-layer, and binary-layer fingerprint checks used by bot-detection services (sannysoft, creepjs, pixelscan, Cloudflare JS challenge).

Done when every Completion Audit item is verified by its listed evidence and all out-of-scope constraints are preserved.

## Scope

In scope:
- `crates/oxide-browser-sidecar/src/browser.rs` — Chromium launch args, binary selection
- `crates/oxide-browser-sidecar/src/stealth.rs` — stealth JS patches, webdriver override fix
- `crates/oxide-browser-sidecar/src/cdp.rs` — isolated world support (createIsolatedWorld, eval with contextId)
- `crates/oxide-browser-sidecar/src/session.rs` — navigate_to flow (stealth application, isolated world lifecycle)
- `crates/oxide-browser-sidecar/src/observe.rs` — move DOM fingerprint + URL/title to isolated world
- `crates/oxide-browser-sidecar/src/dom.rs` — move DOM snapshot to isolated world
- `crates/oxide-browser-sidecar/src/capture.rs` — console interceptor toString hardening, console drain to isolated world
- `crates/oxide-browser-sidecar/src/actions.rs` — split read-only queries (isolated) from page-interacting actions (main world)
- `crates/oxide-browser-sidecar/src/lib.rs` — SPA hash nav stays main world (no change)
- `docs/browser-live.md` — documentation update

Out of scope:
- TLS fingerprint (JA4) — binary-level, not controllable from CDP client
- HTTP/2 SETTINGS ordering — binary-level
- Headless artifacts (screen dims vs window dims) — `--headless=new` limitations
- Behavioral patterns (mouse movement, typing cadence) — agent-controlled, not stealth
- CDP-Patches (Input domain OS-level events) — only relevant for headful browsers, fixed in Chrome v142+
- Adding `Target.setAutoAttach` or `Runtime.enable` — these are architectural invariants we MUST NOT introduce

## Missing Inputs

None. All requirements derivable from RECON + donor code.

## Repository Context

- Relevant entry points: `crates/oxide-browser-sidecar/src/lib.rs` (REST handlers), `browser.rs` (ChromiumProcess::launch), `session.rs` (navigate_to), `cdp.rs` (CdpClient)
- Existing conventions: direct CDP over WebSocket (no Playwright shim), never call `Runtime.enable`/`Target.*`, `thiserror` for errors, `cargo check -p oxide-browser-sidecar` for verification
- Validation infrastructure: `cargo check -p oxide-browser-sidecar`, `cargo clippy -p oxide-browser-sidecar -- -D warnings`, `cargo test -p oxide-browser-sidecar`, `cargo fmt --all -- --check`
- Risky areas: isolated world refactor touches ALL eval sites — must classify read-only vs page-interacting correctly; webdriver override removal depends on Phase 1 flag being set

## References

- `.donor/patchright/` — Patchright donor code (patched Playwright fork)
- `.donor/patchright/src/driver_patches/chromiumSwitchesPatch.ts` — flag removals/additions (Phase 1)
- `.donor/patchright/src/driver_patches/chromiumPatch.ts` — `--headless=new` enforcement, swiftshader removal
- `.donor/patchright/src/driver_patches/browserContextPatch.ts:31` — serviceWorker.register patch (Phase 4)
- `.donor/patchright/src/driver_patches/framesPatch.ts:244-252` — isolated world pattern (Phase 3)
- `.donor/patchright/src/driver_patches/crPagePatch.ts:38,310-317,350-358` — init script tag injection (evaluated, not adopted)
- Benchmark: https://ianlpaterson.com/articles/2026/01/01/stealth-benchmark-2026 (nodriver 28/31, 0 blocked vs Patchright 25/31, 3 blocked)

## Completion Audit

### G1: Command-line flags match Patchright
- Source: chromiumSwitchesPatch.ts:20-33, chromiumPatch.ts:22-27
- Acceptance: Launch args do NOT contain `--disable-extensions`, `--disable-popup-blocking`, `--disable-gpu`. Launch args DO contain `--disable-blink-features=AutomationControlled` and `--disable-features=ImprovedCookieControls,LazyFrameLoading,GlobalMediaControls,DestroyProfileOnBrowserClose,MediaRouter,DialMediaRouteProvider,AcceptCHFrame,AutoExpandDetailsElement,CertificateTransparencyComponentUpdater,AvoidUnnecessaryBeforeUnloadCheckSync,Translate,HttpsUpgrades,PaintHolding,ThirdPartyStoragePartitioning,LensOverlay,PlzDedicatedWorker`.
- Evidence required: unit test asserting flag presence/absence; `cargo test -p oxide-browser-sidecar` green
- Status: verified
- Evidence collected: `browser::tests::launch_args_include_anti_detection_flags` + `browser::tests::launch_args_exclude_fingerprint_flags` pass; 114 tests green

### G2: navigator.webdriver fixed at C++ level, JS override removed
- Source: RECON finding — stealth.rs:28-32 returns `undefined` not `false`; chromiumSwitchesPatch.ts adds `--disable-blink-features=AutomationControlled`
- Acceptance: `STEALTH_PATCHES_JS` does NOT contain `Object.defineProperty(navigator, 'webdriver', ...)`. `apply_stealth` does NOT run `Runtime.evaluate` webdriver patch. `--disable-blink-features=AutomationControlled` flag ensures Blink sets `navigator.webdriver = false` at C++ level.
- Evidence required: unit test verifying webdriver override absence; `cargo test -p oxide-browser-sidecar` green
- Status: verified
- Evidence collected: `stealth::tests::stealth_patches_js_does_not_patch_webdriver` pass; `apply_stealth` no longer calls Runtime.evaluate for webdriver; 114 tests green

### G3: System Chrome binary preferred
- Source: RECON — benchmark shows `channel=chrome` matters; browser.rs:16 defaults to `chromium`
- Acceptance: Launch auto-detects `google-chrome` → `google-chrome-stable` → `$CHROMIUM_BIN` → `chromium`. `CHROMIUM_BIN` env override has highest priority when set.
- Evidence required: unit test for resolution order; `cargo test -p oxide-browser-sidecar` green
- Status: verified
- Evidence collected: `resolve_prefers_env_override_over_system_chrome`, `resolve_prefers_google_chrome_over_chromium`, `resolve_uses_google_chrome_stable_when_google_chrome_absent`, `resolve_falls_back_to_chromium_when_no_system_chrome`, `resolve_ignores_empty_env_override`, `find_in_path_locates_executable`, `find_in_path_ignores_non_executable` all pass; 121 tests green; Docker sets `CHROMIUM_BIN=/usr/bin/chromium` (env override = no behavior change in Docker)

### G4: Isolated world support in CDP client
- Source: framesPatch.ts:244-252 — `Page.createIsolatedWorld` with `worldName`, `Runtime.evaluate` with `contextId`
- Acceptance: `CdpClient` has `create_isolated_world(frame_id, world_name) -> Result<u64>` and `eval_in_context(context_id, expression, timeout) -> Result<Value>`. Isolated world context_id cached per session lifecycle.
- Evidence required: unit test for create_isolated_world + eval_in_context; `cargo test -p oxide-browser-sidecar` green
- Status: verified
- Evidence collected: `parse_execution_context_id_extracts_id`, `parse_execution_context_id_errors_when_missing`, `parse_eval_result_extracts_value`, `parse_eval_result_returns_null_when_no_value`, `parse_eval_result_errors_on_exception` pass; `CdpClient::create_isolated_world` + `CdpClient::eval_in_context` implemented with pure parsing helpers; `navigate_to()` creates isolated world best-effort after load event; `BrowserInner.isolated_context_id` caches the ID; 126 tests green

### G5: Read-only internal JS moved to isolated world
- Source: framesPatch.ts — internal JS (DOM queries, snapshots) runs in isolated world
- Acceptance: DOM fingerprint (observe.rs), URL/title (observe.rs), DOM snapshot (dom.rs), console drain (capture.rs) execute via `eval_in_context` with isolated world context_id. NOT via main-world `Runtime.evaluate`.
- Evidence required: code review confirms all read-only eval sites use isolated world; `cargo test -p oxide-browser-sidecar` green
- Status: pending
- Evidence collected:

### G6: Page-interacting JS stays in main world
- Source: RECON — actions that dispatch events must run in main world
- Acceptance: click, fill, press, scroll, semantic_input, execute_javascript, SPA hash nav (lib.rs) use main-world `Runtime.evaluate`. Console interceptor (capture.rs:53-78) stays in main world via `Page.addScriptToEvaluateOnNewDocument`. Stealth patches (stealth.rs) stay in main world.
- Evidence required: code review confirms no page-interacting action moved to isolated world; `cargo test -p oxide-browser-sidecar` green
- Status: pending
- Evidence collected:

### G7: navigator.serviceWorker.register patched
- Source: browserContextPatch.ts:31
- Acceptance: `STEALTH_PATCHES_JS` contains `if (navigator.serviceWorker) navigator.serviceWorker.register = async () => { };`
- Evidence required: unit test verifying patch presence; `cargo test -p oxide-browser-sidecar` green
- Status: verified
- Evidence collected: `stealth::tests::stealth_patches_js_contains_key_patches` asserts serviceWorker.register presence; 114 tests green

### Q1: No new crates, services, or abstraction layers
- Source: AGENTS.md — implementation bias
- Acceptance: No new Cargo dependencies added to `oxide-browser-sidecar`. Isolated world support implemented in existing `cdp.rs`.
- Evidence required: `Cargo.toml` diff shows no new deps; `cargo check -p oxide-browser-sidecar` green
- Status: verified (for checkpoints 1-3)
- Evidence collected: no Cargo.toml changes; `cargo check -p oxide-browser-sidecar` green

### Q2: Architectural invariants preserved
- Source: AGENTS.md — never call `Runtime.enable` or `Target.*`
- Acceptance: No `Runtime.enable`, `Target.setAutoAttach`, `Target.attachToTarget`, `Console.enable` calls added. Existing tests for these invariants still pass.
- Evidence required: `cargo test -p oxide-browser-sidecar` green; `git grep` confirms no new calls
- Status: verified (for checkpoints 1-3)
- Evidence collected: webdriver `Runtime.evaluate` removed from `apply_stealth` (fewer Runtime calls, not more); 126 tests green; no new `Target.*` or `Console.enable` calls; `Runtime.evaluate` with `contextId` used in `eval_in_context` — this is a command, NOT `Runtime.enable` (event subscription); `git grep` confirms no actual `Runtime.enable`/`Target.*`/`Console.enable` calls

### Q3: clippy + fmt clean
- Source: AGENTS.md — CI enforces both
- Acceptance: `cargo clippy -p oxide-browser-sidecar -- -D warnings` and `cargo fmt --all -- --check` both pass
- Evidence required: command output clean
- Status: pending
- Evidence collected:

### Q4: Documentation updated
- Source: docs/browser-live.md
- Acceptance: `docs/browser-live.md` documents new stealth hardening: flag changes, isolated world architecture, Chrome binary preference.
- Evidence required: file review
- Status: pending
- Evidence collected:

### N1: No script-tag injection method
- Source: RECON Phase 5 analysis — diminishing returns vs complexity
- Must preserve: `Page.addScriptToEvaluateOnNewDocument` for stealth patches (main world) and console interceptor (main world)
- Evidence required: no route interception / `<script class=randomhex>` injection code added
- Status: pending
- Evidence collected:

### N2: No CDP-Patches integration
- Source: RECON — CDP-Patches only relevant for headful, fixed in Chrome v142+
- Must preserve: existing JS-level screenX/screenY patch in stealth.rs stays; no OS-level input event patches
- Evidence required: no CDP-Patches code added
- Status: pending
- Evidence collected:

## Implementation Plan

### Checkpoint 1: Command-line flags + webdriver fix + serviceWorker patch
- Audit IDs: G1, G2, G7, Q1, Q2
- Expected changes:
  - `browser.rs`: remove `--disable-extensions`, `--disable-popup-blocking`, `--disable-gpu`; add `--disable-blink-features=AutomationControlled`; add `--disable-features=...` list
  - `stealth.rs`: remove webdriver override from `STEALTH_PATCHES_JS`; remove webdriver `Runtime.evaluate` from `apply_stealth`; add `navigator.serviceWorker.register` patch
  - Add unit tests for flag presence/absence, webdriver override absence, serviceWorker patch presence
- Validation: `cargo test -p oxide-browser-sidecar`, `cargo clippy -p oxide-browser-sidecar -- -D warnings`, `cargo fmt --all -- --check`
- Exit condition: all tests green, audit items G1/G2/G7 verified

### Checkpoint 2: System Chrome binary preference
- Audit IDs: G3, Q1
- Expected changes:
  - `browser.rs`: auto-detect `google-chrome` → `google-chrome-stable` → `$CHROMIUM_BIN` → `chromium`; `CHROMIUM_BIN` env override highest priority
  - Add unit test for resolution order
- Validation: `cargo test -p oxide-browser-sidecar`, `cargo clippy -p oxide-browser-sidecar -- -D warnings`
- Exit condition: tests green, audit item G3 verified

### Checkpoint 3: Isolated world support in CDP client
- Audit IDs: G4, Q1, Q2
- Expected changes:
  - `cdp.rs`: add `create_isolated_world(frame_id, world_name) -> Result<u64>`; add `eval_in_context(context_id, expression, timeout) -> Result<Value>`
  - `session.rs`: create isolated world after navigation, cache context_id
  - Add unit tests for create_isolated_world + eval_in_context
- Validation: `cargo test -p oxide-browser-sidecar`, `cargo clippy -p oxide-browser-sidecar -- -D warnings`
- Exit condition: tests green, audit item G4 verified

### Checkpoint 4: Move read-only JS to isolated world
- Audit IDs: G5, G6, Q2
- Expected changes:
  - `observe.rs`: DOM fingerprint + URL/title → `eval_in_context`
  - `dom.rs`: DOM snapshot → `eval_in_context`
  - `capture.rs`: console drain → `eval_in_context` (interceptor stays main world)
  - `actions.rs`: verify page-interacting actions stay main world; optionally move get_element_value/wait_for_selector/wait_for_text reads to isolated
  - Verify NO page-interacting action moved to isolated world
- Validation: `cargo test -p oxide-browser-sidecar`, `cargo clippy -p oxide-browser-sidecar -- -D warnings`
- Exit condition: tests green, audit items G5/G6 verified

### Checkpoint 5: Console interceptor toString hardening
- Audit IDs: Q2
- Expected changes:
  - `capture.rs`: replace direct function override with Proxy-based interceptor that returns native `toString()`; harden against `Function.prototype.toString.call()` detection
- Validation: `cargo test -p oxide-browser-sidecar`, `cargo clippy -p oxide-browser-sidecar -- -D warnings`
- Exit condition: tests green

### Checkpoint 6: Documentation + final verification
- Audit IDs: Q3, Q4, N1, N2
- Expected changes:
  - `docs/browser-live.md`: document stealth hardening
  - Run full validation suite: `cargo test -p oxide-browser-sidecar`, `cargo clippy -p oxide-browser-sidecar -- -D warnings`, `cargo fmt --all -- --check`
  - `git grep` for `Runtime.enable`, `Target.setAutoAttach`, `Target.attachToTarget`, `Console.enable` — confirm no new calls
- Validation: all commands clean
- Exit condition: all audit items verified

## Validation Contract

- Static checks: `cargo check -p oxide-browser-sidecar`
- Tests: `cargo test -p oxide-browser-sidecar`
- Lint: `cargo clippy -p oxide-browser-sidecar -- -D warnings`
- Format: `cargo fmt --all -- --check`
- Invariant grep: `git grep -n "Runtime.enable\|Target.setAutoAttach\|Target.attachToTarget\|Console.enable" crates/oxide-browser-sidecar/src/` — only in allowed test/condition-guard contexts
- Done when: all Completion Audit items verified

## Decisions

- 2026-06-20: Phase 1 flag `--disable-blink-features=AutomationControlled` makes JS webdriver override harmful (changes `false`→`undefined`, detectable). Override removed entirely. Blink flag handles `navigator.webdriver = false` at C++ level, undetectable.
- 2026-06-20: Script-tag injection (Patchright crPagePatch.ts) NOT adopted. `Page.addScriptToEvaluateOnNewDocument` stays for stealth patches. Rationale: diminishing returns vs complexity; with Phase 1 (flags) + Phase 3 (isolated worlds) main detection vectors closed; route interception itself is a detection surface.
- 2026-06-20: Console interceptor MUST stay in main world (needs to override page's `console.*`). Only console drain (reading captured array) moves to isolated world.
- 2026-06-20: `CHROMIUM_BIN` env override has highest priority when set, then auto-detect google-chrome variants, then fallback to `chromium`.

## Progress Log

- 2026-06-20 00:00: Checkpoint 0 — goal doc created
  - Changed: `docs/goals/2026-06-20-browser-stealth.md`
  - Evidence: goal doc written
  - Commands: none
  - Audit IDs updated: none
  - Next: Checkpoint 1

- 2026-06-20 00:15: Checkpoint 1 — command-line flags + webdriver fix + serviceWorker patch
  - Changed: `browser.rs` (removed `--disable-extensions`, `--disable-popup-blocking`, `--disable-gpu`; added `--disable-blink-features=AutomationControlled` + `--disable-features=...`; extracted `build_launch_args()`); `stealth.rs` (removed webdriver JS override + Runtime.evaluate; added serviceWorker.register no-op; updated tests)
  - Evidence: `cargo test -p oxide-browser-sidecar` → 114 passed, 0 failed; `cargo clippy -p oxide-browser-sidecar -- -D warnings` clean; `cargo fmt --all -- --check` clean
  - Commands: cargo test, cargo clippy, cargo fmt
  - Audit IDs updated: G1 verified, G2 verified, G7 verified, Q1 verified (CP1), Q2 verified (CP1)
  - Next: Checkpoint 2

- 2026-06-20 00:30: Checkpoint 2 — system Chrome binary preference
  - Changed: `browser.rs` (added `SYSTEM_CHROME_CANDIDATES` constant; added `resolve_chromium_binary()` + `resolve_chromium_binary_impl()` + `find_in_path()`; replaced direct `env::var` in `launch()` with `resolve_chromium_binary()`; added 7 unit tests)
  - Evidence: `cargo test -p oxide-browser-sidecar` → 121 passed, 0 failed; `cargo clippy -p oxide-browser-sidecar -- -D warnings` clean; `cargo fmt --all -- --check` clean; Docker `CHROMIUM_BIN=/usr/bin/chromium` env override preserved (no Docker regression)
  - Commands: cargo check, cargo test, cargo clippy, cargo fmt
  - Audit IDs updated: G3 verified, Q1 verified (CP2)
  - Next: Checkpoint 3

- 2026-06-20 00:50: Checkpoint 3 — isolated world support in CDP client
  - Changed: `cdp.rs` (added `CdpClient::create_isolated_world()` + `CdpClient::eval_in_context()`; added pure helpers `parse_execution_context_id()` + `parse_eval_result()`; added 5 parsing unit tests); `session.rs` (added `isolated_context_id: Option<u64>` to `BrowserInner`; added `isolated_context_id()` getter; changed `navigate_to()` return type to `Result<Option<u64>>`; added `create_isolated_world_for_page()` helper; updated all 3 callers: `new()`, `navigate()`, `force_reload()`); `browser.rs` + `cdp.rs` (replaced all `.unwrap()`/`.unwrap_err()` in tests with `.expect()`/`.expect_err()` to fix pre-existing clippy `--all-targets` violations)
  - Evidence: `cargo test -p oxide-browser-sidecar` → 126 passed, 0 failed; `cargo clippy --all-targets -p oxide-browser-sidecar -- -D warnings` clean; `cargo fmt --all -- --check` clean; `git grep` confirms no `Runtime.enable`/`Target.*`/`Console.enable` calls added
  - Commands: cargo check --all-targets, cargo test, cargo clippy --all-targets, cargo fmt
  - Audit IDs updated: G4 verified, Q1 verified (CP3), Q2 verified (CP3)
  - Next: Checkpoint 4

## Risks and Blockers

- Isolated world refactor touches all eval sites — risk of misclassifying read-only vs page-interacting. Mitigation: explicit classification in Checkpoint 4, test each eval site.
- `--disable-gpu` removal in Docker — risk of rendering issues. Mitigation: `--headless=new` uses SwiftShader by default, does not require GPU.
- Chrome binary detection — `google-chrome` may not be installed in all environments. Mitigation: fallback chain to `chromium`.

## Final Verification

Filled only when complete.
