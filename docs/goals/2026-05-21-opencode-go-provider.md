# Goal: OpenCode Go Provider

Date started: 2026-05-21
Status: active
Codex goal: Implement the OpenCode Go LLM provider from prd/PRD.md for Oxide Agent: add config/capabilities, provider implementation, registration, tests, docs, and commit after each implementation phase.

## Objective

Implement a focused `opencode-go` LLM provider for Oxide Agent Agent Mode using OpenCode Go's OpenAI-compatible Chat Completions endpoint. Done when `opencode-go` can be configured as an agent/sub-agent route, sends valid OpenAI-style tool schemas, preserves tool-call correlation across provider switches, maps capabilities correctly, and passes focused tests plus workspace formatting/checking.

## Scope

In scope:
- Add OpenCode Go settings, defaults, env examples, and provider registration.
- Add provider capability and media capability mapping for `opencode-go` / `opencode_go`.
- Implement `crates/oxide-agent-core/src/llm/providers/opencode_go.rs` with chat completion, tool-enabled chat, request builders, response parsing, model id normalization, and error handling.
- Preserve `ToolCallCorrelation` using `CHAT_LIKE_TOOL_PROFILE` for assistant tool calls and tool results.
- Add route-aware provider credential validation so OpenCode Go-only agent routes do not require a dummy `ZAI_API_KEY`.
- Add focused unit tests and docs/README/.env updates.

Out of scope:
- Streaming/SSE support for OpenCode Go.
- Dynamic `/models` discovery.
- Media understanding or audio transcription through OpenCode Go.
- Broad generic OpenAI-compatible provider refactor.
- Raising global context defaults to 1M.

## Repository Context

- PRD: `prd/PRD.md`.
- Provider entry points: `crates/oxide-agent-core/src/llm/providers/mod.rs`, `crates/oxide-agent-core/src/llm/client.rs`.
- Provider template: `crates/oxide-agent-core/src/llm/providers/openrouter.rs`, `crates/oxide-agent-core/src/llm/providers/openrouter/helpers.rs`.
- Tool correlation helpers: `crates/oxide-agent-core/src/llm/providers/protocol_profiles.rs`, `tool_call_encoder.rs`, `tool_result_encoder.rs`, `tool_correlation.rs`.
- Capabilities: `crates/oxide-agent-core/src/llm/capabilities.rs`.
- Config: `crates/oxide-agent-core/src/config.rs`, `.env.example`, `README.md`, `README-ru.md`.
- Branch observed at start: `dev`; repo instructions say default branch is `testing`, but this run keeps the current branch unless instructed otherwise.

## Implementation Plan

1. Phase 0: Add this goal document and commit it.
2. Phase 1: Add config, defaults, route-aware credential validation, capabilities, and focused tests. Commit after validation.
3. Phase 2: Implement and export `OpenCodeGoProvider` with request builders, parser, correlation handling, and provider-local tests. Commit after validation.
4. Phase 3: Register the provider in `LlmClient`, add client/config registration tests, update docs and `.env.example`. Commit after validation.
5. Phase 4: Run full formatting/check/lint validation, fix issues, update this goal doc final verification, and commit final cleanup if files change.

## Validation Contract

- Focused tests: `cargo test -p oxide-agent-core opencode_go -- --nocapture`.
- Config/capability tests: `cargo test -p oxide-agent-core config capabilities -- --nocapture` or narrower named tests when needed.
- Static check: `cargo check -p oxide-agent-core`.
- Formatting: `cargo fmt --all --check`, then `cargo fmt --all` if needed.
- Lint: `cargo clippy -p oxide-agent-core --all-targets --all-features`.
- Runtime/manual smoke not run unless `OPENCODE_GO_API_KEY` is available; live smoke remains optional/ignored.

Done when:
- `opencode-go` and optional alias `opencode_go` register when `OPENCODE_GO_API_KEY` is configured.
- `deepseek-v4-flash` and `deepseek-v4-pro` are structured-output-capable; unknown OpenCode Go models do not overclaim structured output.
- Tool request bodies include `tools[0].function.name`.
- Native tool response parsing preserves provider wire ids separately from runtime invocation ids.
- `response_format` is omitted when tools are present.
- OpenCode Go-only agent/sub-agent route config can load without `ZAI_API_KEY` when active routes have required credentials.
- Required validation commands pass or any remaining gap is documented explicitly.

## Decisions

- 2026-05-21: Keep OpenCode Go as a dedicated provider, not a mode of `chatgpt`, because ChatGPT uses the ChatGPT Codex Responses backend while OpenCode Go exposes OpenAI-compatible Chat Completions.
- 2026-05-21: Use a single-file provider module first to match project scale and PRD guidance.
- 2026-05-21: Commit after every phase as requested by the user.

## Progress Log

- 2026-05-21 13:54: Goal created from `prd/PRD.md`; RECON confirmed `opencode-go` is not implemented yet and current branch is `dev`.
- 2026-05-21 13:58: Phase 0 document created. Next: commit Phase 0, then implement config/capabilities.
- 2026-05-21 14:08: Phase 1 implemented config keys, route-aware ZAI/OpenCode Go credential validation, OpenCode Go capabilities/media mapping, and focused tests. Validation passed: `cargo fmt --all`, `cargo test -p oxide-agent-core opencode_go -- --nocapture`, `cargo test -p oxide-agent-core settings_ -- --nocapture`, `cargo test -p oxide-agent-core capabilities -- --nocapture`. Next: implement provider module.
- 2026-05-21 14:18: Phase 2 implemented `OpenCodeGoProvider`, OpenAI Chat Completions request builders, native tool-call parser, reasoning/usage parsing, model prefix normalization, and provider module export. Validation passed: `cargo test -p oxide-agent-core opencode_go -- --nocapture`, `cargo check -p oxide-agent-core`. Next: register provider in `LlmClient` and update docs/examples.

## Risks and Blockers

- Live OpenCode Go smoke requires `OPENCODE_GO_API_KEY`; do not add secrets to docs or tests.
- Route-aware credential validation touches startup config behavior; keep the change narrow and covered by tests.
- Strict tool history can expose existing invalid persisted history; rely on existing history repair path rather than bypassing validation.

## Final Verification

Pending.
