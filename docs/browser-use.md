# Browser Use Operations

Status: enabled for `web-embedded-opencode-local` Docker Compose profile.

Browser Use runs through the self-hosted `browser_use` sidecar in `docker-compose.web.yml`. The web profile pins the dedicated Browser Use route to OpenCode Go `mimo-v2.5`, which is treated as vision-capable. OpenCode Go `deepseek-v4-flash` is supported for text-only browsing/extraction tasks.

Other profiles remain runtime-gated: Browser Use tools register only when the `tool-browser-use` feature is compiled and `BROWSER_USE_URL` is non-empty.

## Current breadcrumb

- Bridge code remains under `services/browser_use_bridge/` and is wired into `docker-compose.web.yml` as `browser_use`.
- Rust provider code remains behind the Browser Use integration paths in `oxide-agent-core`.
- Runtime enablement is by setting `BROWSER_USE_URL` to a non-empty bridge URL; the web compose default is `http://127.0.0.1:8002`.
- `BROWSER_USE_ENABLED` is not a supported enable switch.
- Keep topic/context isolation, `navigation_only` guardrails, and runtime liveness/reconnect behavior when touching the bridge.

## What was removed from this runbook

The previous file contained active-looking rollout tails, stage links, and post-v1 notes for Browser Use route inheritance, profile reuse, readiness retry, keep-alive, and low-level browser surface decisions. Those notes were stale while Browser Use is disabled, and two referenced stage documents were not present in the repo.

This file is now the active enablement runbook for the web compose profile.

## Minimal enable checklist

1. Build/start `docker-compose.web.yml` and wait for `browser_use` healthcheck.
2. Verify `GET http://127.0.0.1:8002/health`.
3. Keep the dedicated route on `BROWSER_USE_MODEL_PROVIDER=opencode-go` and `BROWSER_USE_MODEL_ID=mimo-v2.5` for visual tasks.
4. Use `deepseek-v4-flash` only for text-only browsing/extraction tasks.
5. Run a smoke task that exercises navigation, screenshot/content extraction, and session cleanup.
6. Confirm logs do not leak provider API keys or browser-session secrets.

## Non-goals

- Do not expand the public browser tool surface.
- Do not add new Browser Use rollout stages.
- Do not enable Browser Use globally outside the web compose profile by default.
- Do not pass provider API keys into the bridge environment; Oxide sends inherited route secrets server-to-server.
