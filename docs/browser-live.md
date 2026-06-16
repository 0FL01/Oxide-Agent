# Browser Live Agent

Oxide Browser Live is an autonomous headless-browser capability. The agent can
open a browser session, observe pages, execute bounded actions, debug
console/network events, and close the session. The browser is controlled by a
local `chrome-agent-sidecar` container rather than an external service.

## What is supported

- `browser_start` / `browser_observe` / `browser_step` / `browser_debug` / `browser_close` tools
- HTTP/HTTPS navigation only
- Screenshot artifacts, DOM/a11y snapshots, network and console summaries
- Web UI preview panel and Telegram milestone/blocked/final reporting
- OpenCode Go `mimo-v2.5` as the only screenshot-vision route in MVP

## What is not supported in MVP

- Manual control, iframe/VNC, or browser start from Telegram
- Persistent browser profiles or real Chrome profile attach
- `file://`, `chrome://`, `devtools://`, `data:` URLs
- CAPTCHA/2FA/anti-bot bypass; these are safe-stopped with a blocked report
- Direct Xiaomi fallback; no `mimo-v2.5-pro` for vision

## Requirements

- Docker with Compose
- The `chrome-agent-sidecar` service built from `docker/Dockerfile.chrome-agent-sidecar`
- An OpenCode Go API key for the `mimo-v2.5` vision route (or fallback to `MEDIA_MODEL_*` if configured for image input)

## Configuration

Copy the Browser Live section from `.env.example` and set a non-empty token:

```bash
# .env
BROWSER_AGENT_SIDECAR_TOKEN=<set-a-long-random-token>
BROWSER_AGENT_ENABLED=true
BROWSER_AGENT_SIDECAR_BASE_URL=http://127.0.0.1:8787
BROWSER_AGENT_SIDECAR_WS_URL=ws://127.0.0.1:8787
BROWSER_AGENT_MIMO_PROVIDER=opencode-go
BROWSER_AGENT_MIMO_MODEL=mimo-v2.5
```

If `BROWSER_AGENT_MIMO_*` is unset, browser vision falls back to `MEDIA_MODEL_*`.
`mimo-v2.5-pro` is rejected for browser perception because it is text-only for
this route.

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

Web app health:

```bash
curl -fsS http://127.0.0.1:8080/health
```

Expected sidecar response:

```json
{"ok": true, "chrome_agent_available": true, "chrome_agent_status": "stopped"}
```

## Web UI usage

1. Start the web compose stack and open the console.
2. Create a task and ask the agent to use the browser, e.g. "Open a browser,
   go to example.com, and tell me what you see".
3. The agent calls `browser_start`, `browser_observe`, and `browser_close` as
   needed. The Browser Live panel shows the latest screenshot artifact and
   action status. There is no manual browser control.

## Security and limits

- Browser sessions are ephemeral; the profile is purged on `browser_close`.
- The sidecar requires a shared bearer token; keep it secret and out of logs.
- The app and sidecar communicate over loopback only.
- Non-web URL schemes are rejected before the sidecar is called.
- Sub-agents cannot start browser sessions.
- Screenshots are stored as artifact refs; bytes are not persisted in durable
  chat history.
- Large or repeated pages are bounded by the live-frame byte cap and ring-buffer
  eviction.

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
  sidecar logs with `docker compose -f docker-compose.web.yml logs -f chrome-agent-sidecar`.

- **Browser starts but actions fail with timeout**  
  Some sites require longer waits or use anti-bot measures. The recovery engine
  will try bounded retries; if it cannot recover, it returns a blocked report.

- **No screenshot preview in Web UI**  
  Confirm the artifact directory is writable and the sidecar created the artifact
  volume. Screenshot artifact refs are named `artifact://browser/<task>/<session>/`.

## Staging checklist

- [ ] `BROWSER_AGENT_SIDECAR_TOKEN` set in `.env` for both app and sidecar
- [ ] `BROWSER_AGENT_ENABLED=true` set in `.env`
- [ ] `BROWSER_AGENT_SIDECAR_BASE_URL=http://127.0.0.1:8787`
- [ ] `BROWSER_AGENT_MIMO_MODEL=mimo-v2.5` (not `mimo-v2.5-pro`)
- [ ] Sidecar health returns `ok: true`
- [ ] Web app health returns `{"status":"ok"}`
- [ ] A test task can open a browser and observe `https://example.com`
- [ ] `browser_close` purges the profile and returns `sidecar_errors: 0`
- [ ] Telegram does not show any browser start/control commands
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
docker compose -f docker-compose.web.yml stop chrome-agent-sidecar
```
