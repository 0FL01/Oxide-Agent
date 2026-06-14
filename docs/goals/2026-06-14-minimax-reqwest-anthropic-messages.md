# Goal: Migrate MiniMax to reqwest Anthropic Messages Transport

Date started: 2026-06-14
Status: active
Codex goal: not set
Source spec: user request + RECON session
Goal doc owner: Codex
Last updated: 2026-06-14 22:00

## Objective

Move the MiniMax provider off the `claudius` SDK onto a shared internal Anthropic Messages v1 transport built on `reqwest` + raw JSON, by extracting the already-working Anthropic Messages helpers from `opencode_go.rs` into a reusable private module, then rewiring `MiniMaxProvider` to use that module plus the existing `support::http` reqwest client.

Done when every Completion Audit item is verified by its listed evidence, `MiniMaxProvider` no longer imports or uses `claudius`, the `claudius` crate dependency is removed, existing MiniMax route semantics (`provider = "minimax"` / `llm-provider/minimax`) still work, and all formatting/lint/test gates pass.

## Scope

In scope:
- `crates/oxide-agent-core/src/llm/providers/anthropic_messages/` -- new internal module (request builder, response parser, headers, usage parser), provider-neutral.
- `crates/oxide-agent-core/src/llm/providers/opencode_go.rs` -- delegate Anthropic Messages functions to the shared module; remove local duplicates.
- `crates/oxide-agent-core/src/llm/providers/minimax/` -- replace `claudius` client with `reqwest::Client` + shared Anthropic Messages transport.
- `crates/oxide-agent-core/src/llm/providers/mod.rs` -- feature-gate the new module.
- `crates/oxide-agent-core/src/llm/providers/modules.rs` -- add `llm-minimax` to shared `http_client`/`support` gates.
- `crates/oxide-agent-core/src/llm/support/mod.rs` -- add `llm-minimax` to `http` module gate.
- `crates/oxide-agent-core/Cargo.toml` -- change `llm-minimax` from `dep:claudius` to `dep:reqwest`; remove `claudius` dependency.
- `crates/oxide-agent-core/src/capabilities/compiled.rs` -- optionally add `api_base` config property for MiniMax.
- `crates/oxide-agent-core/src/config.rs` -- optionally add `MINIMAX_API_BASE` env override.
- Tests for the shared module and migrated MiniMax provider.

Out of scope:
- Adding a public `llm-provider/anthropic` runtime provider (the shared module is internal only).
- Adding Anthropic SSE streaming (current MiniMax path is non-streaming `stream: false`; this goal preserves that).
- Adding new crates, services, queues, caches, or abstraction layers.
- Changing the `LlmProvider` trait, `LlmProviderModule` trait, or tool-call correlation domain types.
- Changing route semantics: `provider = "minimax"` and `provider = "llm-provider/minimax"` must remain unchanged.
- Reintroducing a direct Google Gemini provider.
- Expanding MiniMax media capabilities (audio transcription / image analysis remain unimplemented stubs).

## Missing Inputs

- None. All code paths are local; no live API key is required for hermetic validation.

## Repository Context

- Existing goal convention: `docs/goals/archives/*.md` uses a durable goal document with Completion Audit, checkpoint plan, validation contract, decisions, progress log, and final verification. Active goals live at `docs/goals/*.md`.
- Recent precedent: `docs/goals/2026-06-14-zai-openai-base-migration.md` (ZAI SDK removal, same migration pattern: extract shared transport, migrate provider, remove SDK dependency).
- MiniMax provider current files:
  - `crates/oxide-agent-core/src/llm/providers/minimax/client.rs` -- `MiniMaxProvider` struct wrapping `claudius::Anthropic`.
  - `crates/oxide-agent-core/src/llm/providers/minimax/messages.rs` -- `Message` -> `claudius::MessageParam` conversion.
  - `crates/oxide-agent-core/src/llm/providers/minimax/response.rs` -- `claudius::Message` -> `ChatResponse` parsing (includes empty tool ID fallback workaround).
  - `crates/oxide-agent-core/src/llm/providers/minimax/tools.rs` -- `ToolDefinition` -> `claudius::ToolUnionParam`.
  - `crates/oxide-agent-core/src/llm/providers/minimax/module.rs` -- `MiniMaxProviderModule` factory.
- OpenCode Go already has a complete raw Anthropic Messages implementation in `opencode_go.rs` (lines 923-1616) that can be extracted.
- Shared HTTP helpers in `crates/oxide-agent-core/src/llm/support/http.rs`: `send_json_request`, `create_http_client`, `parse_retry_after`.
- Tool protocol infrastructure: `ANTHROPIC_CLIENT_TOOL_PROFILE` (`protocol_profiles.rs:97`), `ToolProtocol::AnthropicClientTools` (`types.rs:476`).
- Validation infrastructure from `AGENTS.md`:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo check --workspace --no-default-features --features profile-full`
  - Focused: `cargo test -p oxide-agent-core --no-default-features --features llm-minimax`
- Risky areas:
  - Feature gate propagation: `llm-minimax` must be added to every shared-module `#[cfg(any(...))]` gate that currently excludes it, or compilation fails silently.
  - MiniMax empty tool call ID workaround (`minimax/response.rs:28-40`) must survive the migration.
  - opencode_go behavior must not change when its local functions are replaced by the shared module.

## Completion Audit

### Functional requirements

- G1: Shared `anthropic_messages` internal module exists
  - Source: RECON plan step 1
  - Acceptance: `crates/oxide-agent-core/src/llm/providers/anthropic_messages/` contains `mod.rs`, `request.rs`, `response.rs` with provider-neutral functions: `build_completion_body`, `build_messages_body`, `parse_response`, `anthropic_headers`. No hardcoded provider labels in error messages.
  - Evidence required: file review; `rg "OpenCode Go" crates/oxide-agent-core/src/llm/providers/anthropic_messages` returns no matches.
  - Status: pending
  - Evidence collected:

- G2: Shared module is feature-gated correctly
  - Source: RECON plan step 2
  - Acceptance: `anthropic_messages` module compiles under both `llm-minimax` and `llm-opencode-go` feature gates; `providers/mod.rs` has appropriate `#[cfg(any(...))]` gate.
  - Evidence required: `cargo check -p oxide-agent-core --no-default-features --features llm-minimax` and `cargo check -p oxide-agent-core --no-default-features --features llm-opencode-go` both pass.
  - Status: pending
  - Evidence collected:

- G3: opencode_go delegates to shared module without behavior change
  - Source: RECON plan step 1
  - Acceptance: opencode_go.rs calls shared `anthropic_messages::build_completion_body`, `build_messages_body`, `parse_response`, `anthropic_headers` instead of local functions; local duplicates removed; existing opencode_go tests pass unchanged.
  - Evidence required: `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go --lib` passes; `rg "fn build_anthropic_completion_body|fn parse_anthropic_messages_response|fn anthropic_extra_headers" crates/oxide-agent-core/src/llm/providers/opencode_go.rs` returns no matches (functions moved out).
  - Status: pending
  - Evidence collected:

- G4: MiniMaxProvider uses reqwest + shared module
  - Source: RECON plan step 3
  - Acceptance: `MiniMaxProvider` struct holds `reqwest::Client`, `api_key: String`, `api_base: String` instead of `claudius::Anthropic`. Sends requests via `support::http::send_json_request`. Parses responses via `anthropic_messages::parse_response`. No `claudius` imports remain in `minimax/`.
  - Evidence required: `rg "claudius" crates/oxide-agent-core/src/llm/providers/minimax` returns no matches; `cargo check -p oxide-agent-core --no-default-features --features llm-minimax` passes.
  - Status: pending
  - Evidence collected:

- G5: MiniMax empty tool call ID workaround is preserved
  - Source: `minimax/response.rs:28-40`
  - Acceptance: shared `parse_response` generates a fallback ID (`minimax_fallback_{index}`) when a `tool_use` block has an empty/blank `id` field, matching current behavior. This is configurable via a profile/label parameter so opencode_go can use its own prefix.
  - Evidence required: unit test with empty `tool_use` ID asserts fallback ID is generated; `cargo test -p oxide-agent-core --no-default-features --features llm-minimax --lib` passes.
  - Status: pending
  - Evidence collected:

- G6: MiniMax request body matches current wire format
  - Source: `minimax/client.rs:56-77`
  - Acceptance: MiniMax tool-enabled request body contains `model`, `messages`, `max_tokens`, `temperature`, `stream: false`, `tools` (with `input_schema`), `tool_choice: {"type": "auto"}`, and top-level `system` when system prompt is non-empty.
  - Evidence required: unit test asserting all body fields; `cargo test -p oxide-agent-core --no-default-features --features llm-minimax --lib` passes.
  - Status: pending
  - Evidence collected:

- G7: MiniMax auth headers match Anthropic convention
  - Source: Anthropic Messages API docs
  - Acceptance: requests include `x-api-key: <MINIMAX_API_KEY>`, `anthropic-version: 2023-06-01`, `content-type: application/json`. No `Authorization: Bearer` header for MiniMax.
  - Evidence required: unit test or body/header inspection test.
  - Status: pending
  - Evidence collected:

- G8: `claudius` dependency is removed
  - Source: RECON plan step 4
  - Acceptance: `Cargo.toml` has no `claudius` dependency line; `llm-minimax = ["dep:reqwest"]` instead of `["dep:claudius"]`; `Cargo.lock` does not contain `claudius`.
  - Evidence required: `rg "claudius" crates/oxide-agent-core/Cargo.toml` returns no matches; `cargo tree -i claudius -p oxide-agent-core --no-default-features --features profile-full 2>&1 || true` reports no package match.
  - Status: pending
  - Evidence collected:

- G9: MiniMax route semantics are unchanged
  - Source: `minimax/module.rs:15-20`, `modules.rs:242-243`
  - Acceptance: `provider = "minimax"` and `provider = "llm-provider/minimax"` still resolve to `MiniMaxProviderModule`; aliases `["minimax"]` are preserved.
  - Evidence required: existing module registration tests pass: `cargo test -p oxide-agent-core --no-default-features --features llm-minimax --lib modules` or focused test names.
  - Status: pending
  - Evidence collected:

- G10: MiniMax `api_base` is configurable with sensible default
  - Source: RECON plan step 6
  - Acceptance: MiniMax module reads optional `api_base` from config/env with default `https://api.minimax.io/anthropic/v1/messages`; `MINIMAX_API_BASE` env var is respected when set.
  - Evidence required: module test or config test proving default and override; `rg "MINIMAX_API_BASE" crates/oxide-agent-core/src/capabilities/compiled.rs` returns a match in the MiniMax config properties block.
  - Status: pending
  - Evidence collected:

### Quality and constraint requirements

- Q1: No new crates, services, or abstraction layers
  - Source: `AGENTS.md` implementation bias
  - Acceptance: solution uses existing `reqwest`, `serde_json`, `support::http`, and `ANTHROPIC_CLIENT_TOOL_PROFILE`. No new dependencies added.
  - Evidence required: `git diff Cargo.toml Cargo.lock` shows only removal of `claudius` and feature rewiring; no added dependency lines.
  - Status: pending
  - Evidence collected:

- Q2: Anthropic Messages logic is not duplicated
  - Source: `AGENTS.md` over-engineering prevention
  - Acceptance: there is exactly one implementation of `prepare_anthropic_messages`, `parse_anthropic_messages_response`, `anthropic_extra_headers`, etc. -- in the shared module. opencode_go and minimax both delegate to it.
  - Evidence required: `rg "fn prepare_anthropic_messages" crates/oxide-agent-core/src` returns exactly one match (in `anthropic_messages/`).
  - Status: pending
  - Evidence collected:

- Q3: opencode_go behavior is unchanged after refactor
  - Source: RECON plan step 7
  - Acceptance: existing opencode_go tests pass without modification; no behavior change in request bodies, response parsing, headers, or protocol dispatch.
  - Evidence required: `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go --lib` passes; test count is stable.
  - Status: pending
  - Evidence collected:

- Q4: MiniMax temperatures are preserved
  - Source: `config.rs:32-35`
  - Acceptance: `MINIMAX_CHAT_TEMPERATURE = 1.0` and `MINIMAX_TOOL_TEMPERATURE = 1.0` are still used in the migrated provider's request bodies.
  - Evidence required: unit test or body-builder test asserting temperature values.
  - Status: pending
  - Evidence collected:

- Q5: No streaming is introduced
  - Source: Out of scope
  - Acceptance: MiniMax requests use `stream: false`; no SSE parser code is added.
  - Evidence required: `rg "stream.*true|bytes_stream|sse|event_stream" crates/oxide-agent-core/src/llm/providers/minimax` returns no matches.
  - Status: pending
  - Evidence collected:

- Q6: Tool-call correlation integrity is preserved
  - Source: `ANTHROPIC_CLIENT_TOOL_PROFILE` usage
  - Acceptance: inbound `tool_use` blocks produce `ToolCall` records with correct wire IDs and `AnthropicClientTools` protocol; outbound assistant messages encode tool calls as `tool_use` content blocks.
  - Evidence required: shared module tests for tool call round-trip; existing MiniMax tests pass.
  - Status: pending
  - Evidence collected:

- Q7: Implementation remains minimal and maintainable
  - Source: `AGENTS.md` scale/implementation bias
  - Acceptance: no broad refactor unrelated to the migration; code changes are localized to the shared module, minimax/, opencode_go.rs, and feature gates.
  - Evidence required: final `git diff --stat` and changed-file review.
  - Status: pending
  - Evidence collected:

### Validation requirements

- V1: Hermetic tests for shared Anthropic Messages module
  - Source: RECON plan step 8
  - Acceptance: tests cover text message conversion, assistant text + tool_use, consecutive tool results grouped into one user message, system prompt as top-level `system`, tools use `input_schema`, response parsing for text/tool_use/thinking/redacted_thinking, stop_reason mapping, cache usage tokens, empty tool ID fallback.
  - Evidence required: list of test names and passing `cargo test` output.
  - Status: pending
  - Evidence collected:

- V2: MiniMax provider tests
  - Source: RECON plan step 8
  - Acceptance: tests cover body fields (model, messages, max_tokens, temperature, stream, tools, tool_choice), headers (x-api-key, anthropic-version), no claudius types, empty tool ID fallback.
  - Evidence required: list of test names and passing `cargo test` output.
  - Status: pending
  - Evidence collected:

- V3: Module registration and capability tests
  - Source: existing `modules.rs:568-577`
  - Acceptance: existing MiniMax module tests (`minimax_module_registers_provider_id_and_aliases`, `minimax_module_owns_base_capabilities`) pass after migration.
  - Evidence required: test command output.
  - Status: pending
  - Evidence collected:

- V4: Formatting and lint gates pass
  - Source: `AGENTS.md` format/lint section
  - Acceptance: `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets -- -D warnings` pass.
  - Evidence required: command output recorded in Final Verification.
  - Status: pending
  - Evidence collected:

- V5: Profile-full build check passes
  - Source: `AGENTS.md` build section
  - Acceptance: `cargo check --workspace --no-default-features --features profile-full` passes after `claudius` removal.
  - Evidence required: command output recorded in Final Verification.
  - Status: pending
  - Evidence collected:

### Non-goals and exclusions

- N1: Do not add a public Anthropic provider
  - Source: Out of scope
  - Must preserve: the shared `anthropic_messages` module is `pub(crate)` or internal; no `llm-provider/anthropic` runtime provider is registered.
  - Evidence required: `rg "llm-provider/anthropic|pub struct AnthropicProvider" crates/oxide-agent-core/src` returns no matches.
  - Status: pending
  - Evidence collected:

- N2: Do not add streaming
  - Source: Out of scope
  - Must preserve: MiniMax requests use `stream: false`; no SSE code is added.
  - Evidence required: `rg "bytes_stream|process_sse_event|StreamingChatAccumulator" crates/oxide-agent-core/src/llm/providers/minimax` returns no matches.
  - Status: pending
  - Evidence collected:

- N3: Do not change route provider keys
  - Source: Out of scope
  - Must preserve: `provider = "minimax"` and `provider = "llm-provider/minimax"` remain valid route values.
  - Evidence required: config/route tests pass.
  - Status: pending
  - Evidence collected:

## Implementation Plan

1. Checkpoint 0 -- goal contract and baseline
   - Audit IDs: setup only
   - Expected changes: create this goal doc; commit as standalone.
   - Validation: `git status --short --branch`; diff review of this document.
   - Exit condition: goal doc committed and ready for review.

2. Checkpoint 1 -- extract shared `anthropic_messages` internal module
   - Audit IDs: G1, G2, Q2
   - Expected changes:
     - Create `crates/oxide-agent-core/src/llm/providers/anthropic_messages/mod.rs` with re-exports.
     - Create `crates/oxide-agent-core/src/llm/providers/anthropic_messages/request.rs` with functions extracted from `opencode_go.rs:923-1299`:
       - `build_completion_body`
       - `build_messages_body`
       - `prepare_anthropic_messages`
       - `anthropic_text_message`
       - `anthropic_assistant_message`
       - `anthropic_tool_result_block`
       - `prepare_anthropic_tools_json`
       - `anthropic_system_prompt`
     - Create `crates/oxide-agent-core/src/llm/providers/anthropic_messages/response.rs` with functions extracted from `opencode_go.rs:1360-1616`:
       - `parse_response` (with `AnthropicMessagesProfile` parameter for provider label + empty-tool-id prefix)
       - `map_stop_reason`
       - `parse_usage`
     - Create `AnthropicMessagesProfile` struct with `provider_label: &'static str` and `empty_tool_id_prefix: &'static str`.
     - Add `#[cfg(any(feature = "llm-minimax", feature = "llm-opencode-go"))] pub(crate) mod anthropic_messages;` to `providers/mod.rs`.
     - Add unit tests for request/response helpers.
   - Validation: `cargo test -p oxide-agent-core --no-default-features --features llm-minimax --lib anthropic_messages`; `cargo check -p oxide-agent-core --no-default-features --features llm-opencode-go`.
   - Exit condition: shared module compiles standalone under both feature gates; unit tests pass.

3. Checkpoint 2 -- rewire opencode_go to shared module
   - Audit IDs: G3, Q2, Q3
   - Expected changes:
     - Replace local Anthropic functions in `opencode_go.rs` with calls to `anthropic_messages::*`.
     - Pass `AnthropicMessagesProfile { provider_label: "OpenCode Go", empty_tool_id_prefix: "opencode_go_tool_use" }` to `parse_response`.
     - Remove local duplicates: `build_anthropic_completion_body`, `build_anthropic_messages_body`, `prepare_anthropic_messages`, `anthropic_text_message`, `anthropic_assistant_message`, `anthropic_tool_result_block`, `prepare_anthropic_tools_json`, `anthropic_system_prompt`, `anthropic_extra_headers`, `parse_anthropic_messages_response`, `map_anthropic_stop_reason`, `parse_anthropic_usage`.
   - Validation: `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go --lib`; test count stable; `rg "fn build_anthropic_completion_body|fn parse_anthropic_messages_response" crates/oxide-agent-core/src/llm/providers/opencode_go.rs` returns no matches.
   - Exit condition: opencode_go delegates to shared module; all existing tests pass unchanged.

4. Checkpoint 3 -- migrate MiniMaxProvider to reqwest
   - Audit IDs: G4, G5, G6, G7, Q4, Q5, Q6
   - Expected changes:
     - Add `llm-minimax` to feature gates in: `support/mod.rs:3`, `modules.rs:10`, `modules.rs:21`, `modules.rs:34`, `providers/mod.rs:16-23`.
     - Replace `MiniMaxProvider` struct: `http_client: reqwest::Client`, `api_key: String`, `api_base: String`.
     - Update `MiniMaxProvider::new` to accept shared `reqwest::Client` from `LlmProviderBuildContext`.
     - Update `MiniMaxProviderModule::build_provider` to pass `ctx.http_client`.
     - Implement `chat_with_tools` and `complete_internal_text` using `support::http::send_json_request` + `anthropic_messages::build_messages_body` / `build_completion_body` + `anthropic_messages::parse_response` with `AnthropicMessagesProfile { provider_label: "MiniMax", empty_tool_id_prefix: "minimax_fallback" }`.
     - Delete `minimax/messages.rs` and `minimax/tools.rs` (replaced by shared module).
     - Delete or rewrite `minimax/response.rs` (replaced by shared module).
     - Keep `minimax/client.rs` as the `MiniMaxProvider` struct + `impl LlmProvider`.
   - Validation: `cargo test -p oxide-agent-core --no-default-features --features llm-minimax --lib`; `rg "claudius" crates/oxide-agent-core/src/llm/providers/minimax` returns no matches.
   - Exit condition: MiniMax provider compiles and passes tests without `claudius`.

5. Checkpoint 4 -- remove `claudius` dependency and add `api_base` config
   - Audit IDs: G8, G9, G10, Q1, V3
   - Expected changes:
     - `Cargo.toml`: change `llm-minimax = ["dep:claudius"]` to `llm-minimax = ["dep:reqwest"]`.
     - `Cargo.toml`: remove `claudius = { version = "0.19.0", optional = true }`.
     - `capabilities/compiled.rs`: add `api_base` config property to `MINIMAX_CONFIG_PROPERTIES` with `MINIMAX_API_BASE` env and default `https://api.minimax.io/anthropic/v1/messages`.
     - `minimax/module.rs`: read `api_base` from config/env/default.
     - Update `Cargo.lock` via `cargo update`.
   - Validation: `cargo check -p oxide-agent-core --no-default-features --features llm-minimax`; `cargo tree -i claudius -p oxide-agent-core --no-default-features --features profile-full 2>&1 || true` reports no match; `rg "claudius" crates/oxide-agent-core/Cargo.toml` returns no matches; module tests pass.
   - Exit condition: `claudius` is fully removed; MiniMax default base URL works.

6. Checkpoint 5 -- snapshots, final validation, and audit
   - Audit IDs: V1, V2, V4, V5, Q7, N1, N2, N3
   - Expected changes:
     - Regenerate modular registry snapshots if needed.
     - Update README/.env.example if MiniMax config examples reference `claudius` (unlikely).
     - Fill Completion Audit evidence and Final Verification.
   - Validation:
     - `cargo fmt --all -- --check`
     - `cargo clippy --workspace --all-targets -- -D warnings`
     - `cargo check --workspace --no-default-features --features profile-full`
     - `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib anthropic_messages`
     - `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib minimax`
     - `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib opencode_go`
     - `cargo test -p oxide-agent-core --no-default-features --features profile-full --test modular_registry_snapshots`
   - Exit condition: every audit item is verified with current evidence.

## Validation Contract

- Static checks:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo check --workspace --no-default-features --features profile-full`
- Focused tests:
  - `cargo test -p oxide-agent-core --no-default-features --features llm-minimax --lib`
  - `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go --lib`
  - `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib anthropic_messages`
  - `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib minimax`
  - `cargo test -p oxide-agent-core --no-default-features --features profile-full --test modular_registry_snapshots`
- Artifact verification:
  - `rg "claudius" crates/oxide-agent-core` returns no matches after cleanup.
  - `cargo tree -i claudius -p oxide-agent-core --no-default-features --features profile-full` reports no package match.
  - `rg "fn prepare_anthropic_messages" crates/oxide-agent-core/src` returns exactly one match (in shared module).
- Done when: all Completion Audit items are `verified`.

## Decisions

- 2026-06-14: Use `docs/goals/2026-06-14-minimax-reqwest-anthropic-messages.md` because active goal docs live under `docs/goals/`.
- 2026-06-14: Extract opencode_go Anthropic functions to a shared internal module instead of duplicating logic in MiniMax, because the code already exists and is proven in production.
- 2026-06-14: Use `AnthropicMessagesProfile` struct for provider-specific label and empty-tool-id prefix, instead of generic string parameters or separate parser instances.
- 2026-06-14: Port-before-delete sequencing: extract shared module and rewire opencode_go before migrating MiniMax, so behavior is verifiable at each step.
- 2026-06-14: Keep `stream: false` for MiniMax; do not add SSE streaming in this goal (non-streaming is the current behavior).
- 2026-06-14: Add optional `api_base` config for MiniMax to allow endpoint flexibility without over-engineering; default stays `https://api.minimax.io/anthropic/v1/messages`.

## Progress Log

- 2026-06-14 22:00: Checkpoint 0 started -- goal contract and baseline
  - Changed: created goal document from RECON findings and user plan.
  - Evidence: RECON session mapped all MiniMax/opencode_go/support files and feature gates; existing ZAI migration goal used as precedent for structure.
  - Commands: `git status --short --branch` (pending commit).
  - Audit IDs updated: setup only.
  - Next: user review of goal doc, then implement Checkpoint 1.

## Risks and Blockers

- Feature gate propagation may miss a `#[cfg(any(...))]` that excludes `llm-minimax`.
  - Impact: silent compilation failure or dead code warnings.
  - Evidence: known gates in `support/mod.rs:3`, `modules.rs:10/21/34`, `providers/mod.rs:16-23`.
  - Mitigation: focused `cargo check --no-default-features --features llm-minimax` at each checkpoint.
  - Audit IDs affected: G2.

- opencode_go behavior regression from shared module extraction.
  - Impact: existing Anthropic Messages protocol path breaks for OpenCode Go routes.
  - Evidence: opencode_go has extensive tests covering Anthropic body/response behavior.
  - Mitigation: Checkpoint 2 runs the full opencode_go test suite before and after.
  - Audit IDs affected: G3, Q3.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
