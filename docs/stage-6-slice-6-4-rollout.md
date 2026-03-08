# Stage 6 Slice 6.4 - Feature Flags and Rollout Safety

## Goal

Provide staged enablement for Agent Mode v2 and safe rollback behavior without manual data repair.

## Rollout controls

- `AGENT_MODE_ENABLED` (default: `false`) controls Telegram user-facing Agent Mode activation paths.
- `AGENT_ACCESS_IDS` still controls per-user authorization when Agent Mode is enabled.
- Runtime startup, task registry, task recovery, and observer surfaces remain operational when `AGENT_MODE_ENABLED=false`.

## Staged rollout checklist

1. Deploy with `AGENT_MODE_ENABLED=false`.
2. Verify bot starts, chat mode works, and no Agent Mode activation affordance appears in Telegram main keyboard.
3. Verify task runtime and recovery are healthy (existing tasks can still be reconciled/observed).
4. Enable `AGENT_MODE_ENABLED=true` in a limited environment.
5. Validate Agent Mode activation, task lifecycle controls, and observer links (if web observer is enabled).
6. Expand rollout gradually after telemetry/support checks remain stable.

## Rollback procedure (no manual data repair)

1. Set `AGENT_MODE_ENABLED=false` and redeploy.
2. Confirm Telegram blocks new Agent Mode activations and shows degradation message.
3. Confirm users with persisted `agent_mode` state are automatically downgraded to `chat_mode` on next interaction.
4. Confirm runtime recovery and snapshot/task observability are still available for previously created tasks.
5. Do **not** delete snapshots, registries, or user state manually.

## Observability signals

- Telegram logs for guarded paths:
  - persisted `agent_mode` restore skipped because feature flag disabled,
  - activation rejected due to rollout flag,
  - runtime remains available for existing tasks.
- Existing Stage 5 observer endpoints continue to function when enabled (`/health`, `/api/observer/{token}/snapshot`, `/api/observer/{token}/events`, `/watch/{token}`).
- No spike in task recovery failures after flag changes.

## Support playbook

- User asks why Agent Mode is missing:
  - Explain Agent Mode is temporarily disabled by rollout settings.
  - Guide user to continue in Chat Mode.
- User had active/persisted Agent Mode session before rollback:
  - Confirm automatic downgrade to Chat Mode is expected.
  - If an existing task must be monitored, use observer/read-only telemetry and runtime status tools.
- Escalate if:
  - persisted state does not downgrade,
  - existing tasks fail recovery after rollback,
  - observer snapshots/events are unavailable while Stage 5 observer is enabled.

## Validation matrix

- `AGENT_MODE_ENABLED=false` + allowed user -> no Agent Mode keyboard entry, activation blocked with clear message.
- `AGENT_MODE_ENABLED=true` + allowed user -> Agent Mode activation available and operational.
- `AGENT_MODE_ENABLED=false` + persisted `agent_mode` -> downgraded to `chat_mode` automatically.
- Flag flip from true to false during operation -> no new activations; existing runtime data remains intact.
