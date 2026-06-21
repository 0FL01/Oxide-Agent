# Browser Live Agent

Oxide Browser Live is an autonomous headless-browser capability. The agent has
full control over a browser session: it can open any URL, observe pages, execute
actions, fill forms, submit data, extract structured data, and close the session.
The browser is controlled by a local `browser-sidecar` container rather than
an external service. The sidecar is a native Rust binary (`oxide-browser-sidecar`)
that talks CDP directly to Chromium over a single WebSocket — no Python runtime,
no `chrome-agent` subprocess.

> **Warning:** Browser Live runs in Yolo mode. The agent is allowed to type
> passwords, secrets, and other sensitive data into web pages, and to submit
> forms on your behalf. It acts as an extension of your own credentials. Use it
> only on sites and tasks you trust.

## What is supported

- `browser_start` / `browser_observe` / `browser_execute` / `browser_extract` / `browser_debug` / `browser_close` tools
- Navigation to any URL the Chromium instance can reach, including SPA hash-based URLs with `force_reload`
- Semantic form input via `fill` and `type_text` using native value setters and framework-visible events
- Strict `BrowserAction` schema: per-variant `oneOf` with required fields, numeric bounds, and `additionalProperties: false`
- Deterministic DOM value/attribute extraction through `browser_extract` without ad-hoc JavaScript
- Screenshot artifacts, DOM/a11y snapshots, network and console summaries
- Post-action observation freshness diagnostics: result-only vs capture-after actions, structured DOM snapshot errors
- Web UI preview panel and Telegram milestone/blocked/final reporting

## What is not supported

- Manual control, iframe/VNC, or direct user manipulation of the browser
- Vision-based decision layer or screenshot analysis as a browser control mechanism
- MiMo or any intermediate model deciding actions; the main agent directly plans and executes actions

## Stealth and anti-detection

The sidecar is a direct CDP client (no Playwright/Selenium shim), which is the
strongest starting position per 2026 bot-detection benchmarks. The following
hardening measures are built in:

### Command-line flags

- `--disable-blink-features=AutomationControlled` is set so `navigator.webdriver`
  is `false` at the Blink/C++ level (undetectable by JS getter checks).
- Fingerprint flags removed: `--disable-extensions`,
  `--disable-popup-blocking`, `--disable-gpu` (each is a known automation
  signal when present).
- `--disable-features` includes the Patchright-derived list
  (`ImprovedCookieControls`, `LazyFrameLoading`, `ThirdPartyStoragePartitioning`,
  `Translate`, `HttpsUpgrades`, etc.) to match a stock Chrome profile.

### Chrome binary preference

Launch resolution order: `$CHROMIUM_BIN` env (highest priority when set) >
`google-chrome` > `google-chrome-stable` > `chromium`. Using a real Chrome
binary (not the Chromium build) avoids headless-Chromium-specific fingerprints.
Docker sets `CHROMIUM_BIN=/usr/bin/chromium` so the override is preserved.

### Isolated worlds for read-only JS

All internal read-only JavaScript (DOM fingerprinting, URL/title reads, DOM
snapshots, element queries, selector/text polling) runs in an isolated
execution context created via `Page.createIsolatedWorld`. Page JS cannot
detect or intercept evaluation in an isolated world — only the C++ DOM is
shared. If the isolated context becomes stale (client-side navigation without
going through our `navigate()`), evaluation falls back to the main world
gracefully.

Page-interacting JavaScript (event dispatch, form filling, key presses,
scrolling, SPA hash navigation, stealth patches, console interceptor) stays
in the main world where it must be visible to the page.

### Console interceptor hardening

The console interceptor overrides `console.log/warn/error/debug/info` to
capture entries. A `WeakMap`-backed `Function.prototype.toString` override
ensures every patched method returns its native string
(`"function log() { [native code] }"`) under `toString()`,
`Function.prototype.toString.call()`, and `String()`. The override itself is
registered in the `WeakMap` so it too looks native. Non-patched functions
delegate to the real `toString` with no observable difference.

### Architectural invariants

The sidecar never calls `Runtime.enable`, `Target.setAutoAttach`,
`Target.attachToTarget`, or `Console.enable` — all are well-known CDP
detection vectors. JS evaluation uses `Runtime.evaluate` (a command, not
event subscription) directly on the page target WebSocket.

### Not covered

- TLS fingerprint (JA4) and HTTP/2 SETTINGS ordering are binary-level and not
  controllable from a CDP client.
- Headless screen-dimension artifacts (`--headless=new` limitations).
- Behavioral patterns (mouse movement, typing cadence) are agent-controlled.

## Ad blocking

The sidecar includes optional network-level ad blocking using the
`adblock-rust` engine (Brave's Rust adblock engine) integrated with CDP
`Fetch.enable` request interception. When enabled, ad and tracking requests
are blocked at the network layer before they reach Chromium — improving agent
decision quality (cleaner screenshots, cleaner DOM snapshots, faster page
loads, privacy).

### How it works

1. At startup, `main.rs` builds an `AdblockEngine` from filter list files
   (if `ADBLOCK_ENABLED=true`).
2. The engine is shared via `Arc<AdblockEngine>` across all browser sessions
   — built once, no per-session rebuild.
3. When a session starts, `CaptureCollector::start()` sends `Fetch.enable`
   with patterns for all non-Document resource types (navigation is never
   paused).
4. For each `Fetch.requestPaused` event, the handler:
   - Skips navigation requests and Document resources (defense-in-depth).
   - Builds an `adblock::Request` with the URL, current page URL as source,
     and mapped resource type.
   - Calls `engine.check_network_request()` — if matched, sends
     `Fetch.failRequest` with `BlockedByClient` (same error as Brave/uBlock).
   - If not matched, sends `Fetch.continueRequest` (pass through unmodified).
   - Fail-open: on any error, `continueRequest` is sent (never hang requests).

### Stealth interaction

`Fetch.enable` is an independent CDP domain with zero JS-visible side
effects. It does NOT call `Runtime.enable`, `Target.setAutoAttach`, or
`Console.enable`. Page JS cannot detect Fetch interception — the only signal
is that ads don't load (intended behavior, identical to Brave/uBlock).

When ad blocking is disabled (`ADBLOCK_ENABLED` unset or `false`), no
`Fetch.enable` is sent, no filter lists are loaded — zero behavior change,
zero stealth impact.

### Configuration

Ad blocking is **enabled by default** when filter lists are available. The
Docker image includes EasyList and EasyPrivacy at `/opt/adblock/` and sets
`ADBLOCK_FILTERS` in the Dockerfile — ad blocking activates automatically.

To disable ad blocking, set:

```bash
ADBLOCK_ENABLED=false
```

`ADBLOCK_FILTERS` is pre-set in the Dockerfile. Only `ADBLOCK_ENABLED=false`
needs to be set at runtime to opt out.

### What it blocks

- Ad scripts (doubleclick.net, googlesyndication.com, etc.)
- Tracking pixels and beacons (google-analytics.com, facebook.net, etc.)
- Third-party ad iframes and resources
- Crypto mining scripts

### What it does NOT block

- Cosmetic ad elements already in the DOM (server-side rendered ads) — Phase 2
- Scriptlet injection (uBlock Origin scriptlets) — Phase 2
- Redirect resources (`$redirect=noopjs`) — Phase 2
- Navigation requests — never intercepted

### Filter list updates

Filter lists are baked into the Docker image. To update, rebuild the image
(the Dockerfile re-downloads from easylist.to). No runtime auto-update.

## Requirements

- Docker with Compose
- The `browser-sidecar` service built from `docker/Dockerfile.browser-sidecar` (native Rust binary + Chromium only)

## Configuration

Copy the Browser Live section from `.env.example` and set a non-empty token:

```bash
# .env
BROWSER_AGENT_SIDECAR_TOKEN=<set-a-long-random-token>
BROWSER_AGENT_ENABLED=true
BROWSER_AGENT_SIDECAR_BASE_URL=http://127.0.0.1:8787
BROWSER_AGENT_SIDECAR_WS_URL=ws://127.0.0.1:8787
```

Optional internal sidecar artifact directory (overridden by Docker Compose volumes):

```bash
# BROWSER_AGENT_ARTIFACT_DIR=/var/lib/oxide-browser/artifacts
```

## Deployment

Web UI with local Postgres:

```bash
docker compose -f docker-compose.web.yml -f docker-compose.web.local-services.yml up --build -d
```

Telegram bot with local services:

```bash
docker compose -f docker-compose.telegram.yml -f docker-compose.telegram.local-services.yml up --build -d
```

The sidecar REST port is bound to `127.0.0.1:8787` by default. The app service
connects to this loopback address because it runs in host-network mode. Do not
expose the sidecar port on a public interface.

## Verify

Sidecar health:

```bash
curl -fsS http://127.0.0.1:8787/healthz \
  -H "Authorization: Bearer ${BROWSER_AGENT_SIDECAR_TOKEN}"
```

Sidecar self-test (integration tests, requires Chromium):

```bash
cargo test -p oxide-browser-sidecar --test rest_contract -- --ignored --nocapture
```

Web app health:

```bash
curl -fsS http://127.0.0.1:8080/health
```

Expected sidecar response:

```json
{"ok": true, "native": true}
```

## Tools

### `browser_start`
Start a task-local autonomous headless browser session. Optional `start_url`,
`timezone`, `locale`.

### `browser_observe`
Return compact browser state (url, title, loading state, network/console
summaries, optional DOM snapshot) and attach the latest screenshot as a native
image for vision models. Set `fresh: true` to capture a new screenshot instead
of reusing the cached one. `include_dom` defaults to true.

### `browser_execute`
Execute a single concrete browser action. The `action` field uses a strict
`oneOf` schema with a literal `kind` discriminator. Variants:

| Kind | Fields | Notes |
|------|--------|-------|
| `click_xy` | `x`, `y`, `target_description?` | Coordinate click |
| `click_selector` | `selector` | CSS selector click |
| `fill` | `selector`, `value` | Semantic input via native setters + framework events |
| `type_text` | `selector`, `value` | Same semantic input primitive as `fill` |
| `press` | `key` | Key press |
| `scroll` | `delta_x`, `delta_y` | Scroll deltas |
| `get_element_value` | `selector` | Read scalar value (result-only, no post-observation). For `input[type=checkbox|radio]` returns `"true"`/`"false"` (checked state), not the `value` attribute |
| `execute_javascript` | `expression` | Expression eval; captures post-observation |
| `wait` | `timeout_ms` | Wait (1–60000 ms; no `ms` alias) |
| `wait_for_selector` | `selector`, `timeout_ms` | Wait for element |
| `wait_for_text` | `text`, `timeout_ms` | Wait for text |
| `script` | `steps` | Sequence of non-script actions |
| `navigate` | `url`, `force_reload?` | Navigate; `force_reload: true` restarts browser process for fresh SPA state |

For SPA hash-based URLs (e.g. one-time secret pages) use `navigate` with
`force_reload: true` to guarantee a clean page state — the sidecar replaces the
browser process without profile purge, then opens the exact target URL.

### `browser_extract`
Extract structured data from the current page. Sources:

- **`dom`**: CSS `selector` + optional `attribute` (defaults to `innerText`).
  The requested property/attribute is returned as `matches[].value` with
  `attribute_source` (`property`/`attribute`/`missing`) and `found` flag. Raw
  `properties` and `attributes` are included for diagnostics. No ad-hoc
  `execute_javascript` querySelector hacks needed for normal form values.
- **`network`**: Filter by `url_pattern`, `method`, `status_code`; includes
  response bodies when `include_bodies` is true.

### `browser_debug`
Fetch browser console/network debug summaries as compact artifact-backed
diagnostics. Supports `since_action_seq` and `limit`.

### `browser_close`
Close a browser session and finalize retained browser artifacts. The Chromium
profile lives in a per-session temp directory that is always cleaned up on
close (the `purge_profile` flag is accepted for contract compatibility and
echoed in the response, but profiles are ephemeral by design).
`keep_artifacts` defaults to true (screenshot artifacts are preserved for
debugging).

## Post-action observations

State-changing actions (`click_*`, `fill`, `type_text`, `press`, `scroll`,
`execute_javascript`, `script` with visual steps, `navigate`) return a fresh
post-observation with DOM snapshot or a structured `dom_snapshot_error`.
Read-only actions (`get_element_value`, `wait`) are result-only and skip
post-observation.

Tool output includes `post_observation_diagnostics` with:
- `mode`: `capture_after` or `result_only`
- `source`: `sidecar` or `fallback_observe`
- `fresh_observation` / `fresh_screenshot`: whether observation/screenshot changed
- `action_seq_current`: latest action sequence number
- `dom_snapshot.status`: `captured`, `captured_empty`, or `error`
- DOM snapshot elements include `tag`, `selector`, `attributes`, `href`, `value`,
  and `text`. For form inputs (`input`, `textarea`, `select`), `value` holds the
  current runtime value; `text` is `innerText` and is empty for inputs.

## Web UI usage

1. Start the web compose stack and open the console.
2. Create a task and ask the agent to use the browser, e.g. "Open a browser,
   go to example.com, and tell me what you see".
3. The agent calls `browser_start`, `browser_observe`, `browser_execute`,
   `browser_extract`, and `browser_close` as needed. The Browser Live panel
   shows the latest screenshot artifact and action status. There is no manual
   browser control.

## Limits and warnings

- Browser sessions are ephemeral; the Chromium profile (cookies, localStorage,
  cache) lives in a per-session temp directory that is cleaned up on
  `browser_close`. Artifacts are preserved when `keep_artifacts` is true.
- The sidecar requires a shared bearer token; keep it secret and out of logs.
- The app and sidecar communicate over loopback only.
- Screenshots are stored as artifact refs; bytes are not persisted in durable
  chat history.
- Large or repeated pages are bounded by the live-frame byte cap and ring-buffer
  eviction.
- The agent is allowed to submit forms, type secrets, and interact with any
  page it can reach. Review agent actions before trusting them on sensitive sites.

## Sub-agent access

Browser tools are available to sub-agents when the parent agent explicitly
requests them in the `allowed_tools` whitelist of a `spawn_sub_agents` call.
Sub-agents inherit the parent's `browser_live_context` (storage, user ID,
context key) so screenshot artifacts are stored under the parent's session
scope, not the ephemeral sub-agent scope.

When browser is disabled (feature not compiled or `BROWSER_AGENT_ENABLED=false`),
no browser tools are registered for sub-agents and the context is `None` — zero
behavioral change.

## RAII session cleanup

Browser sessions are closed automatically when an agent run ends, regardless of
the outcome (success, timeout, cancel, or error). This prevents Chromium process
leaks at the sidecar.

- **Parent agent**: after `run_with_timeout` returns, any sessions left open are
  closed via `close_all_sessions` on the provider. The `browser_close` tool is
  an early-release optimization, not the only cleanup path.
- **Sub-agent**: after the sub-agent's `run_with_timeout` returns, the same
  cleanup runs. This covers the common leak scenario: sub-agent opens a browser
  session, then times out or is cancelled before calling `browser_close`.

## Sidecar session cap

The sidecar enforces a maximum number of concurrent browser sessions to prevent
OOM under load. When the cap is reached, new `browser_start` calls are rejected
with a `sidecar_at_capacity` error. The agent receives a human-readable message
advising it to close an existing session before retrying.

Configuration:

- `BROWSER_AGENT_SIDECAR_MAX_SESSIONS` — env var on the sidecar binary (default
  `8`). Set in `docker-compose.web.yml` as `BROWSER_AGENT_SIDECAR_MAX_SESSIONS`.
- `None` (unset via `SessionManager::default()`) means unlimited — backward
  compatible for tests.

## Troubleshooting

- **App fails to start with `BROWSER_AGENT_ENABLED=true requires BROWSER_AGENT_SIDECAR_TOKEN`**
  Set a non-empty `BROWSER_AGENT_SIDECAR_TOKEN` in `.env` and restart both the
  app and sidecar services.

- **Browser tools do not appear in the agent tool list**
  Check that `BROWSER_AGENT_ENABLED=true` and the token is set. Check the
  capability list with the binary for the relevant profile.

- **Browser tool calls fail with `Browser sidecar request was rejected`**
  Verify the sidecar is running, the token is correct, and the REST URL is
  reachable at the configured `BROWSER_AGENT_SIDECAR_BASE_URL`. Check the
  sidecar logs with `docker compose -f docker-compose.web.yml logs -f browser-sidecar`.

- **Browser starts but actions fail with timeout**
  Some sites require longer waits or use anti-bot measures. Increase
  `timeout_ms` (up to 60000) or use `wait_for_selector`/`wait_for_text` before
  acting.

- **No screenshot preview in Web UI**
  Confirm the artifact directory is writable and the sidecar created the artifact
  volume. Screenshot artifact refs are named `artifact://browser/<task>/<session>/`.

- **SPA hash navigation caches stale state**
  Use `navigate` with `force_reload: true`. The sidecar replaces the browser
  process to get a fresh JS heap while preserving profile data and the target
  URL/hash.

- **`type_text` does not trigger SPA state updates**
  Both `fill` and `type_text` use the same semantic input primitive with native
  value setters and framework-visible events (`focus`, `focusin`, `beforeinput`,
  `input`, `change`, `keyup`). If an input still fails, check the selector and
  the post-action DOM snapshot diagnostics.

- **`browser_start` fails with `sidecar_at_capacity`**
  The sidecar has reached its maximum concurrent session count
  (`BROWSER_AGENT_SIDECAR_MAX_SESSIONS`, default 8). Close an existing browser
  session with `browser_close` before retrying, or increase the cap in the
  sidecar environment.

- **DOM value extraction returns `found: false`**
  Check `attribute_source` in the diagnostics. The selector may not match, or
  the requested attribute may not exist. Raw `properties` and `attributes` in
  each match entry show what the element actually has.

- **DOM snapshot shows empty `text` for input/textarea fields**
  This is expected. DOM snapshot `text` is `innerText` — always empty for form
  inputs. The current runtime value is in the `value` field of each snapshot
  element. Use `value` (not `text`) to verify form input state after `fill` or
  `type_text`. Use `get_element_value` to read a single element's value; for
  `input[type=checkbox|radio]` it returns the checked state (`"true"`/`"false"`)
  rather than the `value` attribute.

## Staging checklist

- [ ] `BROWSER_AGENT_SIDECAR_TOKEN` set in `.env` for both app and sidecar
- [ ] `BROWSER_AGENT_ENABLED=true` set in `.env`
- [ ] `BROWSER_AGENT_SIDECAR_BASE_URL=http://127.0.0.1:8787`
- [ ] Sidecar health returns `{"ok": true, "native": true}`
- [ ] Sidecar integration tests pass (`cargo test -p oxide-browser-sidecar --test rest_contract -- --ignored`)
- [ ] Web app health returns `{"status":"ok"}`
- [ ] A test task can open a browser and observe `https://example.com`
- [ ] `browser_close` returns `sidecar_errors: 0`
- [ ] No browser token is present in logs or chat history

## Rollback

Disable the feature without rebuilding:

```bash
# .env
BROWSER_AGENT_ENABLED=false
```

```bash
docker compose -f docker-compose.web.yml up -d --force-recreate
```

Or stop the sidecar service while leaving the web app running:

```bash
docker compose -f docker-compose.web.yml stop browser-sidecar
```
