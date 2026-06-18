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
