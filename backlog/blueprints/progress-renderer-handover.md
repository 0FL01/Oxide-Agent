## Progress renderer de-telegramize (Iteration 1)

- Removed Telegram/HTML rendering from agent progress state.
- Added bot-layer HTML renderer and tests for progress output.
- Updated agent handler to render progress via bot module.

## Universal progress runtime loop (Iteration 2)

- Added transport-agnostic progress runtime loop under `src/agent/runtime`.
- Wired Telegram agent handlers to use the runtime with `TelegramAgentTransport`.

## Transport-agnostic session identity (Iteration 3)

- Introduced `SessionId` newtype in the agent layer.
- Refactored agent sessions and registries to drop Telegram-specific IDs.
