# Goal: Migrate ZAI to OpenAI Base Profile

Date started: 2026-06-14
Status: active
Codex goal: `/goal Implement docs/goals/2026-06-14-zai-openai-base-migration.md until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update the doc after each meaningful verification, commit after each completed checkpoint, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.`
Source spec: `docs/prd/zai-drop.md`
Goal doc owner: Codex
Last updated: 2026-06-14 20:47

## Objective

Move ZAI/GLM support from the dedicated `zai` provider and `zai-rs` SDK into the shared `openai_base` provider as `OPENAI_BASE_PROVIDERS__N__PROFILE=zai`, preserving current ZAI behavior for tools, streaming, reasoning content, JSON mode, structured-output gating, rate-limit parsing, and GLM capability mapping.

Done when every Completion Audit item is verified by its listed evidence, the dedicated ZAI provider and `llm-zai` feature are absent, old `provider = "zai"` routes fail validation explicitly, `provider = "openai-base:zai"` routes pass, required tests and checks pass, and any live `glm-*` validation result or blocker is documented with exact evidence.

## Scope

In scope:
- `crates/oxide-agent-core/src/llm/providers/openai_base/` -- add ZAI profile, body policy, SSE streaming path, streaming aggregator, rate-limit parser, capability/profile tests.
- `crates/oxide-agent-core/src/llm/providers/zai.rs` and `crates/oxide-agent-core/src/llm/providers/zai/` -- source behavior to port, then delete.
- `crates/oxide-agent-core/src/llm/providers/mod.rs`, `modules.rs`, `llm/mod.rs`, `llm/support/mod.rs`, `llm/client.rs`, `llm/capabilities.rs` -- remove dedicated ZAI wiring and dead ZAI-only fallback.
- `crates/oxide-agent-core/Cargo.toml`, `Cargo.lock` -- remove `zai-rs` and `llm-zai`; keep `reqwest` streaming support via `llm-openai-base`.
- `crates/oxide-agent-core/src/capabilities/compiled.rs` -- remove dedicated `llm-provider/zai` manifest and expose any needed OpenAI Base profile config.
- Core tests and snapshots touching provider validation, rate limits, capabilities, modular registry, static env guards, and config parsing.
- Web transport E2E mocks and live ZAI audit setup that currently name/use `SequencedZaiProvider` or `provider = "zai"`.
- Runtime config, CI/env examples, README/docs references that instruct using `ZAI_API_KEY` or `AGENT_MODEL_PROVIDER=zai`.

Out of scope:
- Adding a new provider, SDK wrapper, service, queue, transport, or dependency.
- Supporting legacy `provider = "zai"` routes after migration.
- Adding `ZAI_API_KEY` fallback for OpenAI Base configuration.
- Reintroducing direct Google Gemini provider code.
- Changing the `LlmProvider` trait signature or tool-call correlation domain types.
- Sending ZAI-specific `thinking` fields for non-`zai` OpenAI Base profiles.
- Expanding `with_tool_stream(true)` semantics blindly to every GLM model.

## Missing Inputs

- Live ZAI API access may be unavailable in this environment.
  - Impact: the final live `glm-*` test from the PRD may not be executable locally.
  - Low-risk assumption or fallback: implement and validate with hermetic unit/integration tests; document the exact missing secret/route and smallest external action if live validation cannot run.
  - User/external action needed: provide an OpenAI Base ZAI API key via `OPENAI_BASE_PROVIDERS__N__API_KEY` only if live validation is required before final completion.

## Repository Context

- Existing goal convention: `docs/goals/archives/*.md` uses a durable goal document with Completion Audit, checkpoint plan, validation contract, decisions, progress log, and final verification.
- Current OpenAI Base profile layer already exists from the Mistral migration: `crates/oxide-agent-core/src/llm/providers/openai_base/profile.rs` defines `OpenAICompatibleProfile`, profile policy enums, and `generic()`/`mistral()` constructors.
- Current ZAI provider to remove:
  - `crates/oxide-agent-core/src/llm/providers/zai.rs`
  - `crates/oxide-agent-core/src/llm/providers/zai/module.rs`
  - `crates/oxide-agent-core/src/llm/providers/zai/sdk.rs`
  - `crates/oxide-agent-core/src/llm/providers/zai/sdk/stream.rs`
  - `crates/oxide-agent-core/src/llm/providers/zai/sdk/messages.rs`
- Current feature/dependency wiring includes `zai-rs` and `llm-zai` in `crates/oxide-agent-core/Cargo.toml:52`, `:83`, and `:242`.
- Validation infrastructure from repo instructions:
  - `cargo check --workspace --no-default-features --features profile-full`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo fmt --all -- --check`
  - Focused `cargo test -p oxide-agent-core --no-default-features --features profile-full ...`
- Risky areas:
  - Streaming tool calls: IDs and fragmented `function.arguments` must survive SSE aggregation.
  - `reasoning_content`: must not be mixed into normal content or dropped.
  - `json_mode && tools`: native JSON mode must stay disabled when tools are present.
  - `thinking`: ZAI-only body field; generic OpenAI Base providers must not receive it.
  - 429 handling: ZAI `next_flush_time` must still map to a useful rate-limit wait.
  - Old config cleanup: stale `zai` provider aliases should fail validation instead of silently routing.

## Completion Audit

### Functional requirements

- G1: `openai_base` has a `zai` profile
  - Source: `docs/prd/zai-drop.md:3`-`17`, `:128`
  - Acceptance: `OpenAICompatibleProfile::zai()` exists with base URL `https://api.z.ai/api/coding/paas/v4`, temperatures `0.95`, tools enabled, `ToolCallIdStrategy::Preserve`, JSON mode policy compatible with `json_mode && !tools`, ZAI-only thinking policy, reasoning-content response policy, and model-specific structured-output capability policy.
  - Evidence required: unit test asserting all profile fields; `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base --lib openai_base::profile` passes.
  - Status: verified
  - Evidence collected: `OpenAICompatibleProfile::zai()` added in `crates/oxide-agent-core/src/llm/providers/openai_base/profile.rs` with default API base `https://api.z.ai/api/coding/paas/v4`, `0.95` temperatures, preserved tool IDs, reasoning-content response policy, ZAI thinking/streaming policies, and model-gated structured output. Verified by `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base openai_base --lib` on 2026-06-14 20:16 (`zai_profile_has_expected_values`, `zai_structured_output_is_model_gated`).

- G2: `resolve_profile("zai")` config works
  - Source: `docs/prd/zai-drop.md:5`, `:87`-`:94`, `:111`
  - Acceptance: `OPENAI_BASE_PROVIDERS__N__PROFILE=zai` resolves to the ZAI profile; `provider = "openai-base:zai"` validates and builds a provider instance.
  - Evidence required: module/config unit tests for profile resolution and `openai-base:zai` model route validation pass.
  - Status: verified
  - Evidence collected: `resolve_profile("zai")` added and verified by `llm::providers::openai_base::module::tests::resolve_profile_zai_string`; provider instance build verified by `openai_base_builds_zai_provider_instance`; route/config validation verified by `config::tests::route_validation_accepts_openai_base_zai_instance`. Commands passed on 2026-06-14 20:35: `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base openai_base --lib`, `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base route_validation --lib`, and `cargo test -p oxide-agent-core --no-default-features --features profile-full test_model_routes_parse_from_env_and_override_primary_models --lib`.

- G3: ZAI body policy preserves current request behavior
  - Source: `docs/prd/zai-drop.md:7`-`:16`, `:31`, `:101`-`:104`, `:119`-`:121`
  - Acceptance: ZAI tool/plain chat bodies use temperature `0.95`; tool requests set `stream: true`; plain non-native-JSON ZAI chat sends `thinking: {"type":"enabled"}`; native JSON-only (`json_mode && !tools`) sends `stream: false`, `response_format: {"type":"json_object"}`, and `thinking: {"type":"disabled"}`; JSON mode is not sent when tools are present; non-ZAI profiles do not receive `thinking`.
  - Evidence required: focused body-builder tests covering tools, plain chat, native JSON-only, JSON-with-tools, and generic non-ZAI profile behavior.
  - Status: verified
  - Evidence collected: Body policy added in `crates/oxide-agent-core/src/llm/providers/openai_base/mod.rs`: ZAI sends `thinking: {"type":"enabled"}` and `stream: true` normally, disables both streaming and thinking for native JSON-only, and omits `response_format` when tools are present. Verified by `zai_tool_body_sets_stream_and_enabled_thinking`, `zai_plain_body_without_json_streams_with_enabled_thinking`, `zai_native_json_body_disables_thinking_and_streaming`, `zai_json_with_tools_does_not_use_native_json_mode`, and `generic_tool_body_does_not_send_zai_thinking` in `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base openai_base --lib` on 2026-06-14 20:16.

- G4: OpenAI Base has a reqwest SSE streaming path for ZAI
  - Source: `docs/prd/zai-drop.md:18`-`:31`, `:129`
  - Acceptance: for profile `zai`, except native JSON-only, `openai_base` sends a normal Chat Completions request with `stream: true`, reads SSE `data: ...` chunks using `reqwest`, ignores `[DONE]`, and returns through the normal `LlmProvider` response shape.
  - Evidence required: hermetic SSE parser/transport tests proving streamed content is parsed and non-stream JSON-only still uses the existing response path.
  - Status: verified
  - Evidence collected: `chat_with_tools` in `crates/oxide-agent-core/src/llm/providers/openai_base/mod.rs` now dispatches to a reqwest `bytes_stream()` SSE path whenever the profile/body policy sets `stream: true`; SSE `data:` events are parsed, `[DONE]` is ignored, and native JSON-only ZAI requests keep the existing non-streaming JSON path. Verified by `zai_chat_with_tools_uses_sse_transport` and `zai_native_json_chat_uses_non_streaming_transport` in `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base openai_base --lib` on 2026-06-14 20:23.

- G5: ZAI streaming aggregator preserves content, reasoning, tools, IDs, finish reason, and usage
  - Source: `docs/prd/zai-drop.md:33`-`:45`, `:105`-`:108`, `:117`-`:118`
  - Acceptance: SSE aggregation accumulates `choices[0].delta.content`, separately accumulates `choices[0].delta.reasoning_content`, assembles fragmented `tool_calls`, concatenates fragmented `function.arguments`, preserves provider tool-call IDs, reads `finish_reason`, reads streamed `usage` when present, and errors cleanly on an empty response.
  - Evidence required: unit tests for content chunks, reasoning chunks, fragmented tool arguments, tool-call ID preservation, finish reason/usage, and empty response.
  - Status: verified
  - Evidence collected: OpenAI Base streaming aggregation now accumulates `delta.content`, separately accumulates `delta.reasoning_content`, assembles indexed/fragmented `delta.tool_calls`, concatenates `function.arguments`, preserves provider wire IDs through `CHAT_LIKE_TOOL_PROFILE.inbound_provider_tool_call`, records `finish_reason`, maps streamed `usage`, and returns `EmptyResponse` for empty streams. Verified by `zai_sse_aggregates_content_reasoning_finish_and_usage`, `zai_sse_aggregates_fragmented_tool_arguments_and_preserves_id`, `streaming_tool_calls_handle_empty_id_as_uncorrelated`, and `zai_sse_empty_response_errors_cleanly` in `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base openai_base --lib` on 2026-06-14 20:23.

- G6: ZAI structured-output support is model-gated
  - Source: `docs/prd/zai-drop.md:16`, `:56`, `:104`, `crates/oxide-agent-core/src/llm/providers/zai/module.rs` current behavior
  - Acceptance: only the same GLM models currently allowed by the dedicated ZAI provider report native structured-output support under `openai-base:zai`; unsupported models disable structured output.
  - Evidence required: ported capability tests for supported and unsupported GLM model IDs pass.
  - Status: verified
  - Evidence collected: `OpenAICompatibleProfile::capabilities_for_model` and `OpenAIBaseProviderModule::capabilities_for_model` apply the legacy ZAI GLM allow-list for `openai-base:zai`. Verified by `zai_structured_output_is_model_gated` and `openai_base_zai_capabilities_are_model_gated` in `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base openai_base --lib` on 2026-06-14 20:16.

- G7: ZAI rate-limit parser is preserved in OpenAI Base
  - Source: `docs/prd/zai-drop.md:55`, `:109`, `:122`
  - Acceptance: ZAI 429 bodies containing `next_flush_time` produce a meaningful retry wait/rate-limit error; generic Retry-After handling still works.
  - Evidence required: ported `parse_zai_flush_time` tests and a 429 mapping test pass without `llm-zai`.
  - Status: verified
  - Evidence collected: `parse_zai_flush_time` moved into `crates/oxide-agent-core/src/llm/providers/openai_base/mod.rs`; ZAI streaming and native JSON-only 429 paths now parse `next_flush_time`, while generic streaming 429s still use `Retry-After`. Verified by `parse_zai_flush_time_unix_timestamp`, `parse_zai_flush_time_milliseconds`, `parse_zai_flush_time_iso_datetime`, `parse_zai_flush_time_no_timestamp`, `zai_streaming_429_uses_next_flush_time`, `zai_native_json_429_uses_next_flush_time`, and `generic_streaming_429_uses_retry_after_header` in `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base openai_base --lib` on 2026-06-14 20:35; integration coverage also passed in `cargo test -p oxide-agent-core --no-default-features --features llm-openrouter,llm-openai-base --test rate_limit`.

- G8: Dedicated ZAI provider and SDK are removed
  - Source: `docs/prd/zai-drop.md:49`-`:73`, `:132`
  - Acceptance: files `zai.rs` and `zai/` are deleted; feature `llm-zai`, dependency `zai-rs`, provider module registration, compiled capability manifest module, `llm-provider/zai` mentions, `zai_rs` tracing filter, and ZAI-only media fallback sentinel are gone.
  - Evidence required: `rg "llm-zai|zai-rs|llm-provider/zai|ZAI_FALLBACK_TO_MEDIA|zai_rs"` returns no active code/config references except historical PRD/goal docs if explicitly reviewed; `cargo tree -i zai-rs -p oxide-agent-core --no-default-features --features profile-full` reports no package match.
  - Status: pending
  - Evidence collected:

- G9: Runtime config and docs use `openai-base:zai`
  - Source: `docs/prd/zai-drop.md:75`-`:97`, `:131`, `:133`
  - Acceptance: examples/docs instruct `OPENAI_BASE_PROVIDERS__N__NAME=zai`, `OPENAI_BASE_PROVIDERS__N__API_BASE=https://api.z.ai/api/coding/paas/v4`, `OPENAI_BASE_PROVIDERS__N__API_KEY=...`, `OPENAI_BASE_PROVIDERS__N__PROFILE=zai`, and `AGENT_MODEL_PROVIDER=openai-base:zai`; no `ZAI_API_KEY` fallback remains.
  - Evidence required: focused diff review plus `rg "ZAI_API_KEY|AGENT_MODEL_PROVIDER=zai|SUB_AGENT_MODEL_PROVIDER=zai"` only finds removed/historical references or documented non-goal notes.
  - Status: verified
  - Evidence collected: README, `.env.example`, GitHub workflow env generation, and web live E2E setup now configure ZAI through `OPENAI_BASE_PROVIDERS__1__NAME=zai`, `OPENAI_BASE_PROVIDERS__1__API_BASE=https://api.z.ai/api/coding/paas/v4`, `OPENAI_BASE_PROVIDERS__1__API_KEY=...`, `OPENAI_BASE_PROVIDERS__1__PROFILE=zai`, and `AGENT_MODEL_PROVIDER=openai-base:zai` / `SUB_AGENT_MODEL_PROVIDER=openai-base:zai`. Verified on 2026-06-14 20:47 by `rg 'ZAI_API_KEY|ZAI_API_BASE|AGENT_MODEL_PROVIDER="?zai|SUB_AGENT_MODEL_PROVIDER="?zai' README.md .env.example .github/workflows/ci-cd.yml crates/oxide-agent-transport-web/tests/e2e || true` returning no matches.

- G10: Old `provider = "zai"` route fails validation and `openai-base:zai` passes
  - Source: `docs/prd/zai-drop.md:73`, `:110`-`:111`
  - Acceptance: config/model-route validation rejects old dedicated `zai` provider and accepts `openai-base:zai` with configured OpenAI Base endpoint.
  - Evidence required: config tests for both cases pass.
  - Status: in_progress
  - Evidence collected: New `openai-base:zai` route validation is verified by `config::tests::route_validation_accepts_openai_base_zai_instance` and env route parsing by `test_model_routes_parse_from_env_and_override_primary_models`. Old `provider = "zai"` rejection is verified in the target no-dedicated-provider feature set by `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base route_provider_validation_rejects_removed_direct_zai_provider_when_uncompiled --lib` on 2026-06-14 20:35. Final status remains in_progress until Checkpoint 5 removes `llm-zai`, so the same validation is true under `profile-full`.

- G11: Test mocks are renamed away from ZAI-specific provider names
  - Source: `docs/prd/zai-drop.md:124`
  - Acceptance: web E2E helper `SequencedZaiProvider` and related `wait_for_zai_calls` names are renamed to generic LLM/OpenAI Base names unless they are live ZAI audit tests that specifically validate ZAI behavior.
  - Evidence required: `rg "SequencedZaiProvider|wait_for_zai_calls" crates/oxide-agent-transport-web/tests` returns no matches or only intentionally retained live-test names with documented reason; web E2E tests compile for the relevant profile.
  - Status: verified
  - Evidence collected: Generic web E2E mock `SequencedZaiProvider` was renamed to `SequencedLlmProvider`, `wait_for_zai_calls` to `wait_for_llm_calls`, and related local variables/comments away from ZAI-specific names. Verified on 2026-06-14 20:47 by `rg "SequencedZaiProvider|wait_for_zai_calls|zai_provider" crates/oxide-agent-transport-web/tests` returning no matches and by `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local --tests --no-run` compiling the E2E test binary.

### Quality and constraint requirements

- Q1: ZAI specificity stays inside OpenAI Base profile/utilities, not a provider
  - Source: `docs/prd/zai-drop.md:49`-`:58`, `:136`
  - Acceptance: no new provider or SDK wrapper is introduced; ZAI behavior lives in `openai_base` profile/body/stream/rate-limit utilities.
  - Evidence required: file review and `rg "struct .*ZaiProvider|provider/zai|mod zai" crates/oxide-agent-core/src/llm/providers` after cleanup.
  - Status: pending
  - Evidence collected:

- Q2: Generic OpenAI Base behavior is not polluted by ZAI quirks
  - Source: `docs/prd/zai-drop.md:119`-`:121`
  - Acceptance: generic/mistral profiles do not send `thinking`, do not force ZAI streaming policy, and keep their existing JSON/tool behavior.
  - Evidence required: existing openai_base generic and mistral tests pass plus focused negative tests for `thinking` and streaming flags.
  - Status: in_progress
  - Evidence collected: Checkpoint 1 body tests verify non-ZAI generic tool bodies do not receive `thinking` and retain `stream: false`; focused openai_base test suite passed on 2026-06-14 20:16. Full generic/mistral regression and final lint remain pending.

- Q3: No legacy ZAI API key compatibility branch
  - Source: `docs/prd/zai-drop.md:96`
  - Acceptance: `ZAI_API_KEY` is not read by runtime config/provider module code after migration.
  - Evidence required: `rg "ZAI_API_KEY|ZAI_API_BASE" crates config README.md .env.example` reviewed; active runtime references absent.
  - Status: in_progress
  - Evidence collected: Docs/config/web E2E setup no longer read or instruct `ZAI_API_KEY`/`ZAI_API_BASE`; live ZAI E2E now requires `OPENAI_BASE_PROVIDERS__1__*`. Verified on 2026-06-14 20:47 by the focused grep over README, `.env.example`, CI workflow, and web E2E paths returning no matches. Final runtime status remains in_progress until Checkpoint 5 deletes the dedicated provider code that still owns `ZAI_API_KEY` before removal.

- Q4: No new dependencies or architecture layers
  - Source: repo `AGENTS.md` implementation bias and `docs/prd/zai-drop.md:136`
  - Acceptance: solution uses existing `reqwest`/`serde_json`/`futures-util` stack and existing profile abstraction; no new crates/services/queues/caches.
  - Evidence required: `git diff Cargo.toml Cargo.lock` shows only removal or existing-feature wiring changes; no added dependencies.
  - Status: pending
  - Evidence collected:

- Q5: Tool-call correlation integrity is preserved
  - Source: `docs/prd/zai-drop.md:39`-`:42`, `:117`
  - Acceptance: streamed tool calls become `ToolCall` records with correct provider IDs and concatenated arguments; history repair/tool result correlation semantics are unchanged.
  - Evidence required: streaming aggregator tests plus existing tool-call tests pass.
  - Status: verified
  - Evidence collected: Streaming tool-call aggregation uses provider-correlated tool calls for non-empty ZAI IDs and uncorrelated generated IDs for empty IDs, matching existing chat-like correlation semantics. Verified by `zai_sse_aggregates_fragmented_tool_arguments_and_preserves_id`, `streaming_tool_calls_handle_empty_id_as_uncorrelated`, and existing openai_base tool-call tests in `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base openai_base --lib` on 2026-06-14 20:23.

- Q6: Audio/media fallback dead code is removed or made generic
  - Source: `docs/prd/zai-drop.md:123`
  - Acceptance: `llm/client.rs` has no stale ZAI-only sentinel path; any remaining media fallback is capability-driven and not tied to removed provider names.
  - Evidence required: focused diff review and `rg "ZAI_FALLBACK_TO_MEDIA|fallback_to_media|ZAI" crates/oxide-agent-core/src/llm/client.rs` reviewed.
  - Status: pending
  - Evidence collected:

- Q7: Snapshot/manifest state matches removed provider
  - Source: `docs/prd/zai-drop.md:66`-`:70`
  - Acceptance: modular registry snapshots and compiled capability output no longer list `llm-provider/zai` or `llm-zai`; OpenAI Base profile config is represented as needed.
  - Evidence required: snapshot tests pass and focused snapshot diff review.
  - Status: pending
  - Evidence collected:

- Q8: Implementation remains minimal and maintainable
  - Source: repo `AGENTS.md` scale/implementation bias
  - Acceptance: no broad refactor unrelated to ZAI migration; code changes are localized to provider/profile/config/docs/tests required by audit items.
  - Evidence required: final `git diff --stat` and changed-file review.
  - Status: pending
  - Evidence collected:

### Validation requirements

- V1: Required hermetic tests from PRD exist and pass
  - Source: `docs/prd/zai-drop.md:98`-`:112`
  - Acceptance: tests cover body stream flags, JSON-only behavior, normal thinking, SSE content, SSE reasoning, fragmented tool arguments, tool-call ID preservation, ZAI 429 `next_flush_time`, old provider validation failure, and new provider validation success.
  - Evidence required: list of test names and passing `cargo test` command output.
  - Status: verified
  - Evidence collected: Body stream flags, JSON-only behavior, normal thinking, SSE content, SSE reasoning, fragmented tool arguments, tool-call ID preservation, streamed usage/finish reason, and empty streamed response are covered by focused openai_base tests passing on 2026-06-14 20:23. ZAI 429 `next_flush_time`, old provider validation failure in the no-`llm-zai` target feature set, and new `openai-base:zai` validation success were added and passed on 2026-06-14 20:35 via `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base openai_base --lib`, `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base route_provider_validation_rejects_removed_direct_zai_provider_when_uncompiled --lib`, and `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base route_validation --lib`.

- V2: Focused core validation passes during checkpoints
  - Source: repo validation guidance
  - Acceptance: after relevant Rust code checkpoints, focused `cargo test -p oxide-agent-core --no-default-features --features profile-full ...` or narrower feature tests pass.
  - Evidence required: command output per checkpoint recorded in Progress Log.
  - Status: in_progress
  - Evidence collected: Checkpoint 2 passed `cargo fmt --all`, `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base openai_base --lib` (77 passed, 0 failed), and `cargo clippy -p oxide-agent-core --no-default-features --features profile-full --lib -- -D warnings` on 2026-06-14 20:23. Checkpoint 3 passed `cargo fmt --all`, focused OpenAI Base/config/rate-limit tests, `cargo test -p oxide-agent-core --no-default-features --features profile-full test_model_routes_parse_from_env_and_override_primary_models --lib`, `cargo clippy -p oxide-agent-core --no-default-features --features profile-full --lib -- -D warnings`, and `git diff --check` on 2026-06-14 20:35. Checkpoint 4 passed `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local --tests --no-run`, `cargo fmt --all -- --check`, and `git diff --check` on 2026-06-14 20:47. Final broader validation remains pending.

- V3: Final formatting and lint gates pass
  - Source: `AGENTS.md:146`
  - Acceptance: `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets -- -D warnings` pass.
  - Evidence required: command output recorded in Final Verification.
  - Status: pending
  - Evidence collected:

- V4: Final profile/full build check passes
  - Source: `docs/prd/zai-drop.md:134`, `AGENTS.md:136`
  - Acceptance: `cargo check --workspace --no-default-features --features profile-full` passes.
  - Evidence required: command output recorded in Final Verification.
  - Status: pending
  - Evidence collected:

- V5: Live `glm-*` validation is attempted or precisely blocked
  - Source: `docs/prd/zai-drop.md:134`
  - Acceptance: run a real `glm-*` request through `openai-base:zai` if credentials are available; if not available, record exact missing env/secret and the smallest external action needed.
  - Evidence required: command/log of successful live request or blocker note with missing variable and audit IDs affected.
  - Status: pending
  - Evidence collected:

### Non-goals and exclusions

- N1: Do not preserve old `provider = "zai"` compatibility
  - Source: `docs/prd/zai-drop.md:73`
  - Must preserve: old routes fail validation explicitly.
  - Evidence required: config validation test for old provider failure.
  - Status: in_progress
  - Evidence collected: `provider = "zai"` rejection is covered under `--no-default-features --features llm-openai-base` by `route_provider_validation_rejects_removed_direct_zai_provider_when_uncompiled` on 2026-06-14 20:35. This remains in_progress until Checkpoint 5 removes the compiled `llm-zai` provider from `profile-full`.

- N2: Do not keep `ZAI_API_KEY` fallback
  - Source: `docs/prd/zai-drop.md:96`
  - Must preserve: only OpenAI Base provider env config is supported for ZAI.
  - Evidence required: runtime grep/review and config tests.
  - Status: in_progress
  - Evidence collected: Checkpoint 4 removed legacy ZAI key usage from README, `.env.example`, CI/deploy env generation, web live E2E setup, and ignored live-test requirements. Focused grep over those active docs/config/E2E paths returned no matches for `ZAI_API_KEY`/`ZAI_API_BASE` or old `zai` model-provider examples on 2026-06-14 20:47. Final verification remains in_progress until Checkpoint 5 removes the dedicated provider module that still reads the old env vars.

- N3: Do not create a new ZAI provider or SDK wrapper
  - Source: `docs/prd/zai-drop.md:58`
  - Must preserve: one OpenAI-compatible transport on `reqwest`; ZAI is a profile.
  - Evidence required: provider file review and dependency diff.
  - Status: pending
  - Evidence collected:

## Implementation Plan

1. Checkpoint 0 -- goal contract and baseline map
   - Audit IDs: all pending setup
   - Expected changes: create this goal doc; set Codex goal after loading goal skill; record current repo conventions and validation plan.
   - Validation: `git status --short --branch`; focused diff review of this document.
   - Exit condition: goal doc committed with checkpoint/audit ledger accurate enough for resumption.

2. Checkpoint 1 -- add `zai` profile and request body policy to OpenAI Base
   - Audit IDs: G1, G2, G3, G6, Q2, V1
   - Expected changes: extend profile enums/struct if needed; add `OpenAICompatibleProfile::zai()`, `resolve_profile("zai")`, ZAI structured-output model policy, ZAI `thinking` body policy, and body-builder tests.
   - Validation: focused openai_base profile/module/body tests; `cargo check -p oxide-agent-core --no-default-features --features llm-openai-base`.
   - Exit condition: ZAI profile can be selected and body-policy tests prove stream/JSON/thinking behavior without changing dedicated provider yet.

3. Checkpoint 2 -- add reqwest SSE streaming parser and ZAI aggregator
   - Audit IDs: G4, G5, Q5, V1
   - Expected changes: implement SSE `data:` parser and streaming aggregation in `openai_base`, porting behavior from `zai/sdk/stream.rs` without SDK dependency.
   - Validation: unit tests for content, reasoning, fragmented tool calls/arguments, ID preservation, usage/finish reason, empty response; focused `cargo test` for openai_base streaming.
   - Exit condition: `openai-base:zai` tool/chat path can parse streamed ZAI responses hermetically.

4. Checkpoint 3 -- port ZAI rate limits and capability/config validation
   - Audit IDs: G2, G6, G7, G10, V1, N1
   - Expected changes: move `next_flush_time` parser into OpenAI Base utilities; port capability tests; add route validation tests for `zai` failure and `openai-base:zai` success.
   - Validation: rate-limit tests, capability tests, config/model-route tests.
   - Exit condition: old route is rejected, new route accepted, and 429 wait behavior is preserved before deleting provider.

5. Checkpoint 4 -- switch docs/config/examples/E2E setup to `openai-base:zai`
   - Audit IDs: G9, G11, Q3, V2, N2
   - Expected changes: update README/env examples/CI-like configs/web E2E setup; rename generic sequenced provider mocks away from ZAI-specific names.
   - Validation: focused grep review; relevant web/core tests compile for touched profiles.
   - Exit condition: active docs/config no longer instruct dedicated ZAI provider usage.

6. Checkpoint 5 -- remove dedicated ZAI provider, feature, dependency, manifest, and dead fallback
   - Audit IDs: G8, Q1, Q4, Q6, Q7, N3
   - Expected changes: delete ZAI provider files; remove `llm-zai`, `zai-rs`, provider registration, compiled capability module, `zai_rs` tracing filter, and stale media fallback sentinel.
   - Validation: `rg` cleanup checks, `cargo tree -i zai-rs -p oxide-agent-core --no-default-features --features profile-full`, focused core check.
   - Exit condition: dedicated provider is absent and full profile no longer depends on `zai-rs`.

7. Checkpoint 6 -- snapshots, docs cleanup, and final audit
   - Audit IDs: all remaining, V1-V5, Q8
   - Expected changes: regenerate/update snapshots; finalize README/docs; fill Completion Audit evidence and Final Verification.
   - Validation: `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo check --workspace --no-default-features --features profile-full`; focused tests listed in V1/V2; live `glm-*` test or exact blocker.
   - Exit condition: every audit item is verified or the only remaining item is a documented external live-test blocker with exact required action.

## Validation Contract

- Static checks:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo check --workspace --no-default-features --features profile-full`
- Focused tests:
  - `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base --lib openai_base`
  - `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib openai_base`
  - `cargo test -p oxide-agent-core --no-default-features --features profile-full --test rate_limit`
  - `cargo test -p oxide-agent-core --no-default-features --features profile-full --test modular_registry_snapshots`
  - Additional config/capability test names discovered during implementation.
- Artifact verification:
  - `rg "llm-zai|zai-rs|llm-provider/zai|ZAI_FALLBACK_TO_MEDIA|zai_rs"` reviewed after cleanup.
  - `rg "ZAI_API_KEY|AGENT_MODEL_PROVIDER=zai|SUB_AGENT_MODEL_PROVIDER=zai"` reviewed after docs/config migration.
  - `cargo tree -i zai-rs -p oxide-agent-core --no-default-features --features profile-full` reports no package match.
- Runtime/manual verification:
  - If credentials are available, run one real `glm-*` request through `openai-base:zai`.
  - If credentials are unavailable, record exact missing env and smallest external action needed.
- Done when: all Completion Audit items are `verified`, except a live-test-only item may be `blocked` only with exact external-action evidence and no remaining local implementation/test work.

## Decisions

- 2026-06-14: Loaded `goal-repo-docs` skill before creating the OpenCode goal, matching the user instruction "skill first, plugin after".
- 2026-06-14: Use `docs/goals/2026-06-14-zai-openai-base-migration.md` because active goal docs live under `docs/goals/` and completed goals are archived under `docs/goals/archives/`.
- 2026-06-14: Do not support legacy `provider = "zai"` or `ZAI_API_KEY` fallback; the PRD explicitly prioritizes clean migration over compatibility.
- 2026-06-14: Start with profile/body policy before deleting provider so behavior can be ported and tested while the old implementation is still available as reference.
- 2026-06-14: Represent ZAI request quirks as small profile policy enums (`ThinkingPolicy`, `StreamPolicy`, `StructuredOutputPolicy`) instead of adding a new provider or broad abstraction.
- 2026-06-14: Keep ZAI streaming as a small OpenAI Base SSE parser/aggregator keyed by the existing `stream` request body flag; do not introduce a new transport abstraction or dependency.
- 2026-06-14: Preserve ZAI `next_flush_time` parsing as an OpenAI Base utility and apply it only for the `zai` profile; keep generic `Retry-After` handling as the fallback for non-ZAI OpenAI Base errors.
- 2026-06-14: While the dedicated provider still exists before Checkpoint 5, prove old `provider = "zai"` failure under the target no-`llm-zai` feature set and keep G10/N1 in_progress until `profile-full` no longer compiles `llm-zai`.
- 2026-06-14: Live ZAI E2E setup now requires explicit `OPENAI_BASE_PROVIDERS__1__*` env vars instead of translating legacy `ZAI_API_KEY`; this keeps the no-fallback constraint visible and simple.

## Progress Log

- 2026-06-14 19:50: Checkpoint 0 started -- goal contract and baseline map
  - Changed: created goal document from `docs/prd/zai-drop.md`; recorded scope, audit ledger, checkpoints, validation contract, risks.
  - Evidence: read PRD, repository instructions, existing archived goal convention, workspace Cargo feature wiring, and current `openai_base/profile.rs`; explore agent mapped ZAI/openai_base files and tests.
  - Commands: `git status --short --branch` showed `## dev...origin-ssh/dev [ahead 1]` before edits.
  - Audit IDs updated: setup only; all implementation audit items remain pending.
  - Next: review diff and commit Checkpoint 0, then implement Checkpoint 1.

- 2026-06-14 20:16: Checkpoint 1 implemented -- ZAI OpenAI Base profile and request body policy
  - Changed: added `OpenAICompatibleProfile::zai()`, ZAI profile policy enums, `resolve_profile("zai")`, model-gated ZAI structured-output capabilities for `openai-base:zai`, and body-builder handling for ZAI `thinking`, streaming, and native JSON-only behavior.
  - Evidence: focused tests cover profile fields, profile resolution, model-gated structured output, ZAI tool/plain/native-JSON request bodies, JSON-with-tools behavior, and generic non-ZAI negative behavior.
  - Commands: `cargo fmt --all`; `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base openai_base --lib` passed with 71 tests, 0 failed (warnings from Mistral module being compiled without `llm-mistral` under this narrow feature set were observed and remain non-fatal for this focused command).
  - Audit IDs updated: G1 verified; G2 in_progress; G3 verified; G6 verified; Q2 in_progress; V1 partially covered for body/profile/capability tests.
  - Next: review diff, commit Checkpoint 1, then implement Checkpoint 2 SSE streaming parser/aggregator.

- 2026-06-14 20:23: Checkpoint 2 implemented -- ZAI SSE streaming parser and aggregator in OpenAI Base
  - Changed: added reqwest SSE dispatch for `chat_with_tools` when the OpenAI Base request body has `stream: true`; added SSE `data:` parsing, UTF-8 chunk buffering, `[DONE]` handling, streamed usage parsing, reasoning/content accumulation, fragmented tool-call assembly, provider tool-call ID preservation, and empty-stream error handling.
  - Evidence: hermetic tests cover streamed transport, native JSON-only non-streaming transport, content/reasoning accumulation, fragmented function arguments, provider wire ID preservation, empty IDs as uncorrelated calls, finish reason, usage, and empty response errors.
  - Commands: `cargo fmt --all`; `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base openai_base --lib` passed with 77 tests, 0 failed; `cargo clippy -p oxide-agent-core --no-default-features --features profile-full --lib -- -D warnings` passed.
  - Audit IDs updated: G4 verified; G5 verified; Q5 verified; V1 in_progress; V2 in_progress.
  - Next: review diff, commit Checkpoint 2, then implement Checkpoint 3 rate-limit parser and route validation.

- 2026-06-14 20:35: Checkpoint 3 implemented -- ZAI rate-limit parser and route validation
  - Changed: moved ZAI `next_flush_time` parsing into OpenAI Base, applied ZAI-specific wait parsing to streaming and non-streaming 429 paths, kept generic `Retry-After` handling, ported rate-limit tests away from `llm-zai`, added `openai-base:zai` provider build/config validation, and updated model-route env parsing to use `openai-base:zai` instead of `ZAI_API_KEY`/`zai`.
  - Evidence: tests cover Unix/millisecond/ISO/no timestamp parsing, ZAI streaming and native JSON 429 mapping, generic Retry-After fallback, provider instance build, new route validation success, old route failure in the target no-`llm-zai` feature set, and profile-full env route parsing.
  - Commands: `cargo fmt --all`; `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base openai_base --lib` passed with 86 tests, 0 failed; `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base route_validation --lib` passed; `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base route_provider_validation_rejects_removed_direct_zai_provider_when_uncompiled --lib` passed; `cargo test -p oxide-agent-core --no-default-features --features llm-openrouter,llm-openai-base --test rate_limit` passed with 11 tests, 0 failed; `cargo test -p oxide-agent-core --no-default-features --features profile-full test_model_routes_parse_from_env_and_override_primary_models --lib` passed; `cargo clippy -p oxide-agent-core --no-default-features --features profile-full --lib -- -D warnings` passed; `git diff --check` passed.
  - Audit IDs updated: G2 verified; G7 verified; V1 verified; G10 in_progress; N1 in_progress; V2 in_progress.
  - Next: review diff, commit Checkpoint 3, then implement Checkpoint 4 docs/config/E2E setup migration.

- 2026-06-14 20:47: Checkpoint 4 implemented -- docs/config and web E2E setup migrated to `openai-base:zai`
  - Changed: updated README, `.env.example`, GitHub workflow/deploy env generation, web live ZAI E2E setup, live-test ignore text, Docker/telegram logging filters, and generic web E2E scripted-provider helper names.
  - Evidence: active docs/config/web E2E examples now use OpenAI Base provider env vars and `openai-base:zai`; generic web E2E mocks no longer use `SequencedZaiProvider`, `wait_for_zai_calls`, or `zai_provider` names.
  - Commands: `cargo fmt --all`; `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local --tests --no-run` compiled successfully (same narrow-feature OpenAI Base/Mistral warnings observed); `rg 'ZAI_API_KEY|ZAI_API_BASE|AGENT_MODEL_PROVIDER="?zai|SUB_AGENT_MODEL_PROVIDER="?zai' README.md .env.example .github/workflows/ci-cd.yml crates/oxide-agent-transport-web/tests/e2e || true` returned no matches; `rg "SequencedZaiProvider|wait_for_zai_calls|zai_provider" crates/oxide-agent-transport-web/tests` returned no matches; `cargo fmt --all -- --check` passed; `git diff --check` passed.
  - Audit IDs updated: G9 verified; G11 verified; Q3 in_progress; N2 in_progress; V2 in_progress.
  - Next: review diff, commit Checkpoint 4, then implement Checkpoint 5 dedicated provider/feature/dependency removal.

## Risks and Blockers

- Live `glm-*` validation may require an API key not present locally.
  - Impact: V5 may be blocked even when local implementation and hermetic tests pass.
  - Evidence: not yet checked; will inspect env/config without exposing secrets before final validation.
  - Mitigation or requested decision: if no key is available, document missing `OPENAI_BASE_PROVIDERS__N__API_KEY` and the exact command to rerun.
  - Audit IDs affected: V5 only.

- Large migration touches provider registration, config validation, snapshots, and docs.
  - Impact: broad compile/test failures are likely if deletion happens before behavior is ported.
  - Evidence: current repo still has `llm-zai` feature and `zai-rs` dependency in `crates/oxide-agent-core/Cargo.toml:52`, `:83`, `:242`.
  - Mitigation or requested decision: implement in checkpoints and commit after each verified checkpoint.
  - Audit IDs affected: G1-G11, Q1-Q8, V1-V4.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
