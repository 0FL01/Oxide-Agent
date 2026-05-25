# Browser Use Operations

Status: disabled.

Browser Use is intentionally disabled until a cost-effective, high-quality vision-agent model is available.

The implementation is kept in the repository as dormant code. Do not treat old rollout/stage notes as current operational guidance.

## Current breadcrumb

- Bridge code remains under `services/browser_use_bridge/`.
- Rust provider code remains behind the Browser Use integration paths in `oxide-agent-core`.
- Runtime enablement is by setting `BROWSER_USE_URL` to a non-empty bridge URL.
- `BROWSER_USE_ENABLED` is not a supported enable switch.
- Keep topic/context isolation, `navigation_only` guardrails, and runtime liveness/reconnect behavior when touching the bridge.

## What was removed from this runbook

The previous file contained active-looking rollout tails, stage links, and post-v1 notes for Browser Use route inheritance, profile reuse, readiness retry, keep-alive, and low-level browser surface decisions. Those notes were stale while Browser Use is disabled, and two referenced stage documents were not present in the repo.

If Browser Use is revived, re-audit the code first and write a fresh enablement runbook from the current implementation.

## Minimal enable checklist for a future revival

1. Pick and validate a vision-capable model route.
2. Start the bridge service and verify `GET /health`.
3. Set `BROWSER_USE_URL` for Oxide Agent.
4. Enable Browser Use only for the specific topic/profile that needs it.
5. Run a smoke task that exercises navigation, screenshot/content extraction, and session cleanup.
6. Confirm logs do not leak provider API keys or browser-session secrets.

## Non-goals while disabled

- Do not expand the public browser tool surface.
- Do not add new Browser Use rollout stages.
- Do not enable Browser Use globally by default.
- Do not optimize the bridge until the model/cost constraint is resolved.
