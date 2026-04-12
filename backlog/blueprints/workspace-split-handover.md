# Workspace Split Handover

## Structure
- `crates/oxide-agent-core`: core agent logic, providers, sandbox, storage, shared utils.
- `crates/oxide-agent-runtime`: transport-agnostic runtime helpers and session registry.
- `crates/oxide-agent-transport-telegram`: Telegram transport, handlers, UI, and settings.
- `crates/oxide-agent-telegram-bot`: binary entrypoint wiring config + transport.

## Adding Discord/Slack Transport
1. Create `crates/oxide-agent-transport-discord` (or `-slack`) as a lib.
2. Depend on `oxide-agent-core` + `oxide-agent-runtime`.
3. Implement `AgentTransport` (progress updates, file delivery, loop notifications).
4. Add transport-specific settings (token, allowlists) and a combined `BotSettings` struct if needed.
5. Add a new bin crate (e.g. `oxide-agent-discord-bot`) to wire config + handlers.

## Risks / Follow-ups
- Keep all transport-specific code out of `core`/`runtime` to preserve reusability.
- Ensure env vars remain flat when combining settings (use `#[serde(flatten)]` if needed).
- Update CI and deployment scripts when adding new transport crates.
