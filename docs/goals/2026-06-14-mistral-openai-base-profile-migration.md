# Goal: Migrate Mistral Provider to OpenAI-Compatible Profile

Date started: 2026-06-14
Status: active
Codex goal: `/goal Implement docs/goals/2026-06-14-mistral-openai-base-profile-migration.md until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update this document after each meaningful verification, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.`
Source spec: User-provided migration plan (Mistral -> OpenAI-compatible profile).
Goal doc owner: Codex
Last updated: 2026-06-14 12:45

## Objective

Replace the standalone Mistral provider implementation with a Mistral profile of the shared `OpenAIBaseProvider`. Remove the `async-openai` crate dependency entirely. Preserve all current Mistral behavior: strict tool history, 9-character alphanumeric tool-call ID mapping with reverse mapping, `parallel_tool_calls=true`, reasoning model `reasoning_effort`, Mistral temperature defaults, content-array/reasoning parsing, cached token usage parsing, audio transcription via `/audio/transcriptions` with retry logic, and audio-only media capabilities. Keep `mistral` and `llm-provider/mistral` aliases backward-compatible with `MISTRAL_API_KEY`.

Done when every Completion Audit item is verified by its listed evidence, `cargo tree -i async-openai` finds nothing, and all existing Mistral and openai_base tests pass.

## Scope

In scope:
- `crates/oxide-agent-core/src/llm/providers/openai_base/` -- add `OpenAICompatibleProfile`, tool ID mapper, message layout policies, response parser policies, audio transcription, reasoning policy, request tweaks.
- `crates/oxide-agent-core/src/llm/providers/mistral/` -- thin out to a compatibility wrapper that builds `OpenAIBaseProvider` with `OpenAICompatibleProfile::mistral()`.
- `crates/oxide-agent-core/src/llm/support/openai_compat.rs` -- delete (only used by Mistral via `async-openai`).
- `crates/oxide-agent-core/src/llm/support/common.rs` -- delete (only used by `openai_compat.rs`).
- `crates/oxide-agent-core/src/llm/support/mod.rs` -- adjust feature gates.
- `crates/oxide-agent-core/Cargo.toml` -- change `llm-mistral` feature to depend on `llm-openai-base`, remove `async-openai` dependency.
- `crates/oxide-agent-core/src/llm/providers/modules.rs` -- no changes expected (registration stays, module just builds differently).
- `.env.example` -- update if needed.
- Docs: README, capabilities docs if they reference Mistral implementation internals.

Out of scope:
- Migrating ZAI/GLM or any other provider to the profile framework (future work).
- Adding Google Gemini direct provider.
- Changing tool-call correlation types (`InvocationId`, `ProviderToolCallId`, `ToolCallCorrelation`) in `types.rs`.
- Changing the `LlmProvider` trait signature.
- Enabling Mistral image understanding (stays disabled).
- Streaming support changes.
- New crates, services, queues, or abstraction layers beyond the profile struct.

## Missing Inputs

None. The migration plan was reviewed by the user before goal creation.

## Repository Context

### Current Mistral implementation (to be migrated)

11 files under `crates/oxide-agent-core/src/llm/providers/mistral/`:

| File | Lines | Purpose | Migration target |
|------|-------|---------|-----------------|
| `mod.rs` | 169 | `MistralProvider` struct (fields: `client: Client<OpenAIConfig>`, `http_client`, `api_key`, `id_mapper`), `LlmProvider` impl. Non-reasoning `complete_internal_text` uses `async-openai`; everything else uses raw reqwest. | Thin wrapper or delete; replaced by `OpenAIBaseProvider` |
| `module.rs` | 45 | Module registration, `MISTRAL_API_KEY` config, capabilities: `Strict, true, true`, media: `audio=true, image=false, video=false` | Keep as thin alias builder |
| `id_mapper.rs` | 165 | `ToolCallIdMapper` -- bidirectional UUID-to-9-char ID mapping. `normalize_for_mistral()`: strip non-alphanumeric, take last 9 chars. No collision detection. | Move to `openai_base/tool_ids.rs` |
| `chat.rs` | 216 | Request body builders, HTTP send. `build_tool_chat_body()`: `tool_choice="auto"`, `parallel_tool_calls=true`, reasoning_effort for reasoning models. | Merge into `openai_base` profile logic |
| `messages.rs` | 142 | `prepare_structured_messages()` (tool chat) and `prepare_chat_messages()` (plain chat). History system messages collected and prepended before main system prompt. Assistant tool calls get mapped IDs. Tool results get mapped `tool_call_id` + optional `name`. | Merge into `openai_base` message layout policy |
| `parsing.rs` | 211 | `parse_chat_response()` with content-array support (`thinking`/`reasoning`/`text` chunks), `extract_message_content()`, recursive `extract_text_segments()`, `parse_usage()` with `cached_tokens`, `parse_tool_calls()` with ID reverse mapping. | Merge into `openai_base` response parser policy |
| `transcription.rs` | 299 | Audio transcription via `POST /v1/audio/transcriptions`. Multipart: `file`, `model`, `temperature`. Retry: 5 attempts, 3s base, exponential backoff. 429 with `Retry-After`, 502/503/504, timeout, JSON parse transient. | Move to `openai_base` as profile capability |
| `client.rs` | 18 | `async-openai` client factory + reqwest client factory. | Delete |
| `types.rs` | 7 | `MISTRAL_REASONING_MODEL_ID = "mistral-small-2603"`, `MISTRAL_REASONING_EFFORT = "high"` | Move into profile constants |
| `image.rs` | 23 | Stub returning `LlmError::Unknown("Not implemented for Mistral")`. | Delete (behavior = unsupported media capability) |
| `tests.rs` | 491 | 11 unit tests covering body building, parsing, ID roundtrip. | Port to `openai_base` profile tests |

Temperature constants (in `crates/oxide-agent-core/src/config.rs`):
- `MISTRAL_CHAT_TEMPERATURE = 0.9`
- `MISTRAL_REASONING_TEMPERATURE = 0.7`
- `MISTRAL_TOOL_TEMPERATURE = 0.7`
- `MISTRAL_AUDIO_TRANSCRIBE_TEMPERATURE = 0.4`

### Current openai_base implementation (migration target)

2 files under `crates/oxide-agent-core/src/llm/providers/openai_base/`:

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | 683 | `OpenAIBaseProvider` struct (`http_client`, `api_key: Option<String>`, `api_base`). `LlmProvider` impl. All request building, response parsing in-module. Audio transcription returns unsupported. |
| `module.rs` | 283 | Multi-instance env config (`OPENAI_BASE_PROVIDERS__N__*`), model catalog integration, capabilities: `BestEffort, true, true`, media: `audio=false, image=true, video=false`. |

Key gaps in `openai_base` that need filling from Mistral:
- No profile/strategy system.
- No tool-call ID mapping.
- `prepare_structured_messages()` does not collect/prepend history system messages.
- `parse_chat_response()` does not handle content arrays.
- No reasoning effort support.
- No `parallel_tool_calls`.
- No audio transcription.
- Temperature is flat 0.7 everywhere (`OPENAI_BASE_CHAT_TEMPERATURE`).

### `async-openai` dependency

- Declared at `Cargo.toml:28`: `async-openai = { version = "0.40.2", default-features = false, features = ["rustls", "chat-completion", "audio", "image"], optional = true }`
- Activated only by `feature = "llm-mistral"` (`Cargo.toml:241`).
- Used in 4 files: `mistral/client.rs`, `mistral/mod.rs`, `support/openai_compat.rs`, `support/common.rs`.
- `support/mod.rs` gates `common` and `openai_compat` modules behind `feature = "llm-mistral"`.

### Feature flags

- `llm-mistral = ["dep:async-openai", "dep:reqwest"]` -- must change to `llm-mistral = ["llm-openai-base"]`.
- Profiles including `llm-mistral`: `profile-full` only.
- `llm-openai-base = ["dep:reqwest"]` -- stays as-is.

### Validation infrastructure

- `cargo check --workspace --no-default-features --features profile-full` -- full profile check.
- `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- mistral` -- Mistral tests.
- `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- openai_base` -- OpenAI Base tests.
- `cargo tree -i async-openai -p oxide-agent-core --no-default-features --features profile-full` -- must return no matches after migration.
- `cargo clippy --workspace --all-targets -- -D warnings`.
- `cargo fmt --all -- --check`.

## Completion Audit

### Functional requirements

- G1: `OpenAICompatibleProfile` struct exists in `openai_base/` with all policy fields
  - Source: plan section 1 -- "Introduce OpenAICompatibleProfile"
  - Acceptance: Struct with fields for name, default_api_base, capabilities, media_capabilities, temperature defaults (chat/tool/reasoning/audio), tool_history_mode, tool_call_id_strategy, message_layout_policy, response_content_policy, json_mode_policy, parallel_tool_calls, audio_transcription, reasoning_policy. Must be `const`-constructible (all `&'static str` / enum / struct fields, no heap).
  - Evidence required: `cargo check -p oxide-agent-core --no-default-features --features profile-full` compiles
  - Status: verified
  - Evidence collected: `openai_base/profile.rs` created with `OpenAICompatibleProfile` struct (16 fields, all `Copy`). All policy enums: `ToolCallIdStrategy`, `MessageLayoutPolicy`, `ResponseContentPolicy`, `JsonModePolicy`, `ModelMatchPolicy`, `ReasoningPolicy`, `AudioTranscriptionProfile`. `cargo check` + `cargo clippy -- -D warnings` clean.

- G2: `OpenAICompatibleProfile::mistral()` const constructor exists with all Mistral constants
  - Source: plan section 1, 9 -- Mistral profile values
  - Acceptance: Returns profile with: name="mistral", default_api_base="https://api.mistral.ai/v1", capabilities=Strict/true/true, media=audio=true/image=false/video=false, chat_temp=0.9, tool_temp=0.7, reasoning_temp=0.7, audio_temp=Some(0.4), tool_history=Strict, tool_call_id=MistralNineAlnum, message_layout=MistralStrict, response_content=StringOrChunkArrayWithReasoning, json_mode=standard, parallel_tool_calls=Some(true), audio_transcription=Some(...), reasoning=Mistral{default_effort="high", match="mistral-small-2603"}
  - Evidence required: unit test asserting all field values
  - Status: verified
  - Evidence collected: `mistral_profile_has_expected_values` + `mistral_reasoning_model_match` tests pass in `profile.rs`.

- G3: `OpenAICompatibleProfile::generic()` const constructor exists for default openai_base instances
  - Source: plan section 1, 11 -- generic profile
  - Acceptance: Returns profile with: name="generic", default_api_base="" (from env), capabilities=BestEffort/true/true, media=audio=false/image=true/video=false, tool_history=BestEffort, tool_call_id=Preserve, message_layout=GenericOpenAI, response_content=StringOnly, parallel_tool_calls=None, audio_transcription=None, reasoning=None. Generic openai_base instances use this by default and behavior is unchanged from current.
  - Evidence required: existing openai_base tests pass without expectation changes
  - Status: verified
  - Evidence collected: `generic_profile_has_expected_values` + `generic_never_reasoning` tests pass. All 21 existing openai_base tests pass unchanged. `auth_header_is_optional` test still uses `OpenAIBaseProvider::new()` which now delegates to `new_with_client_and_profile(..., generic())`.

- G4: Tool-call ID mapper moved to `openai_base/` with strategy enum
  - Source: plan section 3 -- "Move Mistral ID mapper into openai_base"
  - Acceptance: `ToolCallIdMapper` and `normalize_for_mistral()` live in `openai_base/` (e.g. `tool_ids.rs`). `ToolCallIdStrategy` enum with variants `Preserve` and `MistralNineAlnum`. All 4 existing id_mapper tests pass from new location.
  - Evidence required: `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- id_mapper` passes
  - Status: verified
  - Evidence collected: `ToolCallIdMapper` lives in `openai_base/tool_ids.rs` with all methods. `mistral/id_mapper.rs` is now a thin re-export (`pub(crate) use ...`). `ToolCallIdStrategy` enum exists in `profile.rs`. All 6 tool_ids tests pass (4 original + collision + stable_fallback). 11 mistral provider tests pass via re-export. `llm-mistral` feature now enables `llm-openai-base` for module visibility. `mistral/mod.rs::id_mapper()` visibility lowered to `pub(crate)` to match `ToolCallIdMapper`'s `pub(crate)` visibility.

- G5: ID mapper collision handling added
  - Source: plan section 3 -- collision mine
  - Acceptance: When generated 9-char ID is already mapped to a different original ID, generate a stable 9-char fallback (hash/base36/base62 of original ID). Existing non-colliding behavior unchanged.
  - Status: verified
  - Evidence collected: `register()` now checks for collision. If truncated ID maps to a different original, generates stable 9-char base36 of `DefaultHasher` hash via `stable_fallback()`. `find_collision_free_id()` tries salted variants (1-35) if base36 also collides (astronomically unlikely). `test_collision_handling` proves two different originals normalizing to same 9 chars get different IDs. `test_stable_fallback_is_deterministic` proves determinism.

- G6: Mistral message layout policy implemented in `openai_base`
  - Source: plan section 4 -- "Move Mistral message layout"
  - Acceptance: `MessageLayoutPolicy::MistralStrict` collects history system messages and prepends them before main system prompt. Assistant tool calls get mapped Mistral IDs. Tool result messages get mapped `tool_call_id` + optional `name`. `MessageLayoutPolicy::GenericOpenAI` preserves current openai_base behavior.
  - Evidence required: ported tests for assistant tool calls and tool result messages pass
  - Status: pending
  - Evidence collected:

- G7: Mistral response parser policy implemented in `openai_base`
  - Source: plan section 7 -- "Transfer response parser"
  - Acceptance: `ResponseContentPolicy::StringOrChunkArrayWithReasoning` handles content as both string and array. Extracts `thinking`/`reasoning` chunks recursively. Falls back to `reasoning_content` top-level field. Parses `usage.prompt_tokens_details.cached_tokens`. Reverse-maps tool call IDs. `ResponseContentPolicy::StringOnly` preserves current generic behavior.
  - Evidence required: ported tests for reasoning chunks, content array, usage parsing, tool call reverse mapping pass
  - Status: pending
  - Evidence collected:

- G8: Mistral request tweaks implemented as profile data
  - Source: plan section 5 -- "Transfer tool request tweaks"
  - Acceptance: Mistral profile produces request bodies with `tool_choice="auto"`, `parallel_tool_calls=true`, correct temperatures (chat/tool/reasoning), `reasoning_effort="high"` only for reasoning models, JSON mode only when `json_mode && !has_tools`. Generic profile does not add `parallel_tool_calls`.
  - Evidence required: ported tests for plain chat body, reasoning body, tool body, JSON mode pass
  - Status: pending
  - Evidence collected:

- G9: Audio transcription moved to `openai_base` as profile capability
  - Source: plan section 8 -- "Add audio transcription"
  - Acceptance: `AudioTranscriptionProfile` with endpoint_path, temperature, timeout_secs, max_retries, initial_backoff_ms. Mistral profile has `Some(...)`, generic has `None`. `transcribe_audio()` builds multipart from base URL (not chat completions URL), sends `file`, `model`, `temperature`. 120s timeout, retry on 429/502/503/504/timeout/JSON-transient. Generic `transcribe_audio()` still returns unsupported.
  - Evidence required: ported tests for multipart, retry delay, mime mapping pass
  - Status: pending
  - Evidence collected:

- G10: `OpenAIBaseProvider` carries profile + tool_id_mapper fields
  - Source: plan section 2 -- "Extend OpenAIBaseProvider"
  - Acceptance: Struct has `profile: OpenAICompatibleProfile` and `tool_id_mapper: Arc<Mutex<ToolCallIdMapper>>` fields. `new_with_profile()` constructor. Mutex lock is never held across `.await` points (build body -> release -> await -> re-acquire for parse).
  - Evidence required: `cargo clippy --workspace --all-targets -- -D warnings` passes (no `await_holding_lock`)
  - Status: verified
  - Evidence collected: `OpenAIBaseProvider` struct now has `profile` + `tool_id_mapper` fields. `new_with_client_and_profile()` constructor added. Existing `new()`/`new_with_client()` delegate to it with `generic()` profile. Clippy clean. Mapper methods not yet wired into provider logic (checkpoint 2+), so no `await` interaction yet.

- G11: `MistralProviderModule` builds `OpenAIBaseProvider` with Mistral profile
  - Source: plan section 10 -- "Redo module/feature layer"
  - Acceptance: `mistral/module.rs::build_provider()` creates `OpenAIBaseProvider::new_with_client_and_profile(Some(api_key), "https://api.mistral.ai/v1", http_client, OpenAICompatibleProfile::mistral())`. Module ID stays `"llm-provider/mistral"`, aliases stay `["mistral"]`, capabilities stay `Strict/true/true`, media stays `audio=true/image=false/video=false`.
  - Evidence required: `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- mistral` passes; capabilities command output unchanged
  - Status: pending
  - Evidence collected:

- G12: `async-openai` dependency removed
  - Source: plan section 8, 10 -- "Remove async-openai"
  - Acceptance: `Cargo.toml` has no `async-openai` line. `llm-mistral` feature no longer references it. `support/openai_compat.rs` and `support/common.rs` deleted. `support/mod.rs` no longer gates them. `mistral/client.rs` deleted.
  - Evidence required: `cargo tree -i async-openai -p oxide-agent-core --no-default-features --features profile-full` returns "not found"
  - Status: pending
  - Evidence collected:

- G13: Old Mistral implementation files deleted
  - Source: plan section 12 -- "Delete Mistral implementation files"
  - Acceptance: `mistral/chat.rs`, `mistral/client.rs`, `mistral/id_mapper.rs`, `mistral/messages.rs`, `mistral/parsing.rs`, `mistral/transcription.rs`, `mistral/image.rs`, `mistral/types.rs`, `mistral/tests.rs` deleted. Only `mistral/mod.rs` (thin) and `mistral/module.rs` remain.
  - Evidence required: `ls crates/oxide-agent-core/src/llm/providers/mistral/` shows only `mod.rs` and `module.rs`
  - Status: pending
  - Evidence collected:

- G14: Optional `PROFILE` env var for openai_base instances
  - Source: plan section 11 -- "Add PROFILE for generic openai-base instances"
  - Acceptance: `OPENAI_BASE_PROVIDERS__N__PROFILE=mistral` makes instance use Mistral profile. `OPENAI_BASE_PROVIDERS__N__PROFILE=generic` or absent uses generic profile. `provider = "mistral"` still works via `MISTRAL_API_KEY`.
  - Evidence required: unit test setting `PROFILE=mistral` env var and asserting profile selection
  - Status: pending
  - Evidence collected:

### Quality requirements

- Q1: Mutex lock not held across `.await`
  - Source: plan mine 2
  - Acceptance: No clippy `await_holding_lock` warning
  - Evidence required: `cargo clippy --workspace --all-targets -- -D warnings` clean
  - Status: pending
  - Evidence collected:

- Q2: Generic openai_base does not inherit Mistral-only behavior
  - Source: plan mine 5, section "Definition of done"
  - Acceptance: Existing openai_base tests pass without expectation changes. No `parallel_tool_calls`, no `reasoning_effort`, no tool ID mapping, no content-array parsing added to generic path unless already present.
  - Evidence required: existing openai_base tests green; `git diff` review of test expectations
  - Status: pending
  - Evidence collected:

- Q3: Mistral image route stays disabled
  - Source: plan mine 5
  - Acceptance: Mistral media capabilities remain `audio=true, image=false, video=false`. Image requests to `mistral` route do not resolve.
  - Evidence required: unit test or capabilities output assertion
  - Status: pending
  - Evidence collected:

- Q4: `reasoning_effort` only sent to matching models
  - Source: plan mine 6
  - Acceptance: `reasoning_effort` field appears only when model matches the reasoning policy model match (currently `mistral-small-2603`, case-insensitive). Other models never receive it.
  - Evidence required: unit test with non-reasoning model asserting no `reasoning_effort` in body
  - Status: pending
  - Evidence collected:

- Q5: `json_mode` policy preserved
  - Source: plan mine 7
  - Acceptance: `response_format: {"type":"json_object"}` only added when `json_mode=true && !has_tools`. Both Mistral and generic profiles follow this rule.
  - Evidence required: ported test for JSON mode without tools; test for no response_format when tools present
  - Status: pending
  - Evidence collected:

- Q6: `parallel_tool_calls` explicitly `true` for Mistral
  - Source: plan mine 8
  - Acceptance: Mistral tool body includes `"parallel_tool_calls": true` explicitly, even though API default is also true. Behavior-preserving.
  - Evidence required: ported test asserting `parallel_tool_calls` is present and true
  - Status: pending
  - Evidence collected:

- Q7: No new crates or dependencies added
  - Source: AGENTS.md implementation principles
  - Acceptance: `Cargo.toml` has fewer dependencies (removed `async-openai`), no new deps added.
  - Evidence required: `git diff Cargo.toml` review
  - Status: pending
  - Evidence collected:

### Validation requirements

- V1: Full workspace check passes
  - Evidence required: `cargo check --workspace --no-default-features --features profile-full` clean
  - Status: pending
  - Evidence collected:

- V2: All tests pass
  - Evidence required: `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib` all green
  - Status: pending
  - Evidence collected:

- V3: Clippy and fmt clean
  - Evidence required: `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all -- --check` both clean
  - Status: pending
  - Evidence collected:

- V4: `async-openai` absent from dependency tree
  - Evidence required: `cargo tree -i async-openai -p oxide-agent-core --no-default-features --features profile-full` returns "not found"
  - Status: pending
  - Evidence collected:

- V5: Provider aliases still work
  - Evidence required: capabilities CLI output shows `mistral` and `llm-provider/mistral` with correct capabilities/media
  - Status: pending
  - Evidence collected:

- V6: Tool-call roundtrip preserves original internal ID
  - Evidence required: unit test: register internal ID -> normalize -> parse response with that ID -> recover original internal ID
  - Status: pending
  - Evidence collected:

### Non-goals / exclusions

- N1: No ZAI/GLM migration in this goal
  - Must preserve: ZAI provider stays as-is
  - Evidence required: `git diff` shows no changes to `llm/providers/zai/`
  - Status: pending
  - Evidence collected:

- N2: No Google Gemini direct provider
  - Must preserve: Gemini stays accessible only via OpenRouter
  - Evidence required: no new Gemini provider code
  - Status: pending
  - Evidence collected:

- N3: No LlmProvider trait signature changes
  - Must preserve: trait methods unchanged
  - Evidence required: `git diff` of `provider.rs` shows no signature changes
  - Status: pending
  - Evidence collected:

- N4: No tool-call correlation type changes
  - Must preserve: `InvocationId`, `ProviderToolCallId`, `ToolCallCorrelation` in `types.rs` unchanged
  - Evidence required: `git diff` of `types.rs` shows no changes to correlation types
  - Status: pending
  - Evidence collected:

## Implementation Plan

### Checkpoint 0: Port Mistral tests as regression baseline
- Audit IDs: G4, G6, G7, G8, G9 (test scaffolding)
- Expected changes:
  - Copy/port Mistral test cases from `mistral/tests.rs` into `openai_base/` as `#[cfg(test)]` tests tagged for the Mistral profile. These tests will initially fail (expected) -- they are the target, not the starting point.
  - Minimum regression tests: plain chat body, reasoning model body, tool body (`tool_choice="auto"`, `parallel_tool_calls=true`), no `response_format` with tools, JSON mode without tools, assistant tool call mapped IDs, tool result mapped `tool_call_id`, inbound Mistral tool call reverse mapping, unknown provider tool call ID correlation, empty tool call ID fallback, content as string, content as array with reasoning, `usage.prompt_tokens_details.cached_tokens`, audio multipart fields.
- Validation: tests compile (may fail assertions -- that is the point)
- Exit condition: ported tests compile and are tagged `#[ignore]` or behind a feature gate so the workspace is still green

### Checkpoint 1: Profile skeleton
- Audit IDs: G1, G2, G3, G10
- Expected changes:
  - Create `openai_base/profile.rs` with `OpenAICompatibleProfile` struct and all policy enums: `ToolCallIdStrategy`, `MessageLayoutPolicy`, `ResponseContentPolicy`, `JsonModePolicy`, `ReasoningPolicy`, `AudioTranscriptionProfile`.
  - Add `OpenAICompatibleProfile::mistral()` and `OpenAICompatibleProfile::generic()` const constructors.
  - Add `profile` and `tool_id_mapper` fields to `OpenAIBaseProvider`. Add `new_with_profile()` and `new_with_client_and_profile()` constructors. Existing constructors call `new_with_client_and_profile(..., OpenAICompatibleProfile::generic(), Noop mapper)`.
  - All provider methods read `self.profile` for decisions but generic behavior is unchanged.
  - No behavioral change for generic openai_base instances.
- Validation: `cargo check -p oxide-agent-core --no-default-features --features profile-full`; existing openai_base tests pass unchanged
- Exit condition: compiles, profile struct exists, generic path unchanged

### Checkpoint 2: Move tool-call ID mapper
- Audit IDs: G4, G5, G10
- Expected changes:
  - Move `ToolCallIdMapper` and `normalize_for_mistral()` from `mistral/id_mapper.rs` to `openai_base/tool_ids.rs`.
  - Add `ToolCallIdStrategy` enum (`Preserve`, `MistralNineAlnum`).
  - Add collision handling: if generated 9-char ID collides with a different original ID, generate stable 9-char base36 fallback.
  - `OpenAIBaseProvider.tool_id_mapper` uses strategy from profile. Generic profile uses `Preserve` (noop mapper).
  - Port all 4 existing id_mapper tests to new location. Add collision test.
  - Keep `mistral/id_mapper.rs` temporarily as `pub(crate) use` re-export from new location (deleted in checkpoint 9).
- Validation: `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- id_mapper`
- Exit condition: all mapper tests pass from new location, collision test passes

### Checkpoint 3: Message layout policy
- Audit IDs: G6, G10
- Expected changes:
  - Implement `MessageLayoutPolicy::MistralStrict` in `openai_base` message preparation:
    - Collect history `"system"` messages, prepend before main system prompt.
    - Map assistant tool call IDs via `tool_id_mapper.mistral_id_for()`.
    - Map tool result `tool_call_id` via mapper.
    - Include optional `name` on tool messages.
  - `MessageLayoutPolicy::GenericOpenAI` wraps current `prepare_structured_messages()` unchanged.
  - Switch `OpenAIBaseProvider` to dispatch on `self.profile.message_layout`.
  - Port Mistral message layout tests (assistant tool calls, tool result messages, system message ordering).
- Validation: `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- message`
- Exit condition: ported message tests pass, generic path unchanged

### Checkpoint 4: Response parser policy
- Audit IDs: G7, G10
- Expected changes:
  - Implement `ResponseContentPolicy::StringOrChunkArrayWithReasoning` in `openai_base` response parsing:
    - `extract_message_content()` -- string or array handling.
    - Recursive `extract_text_segments()`.
    - `thinking`/`reasoning` chunk extraction.
    - `reasoning_content` top-level fallback.
    - `parse_usage()` with `cached_tokens`.
    - Tool call ID reverse mapping via `tool_id_mapper.to_original()`.
  - `ResponseContentPolicy::StringOnly` wraps current `parse_chat_response()` unchanged.
  - Port Mistral parsing tests (content array, reasoning chunks, usage, tool call reverse mapping).
- Validation: `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- parse`
- Exit condition: ported parsing tests pass, generic path unchanged

### Checkpoint 5: Request tweaks (temperatures, parallel_tool_calls, reasoning, JSON mode)
- Audit IDs: G8, Q4, Q5, Q6
- Expected changes:
  - Profile-driven temperature selection: `chat_temperature`, `tool_temperature`, `reasoning_temperature` based on model match.
  - `parallel_tool_calls: Option<bool>` -- Mistral profile adds `Some(true)` to tool body; generic adds `None`.
  - `reasoning_effort` from `ReasoningPolicy::Mistral` -- only when model matches policy.
  - JSON mode: `response_format: {"type":"json_object"}` when `json_mode && !has_tools`.
  - Port Mistral request body tests (plain chat, reasoning, tool, JSON mode, parallel_tool_calls).
- Validation: `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- body`
- Exit condition: ported body tests pass, generic path unchanged

### Checkpoint 6: Audio transcription
- Audit IDs: G9, G10
- Expected changes:
  - Move transcription logic from `mistral/transcription.rs` to `openai_base` as profile-driven capability.
  - `AudioTranscriptionProfile` struct: endpoint_path, temperature, timeout_secs, max_retries, initial_backoff_ms.
  - Endpoint URL built from base URL, not chat completions URL (strip `/chat/completions` if present, append endpoint_path).
  - Multipart: `file` (with MIME-derived extension), `model`, `temperature`. Add `stream=false` explicitly.
  - Retry: 429 with Retry-After, 502/503/504, timeout, JSON transient. Exponential backoff.
  - `transcribe_audio()` returns unsupported when `profile.audio_transcription` is `None`.
  - Port transcription tests (mime mapping, retry delay, rate limit, no server time).
- Validation: `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- transcri`
- Exit condition: ported transcription tests pass, generic `transcribe_audio` still returns unsupported

### Checkpoint 7: MistralProviderModule builds OpenAIBaseProvider
- Audit IDs: G11
- Expected changes:
  - Rewrite `mistral/module.rs::build_provider()` to create `OpenAIBaseProvider::new_with_client_and_profile(Some(api_key), "https://api.mistral.ai/v1", http_client, OpenAICompatibleProfile::mistral())`.
  - Capabilities and media capabilities stay the same values (returned from profile, not hardcoded).
  - Rewrite `mistral/mod.rs` to re-export `OpenAIBaseProvider` as `MistralProvider` (type alias) or thin newtype.
  - All `complete_internal_text`, `chat_with_tools`, `transcribe_audio`, `analyze_image` now go through `OpenAIBaseProvider` impl.
- Validation: `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib`; capabilities CLI output matches old
- Exit condition: all tests pass, Mistral behavior fully via OpenAIBaseProvider

### Checkpoint 8: Remove async-openai
- Audit IDs: G12, Q7
- Expected changes:
  - Delete `support/openai_compat.rs`.
  - Delete `support/common.rs`.
  - Update `support/mod.rs`: remove `common` and `openai_compat` module declarations and their `#[cfg(feature = "llm-mistral")]` gates.
  - Delete `mistral/client.rs`.
  - Remove `async-openai` from `Cargo.toml` dependencies.
  - Change `llm-mistral = ["dep:async-openai", "dep:reqwest"]` to `llm-mistral = ["llm-openai-base"]`.
- Validation: `cargo tree -i async-openai -p oxide-agent-core --no-default-features --features profile-full` returns "not found"; `cargo check --workspace --no-default-features --features profile-full`
- Exit condition: async-openai gone from tree, workspace compiles

### Checkpoint 9: Delete old Mistral implementation files
- Audit IDs: G13
- Expected changes:
  - Delete `mistral/chat.rs`, `mistral/id_mapper.rs`, `mistral/messages.rs`, `mistral/parsing.rs`, `mistral/transcription.rs`, `mistral/image.rs`, `mistral/types.rs`, `mistral/tests.rs`.
  - Slim down `mistral/mod.rs` to module declaration + re-export only.
  - Ensure `mistral/mod.rs` still declares `pub mod module;` and re-exports `MistralProviderModule`.
  - Any remaining `use` of deleted files cleaned up.
- Validation: `ls crates/oxide-agent-core/src/llm/providers/mistral/` shows `mod.rs` and `module.rs` only; `cargo check` clean
- Exit condition: only thin wrapper files remain, workspace compiles, all tests pass

### Checkpoint 10: Optional PROFILE env var + docs + .env.example
- Audit IDs: G14, V5
- Expected changes:
  - Add `OPENAI_BASE_PROVIDERS__N__PROFILE` parsing in `openai_base/module.rs`. Values: `generic` (default), `mistral`. Map to `OpenAICompatibleProfile::generic()` / `::mistral()`.
  - Update `.env.example` with `PROFILE` example and comment.
  - Update README and capabilities docs if they reference Mistral implementation internals or `async-openai`.
  - Run capabilities CLI to verify output matches expectations.
- Validation: `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib`; capabilities CLI `--compiled --json` and `--enabled --json`
- Exit condition: profile env var works, docs updated, all tests green

## Validation Contract

- Static checks:
  - `cargo check --workspace --no-default-features --features profile-full`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo fmt --all -- --check`
- Dependency verification:
  - `cargo tree -i async-openai -p oxide-agent-core --no-default-features --features profile-full` -- no results
- Tests:
  - `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib` -- all green
- Capability verification:
  - `cargo run -p oxide-agent-telegram-bot --no-default-features --features profile-full -- capabilities --compiled --json` -- mistral module present
  - `cargo run -p oxide-agent-telegram-bot --no-default-features --features profile-full -- capabilities --enabled --json` -- mistral aliases present
- Done when: G1-G14, Q1-Q7, V1-V6, N1-N4 all verified.

## Decisions

- 2026-06-14: `OpenAICompatibleProfile` is a `const`-constructible struct (no heap allocation) with enum policies, not a trait object. Follows existing pattern of `ToolProtocolProfile` and `OpenCodeProviderProfile`.
- 2026-06-14: Checkpoints 0-6 build the openai_base side without breaking Mistral. Checkpoint 7 switches Mistral to the new path. Checkpoints 8-9 remove dead code. This ordering ensures the old code exists as a reference until the new path is verified.
- 2026-06-14: Ported tests initially live behind `#[ignore]` or conditional compilation in checkpoint 0, un-ignored one by one as their corresponding checkpoint implements the feature.
- 2026-06-14: ID mapper collision handling added (G5) as a safety improvement during migration. Algorithm: if 9-char truncation collides, use base36 of a hash of the original ID, truncated to 9 chars. Stable and deterministic.
- 2026-06-14: Audio transcription endpoint URL built by stripping trailing `/chat/completions` or `/chat/completions/` from `api_base`, then appending the profile's `endpoint_path` (e.g. `/audio/transcriptions`). This handles both `https://api.mistral.ai/v1` and `https://api.mistral.ai/v1/chat/completions` as `api_base`.
- 2026-06-14: `reasoning_effort` uses exact case-insensitive match against `mistral-small-2603` for first pass. Extensible via `ModelMatchPolicy` but not expanded in this goal.

## Progress Log

- 2026-06-14 12:00: Checkpoint 1 -- Profile skeleton
  - Changed:
    - Created `openai_base/profile.rs`: `OpenAICompatibleProfile` struct (16 fields, const-constructible) + policy enums (`ToolCallIdStrategy`, `MessageLayoutPolicy`, `ResponseContentPolicy`, `JsonModePolicy`, `ModelMatchPolicy`, `ReasoningPolicy`, `AudioTranscriptionProfile`). Const constructors `mistral()` and `generic()`. `is_reasoning_model()` helper. 4 profile tests.
    - Created `openai_base/tool_ids.rs`: `ToolCallIdMapper` (full impl, moved from `mistral/id_mapper.rs`). 4 mapper tests. `#![allow(dead_code)]` until wired in checkpoint 2.
    - Updated `openai_base/mod.rs`: added `profile` + `tool_id_mapper` fields to `OpenAIBaseProvider`. New `new_with_client_and_profile()` constructor. Existing `new()`/`new_with_client()` delegate with `generic()` profile. `#[allow(dead_code)]` on struct until fields are used in checkpoints 2-6.
  - Evidence:
    - `cargo check -p oxide-agent-core --no-default-features --features profile-full` clean
    - `cargo clippy -p oxide-agent-core --no-default-features --features profile-full --all-targets -- -D warnings` clean
    - `cargo fmt --all -- --check` clean
    - `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- openai_base` -- 21 passed (4 profile + 4 mapper + 13 existing unchanged)
    - `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- mistral` -- 30 passed (existing tests unaffected)
  - Commands: all above
  - Audit IDs updated: G1 verified, G2 verified, G3 verified, G10 verified
  - Next: Checkpoint 2 -- move tool-call ID mapper wiring + collision handling

- 2026-06-14 12:45: Checkpoint 2 -- Move tool-call ID mapper + collision handling
  - Changed:
    - Updated `openai_base/tool_ids.rs`: added collision handling to `register()` -- if 9-char truncation collides with a different original, generates stable 9-char base36 fallback via `stable_fallback()` + `find_collision_free_id()`. Added `to_base36()` helper. Added `DefaultHasher`/`Hash`/`Hasher` imports. 2 new tests: `test_collision_handling`, `test_stable_fallback_is_deterministic`.
    - Replaced `mistral/id_mapper.rs` with thin re-export: `pub(crate) use crate::llm::providers::openai_base::tool_ids::ToolCallIdMapper`.
    - Updated `Cargo.toml`: `llm-mistral` now enables `llm-openai-base` (`["dep:async-openai", "dep:reqwest", "llm-openai-base"]`) so the re-export path is visible.
    - Updated `mistral/mod.rs`: lowered `id_mapper()` method visibility from `pub` to `pub(crate)` to match `ToolCallIdMapper`'s `pub(crate)` visibility.
  - Evidence:
    - `cargo check -p oxide-agent-core --no-default-features --features profile-full` clean
    - `cargo clippy -p oxide-agent-core --no-default-features --features profile-full --all-targets -- -D warnings` clean
    - `cargo fmt --all -- --check` clean
    - `cargo test -p oxide-agent-core --no-default-features --features profile-full -- tool_ids` -- 9 passed (6 tool_ids + 3 related from other providers)
    - `cargo test -p oxide-agent-core --no-default-features --features profile-full -- mistral` -- 26 passed (all mistral tests via re-export)
    - `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` clean (other profiles unaffected)
  - Commands: all above
  - Audit IDs updated: G4 verified, G5 verified
  - Next: Checkpoint 3 -- message layout policy (MistralStrict dispatch in OpenAIBaseProvider)

## Risks and Blockers

- Risk: Content-array response parsing edge cases (nested arrays, mixed types) may differ between Mistral docs and actual API behavior. Mitigation: port all existing Mistral parsing tests verbatim; they encode known-good behavior.
- Risk: Tool-call ID collision in production with many tool calls. Mitigation: G5 adds collision handling with deterministic fallback.
- Risk: `OpenAIBaseProvider` struct changes may affect serialization or trait bounds. Mitigation: `OpenAIBaseProvider` has no `Serialize`/`Deserialize` and is used as `Arc<dyn LlmProvider>`; adding fields is safe.
- Risk: Feature flag change (`llm-mistral` depends on `llm-openai-base`) may affect profile compositions that include `llm-mistral` but not `llm-openai-base`. Mitigation: only `profile-full` includes `llm-mistral`, and it also includes `llm-openai-base`. No conflict.

## Final Verification

(filled only when complete)
