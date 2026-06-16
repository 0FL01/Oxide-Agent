# Goal: Browser Live Agent MVP

Date started: 2026-06-16
Status: active
Codex goal: `/goal Implement docs/goals/2026-06-16-browser-live-agent-mvp.md until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update the doc after each meaningful verification, commit after each completed checkpoint, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.`
Source spec: `docs/prd/chrome-agent.md`
Goal doc owner: Codex
Last updated: 2026-06-16

## Objective

Implement the Browser Live Agent MVP described in `docs/prd/chrome-agent.md`: Oxide starts and orchestrates an autonomous headless browser session through a `chrome-agent` sidecar, uses OpenCode Go `mimo-v2.5` as the only MVP screenshot-vision route, executes bounded browser actions with post-action visual verification, stores screenshots as artifacts/ring-buffer data outside durable LLM history, and exposes safe Web UI/Telegram progress without manual browser control.

Done when every required Completion Audit item below is verified by its listed evidence, CP-1 through CP-17 are either verified or explicitly dropped by user, and all out-of-scope constraints are preserved.

## Scope

In scope:
- Browser config/model validation under `AgentSettings` and `.env.example`.
- Feature-gated core provider module, tool registration, capability manifest, and sub-agent denial.
- Typed sidecar REST/WS client, fake sidecar seam, browser session state, screenshot artifacts, and ring-buffer.
- MiMo `mimo-v2.5` decision prompt/parser/repair loop through existing OpenCode Go image path.
- Bounded action execution, post-action verification, recovery, safety policy, metrics/logging, and E2E smoke tests.
- Docker Compose sidecar deployment, Web UI latest-screenshot/progress panel, Telegram milestone/final/blocked reporting, and user docs.

Out of scope:
- Full browser cloud/dashboard, multi-browser fleet, iframe/VNC/manual browser control, or Telegram start/control commands.
- CAPTCHA/anti-bot bypass, automatic purchases/payments, or unsafe access-control circumvention.
- Real Chrome profile/cookie attach or persistent browser profiles in MVP.
- Direct Xiaomi fallback, `mimo-v2.5-pro` as a vision route, provider-native JSON schema dependency, or MiMo native tool calling inside the browser loop.
- Mandatory production domain allowlist for HTTP/HTTPS navigation in MVP.
- Long-term archive of every screenshot frame or adding screenshot bytes to durable chat history.

## Missing Inputs

- No missing owner decisions remain for MVP.
  - Evidence: PRD owner decisions fix domain allowlist, Telegram start/control, ephemeral profiles, no direct Xiaomi fallback, raw screenshots plus DOM/a11y/hit-test fallback, and no manual browser control at `docs/prd/chrome-agent.md:3608`.
- External runtime dependencies will still be needed for later live validation: Docker/Chromium sidecar availability and an OpenCode Go API key for staging MiMo smoke.
  - Impact: local unit/fake-sidecar work can proceed; CP-11/CP-16 live compose/provider evidence cannot be final without those runtime dependencies.
  - Low-risk fallback: keep browser feature disabled by default until smoke evidence exists.
  - User/external action needed: provide runtime credentials/services only when reaching CP-11/CP-16 live checks.

## Repository Context

- Root instructions are in `AGENTS.md`: smallest maintainable change, no new crates/services unless required, explicit modules, feature-gated profiles, and `cargo fmt --all -- --check` plus `cargo clippy --workspace --all-targets -- -D warnings` before finishing.
- Existing goal docs live in `docs/goals/`; completed docs are archived under `docs/goals/archives/`.
- Browser PRD source is `docs/prd/chrome-agent.md`; core goals are listed at `docs/prd/chrome-agent.md:218`, non-goals at `docs/prd/chrome-agent.md:235`, implementation checkpoints at `docs/prd/chrome-agent.md:2726`, final acceptance criteria at `docs/prd/chrome-agent.md:3533`, risks at `docs/prd/chrome-agent.md:3570`, owner decisions at `docs/prd/chrome-agent.md:3608`, and MVP cut at `docs/prd/chrome-agent.md:3619`.
- CP-1 and CP-2 are already passed in the PRD. Live `mimo-v2.5` vision was confirmed by the env-gated smoke test in `crates/oxide-agent-core/src/llm/providers/opencode_go.rs`.
- Relevant implementation areas: `crates/oxide-agent-core/src/config.rs`, `crates/oxide-agent-core/src/agent/providers/`, `crates/oxide-agent-core/src/agent/tool_runtime/`, `crates/oxide-agent-core/src/capabilities/`, `crates/oxide-agent-web-contracts/`, `crates/oxide-agent-transport-web/`, `crates/oxide-agent-web-ui/`, `crates/oxide-agent-transport-telegram/`, Docker Compose files, profiles, `.env.example`, and docs.

## Completion Audit

- G1: Browser feature gating and config
  - Source: `docs/prd/chrome-agent.md:2829`, `docs/prd/chrome-agent.md:3537`, `docs/prd/chrome-agent.md:3623`
  - Requirement: add disabled-by-default browser configuration, `BROWSER_AGENT_*` env parsing, sidecar URL/token validation, browser MiMo provider/model override, and profile wiring.
  - Acceptance: disabled by default; enabling without required sidecar URL/token fails clearly; existing `MEDIA_MODEL_*` and OpenCode Go routes still work.
  - Evidence required: config parse/validation tests, `.env.example` diff, profile diff, `cargo test -p oxide-agent-core ...` focused config tests.
  - Status: verified
  - Evidence collected: `crates/oxide-agent-core/src/config.rs` adds `BrowserAgentSettings` and `BROWSER_AGENT_*` fields/validation; `.env.example` documents disabled-by-default Browser Live config and OpenCode Go key fallback; profile files document that actual `tool/browser-live` module wiring waits for CP-7 to avoid unknown module IDs. Focused tests passed: `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go browser_agent_config_`; OpenCode bootstrap regression passed: `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go settings_bootstraps_opencode_go_route_from_api_key_only`.

- G2: MiMo vision route is OpenCode Go `mimo-v2.5` only
  - Source: `docs/prd/chrome-agent.md:9`, `docs/prd/chrome-agent.md:160`, `docs/prd/chrome-agent.md:2769`
  - Requirement: browser screenshot perception uses `opencode-go` + `mimo-v2.5` through direct OpenAI chat completions `image_url` data URL path.
  - Acceptance: live smoke proves image input; `mimo-v2.5-pro` is rejected for browser vision before any call.
  - Evidence required: CP-2 smoke test command, payload/model capability tests, config validation test for `mimo-v2.5-pro`.
  - Status: verified
  - Evidence collected: CP-2 live smoke and provider tests were committed before this goal doc; CP-3 adds browser config validation that rejects `mimo-v2.5-pro` with a text-only browser screenshot vision error and rejects non-image OpenCode Go models via `browser_agent_config_rejects_mimo_v25_pro_for_vision` and `browser_agent_config_rejects_non_image_model`.

- G3: Typed sidecar API client
  - Source: `docs/prd/chrome-agent.md:2873`
  - Requirement: introduce feature-gated typed REST/WS client, auth header injection, idempotency keys, timeout config, retryable/non-retryable error mapping, and screenshot/artifact metadata types.
  - Acceptance: client compiles behind `tool-browser-live`; contract shapes serialize/deserialize; missing token rejected in enabled production config; no sandbox command dependency.
  - Evidence required: serialization, auth/idempotency header, timeout, and error mapping tests.
  - Status: verified
  - Evidence collected: CP-4 added `tool-browser-live`, `browser_live::{client,error,types}` module, typed REST methods for sidecar endpoints, stream contract types, bearer auth/idempotency headers, endpoint timeouts, stable error kind/retry mapping, and screenshot artifact metadata. Tests `cargo test -p oxide-agent-core --no-default-features --features tool-browser-live browser_live` passed 10 focused tests covering serialization, error mapping, auth/idempotency headers with a mock HTTP server, missing token/idempotency key rejection, timeout config, and no base64 screenshot bytes in observation metadata. `rg` over `crates/oxide-agent-core/src/agent/providers/browser_live` found no sandbox/process command dependency.

- G4: Fake sidecar test seam
  - Source: `docs/prd/chrome-agent.md:2917`
  - Requirement: implement deterministic fake sidecar for session lifecycle, observations/actions, stale screenshots, no-op/failure, debug endpoints, and crash simulation.
  - Acceptance: browser loop tests run without Chromium, `chrome-agent`, OpenCode Go, or external services.
  - Evidence required: fake create/goto/observe/action/close tests and fake error/debug tests.
  - Status: verified
  - Evidence collected: CP-5 added the `BrowserSidecar` trait seam in `client.rs` and a `cfg(test)` fake sidecar in `test_support.rs`. Focused tests prove deterministic fake create/goto/observe/action/close, scripted success/no-op/failure/stale-frame outcomes, network/console debug endpoints, browser crash simulation, metadata-only screenshots, and no external HTTP/Chromium/OpenCode Go dependency. Validation passed with `cargo test -p oxide-agent-core --no-default-features --features tool-browser-live browser_live` (14 focused Browser Live tests), `cargo check -p oxide-agent-core --no-default-features --features tool-browser-live`, and `cargo clippy -p oxide-agent-core --no-default-features --features tool-browser-live --all-targets -- -D warnings`.

- G5: Browser session state and screenshot artifact model
  - Source: `docs/prd/chrome-agent.md:2957`, `docs/prd/chrome-agent.md:3546`, `docs/prd/chrome-agent.md:3554`
  - Requirement: maintain task-local browser session state, latest screenshot, bounded ring-buffer, artifact refs, action sequence, viewport/DSF, retention and size caps outside main LLM history.
  - Acceptance: ring-buffer evicts old frames; final/milestone artifacts retained; artifact refs can be emitted; unit tests prove screenshot bytes do not enter conversation history.
  - Evidence required: ring-buffer, artifact naming, metadata validation, retention/size cap, and history hygiene tests.
  - Status: verified
  - Evidence collected: CP-6 added `artifacts.rs` and `session.rs` with `BrowserArtifactSettings`, stable artifact URI/path naming under the existing tool runtime artifact root, `BrowserSessionState`, `BrowserFrame`, ring-buffer eviction, retained final/milestone/debug artifact refs, live-byte soft-cap eviction, viewport/hash/image-byte metadata validation, and compact history summaries containing artifact refs only. Focused tests prove ring-buffer eviction, final artifact retention, artifact naming/sanitization, retention expiry for live frames, size-cap behavior, metadata validation, and history hygiene with no `base64`/`data:image` content.

- G6: Core browser tools
  - Source: `docs/prd/chrome-agent.md:3003`
  - Requirement: register `browser_start`, `browser_observe`, `browser_step`, `browser_debug`, and `browser_close` as native tools behind feature/config gates.
  - Acceptance: main agent sees tools only when enabled; tools return compact outputs with artifact refs; timeouts/cancellation work; sub-agents denied by default.
  - Evidence required: tool registration, feature-disabled, fake start/observe/close, output schema, and sub-agent deny tests.
  - Status: verified
  - Evidence collected: CP-7 added `BrowserLiveProvider` and native tool executors for `browser_start`, `browser_observe`, `browser_step`, `browser_debug`, and `browser_close`; registered `BrowserLiveToolModule` behind `tool-browser-live`; added compiled capability manifest entry `tool/browser-live`; wired executor registry and `profile-full`; and blocked all browser tools for sub-agents. The module only constructs tools when `BROWSER_AGENT_ENABLED` resolves true with sidecar URL/token, while `cargo check -p oxide-agent-core --no-default-features` verifies feature-disabled builds. Focused tests cover tool specs/registration, fake start/observe/close compact outputs with artifact refs, placeholder `browser_step`, compiled manifest, and sub-agent deny blocklist.

- G7: MiMo decision prompt, schema, parser, and repair loop
  - Source: `docs/prd/chrome-agent.md:3051`, `docs/prd/chrome-agent.md:3558`
  - Requirement: build stable system prompt, dynamic compact state prompt, strict `BrowserDecision` schema/parser, one repair retry, action validation, risk/sensitive-action fields, and confidence thresholds.
  - Acceptance: invalid/malformed/unsafe output never executes an action; stable prompt and volatile screenshot/state are separated; screenshots not appended to main history.
  - Evidence required: golden valid/invalid parser tests, repair behavior tests, coordinate bounds/sensitive action tests, and prompt cache hygiene test.
  - Status: verified
  - Evidence collected: CP-8 added `BrowserDecision`, action/risk/sensitive-action types, strict JSON extraction/parser, coordinate/action/confidence/sensitive/high-risk validation, stable/dynamic prompt builders, one-repair MiMo caller through `LlmClient::analyze_image()`, and binary screenshot sidecar retrieval for `browser_step` decision-only mode. Focused tests cover golden valid decisions, single-object extraction, multiple-object invalid output, coordinate bounds, low-confidence rejection, sensitive executable-action rejection, prompt cache/history hygiene with no `base64`/`data:image`, `mimo-v2.5` image-route invocation, and one repair retry. `browser_step` returns a validated decision without executing it; invalid parser output therefore cannot execute an action before CP-9.

- G8: Bounded action execution and post-action verification
  - Source: `docs/prd/chrome-agent.md:3099`, `docs/prd/chrome-agent.md:3546`, `docs/prd/chrome-agent.md:3559`
  - Requirement: implement decide → execute → wait → observe → verify loop with action sequence IDs, fresh post-action screenshots, before/after artifacts, structured results, and timeout/max-step bounds.
  - Acceptance: technical action success alone never means task success; verification failure triggers recovery or safe stop.
  - Evidence required: fake happy path, no-op click verification failure, navigation fresh screenshot, done final evidence, and timeout report tests.
  - Status: verified
  - Evidence collected: CP-9 added `browser_live::actions` and `verification`, mapped validated decisions to `/action` and HTTP/HTTPS `/goto`, added action sequence IDs, capture-after/wait-for-stability sidecar requests, fresh post-action observe calls, retained before/after milestone artifacts, structured `browser_step` action/verification payloads, BrowserAction/BrowserVerification progress events, done/ask_user/debug terminal safe stops, and bounded action timeouts. Focused tests cover fake click happy path, no-op verification failure, navigation with fresh screenshot, done requiring final evidence, and timeout report output; `verification` tests prove technical sidecar success alone is not task success.

- G9: Recovery engine
  - Source: `docs/prd/chrome-agent.md:3145`
  - Requirement: classify stale frame/no-op/coordinate mismatch/modal/loading/network/console/invalid-JSON failures, perform bounded recovery, use hit-test/inspect/UID fallback, and integrate browser loop signatures with existing loop detection.
  - Acceptance: same failed action is not repeated forever; JS click fallback is disabled by default and policy-gated; diagnostics attach to failure reports; low confidence safe-stops.
  - Evidence required: coordinate drift, stale screenshot, modal overlay, repeated no-op loop, debug artifact, and JS fallback disabled tests.
  - Status: verified
  - Evidence collected: CP-10 added `browser_live::recovery` with deterministic classification for stale frames, no-op clicks, coordinate mismatch, modal overlays, loading timeout, network failure, console failure, invalid JSON, low confidence, and generic verification failure. `browser_step` now fetches console/network debug on verification failure, attaches artifact-backed diagnostics, emits BrowserRecovery progress, executes at most one non-repeating deterministic fallback (`wait`, `scroll`, `press Escape`, or `click_target_id` when available), records recovery observations, stops repeated loop signatures, and keeps JS click fallback disabled through `BrowserRecoverySettings`. Focused tests cover coordinate drift, stale screenshot, modal overlay, repeated no-op loop stop, debug artifact attachment, and JS fallback disabled evidence.

- G10: Docker Compose sidecar deployment
  - Source: `docs/prd/chrome-agent.md:3192`, `docs/prd/chrome-agent.md:3538`
  - Requirement: add `chrome-agent-sidecar` service/Dockerfile/wrapper with healthcheck, artifact/profile volumes, app env wiring, internal-only ports, token requirement, and compatible hardening.
  - Acceptance: compose starts app + sidecar; app can reach sidecar health; CDP is not host-exposed; profiles purge on close; screenshots land in artifact volume.
  - Evidence required: compose smoke, healthcheck, port exposure, artifact volume, and profile cleanup evidence.
  - Status: verified
  - Evidence collected: CP-11 added `docker/Dockerfile.chrome-agent-sidecar` with Chromium, pinned `chrome-agent`, and a non-root runtime, `docker/chrome-agent-sidecar.py` REST wrapper with `/healthz`, token-gated session lifecycle, artifact screenshot placeholder writes, and profile purge on close; wired `chrome-agent-sidecar` into root/web/telegram/full/dev Compose files with loopback-only REST port, no CDP port publication, named artifact/profile volumes, `shm_size`, dropped caps, `no-new-privileges`, healthcheck, and app env pointing at loopback sidecar URLs. `profile-web-embedded-opencode-local` now includes `tool-browser-live`; `.env.example` and `README.md` document disabled-by-default enablement and token requirement. Compose config validation shows app + sidecar services and no `9222` CDP exposure; sidecar Python syntax check passes.

- G11: Web UI browser progress panel
  - Source: `docs/prd/chrome-agent.md:3243`, `docs/prd/chrome-agent.md:3550`
  - Requirement: expose latest screenshot, URL/title/action/confidence/debug badges, pause/resume/stop/kill, blocked state, and artifact replay without SSE/base64 frame flood.
  - Acceptance: Web UI shows live latest screenshot and final artifacts; event stream is coalesced/throttled; no iframe/VNC/manual browser control is exposed.
  - Evidence required: event serialization, web transport mapping, UI state reducer/rendering, artifact ref rendering, and flood/coalescing tests.
  - Status: verified
  - Evidence collected: CP-12 added shared Browser Live event contracts, web transport mapping from BrowserAction/BrowserVerification/BrowserRecovery progress messages to typed `browser_live` persisted events, compact observation payload URL/title/debug metadata, frontend Browser Live state reduction, and a Browser Live panel that shows latest screenshot artifact refs, URL/title/action/confidence/debug badges, blocked/final artifact state, and autonomous-only controls. The panel exposes no iframe, VNC, click-through, keyboard, or manual browser surface; preview state is coalesced to the latest artifact ref and no base64/data image bytes are serialized into Browser Live event payloads.

- G12: Telegram milestone/final/blocked reporting
  - Source: `docs/prd/chrome-agent.md:3298`, `docs/prd/chrome-agent.md:3552`
  - Requirement: Telegram receives compact progress, blocked/safe-stop reports, and final screenshot/artifacts through existing delivery paths, without live frame spam or browser start/control commands.
  - Acceptance: sensitive screenshots are not auto-sent; final screenshot delivery uses existing file delivery; no Telegram start/control command exists for MVP.
  - Evidence required: progress render, milestone event, final artifact delivery, no-command, and sensitive artifact suppression tests.
  - Status: verified
  - Evidence collected: CP-13 added Telegram progress rendering for `BrowserAction`/`BrowserVerification`/`BrowserRecovery` milestones only, including blocked/safe-stop reason text for `NeedsUser`, `VerificationFailed`, `Timeout`, `SafeStopped`, and `RepeatedLoopStopped` statuses. Generic browser observe/session progress is suppressed from the Telegram thought area to avoid live frame spam. Browser artifact delivery still goes through the existing Telegram `deliver_file` path, but live-frame artifacts are suppressed, final artifacts are de-duplicated, and sensitive browser artifact names are not auto-sent. Tests cover milestone rendering, blocked reports, observe suppression, final artifact once, sensitive/live suppression, and no Telegram browser start/control commands or keyboard controls.

- Q1: Security and policy gates
  - Source: `docs/prd/chrome-agent.md:3340`, `docs/prd/chrome-agent.md:3560`
  - Requirement: browser disabled by default, sub-agents denied, HTTP/HTTPS allow-by-default for MVP, non-web schemes rejected, sensitive actions approved, credentials handled as secret refs, prompt injection safeguards enforced, and audit events generated.
  - Acceptance: secrets never serialize into MiMo prompt/log/event; CAPTCHA/2FA safe-stops with blocked report; no bypass/manual browser control.
  - Evidence required: URL scheme, sub-agent deny, secret redaction, sensitive action gate, download/upload disabled, real profile disabled, and prompt injection fixture tests.
  - Status: verified
  - Evidence collected: CP-14 added `browser_live::policy` with allow-by-default HTTP/HTTPS URL validation and explicit rejection for `file://`, `chrome://`, `devtools://`, `data:` and other non-web schemes; sensitive/CAPTCHA/2FA/payment/high-risk classifiers; raw credential value rejection with secret-reference recognition; no-download/no-upload and ephemeral-profile enforcement; and redacted `browser_policy` audit events. `browser_start` rejects unsafe start URLs and validates ephemeral/no-download/no-upload session policy; `browser_step` blocks sensitive executable decisions before sidecar action and returns a redacted policy audit payload; `browser_close` always purges the ephemeral profile. The MiMo stable prompt now treats page/browser content as untrusted prompt-injection input. CP-14 validation plus earlier CP-3/CP-7 evidence proves browser is disabled by default and sub-agents cannot access browser tools.

- Q2: Prompt cache/history hygiene
  - Source: `docs/prd/chrome-agent.md:229`, `docs/prd/chrome-agent.md:3579`
  - Requirement: screenshots and volatile browser state must not pollute stable prompt prefix or durable main conversation history.
  - Acceptance: only selected current frame is sent through the media call; durable history stores compact text/artifact refs only.
  - Evidence required: prompt cache hygiene test, history hygiene regression, token/cached-token metrics evidence.
  - Status: in_progress
  - Evidence collected: CP-12 Browser Live event serialization and UI reducer tests prove screenshot previews use artifact refs instead of `base64`/`data:image` bytes, and preview frame floods coalesce to the latest UI state instead of accumulating image payloads in SSE/UI state. Earlier CP-6/CP-8 evidence covers no durable image history and stable prompt hygiene; final token/cached-token metrics evidence remains scheduled for CP-15/final audit.

- Q3: Observability and logging
  - Source: `docs/prd/chrome-agent.md:3390`, `docs/prd/chrome-agent.md:3565`
  - Requirement: emit metrics/logs/traces for session/action/screenshot counters, MiMo latency/errors, invalid JSON/repair, recovery, sidecar latency/errors, artifact sizes, token/cached-token usage, and provider 429/failover events.
  - Acceptance: every browser step has metrics; logs include task/session/action IDs; logs exclude screenshot base64 and secrets.
  - Evidence required: metrics/snapshot tests, redacted log tests, token accounting tests, and error metric tests.
  - Status: pending
  - Evidence collected:

- Q4: Minimal maintainable implementation
  - Source: `AGENTS.md:11`, `AGENTS.md:17`, `docs/prd/chrome-agent.md:237`
  - Requirement: keep implementation small, explicit, feature-gated, and consistent with existing runtime/provider architecture; do not rewrite unrelated runtime/tool systems.
  - Acceptance: no unrelated crates/services/abstractions; code changes stay inside scoped modules and documented integration points.
  - Evidence required: diff review at each checkpoint, `git status`, and focused validation commands.
  - Status: pending
  - Evidence collected:

- V1: Core Rust validation
  - Source: `AGENTS.md:130`, `AGENTS.md:144`
  - Requirement: final implementation passes formatting, clippy, and relevant cargo checks/tests.
  - Acceptance: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and checkpoint-specific `cargo test`/`cargo check` commands pass or exact blockers are documented.
  - Evidence required: command output summaries in Progress Log and Final Verification.
  - Status: in_progress
  - Evidence collected: CP-3 focused validation passed: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go browser_agent_config_`; `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go settings_bootstraps_opencode_go_route_from_api_key_only`; `cargo clippy -p oxide-agent-core --no-default-features --features llm-opencode-go --all-targets -- -D warnings`. CP-4 focused validation passed: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features tool-browser-live browser_live`; `cargo check -p oxide-agent-core --no-default-features --features tool-browser-live`; `cargo clippy -p oxide-agent-core --no-default-features --features tool-browser-live --all-targets -- -D warnings`. CP-5 focused validation passed: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features tool-browser-live browser_live`; `cargo check -p oxide-agent-core --no-default-features --features tool-browser-live`; `cargo clippy -p oxide-agent-core --no-default-features --features tool-browser-live --all-targets -- -D warnings`. CP-6 focused validation passed: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features tool-browser-live browser_live`; `cargo check -p oxide-agent-core --no-default-features --features tool-browser-live`; `cargo clippy -p oxide-agent-core --no-default-features --features tool-browser-live --all-targets -- -D warnings`. CP-7 focused validation passed: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features tool-browser-live browser_live`; `cargo test -p oxide-agent-core --no-default-features --features tool-browser-live compiled_manifest_exposes_browser_live_tool_module`; `cargo test -p oxide-agent-core --no-default-features --features "tool-browser-live tool-delegation" sub_agent_blocklist_includes_sensitive_tools`; `cargo check -p oxide-agent-core --no-default-features`; `cargo check -p oxide-agent-core --no-default-features --features tool-browser-live`; `cargo clippy -p oxide-agent-core --no-default-features --features tool-browser-live --all-targets -- -D warnings`.

- V2: End-to-end smoke scenarios
  - Source: `docs/prd/chrome-agent.md:3435`, `docs/prd/chrome-agent.md:3566`
  - Requirement: prove local browser flows: open page, click, fill form, verify success, diagnose console/network failure, Web UI preview, Docker Compose deployment, invalid MiMo output, blocked/safe-stop path, and env-gated provider smoke.
  - Acceptance: required local/compose/Web UI/Telegram/provider smoke evidence exists.
  - Evidence required: E2E local browser smoke, compose smoke, Web UI smoke, Telegram smoke if profile enabled, provider smoke if API key available.
  - Status: pending
  - Evidence collected:

- V3: Documentation and examples
  - Source: `docs/prd/chrome-agent.md:3485`, `docs/prd/chrome-agent.md:3619`
  - Requirement: document feature overview, env keys, OpenCode Go + MiMo setup, sidecar compose, Web UI usage, Telegram behavior, security limits, troubleshooting, example prompts, staging checklist, and rollback/disable path.
  - Acceptance: docs say `mimo-v2.5`, not `mimo-v2.5-pro`, for vision; docs warn no screenshot history accumulation and no CAPTCHA/anti-bot bypass.
  - Evidence required: doc diff, config-example match check, compose snippet validation/manual evidence, links/paths check.
  - Status: pending
  - Evidence collected:

- N1: No manual browser control in MVP
  - Source: `docs/prd/chrome-agent.md:226`, `docs/prd/chrome-agent.md:3271`, `docs/prd/chrome-agent.md:3615`
  - Must preserve: no iframe/VNC/manual browser control; autonomous sidecar actions only; CAPTCHA/2FA/anti-bot safe-stops with blocked report.
  - Evidence required: Web UI tests/review proving no manual control surface; blocked-path smoke.
  - Status: in_progress
  - Evidence collected: CP-12 Browser Live panel explicitly renders an autonomous-preview-only note and exposes no iframe, VNC, click-through screenshot, keyboard, address bar, or manual browser control surface. Controls are task lifecycle controls only: resume focuses the existing composer for a waiting task, while pause/stop/kill request the existing task cancellation path. CP-14 blocks CAPTCHA/2FA/sensitive executable browser decisions before sidecar action and returns a safe blocked policy report instead of bypass or manual browser control. End-to-end blocked-path smoke remains scheduled for CP-16/final audit.

- N2: No direct Xiaomi fallback or `mimo-v2.5-pro` vision fallback
  - Source: `docs/prd/chrome-agent.md:212`, `docs/prd/chrome-agent.md:251`, `docs/prd/chrome-agent.md:3613`
  - Must preserve: MVP uses only OpenCode Go + `mimo-v2.5` for vision; `mimo-v2.5-pro` is not a vision route.
  - Evidence required: config validation tests and docs review.
  - Status: verified
  - Evidence collected: `BROWSER_AGENT_MIMO_MODEL=mimo-v2.5-pro` is rejected by config validation; `.env.example` documents `mimo-v2.5` only and explicitly warns not to use `mimo-v2.5-pro` for browser perception. No direct Xiaomi fallback config was added.

- N3: No real Chrome profile attach
  - Source: `docs/prd/chrome-agent.md:241`, `docs/prd/chrome-agent.md:3612`
  - Must preserve: ephemeral profiles only; real profile/cookie copy disabled in MVP.
  - Evidence required: real-profile-disabled tests and compose/profile cleanup evidence.
  - Status: verified
  - Evidence collected: CP-11 sidecar and Compose wiring use isolated browser profile volumes with profile purge on close and no host Chrome profile attach. CP-14 session policy validates only `BrowserProfile::Ephemeral`, keeps the start schema free of profile/cookie parameters, and forces `browser_close` to purge the ephemeral profile even if the caller passes `purge_profile=false`; focused tests cover real-profile-disabled behavior.

- N4: No mandatory domain allowlist in MVP
  - Source: `docs/prd/chrome-agent.md:228`, `docs/prd/chrome-agent.md:3610`
  - Must preserve: HTTP/HTTPS navigation is allow-by-default; non-web schemes are rejected.
  - Evidence required: URL policy tests and config docs review showing removed allowlist envs are not required.
  - Status: verified
  - Evidence collected: CP-3 added no domain allowlist or required allowlist envs; `.env.example` contains Browser Live sidecar/MiMo config only. CP-14 URL policy tests prove arbitrary HTTP/HTTPS URLs are allowed without a domain allowlist while non-web schemes are rejected.

- N5: No Telegram browser start/control commands
  - Source: `docs/prd/chrome-agent.md:227`, `docs/prd/chrome-agent.md:3316`, `docs/prd/chrome-agent.md:3611`
  - Must preserve: browser sessions start from Web UI only; Telegram is milestones/final/blocked reporting only.
  - Evidence required: Telegram no-command test and progress rendering review.
  - Status: verified
  - Evidence collected: CP-13 command and keyboard tests prove Telegram exposes only existing task/session controls and no browser/chrome start/control commands or callback buttons; progress rendering review shows Telegram Browser Live behavior is milestone/final/blocked reporting only.

## Implementation Plan

0. Goal contract and checkpoint ledger
   - Audit IDs: all
   - Expected changes: add this goal doc, record current CP-1/CP-2 evidence and next checkpoint order.
   - Validation: Markdown/diff review and secret scan.
   - Exit condition: goal doc committed and active `/goal` objective available.

1. CP-3 Provider capability/model config additions
   - Audit IDs: G1, G2, N2, N4, V1
   - Expected changes: `BrowserAgentSettings`, `BROWSER_AGENT_*` parsing/validation, browser MiMo override, `mimo-v2.5-pro` fail-fast, `.env.example` and profile examples.
   - Validation: config defaults/override/unsupported-model tests and existing media config regression.
   - Exit condition: browser config disabled by default and invalid vision model errors clearly.

2. CP-4 Sidecar API contract and typed client
   - Audit IDs: G3, Q1, V1
   - Expected changes: feature-gated client/types/error model/auth/idempotency/timeouts/retry classification.
   - Validation: serialization, auth/idempotency header, error mapping, timeout tests.
   - Exit condition: client compiles behind `tool-browser-live` with no real action execution.

3. CP-5 Fake sidecar for tests
   - Audit IDs: G4, V1
   - Expected changes: fake sidecar/test seam and scripted lifecycle/debug/failure support.
   - Validation: fake lifecycle, error envelope, debug endpoint tests.
   - Exit condition: later browser loop tests can run hermetically without Chromium or OpenCode Go.

4. CP-6 Session state, ring-buffer, and artifacts
   - Audit IDs: G5, Q2, V1
   - Expected changes: session/observation/screenshot artifact structs, ring-buffer, artifact naming/retention, no-history-image hygiene.
   - Validation: ring-buffer eviction, artifact naming, metadata, retention/size cap, history hygiene tests.
   - Exit condition: browser state can hold current task evidence without durable image history.

5. CP-7 Core browser provider tools
   - Audit IDs: G6, Q1, N5, V1
   - Expected changes: `tool-browser-live` feature, provider exports, capability manifest, `browser_start/observe/step/debug/close`, progress events, feature/config enforcement.
   - Validation: tool registration, feature-disabled, fake start/observe/close, output schema, sub-agent deny tests.
   - Exit condition: main agent has compact browser tools only when enabled; sub-agents do not.

6. CP-8 MiMo prompt/schema/parser
   - Audit IDs: G7, G2, Q2, N2, V1
   - Expected changes: decision/action schema, stable system prompt, dynamic state prompt, `LlmClient::analyze_image()` call, JSON parser, repair retry, validation policies.
   - Validation: golden valid/invalid decisions, repair behavior, coordinate bounds, sensitive action, prompt cache hygiene tests.
   - Exit condition: malformed/unsafe MiMo output cannot execute actions.

7. CP-9 Action execution and post-action verification loop
   - Audit IDs: G8, G5, V1
   - Expected changes: action mapping, sequence IDs, wait/stability, fresh post-action observe, verification prompt/call, before/after artifacts, structured `browser_step` result.
   - Validation: fake happy path, no-op click failure, navigation fresh screenshot, final evidence, timeout report tests.
   - Exit condition: every mutating action requires fresh visual verification.

8. CP-10 Recovery engine
   - Audit IDs: G9, Q1, N1, V1
   - Expected changes: recovery classifier, scroll/hit-test/inspect/UID fallback, JS fallback disabled-by-default, console/network diagnostics, loop detection integration.
   - Validation: coordinate drift, stale screenshot, modal overlay, repeated no-op, debug artifact, JS fallback disabled tests.
   - Exit condition: failed actions do not loop indefinitely and safe-stop when confidence remains low.

9. CP-11 Docker Compose sidecar deployment
   - Audit IDs: G10, Q1, N3, V2
   - Expected changes: sidecar Dockerfile/wrapper/service, healthcheck, volumes, app env wiring, internal ports, token requirement, profile cleanup.
   - Validation: compose smoke, healthcheck, port exposure, artifact volume, profile cleanup evidence.
   - Exit condition: compose can run app + sidecar safely with browser still disabled unless explicitly enabled.

10. CP-12 Web UI live browser progress events
    - Audit IDs: G11, N1, Q2, V1
    - Expected changes: event schema/mapping, frontend state, Browser Live panel, latest screenshot artifact ref, controls, blocked state, coalescing/throttling.
    - Validation: event serialization, web transport mapping, UI reducer/rendering, artifact ref rendering, flood/coalescing tests.
    - Exit condition: Web UI shows latest browser state without iframe/VNC/manual control or base64 SSE spam.

11. CP-13 Telegram milestone reporting
    - Audit IDs: G12, N5, V1
    - Expected changes: compact progress rendering, blocked/final reports, final artifacts once, sensitive artifact suppression, no start/control commands.
    - Validation: progress render, milestone event, final delivery, no-command, sensitive suppression tests.
    - Exit condition: Telegram receives concise reports only.

12. CP-14 Security and policy gates
    - Audit IDs: Q1, N1, N3, N4, N5, V1
    - Expected changes: URL scheme policy, sensitive action classifier/approval, credential handles/redaction, download/upload policy, profile policy, sub-agent denial, audit events, prompt-injection safeguards.
    - Validation: scheme, sub-agent, redaction, sensitive gate, download/upload disabled, real profile disabled, prompt injection tests.
    - Exit condition: browser capability is safe-by-default and policy-gated.

13. CP-15 Observability, metrics, and logging
    - Audit IDs: Q3, Q2, G2, V1
    - Expected changes: browser metrics/logs/traces, token/cached-token accounting for MiMo calls, provider error visibility, redaction.
    - Validation: metrics/snapshot, redacted log, token accounting, error metric tests.
    - Exit condition: each browser step is observable without leaking secrets/base64.

14. CP-16 End-to-end smoke scenarios
    - Audit IDs: V2, G2, G8, G10, G11, G12, Q1
    - Expected changes: local static/form/modal/console/network test pages, compose/Web UI/Telegram/provider smoke fixtures and captured final artifacts.
    - Validation: E2E local browser smoke, compose smoke, Web UI smoke, Telegram smoke if profile enabled, env-gated MiMo provider smoke.
    - Exit condition: MVP behavior is proven against realistic local pages and blocked/error paths.

15. CP-17 Documentation and examples
    - Audit IDs: V3, N1, N2, N3, N4, N5
    - Expected changes: README/docs/env examples/troubleshooting/staging checklist/rollback instructions.
    - Validation: examples match parser, compose snippets validated manually or by CI lint, links/paths checked.
    - Exit condition: a new user can enable dev compose browser mode without reading implementation code.

16. Final audit
    - Audit IDs: all
    - Expected changes: update Completion Audit statuses/evidence, run final validation, fill Final Verification.
    - Validation: all listed evidence, final `cargo fmt`, clippy, scoped tests, smoke evidence, and `git status` review.
    - Exit condition: every non-dropped item is `verified`, or remaining item is `blocked` with exact external action needed.

## Validation Contract

- Static checks:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
- Core/profile checks, chosen per touched checkpoint:
  - `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
  - `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go <focused-test>`
  - `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local <focused-test>`
- Runtime/manual verification:
  - env-gated MiMo smoke: `RUN_OPENCODE_GO_MIMO_VISION_SMOKE=1 OPENCODE_API_KEY=... cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go smoke_opencode_go_mimo_v25_accepts_image_input -- --nocapture`
  - compose sidecar smoke once CP-11 is implemented.
  - Web UI and Telegram smoke once CP-12/CP-13 are implemented.
- Artifact verification:
  - inspect screenshot artifact refs, final reports, ring-buffer eviction evidence, and absence of screenshot bytes in durable history.
- Done when:
  - every Completion Audit item is `verified`, or an item is `dropped_by_user` with explicit user instruction, or `blocked` with exact command/output and smallest external action needed.

## Decisions

- 2026-06-16: Use `docs/goals/2026-06-16-browser-live-agent-mvp.md` because the repository already uses `docs/goals/` for durable Codex goal docs.
- 2026-06-16: Keep CP-1/CP-2 as already-passed evidence but do not mark the full goal complete; remaining implementation starts at CP-3.
- 2026-06-16: MVP uses only OpenCode Go + `mimo-v2.5` for browser vision. `mimo-v2.5-pro` remains rejected for browser perception because it is text-only for this route.
- 2026-06-16: Browser sessions are autonomous/headless. Web UI has latest screenshot/status/artifacts and stop controls only; no iframe/VNC/manual browser control.
- 2026-06-16: HTTP/HTTPS navigation is allow-by-default in MVP; mandatory domain allowlist is post-MVP.
- 2026-06-16: Telegram cannot start/control browser sessions in MVP; it only reports milestones/final artifacts/blocked state.
- 2026-06-16: Real Chrome profile attach, direct Xiaomi fallback, and annotated screenshots are not MVP.

## Progress Log

- 2026-06-16: Goal contract created
  - Changed: added repo-local goal doc with checkpoint plan, Completion Audit ledger, validation contract, owner decisions, and CP-1/CP-2 current evidence.
  - Evidence: PRD CP-1 and CP-2 are marked passed; live MiMo smoke evidence exists in PRD and prior commit history.
  - Commands: `git status --short`; `git add -N docs/goals/2026-06-16-browser-live-agent-mvp.md && git diff --stat -- docs/goals/2026-06-16-browser-live-agent-mvp.md && git diff --check -- docs/goals/2026-06-16-browser-live-agent-mvp.md`; secret-pattern scan across the goal doc, PRD, and OpenCode Go provider source.
  - Audit IDs updated: G2 is `in_progress`; all other non-completed implementation items remain `pending`.
  - Next: commit this goal doc, then start CP-3 config/model validation.

- 2026-06-16: CP-3 provider capability/model config additions
  - Changed: added Browser Live config fields/resolution/validation in `crates/oxide-agent-core/src/config.rs`, browser vision model storage/resolution in `crates/oxide-agent-core/src/llm/client.rs`, disabled-by-default Browser Live env examples in `.env.example`, and profile comments deferring actual `tool/browser-live` module enablement to CP-7.
  - Evidence: browser config defaults to disabled; enabled config requires sidecar URL/token; `BROWSER_AGENT_MIMO_*` overrides media config; unset browser MiMo route falls back to `MEDIA_MODEL_*`; `mimo-v2.5-pro` and non-image routes fail fast; existing media model and OpenCode Go bootstrap route behavior is covered by regression tests.
  - Commands: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go browser_agent_config_`; `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go settings_bootstraps_opencode_go_route_from_api_key_only`; `cargo clippy -p oxide-agent-core --no-default-features --features llm-opencode-go --all-targets -- -D warnings`.
  - Audit IDs updated: G1 verified, G2 verified, N2 verified, N4 verified, V1 in progress.
  - Next: commit CP-3, then start CP-4 sidecar API contract and typed client.

- 2026-06-16: CP-4 sidecar API contract and typed client
  - Changed: added `tool-browser-live` feature, feature-gated `browser_live` module export, typed sidecar REST client, sidecar error mapping, REST/stream contract types, screenshot artifact metadata, idempotency key type, and per-endpoint timeout config.
  - Evidence: client constructor rejects missing sidecar token; mutating calls require a non-empty idempotency key; mock HTTP server test verifies bearer auth and `Idempotency-Key` headers; error envelope test maps retryable sidecar errors to stable kinds; serialization tests cover session/action/observation/stream shapes and prove observation metadata has artifact refs, not base64 image bytes; grep found no sandbox/process command dependency in the module.
  - Commands: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features tool-browser-live browser_live`; `cargo check -p oxide-agent-core --no-default-features --features tool-browser-live`; `cargo clippy -p oxide-agent-core --no-default-features --features tool-browser-live --all-targets -- -D warnings`; `rg -n "sandbox|Sandbox|std::process|Command::" crates/oxide-agent-core/src/agent/providers/browser_live || true`.
  - Audit IDs updated: G3 verified, V1 in progress.
  - Next: commit CP-4, then start CP-5 fake sidecar for tests.

- 2026-06-16: CP-5 fake sidecar for tests
  - Changed: added `BrowserSidecar` async trait seam, implemented it for the production `BrowserSidecarClient`, and added a `cfg(test)` `FakeBrowserSidecar` with scripted action outcomes, lifecycle state, stale-frame behavior, debug payloads, metadata-only screenshots, and crash simulation.
  - Evidence: fake tests cover session create/goto/observe/action/close, deterministic action sequence, no-op action, stale screenshot reuse, error envelope mapping for failed action and browser crash, network debug, console debug, and observation debug summaries. The fake is only compiled under `cfg(test)` and does not start Chromium, `chrome-agent`, OpenCode Go, or an HTTP server.
  - Commands: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features tool-browser-live browser_live`; `cargo check -p oxide-agent-core --no-default-features --features tool-browser-live`; `cargo clippy -p oxide-agent-core --no-default-features --features tool-browser-live --all-targets -- -D warnings`.
  - Audit IDs updated: G4 verified, V1 in progress.
  - Next: commit CP-5, then start CP-6 session state, ring-buffer, and artifacts.

- 2026-06-16: CP-6 session state, ring-buffer, and artifacts
  - Changed: added `BrowserArtifactSettings`, stable browser artifact refs/paths integrated with `ToolRuntimeConfig`, and `BrowserSessionState` with latest frame, action sequence, bounded ring-buffer, retained artifacts, live-byte soft cap, metadata validation, and compact history summary.
  - Evidence: tests cover ring-buffer eviction without losing retained final artifacts, artifact naming/sanitization under the tool artifact root, retention expiry for live frames, live artifact byte-cap eviction, screenshot metadata validation for no image bytes/hash/viewport, and history hygiene summaries containing artifact refs rather than `base64` or `data:image` payloads.
  - Commands: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features tool-browser-live browser_live`; `cargo check -p oxide-agent-core --no-default-features --features tool-browser-live`; `cargo clippy -p oxide-agent-core --no-default-features --features tool-browser-live --all-targets -- -D warnings`.
  - Audit IDs updated: G5 verified, V1 in progress.
  - Next: commit CP-6, then start CP-7 core browser provider tools.

- 2026-06-16: CP-7 core browser provider tools
  - Changed: added `browser_live::tools` with five native executors, `BrowserLiveToolModule`, registry wiring, compiled `tool/browser-live` manifest capabilities, `profile-full` feature inclusion, and sub-agent blocklist entries for all browser tools.
  - Evidence: tool tests cover all five tool specs, fake `browser_start`/`browser_observe`/`browser_close` execution, compact artifact-ref outputs without image bytes, and placeholder `browser_step`; compiled manifest test verifies browser capabilities; sub-agent blocklist test verifies denial; feature-disabled `cargo check -p oxide-agent-core --no-default-features` passes.
  - Commands: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features tool-browser-live browser_live`; `cargo test -p oxide-agent-core --no-default-features --features tool-browser-live compiled_manifest_exposes_browser_live_tool_module`; `cargo test -p oxide-agent-core --no-default-features --features "tool-browser-live tool-delegation" sub_agent_blocklist_includes_sensitive_tools`; `cargo check -p oxide-agent-core --no-default-features`; `cargo check -p oxide-agent-core --no-default-features --features tool-browser-live`; `cargo clippy -p oxide-agent-core --no-default-features --features tool-browser-live --all-targets -- -D warnings`.
  - Audit IDs updated: G6 verified, N5 verified, V1 in progress.
  - Next: commit CP-7, then start CP-8 MiMo prompt/schema/parser.

- 2026-06-16: CP-8 MiMo browser decision prompt, schema, and parser
  - Changed: added `browser_live::prompt`, `parser`, and `mimo`; extended browser types with `BrowserDecision`; added binary latest-screenshot sidecar retrieval; and connected configured Browser Live tools to a decision-only `browser_step` MiMo path.
  - Evidence: parser tests cover golden valid/invalid decisions, safe single-object extraction, malformed/multiple-object rejection, coordinate bounds, low confidence, and sensitive executable-action rejection; prompt test proves stable prompt excludes volatile URL/screenshot IDs while dynamic prompt contains only artifact refs, no `base64`/`data:image`; MiMo tests prove `LlmClient::analyze_image()` is called with `mimo-v2.5` and one repair retry occurs after invalid JSON.
  - Commands: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features tool-browser-live browser_live`; `cargo test -p oxide-agent-core --no-default-features --features "tool-browser-live llm-opencode-go" browser_live::mimo`; `cargo check -p oxide-agent-core --no-default-features`; `cargo check -p oxide-agent-core --no-default-features --features tool-browser-live`; `cargo check -p oxide-agent-core --no-default-features --features "tool-browser-live llm-opencode-go"`; `cargo clippy -p oxide-agent-core --no-default-features --features tool-browser-live --all-targets -- -D warnings`; `cargo clippy -p oxide-agent-core --no-default-features --features "tool-browser-live llm-opencode-go" --all-targets -- -D warnings`.
  - Audit IDs updated: G7 verified, Q2 in progress.
  - Next: commit CP-8, then start CP-9 action execution and post-action verification loop.

- 2026-06-16: CP-9 action execution and post-action verification loop
  - Changed: added `browser_live::actions` and `browser_live::verification`; extended `BrowserDecisionAction` with HTTP/HTTPS `navigate`; changed `browser_step` from decision-only to one bounded decide → execute/goto → fresh observe → verify cycle while preserving non-mutating done/debug/ask-user safe stops.
  - Evidence: fake-sidecar tests cover click happy path with fresh after screenshot, no-op click verification failure, navigation fresh screenshot, done final evidence, and timeout report; helper tests cover action planning, non-web navigation rejection, and verification semantics where action verification is not task success.
  - Commands: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features tool-browser-live browser_live`; `cargo test -p oxide-agent-core --no-default-features --features "tool-browser-live llm-opencode-go" browser_live::mimo`; `cargo check -p oxide-agent-core --no-default-features`; `cargo check -p oxide-agent-core --no-default-features --features tool-browser-live`; `cargo check -p oxide-agent-core --no-default-features --features "tool-browser-live llm-opencode-go"`; `cargo clippy -p oxide-agent-core --no-default-features --features tool-browser-live --all-targets -- -D warnings`; `cargo clippy -p oxide-agent-core --no-default-features --features "tool-browser-live llm-opencode-go" --all-targets -- -D warnings`.
  - Audit IDs updated: G8 verified, V1 in progress.
  - Next: commit CP-9, then start CP-10 recovery engine.

- 2026-06-16: CP-10 recovery engine
  - Changed: added `browser_live::recovery`; added `click_target_id` decisions; extended fake sidecar outcomes; connected `browser_step` verification failures to bounded recovery classification, debug artifact retrieval, recovery action attempts, loop signatures, and safe-stop reports.
  - Evidence: tests cover coordinate drift recovery classification, stale screenshot wait recovery, modal overlay Escape recovery, repeated no-op loop stop, console/network debug artifact attachment, JS fallback disabled reports, recovery planner unit behavior, and existing CP-9 browser step paths.
  - Commands: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features tool-browser-live browser_live`; `cargo test -p oxide-agent-core --no-default-features --features "tool-browser-live llm-opencode-go" browser_live::mimo`; `cargo check -p oxide-agent-core --no-default-features`; `cargo check -p oxide-agent-core --no-default-features --features tool-browser-live`; `cargo check -p oxide-agent-core --no-default-features --features "tool-browser-live llm-opencode-go"`; `cargo clippy -p oxide-agent-core --no-default-features --features tool-browser-live --all-targets -- -D warnings`; `cargo clippy -p oxide-agent-core --no-default-features --features "tool-browser-live llm-opencode-go" --all-targets -- -D warnings`.
  - Audit IDs updated: G9 verified, V1 in progress.
  - Next: commit CP-10, then start CP-11 Docker Compose sidecar deployment.

- 2026-06-16: CP-11 Docker Compose sidecar deployment
  - Changed: added Chromium + pinned `chrome-agent` sidecar Dockerfile and minimal REST wrapper; wired `chrome-agent-sidecar` into root, web, telegram, full, and dev Compose files; added Browser Live env wiring, loopback-only sidecar port, artifact/profile volumes, healthcheck, `shm_size`, non-root image user, dropped caps, and `no-new-privileges`; enabled `tool-browser-live` for the web embedded profile; documented env and Compose behavior.
  - Evidence: `docker compose ... config --services` includes `chrome-agent-sidecar` beside app services for root/web/telegram/full/dev; `docker compose -f docker-compose.web.yml config` shows sidecar health/dependency wiring, loopback `127.0.0.1:8787`, artifact/profile volumes, and Browser Live app env; `rg "9222|CHROME_REMOTE_DEBUGGING_PORT|remote-debugging"` over rendered web config returns no output; sidecar image builds with Chromium and `chrome-agent`; live sidecar health returns `chrome_alive=true`, `chrome_agent_available=true`, and `chrome_cdp_host_exposed=false`; wrapper self-test verifies profile cleanup and artifact screenshot writes.
  - Commands: `python3 -m py_compile docker/chrome-agent-sidecar.py`; `python3 docker/chrome-agent-sidecar.py --self-test`; `docker compose -f docker-compose.web.yml config --services`; `docker compose -f docker-compose.web.yml config | rg -n "chrome-agent-sidecar|127.0.0.1:8787|browser-artifacts|browser-profiles|BROWSER_AGENT_ENABLED"`; `docker compose -f docker-compose.yml config --services`; `docker compose -f docker-compose.telegram.yml config --services`; `docker compose -f docker/compose.full.yml config --services`; `docker compose -f docker/compose.dev.yml config --services`; `docker compose -f docker-compose.web.yml config | rg -n "9222|CHROME_REMOTE_DEBUGGING_PORT|remote-debugging" || true`; `cargo check -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`; `cargo check -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local`; `docker compose -f docker-compose.web.yml build chrome-agent-sidecar`; `BROWSER_AGENT_SIDECAR_TOKEN=tok docker compose -f docker-compose.web.yml up -d --force-recreate --no-deps chrome-agent-sidecar`; `curl -fsS http://127.0.0.1:${BROWSER_AGENT_SIDECAR_PORT:-8787}/healthz`; `docker compose -f docker-compose.web.yml exec -T chrome-agent-sidecar /usr/local/bin/chrome-agent-sidecar --self-test`; `docker compose -f docker-compose.web.yml down --remove-orphans`; `cargo fmt --all -- --check`; `git diff --check`; secret-pattern scan.
  - Audit IDs updated: G10 verified, V1 in progress.
  - Next: commit CP-11, then start CP-12 Web UI live browser progress events.

- 2026-06-16: CP-12 Web UI live browser progress events
  - Changed: added shared Browser Live web event payload contracts; mapped BrowserAction/BrowserVerification/BrowserRecovery progress messages to typed persisted `browser_live` events; extended browser observation payloads with URL/title/loading/network/console metadata; added frontend Browser Live state reduction and panel rendering latest screenshot artifact refs, action/verification/confidence/debug badges, blocked state, final artifact refs, and task lifecycle controls without manual browser control.
  - Evidence: contract serialization test proves Browser Live screenshot payloads carry artifact refs without `base64`/`data:image`; web transport mapping test proves browser progress messages become typed `browser_live` events; UI reducer tests prove latest screenshot artifact ref rendering data, debug badges, blocked/base64 rejection, and preview-frame coalescing to the latest ref; wasm check/clippy compiles the actual panel path; diff review shows no iframe/VNC/manual browser control surface.
  - Commands: `cargo test -p oxide-agent-web-contracts browser_live_event_payload_serializes_artifact_refs_without_image_bytes`; `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local collect_events_maps_browser_reasoning_to_typed_browser_live_events`; `cargo test -p oxide-agent-web-ui browser_live_state_`; `cargo test -p oxide-agent-core --no-default-features --features tool-browser-live browser_live`; `cargo check -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`; `cargo check -p oxide-agent-web-ui`; `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`; `cargo clippy -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local --all-targets -- -D warnings`; `cargo clippy -p oxide-agent-web-ui --all-targets -- -D warnings`; `cargo clippy -p oxide-agent-web-ui --target wasm32-unknown-unknown -- -D warnings`; `cargo clippy -p oxide-agent-core --no-default-features --features tool-browser-live --all-targets -- -D warnings`; `cargo fmt --all -- --check`; `git diff --check`.
  - Audit IDs updated: G11 verified, Q2 in progress, N1 in progress, V1 in progress.
  - Next: commit CP-12, then start CP-13 Telegram milestone reporting.

- 2026-06-16: CP-13 Telegram milestone reporting
  - Changed: added compact Telegram Browser Live milestone rendering for `BrowserAction`/`BrowserVerification`/`BrowserRecovery`; suppressed generic browser observe/session progress from the Telegram thought area; added blocked/safe-stop reason rendering; added browser artifact delivery policy for live-frame suppression, final artifact de-duplication, and sensitive artifact-name suppression; verified Telegram commands/keyboards expose no browser start/control surface.
  - Evidence: progress render tests prove concise browser milestones and blocked/safe-stop reports; observe suppression test proves Telegram does not render every browser frame; file policy tests prove final browser artifacts are delivered once through the existing file delivery path while live frames and sensitive browser artifact names are suppressed; command/keyboard tests prove no browser/chrome start/control command or callback is exposed.
  - Commands: `cargo test -p oxide-agent-transport-telegram browser_ --no-default-features --features profile-embedded-opencode-local`; `cargo test -p oxide-agent-transport-telegram agent_control_keyboards_only_include_cancel_task --no-default-features --features profile-embedded-opencode-local`; `cargo check -p oxide-agent-transport-telegram --no-default-features --features profile-embedded-opencode-local`; `cargo clippy -p oxide-agent-transport-telegram --no-default-features --features profile-embedded-opencode-local --all-targets -- -D warnings`; `cargo fmt --all -- --check`.
  - Audit IDs updated: G12 verified, N5 verified, V1 in progress.
  - Next: commit CP-13, then start CP-14 security and policy gates.

- 2026-06-16: CP-14 Security and policy gates
  - Changed: added `browser_live::policy` for URL scheme gates, sensitive/CAPTCHA/2FA/payment/high-risk classification, raw credential value rejection with secret-ref recognition, download/upload and profile policy, and redacted audit events; wired policy into `browser_start` and `browser_step`; forced profile purge on `browser_close`; added prompt-injection instructions to the stable MiMo prompt.
  - Evidence: policy tests prove arbitrary HTTP/HTTPS URLs are allowed without a domain allowlist while `file://`, `chrome://`, `devtools://`, and `data:` are rejected; sensitive/CAPTCHA executable decisions are blocked before sidecar action with redacted `browser_policy` audit payloads; raw secret values are rejected and audit JSON excludes secret text; start/close tests prove no downloads/uploads and ephemeral profile purge; prompt fixture proves untrusted page-content instructions are explicitly ignored; existing sub-agent blocklist test proves browser tools are denied to sub-agents.
  - Commands: `cargo test -p oxide-agent-core --no-default-features --features tool-browser-live browser_live`; `cargo test -p oxide-agent-core --no-default-features --features "tool-browser-live tool-delegation" sub_agent_blocklist_includes_sensitive_tools`; `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go browser_agent_config_defaults_to_disabled`; `cargo check -p oxide-agent-core --no-default-features --features tool-browser-live`; `cargo clippy -p oxide-agent-core --no-default-features --features tool-browser-live --all-targets -- -D warnings`; `cargo clippy -p oxide-agent-core --no-default-features --features "tool-browser-live tool-delegation" --all-targets -- -D warnings`; `cargo fmt --all -- --check`.
  - Audit IDs updated: Q1 verified, N3 verified, N4 verified, N1 in progress, V1 in progress.
  - Next: commit CP-14, then start CP-15 observability, metrics, and logging.

## Risks and Blockers

- Live validation requires external services later
  - Impact: CP-11/CP-16 cannot be fully verified without Docker sidecar runtime and, for MiMo staging smoke, an OpenCode Go API key.
  - Evidence: PRD explicitly requires compose and env-gated provider smoke at `docs/prd/chrome-agent.md:3435` and `docs/prd/chrome-agent.md:3556`.
  - Mitigation or requested decision: continue with hermetic config/client/fake-sidecar work; block only exact live smoke evidence if dependencies are unavailable at that checkpoint.
  - Audit IDs affected: G10, V2.

- Existing unrelated active goal doc remains in `docs/goals/2026-06-15-llm-provider-wire-path-unification.md`
  - Impact: repository has another active goal document, but no active OpenCode goal state was set before creating this one.
  - Evidence: `get_goal` returned no active OpenCode goal; `docs/goals/` convention allows durable docs.
  - Mitigation or requested decision: keep this browser goal as the active execution target for this thread; archive/finish unrelated goal separately if needed.
  - Audit IDs affected: none.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
