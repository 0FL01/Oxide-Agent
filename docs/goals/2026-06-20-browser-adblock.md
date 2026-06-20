# Goal: Browser Live Ad Blocking

Date started: 2026-06-20
Status: active
Codex goal: not set
Source spec: RECON of `.donor/adblock-rust/` (brave/adblock-rust v0.12.5) + CDP Fetch domain protocol docs + user-approved plan
Goal doc owner: Codex
Last updated: 2026-06-20 00:00

## Objective

Implement network-level ad blocking in the browser-live sidecar using `adblock-rust` engine + CDP `Fetch.enable` request interception. Block ad/tracking requests at the network layer before they reach Chromium, improving agent decision quality (cleaner screenshots, cleaner DOM snapshots, faster page loads, privacy).

Done when every Completion Audit item is verified by its listed evidence and all out-of-scope constraints are preserved.

## Scope

In scope:
- `crates/oxide-browser-sidecar/Cargo.toml` — add `adblock` crate dependency
- `crates/oxide-browser-sidecar/src/adblock.rs` — new module: engine wrapper, filter list loading, request checking, CDP type mapping
- `crates/oxide-browser-sidecar/src/capture.rs` — `Fetch.enable` integration, `Fetch.requestPaused` handler in event loop
- `crates/oxide-browser-sidecar/src/session.rs` — pass engine to `CaptureCollector`
- `crates/oxide-browser-sidecar/src/lib.rs` — `AppState.adblock` field, pass engine to session constructor
- `crates/oxide-browser-sidecar/src/main.rs` — build engine at startup from env
- `docker/Dockerfile.browser-sidecar` — download filter lists, set env
- `docs/browser-live.md` — documentation update

Out of scope:
- Cosmetic filtering (hiding ad elements already in DOM) — Phase 2, requires CSS injection
- Scriptlet injection (uBlock Origin scriptlets) — Phase 2, requires `adblock-resources` + JS injection
- Redirect resources (`$redirect=noopjs`) — Phase 2, requires `Fetch.fulfillRequest`
- Engine serialization cache for fast startup — future optimization
- Filter list auto-update at runtime — baked into Docker image, updated on rebuild
- Per-domain allowlist — future feature

## Missing Inputs

None. All requirements derivable from RECON + donor code + CDP protocol docs.

## Repository Context

- Relevant entry points: `main.rs` (startup), `lib.rs` (AppState + routes), `session.rs` (BrowserSession::new/force_reload), `capture.rs` (CaptureCollector::start + event loop)
- Existing conventions: direct CDP over WebSocket, never call `Runtime.enable`/`Target.*`, `thiserror` for errors, `unwrap_used = "forbid"` + `unsafe_code = "forbid"` + `too_many_lines = "forbid"` + `too_many_arguments = "forbid"` in Cargo.toml lints
- Validation infrastructure: `cargo check -p oxide-browser-sidecar`, `cargo clippy --all-targets -p oxide-browser-sidecar -- -D warnings`, `cargo test -p oxide-browser-sidecar`, `cargo fmt --all -- --check`
- Risky areas: `CaptureCollector::new()` signature change affects 2 call sites (session.rs:80, session.rs:186); `Fetch.enable` is a new CDP domain — must verify it doesn't break stealth invariants; adblock crate adds transitive deps — must compile with `unsafe_code = "forbid"`

## References

- `.donor/adblock-rust/` — git clone of brave/adblock-rust v0.12.5
- `.donor/adblock-rust/src/engine.rs:324-331` — `Send + Sync` assertion when `single-thread` feature disabled
- `.donor/adblock-rust/src/request.rs:53-76` — `cpt_match_type` string → RequestType mapping
- `.donor/adblock-rust/data/easylist.to/easylist/easylist.txt` — 88,551 lines (standard ad blocking)
- `.donor/adblock-rust/data/easylist.to/easylist/easyprivacy.txt` — 56,189 lines (tracking/privacy)
- CDP Fetch domain: `Fetch.enable`, `Fetch.requestPaused`, `Fetch.continueRequest`, `Fetch.failRequest` — verified from protocol docs
- User-provided article: "Вариант A" — `adblock-rust` inside sidecar + CDP interception
- Previous goal: `docs/goals/2026-06-20-browser-stealth.md` — stealth hardening (completed, commits 487687d0..f53736f4)

## Completion Audit

### G1: adblock crate added with Send+Sync features
- Source: `.donor/adblock-rust/src/engine.rs:324-331` — Engine is `Send + Sync` when `single-thread` feature is disabled
- Acceptance: `Cargo.toml` has `adblock = { version = "0.12", default-features = false, features = ["embedded-domain-resolver", "full-regex-handling"] }`. Engine is `Send + Sync` (no `single-thread` feature). Compiles with `unsafe_code = "forbid"`.
- Evidence required: `cargo check -p oxide-browser-sidecar` green; `Cargo.toml` diff
- Status: pending
- Evidence collected:

### G2: AdblockEngine module with construction and request checking
- Source: RECON — `Engine::from_filter_set`, `Engine::check_network_request`, `Request::new`
- Acceptance: `src/adblock.rs` defines `AdblockEngine` wrapping `adblock::Engine`. `from_env()` reads `ADBLOCK_ENABLED` + `ADBLOCK_FILTERS`, builds engine from filter list files. `should_block(url, source_url, resource_type) -> bool` builds `Request` and calls `check_network_request`. Returns `false` on `Request::new` errors (malformed URLs don't block).
- Evidence required: unit tests for construction, block/allow decisions, malformed URL handling; `cargo test -p oxide-browser-sidecar` green
- Status: pending
- Evidence collected:

### G3: CDP ResourceType to adblock request type complete mapping
- Source: `.donor/adblock-rust/src/request.rs:53-76` — `cpt_match_type`; CDP protocol ResourceType enum
- Acceptance: `adblock.rs` has `cdp_type_to_adblock(cdp_type: &str) -> &str` mapping all CDP ResourceType values: Script→"script", Stylesheet→"stylesheet", Image→"image", Font→"font", Media→"media", XHR→"xhr", Fetch→"xhr", WebSocket→"websocket", Ping→"ping", EventSource→"xhr", Manifest→"other", CSPViolationReport→"csp_report", Prefetch→"other", SignedExchange→"other", Other→"other", Document→"document" (but Document is excluded from Fetch patterns).
- Evidence required: unit test covering all CDP ResourceType values; `cargo test -p oxide-browser-sidecar` green
- Status: pending
- Evidence collected:

### G4: Fetch.enable in capture.rs — only when engine present, excludes navigation
- Source: CDP protocol docs — `Fetch.enable` with `patterns` field; `ResourceType` in patterns
- Acceptance: `CaptureCollector::start()` calls `Fetch.enable` with patterns for all non-Document resource types ONLY when `engine.is_some()`. When `engine.is_none()`, `Fetch.enable` is NOT called (zero behavior change). Navigation requests (Document type) are never paused.
- Evidence required: code review; unit test verifying patterns exclude Document; `cargo test -p oxide-browser-sidecar` green
- Status: pending
- Evidence collected:

### G5: Fetch.requestPaused handler — skip navigation, check engine, fail/continue
- Source: CDP protocol — `Fetch.requestPaused` event, `Fetch.failRequest` with `BlockedByClient`, `Fetch.continueRequest`
- Acceptance: `process_event()` has a `"Fetch.requestPaused"` arm. Handler extracts `requestId`, `request.url`, `resourceType`, `request.isNavigationRequest`. Skips navigation requests (immediate `continueRequest`). Builds `Request` with `source_url` from `current_url()`, calls `engine.should_block()`. If blocked: `Fetch.failRequest` with `errorReason: "BlockedByClient"`. If not blocked: `Fetch.continueRequest`. Errors in handler log and fall through to `continueRequest` (fail-open, don't hang requests).
- Evidence required: unit tests for handler decision logic; `cargo test -p oxide-browser-sidecar` green
- Status: pending
- Evidence collected:

### G6: Engine shared via Arc across all sessions — built once at startup
- Source: RECON — `check_network_request(&self)` is immutable, `Arc<Engine>` for shared read-only
- Acceptance: `main.rs` builds `Option<Arc<AdblockEngine>>` at startup. `AppState` has `adblock: Option<Arc<AdblockEngine>>` field. `create_session` passes `state.adblock` to `BrowserSession::new()`. `force_reload()` reuses the same `Arc<AdblockEngine>`. No per-session engine rebuild.
- Evidence required: code review; `cargo check -p oxide-browser-sidecar` green
- Status: pending
- Evidence collected:

### G7: Dockerfile includes filter lists + ADBLOCK_FILTERS env
- Source: user-approved plan — EasyList + EasyPrivacy baked into image
- Acceptance: `docker/Dockerfile.browser-sidecar` downloads `easylist.txt` + `easyprivacy.txt` to `/opt/adblock/`. Sets `ADBLOCK_FILTERS=/opt/adblock/easylist.txt,/opt/adblock/easyprivacy.txt`. `ADBLOCK_ENABLED` NOT set in Dockerfile (default false = opt-in).
- Evidence required: Dockerfile review
- Status: pending
- Evidence collected:

### Q1: Stealth invariants preserved
- Source: previous stealth goal `docs/goals/2026-06-20-browser-stealth.md` — G1-G7, Q1-Q4, N1-N2
- Acceptance: `Fetch.enable` does NOT call `Runtime.enable`, `Target.setAutoAttach`, `Target.attachToTarget`, or `Console.enable`. Fetch domain has zero JS-visible side effects (no execution context changes, no console events, no global object changes). `git grep` confirms no new forbidden CDP calls.
- Evidence required: `git grep` for forbidden calls; `cargo test -p oxide-browser-sidecar` green (existing stealth tests pass)
- Status: pending
- Evidence collected:

### Q2: clippy + fmt clean
- Source: AGENTS.md — CI enforces both
- Acceptance: `cargo clippy --all-targets -p oxide-browser-sidecar -- -D warnings` and `cargo fmt --all -- --check` both pass
- Evidence required: command output clean
- Status: pending
- Evidence collected:

### Q3: Documentation updated
- Source: docs/browser-live.md
- Acceptance: `docs/browser-live.md` documents ad blocking: env vars, how it works, what it blocks/doesn't block, stealth interaction.
- Evidence required: file review
- Status: pending
- Evidence collected:

### N1: No cosmetic filtering
- Source: out of scope — Phase 2
- Must preserve: no CSS injection for cosmetic ad hiding
- Evidence required: no `url_cosmetic_resources` or `hidden_class_id_selectors` calls in sidecar src
- Status: pending
- Evidence collected:

### N2: No scriptlet injection
- Source: out of scope — Phase 2
- Must preserve: no `adblock-resources` dependency, no scriptlet JS injection
- Evidence required: no `adblock-resources` in Cargo.toml, no scriptlet injection code
- Status: pending
- Evidence collected:

### N3: No redirect resources
- Source: out of scope — Phase 2
- Must preserve: no `Fetch.fulfillRequest` with fake responses for redirect rules
- Evidence required: no `Fetch.fulfillRequest` calls in sidecar src
- Status: pending
- Evidence collected:

### N4: ADBLOCK_ENABLED=false by default — zero behavior change when disabled
- Source: user-approved plan — opt-in
- Must preserve: when `ADBLOCK_ENABLED` is unset or "false", no `Fetch.enable` is sent, no filter lists loaded, no adblock code executes. Sidecar behavior identical to pre-adblock.
- Evidence required: code review; `cargo test -p oxide-browser-sidecar` green with adblock disabled
- Status: pending
- Evidence collected:

## Implementation Plan

### Checkpoint 1: adblock crate + AdblockEngine module
- Audit IDs: G1, G2, G3
- Expected changes:
  - `Cargo.toml`: add `adblock = { version = "0.12", default-features = false, features = ["embedded-domain-resolver", "full-regex-handling"] }`
  - `src/adblock.rs`: new module — `AdblockEngine` struct, `from_env()`, `should_block()`, `cdp_type_to_adblock()`
  - `src/lib.rs`: add `pub mod adblock;`
  - Unit tests: construction from inline rules, block/allow decisions, malformed URL handling, all CDP type mappings
- Validation: `cargo check -p oxide-browser-sidecar`, `cargo test -p oxide-browser-sidecar`, `cargo clippy --all-targets -p oxide-browser-sidecar -- -D warnings`, `cargo fmt --all -- --check`
- Exit condition: tests green, audit items G1/G2/G3 verified

### Checkpoint 2: CDP Fetch integration in capture.rs
- Audit IDs: G4, G5, Q1, N4
- Expected changes:
  - `capture.rs`: `CaptureCollector` gets `engine: Option<Arc<AdblockEngine>>` field; `new(engine)` signature; `start()` calls `Fetch.enable` when engine present; `process_event()` adds `Fetch.requestPaused` arm with handler
  - `session.rs`: update `CaptureCollector::new()` call sites to pass engine
  - Unit tests: patterns exclude Document, handler decision logic (block/continue/skip-nav), fail-open on error
- Validation: `cargo test -p oxide-browser-sidecar`, `cargo clippy --all-targets -p oxide-browser-sidecar -- -D warnings`, `cargo fmt --all -- --check`
- Exit condition: tests green, audit items G4/G5/Q1/N4 verified

### Checkpoint 3: Session + AppState wiring
- Audit IDs: G6, N4
- Expected changes:
  - `session.rs`: `BrowserSession::new()` and `force_reload()` accept `Option<Arc<AdblockEngine>>`
  - `lib.rs`: `AppState` gets `adblock` field; `create_session` passes engine to session
  - `main.rs`: build engine at startup from env, pass to AppState
- Validation: `cargo check -p oxide-browser-sidecar`, `cargo test -p oxide-browser-sidecar`, `cargo clippy --all-targets -p oxide-browser-sidecar -- -D warnings`
- Exit condition: compiles, tests green, audit item G6 verified

### Checkpoint 4: Dockerfile + filter lists
- Audit IDs: G7
- Expected changes:
  - `docker/Dockerfile.browser-sidecar`: download easylist.txt + easyprivacy.txt, set ADBLOCK_FILTERS env
- Validation: Dockerfile review (no Docker build in CI for this)
- Exit condition: audit item G7 verified

### Checkpoint 5: Documentation + final verification
- Audit IDs: Q2, Q3, N1, N2, N3, N4
- Expected changes:
  - `docs/browser-live.md`: add "Ad blocking" section
  - Run full validation suite
  - `git grep` for forbidden CDP calls, cosmetic filtering, scriptlet injection, fulfillRequest
- Validation: all commands clean
- Exit condition: all audit items verified

## Validation Contract

- Static checks: `cargo check -p oxide-browser-sidecar`
- Tests: `cargo test -p oxide-browser-sidecar`
- Lint: `cargo clippy --all-targets -p oxide-browser-sidecar -- -D warnings`
- Format: `cargo fmt --all -- --check`
- Invariant grep: `git grep -n "Runtime\.enable\|Target\.setAutoAttach\|Target\.attachToTarget\|Console\.enable" crates/oxide-browser-sidecar/src/` — only in allowed comment/assertion contexts
- Non-goal grep: `git grep -n "url_cosmetic_resources\|hidden_class_id_selectors\|adblock-resources\|fulfillRequest" crates/oxide-browser-sidecar/src/` — no matches
- Done when: all Completion Audit items verified

## Decisions

- 2026-06-20: `adblock` crate with `default-features = false, features = ["embedded-domain-resolver", "full-regex-handling"]` — disables `single-thread` feature so `Engine` is `Send + Sync`, enabling `Arc<Engine>` shared access without Mutex. `check_network_request(&self)` is immutable.
- 2026-06-20: `Fetch.enable` with explicit patterns excluding Document — navigation requests are never paused. Handler still checks `isNavigationRequest` as defense-in-depth.
- 2026-06-20: Fail-open in `Fetch.requestPaused` handler — if engine check errors, `continueRequest` (don't hang requests). Blocking ads is a feature, hanging the page is a bug.
- 2026-06-20: `ADBLOCK_ENABLED=false` by default — ad blocking is opt-in. When disabled, no `Fetch.enable`, no filter list loading, zero behavior change. This preserves stealth by default.
- 2026-06-20: No engine serialization cache in this goal — building from filter lists takes 1-3s, acceptable for sidecar startup. Cache is a future optimization.
- 2026-06-20: Filter lists baked into Docker image, updated on image rebuild. No runtime auto-update.

## Progress Log

- 2026-06-20 00:00: Checkpoint 0 — goal doc created
  - Changed: `docs/goals/2026-06-20-browser-adblock.md`
  - Evidence: goal doc written
  - Commands: none
  - Audit IDs updated: none
  - Next: Checkpoint 1

## Risks and Blockers

- adblock crate transitive deps may conflict with `unsafe_code = "forbid"` — Mitigation: the lint applies to our crate code, not dependencies. Verify with `cargo check`.
- adblock crate may not compile on Rust 1.94 — Mitigation: crate is well-maintained, Rust 1.94 is recent. Verify with `cargo check`.
- `Fetch.enable` may interfere with existing `Network.enable` event flow — Mitigation: different event streams, correlated via `networkId`. Verified from CDP protocol docs. Test with existing capture tests.
- `CaptureCollector::new()` signature change — 2 call sites (session.rs:80, session.rs:186). Both need engine parameter.
- `too_many_arguments = "forbid"` clippy lint — if handler functions have too many params, use struct params.

## Final Verification

Filled only when complete.
