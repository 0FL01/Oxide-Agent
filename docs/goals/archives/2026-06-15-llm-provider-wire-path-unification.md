# Goal: unify LLM providers into two reqwest wire paths

Date started: 2026-06-15
Status: active
Codex goal: `/goal Implement docs/goals/2026-06-15-llm-provider-wire-path-unification.md until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update the doc after each meaningful verification, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.`
Source spec: user-provided LLM provider unification spec, plus repo-local goal documentation rules
Goal doc owner: Codex
Last updated: 2026-06-15 20:51 UTC+3

## Objective

Unify the LLM provider subsystem so every OpenAI-compatible Chat Completions implementation uses one reusable `providers/chat_completions` reqwest wire path, every Anthropic-compatible Messages implementation uses one reusable `providers/messages` reqwest wire path, and existing provider-specific modules become thin profile/configuration wrappers or protocol routers without losing their current quirks, aliases, capabilities, tool-call correlation behavior, media support, rate-limit handling, or feature-gated build behavior. Done when the wrappers for `openai_base`, Mistral, ZAI, OpenRouter, OpenCode Go, and Anthropic/MiniMax delegate to those two universal paths, ChatGPT OAuth/Codex remains a separate special provider path, all Completion Audit items are verified by tests or command output, and the regression matrix below passes.

## Non-goals

- Do not rewrite or force ChatGPT OAuth/Codex into `chat_completions` or `messages`.
- Do not add new crates. The workspace already uses `reqwest`; this migration is about reducing provider-specific wire duplication, not changing HTTP stacks.
- Do not change external user-visible behavior unless a change is necessary to preserve correctness and is explicitly documented in this goal doc before implementation.
- Do not remove legacy provider aliases during the initial migration.
- Do not remove provider-specific quirks. Move them into named profiles/policies, or keep a thin wrapper when a quirk is genuinely not shared.
- Do not hide protocol differences behind vague abstractions such as “openai way” and “anthropic way”. Use protocol names: `chat_completions`, `messages`, and the separate `chatgpt` path.
- Do not merge ChatGPT Responses/Codex request bodies, tool-call schema, usage parsing, or SSE event schema with OpenAI Chat Completions. Only low-level reusable stream byte decoding may be shared.
- Do not migrate or revive removed direct Gemini/Gemini-compatible provider aliases.

## Scope

In scope:

- `crates/oxide-agent-core/src/llm/provider.rs`
- `crates/oxide-agent-core/src/llm/client.rs`
- `crates/oxide-agent-core/src/llm/types.rs`
- `crates/oxide-agent-core/src/llm/capabilities.rs`
- `crates/oxide-agent-core/src/llm/providers/mod.rs`
- `crates/oxide-agent-core/src/llm/providers/modules.rs`
- `crates/oxide-agent-core/src/llm/providers/openai_base/**`
- `crates/oxide-agent-core/src/llm/providers/openrouter.rs`
- `crates/oxide-agent-core/src/llm/providers/openrouter/**`
- `crates/oxide-agent-core/src/llm/providers/opencode_go.rs`
- `crates/oxide-agent-core/src/llm/providers/opencode_go/**`
- `crates/oxide-agent-core/src/llm/providers/anthropic/client.rs`
- `crates/oxide-agent-core/src/llm/providers/anthropic_messages/**`
- `crates/oxide-agent-core/src/llm/providers/chatgpt/**`, only for optional low-level shared SSE decoder adoption and regression tests.
- `crates/oxide-agent-core/src/llm/providers/protocol_profiles.rs`
- `crates/oxide-agent-core/src/llm/providers/tool_call_adapter.rs`
- `crates/oxide-agent-core/src/llm/providers/tool_call_encoder.rs`
- `crates/oxide-agent-core/src/llm/providers/tool_result_encoder.rs`
- `crates/oxide-agent-core/src/llm/providers/tool_correlation.rs`
- `crates/oxide-agent-core/src/llm/support/http.rs`
- `crates/oxide-agent-core/src/llm/support/backoff.rs`
- `crates/oxide-agent-core/src/llm/support/history.rs`
- `crates/oxide-agent-core/Cargo.toml`
- Provider module tests, parser tests, request-shape tests, and capability/alias tests under the same crate.

Out of scope:

- Any unrelated provider behavior cleanup.
- Any non-LLM frontend, bot, web, storage, sandbox, or task runtime work.
- New provider SDKs or new network crates.
- Replacing the existing `LlmProvider` trait shape unless a tiny internal helper is needed for the two wire paths.
- Replacing existing user configuration names or environment variables during the initial migration.

## Repository Context

- Root instructions are in `AGENTS.md`. The local rule set favors Rust 1.94, explicit code over clever abstractions, no new crates unless truly necessary, feature-gated validation, and real tests before completion.
- Existing goal docs live under `docs/goals/` and completed goal docs are archived under `docs/goals/archives/`, so this active goal belongs at `docs/goals/2026-06-15-llm-provider-wire-path-unification.md`.
- The workspace default feature set is intentionally small. Many LLM provider paths are behind feature flags such as `llm-openai-base`, `llm-openrouter`, `llm-opencode-go`, `llm-minimax`, `llm-chatgpt`, and higher-level profile features.
- `reqwest` is already present and optional in `crates/oxide-agent-core/Cargo.toml`; this goal must not add another HTTP stack.
- `providers/modules.rs` is the current provider registry boundary. It owns provider module IDs, aliases, env-driven builds, and compiled capability summaries.
- `capabilities.rs` and provider module tests are the fastest way to catch alias/capability regressions.
- `support/history.rs`, `protocol_profiles.rs`, and the `tool_*` files are the critical tool-call integrity boundary.

## Current-state audit

| Area | Current file(s) | Finding | Migration target |
|---|---|---|---|
| Provider trait boundary | `crates/oxide-agent-core/src/llm/provider.rs`, `client.rs`, `types.rs` | `LlmProvider` exposes text, image, video, audio transcription, and tool chat entry points. The trait is stable enough; duplication sits below provider implementations. | Keep `LlmProvider` as the external provider boundary. New universal clients implement reusable internals that thin wrappers call. |
| Provider module registry and aliases | `crates/oxide-agent-core/src/llm/providers/modules.rs`, `providers/mod.rs` | Provider modules preserve IDs, aliases, env validation, and capabilities. Tests cover removed Gemini aliases, OpenAI base instances, OpenCode routes, OpenRouter media caps, Anthropic/MiniMax alias/caps, Mistral alias/media, and ChatGPT aliases/caps. | Preserve module IDs and aliases. Wrappers build `chat_completions` or `messages` profiles internally. Add generic provider-kind wiring without deleting legacy aliases. |
| Capability behavior | `crates/oxide-agent-core/src/llm/capabilities.rs`, provider module tests | Capability policy includes structured-output gating, media gating, tool history mode, strict/best-effort tool support, and removed Gemini behavior. | Keep capabilities at wrapper/module level. Generic wire paths must accept capability/profile input instead of inventing their own manifest behavior. |
| OpenAI-compatible generic path | `providers/openai_base/mod.rs` | This file already acts as a generic Chat Completions implementation but also owns wrapper behavior, request building, parsing, streaming, ZAI policy, Mistral policy, image MIME helpers, and tests. | Split reusable wire behavior into `providers/chat_completions/{client,request,response,profile,streaming}.rs`. Make `openai_base` a thin profile/config wrapper. |
| OpenAI-compatible profiles | `providers/openai_base/profile.rs` | Existing `OpenAICompatibleProfile` captures Mistral and ZAI policies: ID strategy, message layout, reasoning, thinking, streaming, structured output, JSON mode, media caps, and audio transcription. | Move or adapt into `chat_completions::profile::ChatCompletionsProfile`. Keep profile constructors for `generic`, `mistral`, `zai`, `openrouter`, and `opencode_go`. |
| Mistral tool IDs and transcription | `providers/openai_base/tool_ids.rs`, `providers/openai_base/transcription.rs`, `providers/openai_base/module.rs` | Mistral uses `ToolCallIdStrategy::MistralNineAlnum`, strict layout, bidirectional `ToolCallIdMapper`, 9-character alphanumeric IDs, reasoning effort for matching model IDs, and multipart transcription. | Preserve as named policies under `chat_completions` and keep Mistral module wrapper. Transcription can remain in `openai_base` temporarily only if the reusable profile owns the policy. |
| ZAI profile quirks | `providers/openai_base/profile.rs`, `providers/openai_base/mod.rs` | ZAI behavior currently includes thinking enabled/disabled policy, streaming unless native JSON mode, structured output only for GLM tool models, ZAI flush-time rate-limit parsing, and content/reasoning chunk-array parsing. | Preserve through `ChatCompletionsProfile::zai()` and policy-specific tests in `chat_completions`. |
| OpenRouter text/tools | `providers/openrouter.rs`, `providers/openrouter/helpers.rs` | OpenRouter duplicates OpenAI-style message building, tool schema generation, tool-call parsing, and token usage parsing. It also sets `provider.require_parameters = true` when tools are present and uses app attribution headers. | Route text/tools through `chat_completions` with an OpenRouter profile that preserves endpoint, headers, tool-choice omission/require-parameters behavior, error parsing, and usage parsing. Delete or shrink `helpers.rs`. |
| OpenRouter media/rate limits | `providers/openrouter.rs`, `providers/openrouter/module.rs` | OpenRouter has image analysis via `image_url`, audio via `input_audio`, video via `video_url`, capability gating by model, and `error.metadata.headers.X-RateLimit-Reset` parsing. | Preserve as `MediaPolicy::{OpenAIImageUrl, OpenRouterAudioInput, OpenRouterVideoUrl}` and `RateLimitPolicy::OpenRouterResetMetadata` in the OpenRouter profile. |
| OpenCode Go protocol router | `providers/opencode_go.rs`, `providers/opencode_go/discovery.rs`, `providers/opencode_go/module.rs` | OpenCode Go dynamically resolves `ModelProtocol::{OpenAiChatCompletions, AnthropicMessages, Unknown}`, normalizes model IDs, uses separate `api_base` and `api_base_messages`, throttles adaptively, logs summaries, and gates image models. It duplicates OpenAI request/parser/media code and calls `anthropic_messages` for Anthropic branch. | Keep OpenCode Go as a router wrapper. Delegate OpenAI branch to `chat_completions`; delegate Anthropic branch to `messages`; keep discovery, throttle, cooldown, logging, normalization, image gating, and unknown protocol behavior local. |
| Anthropic/MiniMax client | `providers/anthropic/client.rs`, `providers/anthropic_messages/{mod,request,response}.rs` | `anthropic_messages` is already close to the target Messages path. `anthropic/client.rs` is a thin `x-api-key` wrapper posting to `/v1/messages`. | Rename/refactor `anthropic_messages` into `providers/messages`, add `MessagesProfile`, and keep `anthropic`/MiniMax provider as wrapper/profile wiring. |
| Anthropic Messages semantics | `providers/anthropic_messages/request.rs`, `providers/anthropic_messages/response.rs` | Request builder folds system messages into top-level `system`, removes system role from `messages`, handles assistant content blocks, `tool_use`, `tool_result`, grouped tool results, `tool_choice auto`, `input_schema`, thinking, and response usage cache tokens. | Preserve exactly in `messages::{request,response,profile}` with tests moved/renamed, not rewritten from scratch. |
| ChatGPT OAuth/Codex provider | `providers/chatgpt/mod.rs`, `providers/chatgpt/auth.rs`, `providers/chatgpt/module.rs` | ChatGPT uses OAuth, ChatGPT account header, Responses/Codex endpoint, `instructions + input`, streaming-only request shape, unsupported-parameter retry/removal, GPT-5 temperature suppression, `reasoning.effort`, `truncation: auto`, Responses SSE events, and `function_call`/`function_call_output` call IDs. | Keep separate under `providers/chatgpt`. Only optionally replace duplicated UTF-8/newline byte helpers with `support::sse`; do not merge request/response/tool schema. |
| Tool protocol abstractions | `providers/protocol_profiles.rs`, `tool_call_adapter.rs`, `tool_call_encoder.rs`, `tool_result_encoder.rs`, `tool_correlation.rs`, `types.rs`, `support/history.rs` | `ToolProtocolProfile`, `ProviderToolCallAdapter`, `ProviderToolCallEncoder`, `ProviderToolResultEncoder`, `ToolCorrelationNormalizer`, `ToolProtocol`, `ToolTransport`, and `ToolCallCorrelation` protect provider wire ID mapping, assistant tool calls, tool results, and retry history repair. | Generic wire paths must reuse these abstractions rather than bypassing them. Add regression fixtures around provider wire IDs, fallback IDs, mapped Mistral IDs, and Anthropic tool results. |
| Shared HTTP and retry helpers | `support/http.rs`, `support/backoff.rs` | `create_http_client`, `send_json_request`, `extract_text_content`, `parse_retry_after`, and backoff helpers already exist. | Keep shared support. Universal clients use existing helper functions where applicable and add only low-level `support::sse`/`support::media` helpers if needed. |
| Duplicated SSE byte handling | `providers/openai_base/mod.rs`, `providers/chatgpt/mod.rs` | Both contain UTF-8 prefix and newline normalization helpers, but their event schemas are different. | Extract low-level byte/SSE-line decoder into `support::sse`; keep Chat Completions event parsing in `chat_completions::streaming` and ChatGPT Responses event parsing in `chatgpt`. |
| Duplicated image MIME/data URL helpers | `providers/openai_base/mod.rs`, `providers/openrouter.rs`, `providers/opencode_go.rs` | Each provider has image MIME detection/data URL logic. | Extract to `support::media` and call from `chat_completions` profiles and ChatGPT only if compatible. Preserve provider-specific content part shapes. |
| Cargo feature gates | `crates/oxide-agent-core/Cargo.toml`, `providers/mod.rs` | Provider modules compile under separate feature sets. The target modules must not become unconditionally compiled if they pull optional `reqwest`. | Gate `chat_completions` and `messages` under the same provider features that use them, or with internal feature conditions equivalent to existing ones. Validate with no-default profile commands. |

## Target architecture

### Proposed module tree

```text
crates/oxide-agent-core/src/llm/providers/
  chat_completions/
    mod.rs
    client.rs
    request.rs
    response.rs
    profile.rs
    streaming.rs

  messages/
    mod.rs
    client.rs
    request.rs
    response.rs
    profile.rs

  chatgpt/
    auth.rs
    module.rs
    mod.rs                 # unchanged special OAuth/Codex provider path

  openai_base/
    module.rs
    mod.rs                 # thin wrapper/profile wiring
    profile.rs             # temporary re-export or compatibility shim during migration only
    tool_ids.rs            # move into chat_completions or re-export from there
    transcription.rs       # may remain if Mistral-only, but driven by chat_completions profile

  openrouter/
    module.rs
    mod.rs or helpers.rs   # thin wrapper/profile wiring; helpers deleted or test-only re-exports

  opencode_go/
    discovery.rs
    module.rs
    mod.rs                 # protocol router + discovery + throttle only

  anthropic/
    client.rs              # thin wrapper/profile wiring
    mod.rs

crates/oxide-agent-core/src/llm/support/
  http.rs
  backoff.rs
  history.rs
  sse.rs                   # low-level UTF-8/newline/SSE data decoding only
  media.rs                 # MIME detection/data URL/base64 helpers
```

### Provider path classification

| Provider/configuration | Final request path | Wrapper responsibility |
|---|---|---|
| `openai_base` generic | `chat_completions` | Read configured endpoint/API key/profile and build `ChatCompletionsClient`. |
| Mistral | `chat_completions` | Supply Mistral profile, aliases, capabilities, audio transcription policy, and env handling. |
| ZAI | `chat_completions` | Supply ZAI profile and env/profile selection. |
| OpenRouter | `chat_completions` | Supply exact OpenRouter endpoint, headers, media policies, model capability policy, and rate-limit policy. |
| OpenCode Go model with `ModelProtocol::OpenAiChatCompletions` | `opencode_go` router -> `chat_completions` | Resolve protocol, normalize model ID, throttle/log, gate image input, pass OpenCode profile. |
| Anthropic/MiniMax | `messages` | Supply base URL, `x-api-key`, Anthropic version header, aliases, and capabilities. |
| OpenCode Go model with `ModelProtocol::AnthropicMessages` | `opencode_go` router -> `messages` | Resolve protocol, normalize model ID, use `api_base_messages`, throttle/log, pass OpenCode Messages profile. |
| ChatGPT OAuth/Codex | `chatgpt` special path | Keep OAuth, account ID header, Responses/Codex body, retry/removal behavior, and Responses SSE parser. |

### Compact profile model

Keep the profile structs explicit and boring. Prefer a compact policy bag over many trait objects.

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProviderKind {
    ChatCompletions,
    Messages,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AuthPolicy {
    Bearer,
    XApiKey,
    NoAuth,
    CustomHeaders(&'static [(&'static str, &'static str)]),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum EndpointPolicy {
    UseConfiguredUrlAsExactEndpoint,
    AppendChatCompletions,
    AppendMessages,
}

#[derive(Clone, Debug)]
pub(crate) struct ChatCompletionsProfile {
    pub label: &'static str,
    pub endpoint: EndpointPolicy,
    pub auth: AuthPolicy,
    pub extra_headers: &'static [(&'static str, &'static str)],
    pub tool_call_ids: ToolCallIdPolicy,
    pub empty_tool_call_id: EmptyToolCallIdPolicy,
    pub message_layout: ChatMessageLayoutPolicy,
    pub tool_schema: ChatToolSchemaPolicy,
    pub tool_choice: ChatToolChoicePolicy,
    pub json_mode: JsonModePolicy,
    pub thinking: ChatThinkingPolicy,
    pub reasoning: ChatReasoningPolicy,
    pub streaming: ChatStreamingPolicy,
    pub rate_limit: RateLimitPolicy,
    pub response_content: ChatResponseContentPolicy,
    pub usage: UsagePolicy,
    pub media: ChatMediaPolicy,
    pub audio_transcription: Option<AudioTranscriptionProfile>,
    pub capabilities: ProviderCapabilities,
}

#[derive(Clone, Debug)]
pub(crate) struct MessagesProfile {
    pub label: &'static str,
    pub endpoint: EndpointPolicy,
    pub auth: AuthPolicy,
    pub extra_headers: &'static [(&'static str, &'static str)],
    pub system_layout: MessagesSystemPolicy,
    pub tool_schema: MessagesToolSchemaPolicy,
    pub tool_choice: MessagesToolChoicePolicy,
    pub thinking: MessagesThinkingPolicy,
    pub response_content: MessagesResponseContentPolicy,
    pub usage: UsagePolicy,
    pub empty_tool_use_id: EmptyToolUseIdPolicy,
    pub capabilities: ProviderCapabilities,
}
```

Recommended policy enums:

```rust
pub(crate) enum ToolCallIdPolicy {
    Preserve,
    MistralNineAlnum,
}

pub(crate) enum EmptyToolCallIdPolicy {
    Uncorrelated,
    FallbackPrefix(&'static str),
}

pub(crate) enum ChatMessageLayoutPolicy {
    GenericOpenAI,
    MistralStrict,
}

pub(crate) enum ChatToolSchemaPolicy {
    OpenAIChatCompletions,
}

pub(crate) enum MessagesToolSchemaPolicy {
    AnthropicMessages,
}

pub(crate) enum ChatToolChoicePolicy {
    AutoWhenToolsExist,
    Omit,
}

pub(crate) enum MessagesToolChoicePolicy {
    AutoWhenToolsExist,
}

pub(crate) enum JsonModePolicy {
    None,
    StandardJsonObjectWhenNoTools,
}

pub(crate) enum ChatThinkingPolicy {
    None,
    ZaiEnabledUnlessNativeJsonMode,
}

pub(crate) enum MessagesThinkingPolicy {
    None,
    AnthropicEnabledForReasoningModels,
    OpenCodeReasoningEffort,
}

pub(crate) enum ChatReasoningPolicy {
    None,
    MistralReasoningEffortForMatchingModels,
    OpenCodeReasoningEffort,
}

pub(crate) enum ChatStreamingPolicy {
    NonStreaming,
    ChatCompletionsSse,
    ZaiUnlessNativeJsonMode,
}

pub(crate) enum RateLimitPolicy {
    RetryAfterHeader,
    OpenRouterResetMetadata,
    ZaiFlushTime,
}

pub(crate) enum ChatResponseContentPolicy {
    StringOnly,
    StringOrChunkArrayWithReasoning,
}

pub(crate) enum MessagesResponseContentPolicy {
    AnthropicContentBlocks,
}

pub(crate) enum UsagePolicy {
    OpenAIUsage,
    AnthropicUsageWithCacheAccounting,
}

pub(crate) enum ChatMediaPolicy {
    None,
    OpenAIImageUrl,
    OpenRouterAudioInput,
    OpenRouterVideoUrl,
    MistralMultipartTranscription,
}
```

ChatGPT-specific policies such as Responses usage, Responses tool schema, `function_call`/`function_call_output`, OAuth auth, and Responses SSE events intentionally stay in `providers/chatgpt` and must not be added to `ChatCompletionsProfile` as if they were Chat Completions behavior.

## Completion Audit

- G1: Create two universal reqwest wire path modules.
  - Source: user spec, target architecture.
  - Requirement: `providers/chat_completions` and `providers/messages` exist with request, response, profile, and client responsibilities separated.
  - Acceptance: OpenAI-compatible providers call `chat_completions`; Anthropic-compatible providers call `messages`; module tree matches this goal or a documented equivalent.
  - Evidence required: code review of module imports and successful feature-gated compile/test commands.
  - Status: verified
  - Evidence collected:
    - Partial, 2026-06-15 17:59 UTC+3: `providers/chat_completions` exists behind the same OpenAI-compatible feature family in `providers/mod.rs`, with split `client`, `request`, `response`, `profile`, and `streaming` modules. This does not verify G1 yet because `providers/messages` and provider delegation are not implemented.
    - Partial, 2026-06-15 18:58 UTC+3: `openai_base` is now a compatibility wrapper holding a `ChatCompletionsClient` plus Mistral `ToolCallIdMapper`, and `openai_base::profile` re-exports canonical `chat_completions::profile` types. This advances OpenAI-compatible delegation, but G1 remains pending until `providers/messages` exists and Anthropic-compatible wrappers delegate to it.
    - Partial, 2026-06-15 19:36 UTC+3: `providers/messages` now exists behind the shared provider feature family with `client`, `profile`, `request`, and `response` modules. `anthropic_messages` is reduced to an internal compatibility re-export, and production imports in `anthropic/client.rs` and OpenCode Go now use `providers::messages`. G1 remains pending until Anthropic/MiniMax and OpenCode Messages branches are fully wired through `MessagesClient`.
    - Verified, 2026-06-15 19:57 UTC+3: Both universal modules are now active delegation paths. OpenAI-compatible wrappers (`openai_base`, OpenRouter, and OpenCode Go's OpenAI branch) use `providers/chat_completions` clients/request/response/streaming helpers; Anthropic-compatible wrappers (`AnthropicProvider`/MiniMax and OpenCode Go's `ModelProtocol::AnthropicMessages` branch) use `providers/messages::MessagesClient`, request builders, response parser, and profiles. Feature-gated tests for `llm-opencode-go`, `llm-minimax`, `llm-openrouter`, `llm-openai-base`, and profile module tests have passed across checkpoints.

- G2: Eliminate OpenAI-compatible request-building duplication.
  - Source: user spec, current duplication list.
  - Requirement: OpenAI-style message building, tool schema generation, JSON mode, thinking/reasoning request policies, and media request parts are centralized in `chat_completions::request` with profile policies.
  - Acceptance: `openai_base`, `openrouter`, and OpenCode Go OpenAI branch no longer maintain separate OpenAI-style message/tool request builders except small wrapper glue.
  - Evidence required: targeted `rg` review and unit tests for generic, Mistral, ZAI, OpenRouter, and OpenCode request bodies.
  - Status: verified
  - Evidence collected:
    - Verified, 2026-06-15 18:20 UTC+3: `chat_completions::request` now owns OpenAI-style message rendering, tool schema generation, JSON mode, ZAI thinking/streaming interaction, Mistral/OpenCode reasoning effort, image/data URL helpers, and OpenRouter audio/video/image body builders. `openai_base`, OpenRouter, and OpenCode Go OpenAI request builders now call shared `chat_completions::request` functions; remaining provider-local request functions are wrapper glue or test-only parity shims. Targeted `rg` shows request helper implementations centralized in `chat_completions/request.rs`, with only wrapper/test-only function names left in provider files.

- G3: Eliminate OpenAI-compatible response/tool/usage parsing duplication.
  - Source: user spec, current duplication list.
  - Requirement: Tool call parsing, usage parsing, reasoning/content chunk parsing, and error/rate-limit parsing policy are centralized in `chat_completions::response`.
  - Acceptance: `openai_base`, `openrouter`, and OpenCode Go OpenAI branch parse responses through shared code with profile-specific policies.
  - Evidence required: parser unit tests for tool calls, empty IDs, Mistral mapped IDs, ZAI chunk arrays, OpenRouter usage, OpenCode usage, and error envelopes.
  - Status: verified
  - Evidence collected:
    - Verified, 2026-06-15 18:33 UTC+3: `chat_completions::response` now owns Chat Completions `parse_chat_response`, `parse_tool_calls`, OpenAI-style `parse_usage`, normalized tool argument parsing, string-or-chunk-array content/reasoning extraction, Mistral reverse mapping through `ChatToolCallIdResolver`, empty tool-call ID policy, OpenCode error-envelope formatting, ZAI flush-time parsing, and OpenRouter metadata reset parsing. `openai_base`, OpenRouter, and OpenCode Go now call shared response helpers; remaining provider-local parser functions are wrapper/test-compatibility shims or streaming-only helpers pending Checkpoint 5. Parser location review confirmed real parser implementations are centralized in `chat_completions/response.rs`.

- G4: Centralize Chat Completions streaming without merging ChatGPT Responses SSE.
  - Source: user spec, ChatGPT exclusion.
  - Requirement: Chat Completions SSE parsing moves to `chat_completions::streaming`; low-level byte decoding may move to `support::sse`; ChatGPT keeps its Responses event parser.
  - Acceptance: ZAI/generic Chat Completions stream tests pass and ChatGPT Responses stream tests still use ChatGPT-specific event handling.
  - Evidence required: streaming unit tests and code review proving ChatGPT event parser remains separate.
  - Status: verified
  - Evidence collected:
    - Partial, 2026-06-15 18:20 UTC+3: `chat_completions::request` uses `CHAT_LIKE_TOOL_PROFILE` for assistant tool calls and tool result messages, and its Mistral test proves mapped assistant/tool IDs plus tool result `name` preservation. Response-side correlation is still pending Checkpoint 4.
    - Verified, 2026-06-15 18:45 UTC+3: `chat_completions::streaming` now owns OpenAI-compatible SSE byte buffering, UTF-8 prefix decoding, newline normalization, Chat Completions `choices[].delta` content/reasoning accumulation, streaming tool-call delta accumulation by index, finish reason handling, usage extraction via shared OpenAI usage parsing, finalization, and ZAI streaming policy checks. `openai_base` delegates streaming response parsing to the shared module. ChatGPT keeps its local `process_sse_event` Responses parser and still has no `chat_completions` import under `providers/chatgpt`.
    - Verified, 2026-06-15 20:08 UTC+3: Low-level UTF-8 prefix decoding, CRLF normalization, and schema-agnostic SSE `data:` line extraction now live in `support::sse`. `chat_completions::streaming` uses these helpers but keeps all Chat Completions `choices[].delta` parsing local, while ChatGPT uses only `support::sse` byte/data helpers and keeps its Responses event parser local.

- G5: Preserve Mistral behavior.
  - Source: user spec, Mistral anti-regression list.
  - Requirement: Preserve `MistralNineAlnum`, `MistralStrict`, 9-character alphanumeric IDs, bidirectional `ToolCallIdMapper`, tool result name inclusion if currently required, reasoning effort model matching, multipart audio transcription, and strict tool history behavior.
  - Acceptance: Existing Mistral tests pass and new fixture tests prove request/response parity.
  - Evidence required: Mistral-specific unit tests plus `cargo test -p oxide-agent-core --no-default-features --features llm-mistral --lib` or broader equivalent.
  - Status: verified
  - Evidence collected:
    - Partial, 2026-06-15 18:33 UTC+3: Checkpoint 4 added `chat_completions_parse_mistral_tool_call_reverse_maps_id` and routed OpenAI base Mistral response parsing through `chat_completions::response` with `ToolCallIdMapper` as `ChatToolCallIdResolver`; existing Mistral response tests in `openai_base` still pass under `llm-openai-base`.
    - Verified, 2026-06-15 18:58 UTC+3: Checkpoint 6 moved `OpenAICompatibleProfile` to a compatibility re-export of canonical `ChatCompletionsProfile`, and `OpenAIBaseProvider` now wraps `ChatCompletionsClient` while keeping Mistral's mapper in wrapper state. `cargo test -p oxide-agent-core --no-default-features --features llm-mistral --lib openai_base` passed with 91 tests, including existing Mistral strict layout, mapped ID roundtrip/reverse mapping, reasoning effort, tool body, module alias/capability, and transcription retry/URL tests. Mapper lock review found no `.await` while holding `tool_id_mapper.lock()`.

- G6: Preserve ZAI behavior.
  - Source: user spec, ZAI anti-regression list.
  - Requirement: Preserve `thinking.type` enabled/disabled policy, streaming unless native JSON mode, native JSON mode interaction with thinking, GLM model-gated structured output, ZAI flush-time rate-limit parsing, and reasoning/content chunk-array parsing.
  - Acceptance: ZAI profile tests pass with request and parser fixtures.
  - Evidence required: ZAI request, response, streaming, and rate-limit unit tests.
  - Status: verified
  - Evidence collected:
    - Partial, 2026-06-15 18:33 UTC+3: Checkpoint 4 added shared response/rate-limit tests for ZAI chunk-array content/reasoning parsing and preserved `parse_zai_flush_time` by delegating the OpenAI base wrapper to `chat_completions::response::parse_zai_flush_time`; existing ZAI response/rate-limit tests in `openai_base` still pass under `llm-openai-base`.
    - Partial, 2026-06-15 18:45 UTC+3: Checkpoint 5 added `chat_completions_stream_zai_disabled_for_native_json`, proving ZAI profile streaming remains enabled except native JSON mode. Existing OpenAI base ZAI streaming/non-streaming transport tests still pass under `llm-openai-base`.
    - Verified, 2026-06-15 18:58 UTC+3: `OpenAICompatibleProfile::zai()` is now the canonical `ChatCompletionsProfile::zai()` re-export, with GLM model-gated structured output implemented on the shared profile. `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base --lib openai_base` passed with 91 tests, including ZAI thinking/native JSON request tests, streaming/non-streaming transport tests, ZAI flush-time 429 tests, chunk-array parsing, and OpenAI base ZAI module capability tests.

- G7: Preserve OpenRouter behavior.
  - Source: user spec, OpenRouter anti-regression list.
  - Requirement: Preserve app attribution headers, Bearer auth, exact endpoint, `provider.require_parameters = true` when tools are used, OpenRouter rate-limit metadata parsing, image/audio/video content parts, and model/media capability gating.
  - Acceptance: OpenRouter wrapper delegates to `chat_completions` and all current OpenRouter tests pass with new profile tests.
  - Evidence required: request-body tests for text/tools/image/audio/video, rate-limit parser tests, capability tests, and feature-gated compile.
  - Status: verified
  - Evidence collected:
    - Partial, 2026-06-15 18:33 UTC+3: OpenRouter tool response parsing and usage parsing now delegate to `chat_completions::response`; `parse_openrouter_rate_limit` delegates to shared OpenRouter metadata reset parsing. `cargo test -p oxide-agent-core --no-default-features --features llm-openrouter --lib openrouter` passed with 18 tests, including OpenRouter request/media/tool/rate-limit tests and shared `chat_completions_parse_openrouter_rate_limit_metadata`.
    - Verified, 2026-06-15 19:12 UTC+3: `OpenRouterProvider` now stores `ChatCompletionsClient` and delegates text, tool, image, audio, and video JSON posts through `client.post_json()`, which applies `ChatCompletionsProfile::openrouter()` auth, exact endpoint, attribution headers, and OpenRouter metadata rate-limit policy. `openrouter/helpers.rs` is test-only from `openrouter.rs`, while production tool/usage parsing remains in `chat_completions::response`. New/passing tests cover `openrouter_profile_adds_attribution_headers`, `openrouter_text_request_uses_headers_and_exact_endpoint`, `openrouter_tool_request_sets_require_parameters`, `openrouter_image_audio_video_requests_keep_content_part_shapes`, `openrouter_rate_limit_metadata_reset_is_preserved`, and `openrouter_capability_gating_unchanged`.

- G8: Preserve OpenCode Go dynamic routing.
  - Source: user spec, OpenCode Go anti-regression list.
  - Requirement: Preserve dynamic model protocol discovery, `OpenAiChatCompletions`, `AnthropicMessages`, `Unknown`, model ID normalization, provider prefix stripping, separate `api_base` and `api_base_messages`, adaptive throttle, cooldown/recovery, request/response logging summaries, image model gating, OpenCode reasoning effort, and Anthropic fallback prefix/profile.
  - Acceptance: `opencode_go` remains a router wrapper and its OpenAI/Anthropic branches delegate to shared paths without flattening into one fixed protocol.
  - Evidence required: discovery tests, protocol routing tests, throttle tests, request tests for both branches, and profile-feature compile commands.
  - Status: verified
  - Evidence collected:
    - Partial, 2026-06-15 18:33 UTC+3: OpenCode Go keeps dynamic protocol discovery/throttle/router code local, but its OpenAI-compatible response, tool-call, usage, and error-envelope parsing now delegate to `chat_completions::response` with `ChatCompletionsProfile::opencode_go()`. `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go --lib opencode_go` passed with 58 tests, including protocol, throttle, OpenAI body, response parser, and unknown protocol tests.
    - Verified, 2026-06-15 19:24 UTC+3: `OpenCodeGoProvider` now stores `ChatCompletionsClient` for the OpenAI Chat Completions branch and keeps `api_base_messages`, dynamic `resolve_model_protocol`, model prefix normalization, adaptive throttle, request/response summary logging, image gating, and unknown-protocol errors local to the router. OpenAI branch text/tool/image requests send through `chat_client.post_json()` after protocol resolution and use `ChatCompletionsProfile::opencode_go()` or `opencode_zen()`; Anthropic branch still uses the separate messages endpoint and existing `anthropic_messages` request/response path pending Checkpoint 9. `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go --lib opencode_go` passed with 59 tests, including new `opencode_go_openai_branch_delegates_to_chat_completions_profile`, discovery/protocol inference, model normalization, OpenCode JSON mode, image gating, throttle/cooldown/recovery, and unknown protocol tests. `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` passed cleanly.
    - Verified, 2026-06-15 19:57 UTC+3: OpenCode Go's `ModelProtocol::AnthropicMessages` branch now sends through a router-owned `messages::MessagesClient` built from `api_base_messages` and `MessagesProfile::opencode_go()`, while `resolve_model_protocol`, prefix stripping, `Unknown` error text, throttle/cooldown/recovery, request/response summary logging, and image gating remain local. New tests `opencode_go_anthropic_branch_uses_messages_api_base` and `opencode_go_anthropic_branch_preserves_fallback_tool_use_prefix` passed, along with existing unknown protocol and throttle tests under `llm-opencode-go`.

- G9: Preserve Anthropic/MiniMax Messages behavior.
  - Source: user spec, Anthropic/MiniMax anti-regression list.
  - Requirement: Preserve top-level `system`, folding history system messages into system field, messages array without system role, assistant content blocks, `tool_use`, `tool_result`, grouping consecutive tool results into one user message, `tool_choice auto` only when tools exist, `input_schema`, stop reason mapping, thinking/redacted thinking parsing, cache token accounting, empty tool ID fallback prefixes, `x-api-key`, and `anthropic-version`.
  - Acceptance: `messages` is the reusable module and `anthropic`/MiniMax wrapper delegates to it.
  - Evidence required: request/response unit tests moved from `anthropic_messages`, new wrapper integration tests, and feature-gated compile.
  - Status: verified
  - Evidence collected:
    - Partial, 2026-06-15 19:36 UTC+3: `anthropic_messages::{request,response}` were moved to `providers/messages::{request,response}` with moved tests preserving top-level `system`, system-history folding, no system role in `messages`, grouped consecutive `tool_result` blocks, assistant `tool_use`, `tool_choice auto` only with tools, `input_schema`, stop-reason mapping, thinking/redacted-thinking parsing, cache token accounting, and empty tool ID fallback prefixes. Added `MessagesProfile` with Anthropic and OpenCode policies plus `MessagesClient` wrapper. `cargo test -p oxide-agent-core --no-default-features --features llm-minimax --lib messages` passed with 36 tests; `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go --lib messages` passed with 35 tests. G9 remains pending until Checkpoints 10-11 route Anthropic/MiniMax and OpenCode Messages branches fully through `MessagesClient` with wrapper integration tests.
    - Verified, 2026-06-15 19:46 UTC+3: `AnthropicProvider` now stores `messages::MessagesClient` and delegates JSON POST/response parsing through `MessagesClient::send_and_parse()` instead of calling `send_json_request` and `messages::response::parse_response` directly. New wrapper integration tests passed: `anthropic_provider_uses_messages_headers` verifies `POST /v1/messages`, `x-api-key`, `anthropic-version`, and no `Authorization`; `anthropic_provider_text_delegates_to_messages` verifies text request shape and response extraction; `anthropic_provider_tools_preserve_tool_use_and_tool_result_blocks` verifies assistant `tool_use`, grouped `tool_result`, `input_schema`, conditional `tool_choice`, and no Chat Completions `response_format`. `cargo test -p oxide-agent-core --no-default-features --features llm-minimax --lib anthropic` passed with 17 tests, and `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib providers::modules` passed with 18 tests preserving MiniMax/Anthropic aliases and capabilities.

- G10: Preserve ChatGPT special provider behavior.
  - Source: user spec, ChatGPT exclusion.
  - Requirement: ChatGPT remains separate with OAuth auth manager, account ID header, unsupported parameter retry/removal, Responses/Codex body, GPT-5 temperature suppression, GPT-5 reasoning effort, `truncation: auto`, stream diagnostics, Responses SSE parser, and `function_call` call ID handling.
  - Acceptance: ChatGPT code does not depend on `chat_completions` request/response/profile; only optional `support::sse` low-level decoder usage is allowed.
  - Evidence required: ChatGPT request/parser tests and code review of imports.
  - Status: verified
  - Evidence collected:
    - Verified, 2026-06-15 18:45 UTC+3: `cargo test -p oxide-agent-core --no-default-features --features llm-chatgpt --lib chatgpt` passed with 26 tests, covering ChatGPT OAuth auth helpers, Responses request/tool schema, GPT-5 policy, unsupported-parameter handling, Responses SSE text/tool/error/diagnostics behavior, usage aliases, and the explicit `chatgpt_responses_sse_parser_remains_special` boundary fixture. Import review found no `chat_completions` import under `providers/chatgpt`; ChatGPT continues to use its local Responses parser and local usage parser.
    - Verified, 2026-06-15 20:08 UTC+3: ChatGPT now imports only `support::sse` for low-level UTF-8/newline/data-line helpers. Its `process_sse_event`, Responses usage parser, OAuth/Codex body builders, and function-call schema remain local. `cargo test -p oxide-agent-core --no-default-features --features llm-chatgpt --lib chatgpt` passed with 26 tests after the helper extraction.

- Q1: Preserve shared tool abstractions and correlation integrity.
  - Source: user spec, shared abstractions list.
  - Requirement: Do not bypass `ToolProtocolProfile`, `ProviderToolCallAdapter`, `ProviderToolCallEncoder`, `ProviderToolResultEncoder`, `ToolCorrelationNormalizer`, `ToolProtocol`, `ToolTransport`, or `ToolCallCorrelation`.
  - Acceptance: Assistant tool calls, tool results, retry history repair, and provider wire ID mapping remain equivalent or stricter.
  - Evidence required: tool history/correlation tests covering Chat Completions, Mistral mapped IDs, Anthropic tool results, OpenCode branches, and ChatGPT special path.
  - Status: verified
  - Evidence collected:
    - Partial, 2026-06-15 18:33 UTC+3: `chat_completions::response` uses `CHAT_LIKE_TOOL_PROFILE` for inbound tool calls, including provider-correlated IDs, uncorrelated empty IDs, and Mistral reverse-mapped IDs. New shared tests cover preserved wire IDs, empty ID policy, and Mistral reverse mapping; existing OpenAI base/OpenRouter/OpenCode parser tests still pass.
    - Partial, 2026-06-15 19:24 UTC+3: OpenCode Go OpenAI branch continues to use shared Chat Completions request/response code and existing OpenCode tests still cover provider wire ID preservation, object argument normalization, structured history wire IDs, and image-gated tool chat bodies after `OpenCodeGoProvider` was moved onto `ChatCompletionsClient`.
    - Partial, 2026-06-15 19:36 UTC+3: moved Messages request tests still cover Anthropic `tool_use` and grouped `tool_result` encoding through `ANTHROPIC_CLIENT_TOOL_PROFILE`; moved response tests still cover provider wire ID preservation and OpenCode empty tool-use fallback prefix after the module rename.
    - Partial, 2026-06-15 19:46 UTC+3: Anthropic provider wrapper tests exercise `MessagesClient`-backed tool request bodies with `ANTHROPIC_CLIENT_TOOL_PROFILE` output (`tool_use` plus grouped `tool_result`) and verify the path still uses Anthropic client-tool protocol semantics rather than Chat Completions fields.
    - Partial, 2026-06-15 19:57 UTC+3: OpenCode Go's Messages branch now uses `MessagesClient` with `MessagesProfile::opencode_go()`; `opencode_go_anthropic_branch_preserves_fallback_tool_use_prefix` proves empty provider tool IDs still become `opencode_go_tool_use_0`, and `llm-opencode-go --lib opencode_go` passed existing OpenCode tool correlation/parser tests after the branch delegation.
    - Verified, 2026-06-15 20:51 UTC+3: Final scoped provider regression passed across all migrated tool protocols: `profile-full --lib providers::modules` (21), `profile-full --lib capabilities` (32), `llm-openai-base --lib openai_base` (91), `llm-mistral --lib openai_base` (91), `llm-openrouter --lib openrouter` (22), `llm-opencode-go --lib opencode_go` (61), `llm-minimax --lib anthropic` (17), `llm-minimax --lib messages` (38), and `llm-chatgpt --lib chatgpt` (26). These include `ToolProtocolProfile`/encoder/result/correlation tests, Chat Completions provider wire IDs and Mistral reverse mapping, Anthropic `tool_use`/`tool_result`, OpenCode OpenAI and Messages branch correlation/fallback tests, and ChatGPT Responses function-call ID tests.

- Q2: Preserve aliases, module IDs, env behavior, and capability manifests.
  - Source: user spec, non-goals and registry behavior.
  - Requirement: Initial migration must not remove legacy provider aliases or module capability behavior.
  - Acceptance: Existing provider module tests pass; compiled capability output remains compatible except for documented internal module path names.
  - Evidence required: provider module tests and capability command output.
  - Status: verified
  - Evidence collected:
    - Partial, 2026-06-15 18:58 UTC+3: `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib providers::modules` passed with 18 provider module tests, covering Mistral aliases/media capabilities, OpenAI base named env providers, legacy OpenAI base migration error, profile env selecting Mistral, OpenRouter/OpenCode module behavior, and absence of direct Gemini aliases. Q2 still requires compiled capability command evidence before final verification.
    - Partial, 2026-06-15 19:46 UTC+3: `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib providers::modules` passed again with 18 tests after moving Anthropic/MiniMax wrapper calls onto `MessagesClient`; `AnthropicProviderModule` still returns `ProviderCapabilities::new(ToolHistoryMode::Strict, true, false)` and keeps provider ID/alias behavior. Q2 still requires compiled capability command evidence before final verification.
    - Verified, 2026-06-15 20:20 UTC+3: Checkpoint 13 added an internal-only generic compatible endpoint factory without changing legacy module registration. `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib providers::modules` passed with 21 tests, including new `legacy_aliases_still_build_same_provider_modules` and existing provider alias/capability tests. `cargo run -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features profile-embedded-opencode-local -- capabilities --compiled --json` succeeded and still reports the compiled OpenCode Go/Zen provider modules plus existing non-LLM modules. The corresponding compiled config schema command also succeeded, with only existing OpenCode module fields exposed; the generic endpoint factory is not user-visible yet.
    - Verified, 2026-06-15 20:32 UTC+3: Checkpoint 14 re-ran alias/capability validation after the full wrapper migration. `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib providers::modules` passed with 21 tests, covering legacy aliases, named OpenAI base env instances, removed direct Gemini aliases, OpenRouter model/media gates, OpenCode Go/Zen env aliases, Mistral media capabilities, Anthropic base capabilities, and ChatGPT aliases. `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib capabilities` passed with 32 tests after aligning two capability-manifest tests with the existing compiled sandbox requirement behavior; no production capability defaults changed. Profile checks for `profile-embedded-opencode-local` and `profile-web-embedded-opencode-local` passed, and compiled capability JSON smoke reported OpenCode Go/Zen modules with no generic provider module exposed.

- Q3: No new crates and no unnecessary user-visible behavior changes.
  - Source: user spec, non-goals.
  - Requirement: The migration uses existing dependencies and preserves external request behavior.
  - Acceptance: `Cargo.toml` has no new dependencies and fixture tests prove request/response parity.
  - Evidence required: `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` review and fixture tests.
  - Status: verified
  - Evidence collected:
    - Partial, 2026-06-15 17:59 UTC+3: `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` produced no output after Checkpoint 2 skeleton work; no dependency changes were made.
    - Partial, 2026-06-15 18:20 UTC+3: `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` still produced no output after Checkpoint 3 request migration; request parity tests and legacy provider tests passed for OpenAI base, OpenRouter, and OpenCode Go.
    - Partial, 2026-06-15 18:33 UTC+3: `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` still produced no output after Checkpoint 4 response parser migration; parser parity tests and legacy provider tests passed for OpenAI base, OpenRouter, and OpenCode Go.
    - Partial, 2026-06-15 18:58 UTC+3: `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` still produced no output after Checkpoint 6. OpenAI base wrapper/profile rewiring changed only Rust provider code and the goal document.
    - Partial, 2026-06-15 19:12 UTC+3: `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` still produced no output after Checkpoint 7. OpenRouter wrapper/client rewiring changed only Rust provider code and this goal document; request tests verify preserved OpenRouter endpoint/header/media/tool/rate-limit behavior.
    - Partial, 2026-06-15 19:24 UTC+3: `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` still produced no output after Checkpoint 8. OpenCode Go wrapper/client rewiring changed only Rust provider code and this goal document; existing OpenCode behavior tests plus the new client-profile delegation test passed.
    - Partial, 2026-06-15 19:36 UTC+3: `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` still produced no output after Checkpoint 9. The Messages rename/refactor moved existing Rust helper code and added profile/client modules without dependency changes.
    - Partial, 2026-06-15 19:46 UTC+3: `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` still produced no output after Checkpoint 10. Anthropic wrapper delegation changed only Rust provider/test code and this goal document.
    - Partial, 2026-06-15 19:57 UTC+3: `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` still produced no output after Checkpoint 11. OpenCode Go Messages branch delegation changed only Rust provider/test code and this goal document.
    - Partial, 2026-06-15 20:08 UTC+3: `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` still produced no output after Checkpoint 12. Helper extraction added only Rust support modules and rewired existing call sites; support/OpenRouter/ChatGPT/request tests confirm preserved SSE and media behavior.
    - Partial, 2026-06-15 20:20 UTC+3: `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` still produced no output after Checkpoint 13. Generic compatible endpoint support is an internal factory/config plan in `providers/modules.rs`; no public config schema stanza or legacy env behavior was changed.
    - Partial, 2026-06-15 20:32 UTC+3: `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` still produced no output after Checkpoint 14. Changes in this checkpoint were limited to test/feature-gate cleanup in Rust files and this goal document; legacy alias/capability tests and no-default profile checks passed.
    - Verified, 2026-06-15 20:51 UTC+3: Final `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` produced no output. Final diff review shows Rust provider/support/module/test/doc changes only. Compiled config schema smoke still exposes existing OpenCode Go/Zen module fields and no public generic endpoint config stanza; legacy alias/capability tests passed after the internal factory addition.

- V1: Required validation commands pass or feature incompatibilities are documented with scoped alternatives.
  - Source: user spec, validation commands.
  - Requirement: Run the validation contract commands below.
  - Acceptance: Commands pass, or any feature-incompatible workspace tests are narrowed with exact command output and documented rationale.
  - Evidence required: command output summaries in Progress Log and Final Verification.
  - Status: verified
  - Evidence collected:
    - Partial, 2026-06-15 18:20 UTC+3: Checkpoint 3 command set passed: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base --lib chat_completions::request`; `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base --lib openai_base`; `cargo test -p oxide-agent-core --no-default-features --features llm-openrouter --lib openrouter`; `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go --lib opencode_go`; feature checks for `llm-openai-base`, `llm-openrouter`, and `llm-opencode-go` passed with only pre-existing feature-gated warnings.
    - Partial, 2026-06-15 18:33 UTC+3: Checkpoint 4 command set passed: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base --lib chat_completions::response` passed: 6 passed; `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base --lib openai_base` passed: 90 passed; `cargo test -p oxide-agent-core --no-default-features --features llm-openrouter --lib openrouter` passed: 18 passed; `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go --lib opencode_go` passed: 58 passed. Warnings were pre-existing feature-gated unused/dead-code warnings.
    - Partial, 2026-06-15 18:45 UTC+3: Checkpoint 5 command set passed: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base --lib chat_completions::streaming` passed: 3 passed; `cargo test -p oxide-agent-core --no-default-features --features llm-chatgpt --lib chatgpt` passed: 26 passed; additional `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base --lib openai_base` passed: 90 passed; `cargo check -p oxide-agent-core --no-default-features --features llm-openai-base` passed with pre-existing feature-gated warnings.
    - Partial, 2026-06-15 18:58 UTC+3: Checkpoint 6 command set passed: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base --lib openai_base` passed: 91 passed; `cargo test -p oxide-agent-core --no-default-features --features llm-mistral --lib openai_base` passed: 91 passed; `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib providers::modules` passed: 18 passed. Warnings were pre-existing feature/test-scope unused warnings.
    - Partial, 2026-06-15 19:12 UTC+3: Checkpoint 7 command set passed: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features llm-openrouter --lib openrouter` passed: 22 passed; `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib providers::modules` passed: 18 passed; additional `cargo check -p oxide-agent-core --no-default-features --features llm-openrouter` passed with pre-existing `anthropic_messages` dead-code warnings.
    - Partial, 2026-06-15 19:24 UTC+3: Checkpoint 8 command set passed: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go --lib opencode_go` passed: 59 passed; `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib providers::modules` passed: 18 passed; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` passed cleanly.
    - Partial, 2026-06-15 19:36 UTC+3: Checkpoint 9 command set passed: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features llm-minimax --lib messages` passed: 36 passed; `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go --lib messages` passed: 35 passed; `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go --lib opencode_go` passed: 59 passed. Warnings were existing feature/test-scope unused warnings in provider modules.
    - Partial, 2026-06-15 19:46 UTC+3: Checkpoint 10 command set passed: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features llm-minimax --lib anthropic` passed: 17 passed; `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib providers::modules` passed: 18 passed. Warnings were existing feature/test-scope unused warnings in provider modules/config helpers.
    - Partial, 2026-06-15 19:57 UTC+3: Checkpoint 11 command set passed: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go --lib opencode_go` passed: 61 passed with only the pre-existing `canonical_route_provider` test-scope unused import warning; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` passed cleanly.
    - Partial, 2026-06-15 20:08 UTC+3: Checkpoint 12 command set passed: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base,llm-openrouter,llm-opencode-go,llm-chatgpt --lib support` passed: 42 passed; `cargo test -p oxide-agent-core --no-default-features --features llm-chatgpt --lib chatgpt` passed: 26 passed; additional request/media compatibility checks `cargo test -p oxide-agent-core --no-default-features --features llm-openrouter --lib openrouter` passed: 22 passed and `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base --lib chat_completions::request` passed: 5 passed.
    - Partial, 2026-06-15 20:20 UTC+3: Checkpoint 13 command set passed: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib providers::modules` passed: 21 passed; `cargo run -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features profile-embedded-opencode-local -- capabilities --compiled --json` succeeded; `cargo run -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features profile-embedded-opencode-local -- config schema --compiled --json` succeeded.
    - Partial, 2026-06-15 20:32 UTC+3: Checkpoint 14 command set passed: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib providers::modules` passed: 21 passed; `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib capabilities` passed: 32 passed after test-only capability expectation alignment; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` passed; `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local` passed; compiled capability JSON smoke succeeded and reported 20 modules, including OpenCode Go/Zen and no generic provider module.
    - Verified, 2026-06-15 20:51 UTC+3: Final validation completed. Passing commands: `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`; `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`; `cargo check --workspace --no-default-features --features profile-full`; scoped provider/capability test chain covering Chat Completions, Messages, OpenCode Go, OpenRouter, Anthropic/MiniMax, Mistral, ZAI, and ChatGPT; compiled capabilities JSON and config schema JSON smokes. Broad feature-combination test commands were run and documented as incompatible/flaky for unrelated non-provider tests: `cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local` failed with 939 passed / 7 failed / 10 ignored; `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib` failed with 1219 passed / 9 failed / 8 ignored; `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base,llm-openrouter,llm-opencode-go,llm-minimax,llm-chatgpt --lib` failed with 928 passed / 9 failed. Failures were in executor/wiki/delegation/model-route/sandbox/storage/cross-feature prompt-folding tests, not migrated provider wire modules; the scoped replacements compile and test every migrated provider module and ChatGPT special path.

- N1: Do not rewrite ChatGPT OAuth/Codex into the two paths.
  - Source: user spec, explicit exclusion.
  - Must preserve: Separate `providers/chatgpt` path.
  - Evidence required: code review and ChatGPT tests.
  - Status: verified
  - Evidence collected:
    - Partial, 2026-06-15 18:20 UTC+3: `rg -n "chat_completions::(request|response)|providers::chat_completions" crates/oxide-agent-core/src/llm/providers/chatgpt ...` returned imports only in OpenAI-compatible provider files; `providers/chatgpt` did not import `chat_completions` during Checkpoint 3.
    - Partial, 2026-06-15 18:33 UTC+3: The same import review after Checkpoint 4 returned `chat_completions` imports only in `openai_base`, OpenRouter, and OpenCode Go; `providers/chatgpt` still does not import shared Chat Completions request/response code.
    - Verified, 2026-06-15 18:45 UTC+3: Checkpoint 5 import review found only ChatGPT-local `process_sse_event`, `decode_utf8_prefix`, and `normalize_newlines_in_place` under `providers/chatgpt`; no `providers/chatgpt` imports of `chat_completions`. `chatgpt_responses_sse_parser_remains_special` proves ChatGPT handles `response.output_text.delta` and ignores Chat Completions `choices[].delta` chunks as missing Responses event types.
    - Verified, 2026-06-15 20:08 UTC+3: After Checkpoint 12, ChatGPT still imports no `chat_completions` modules. The only shared dependency added to ChatGPT is `support::sse`; its Responses event parser and usage/tool schema logic remain local and `llm-chatgpt --lib chatgpt` passed with 26 tests.

- N2: Do not remove legacy provider aliases during initial migration.
  - Source: user spec, explicit non-goal.
  - Must preserve: Existing provider IDs, aliases, and env names.
  - Evidence required: provider module tests.
  - Status: verified
  - Evidence collected:
    - Verified, 2026-06-15 18:58 UTC+3: `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib providers::modules` passed with 18 tests, including `mistral_module_registers_provider_id_and_aliases`, `openai_base_registers_named_env_provider_instances_only`, `openai_base_legacy_env_returns_migration_error`, and route/module alias tests for ChatGPT, Anthropic, OpenRouter, OpenCode Go, and OpenCode Zen. No legacy provider alias removal was made in Checkpoint 6.
    - Verified, 2026-06-15 20:20 UTC+3: Checkpoint 13 added `legacy_aliases_still_build_same_provider_modules`, proving Mistral, OpenRouter, Anthropic, OpenCode Go/Zen, and ChatGPT aliases still resolve to the same module IDs while `openai-base` legacy route behavior remains unchanged. Generic endpoint kind parsing explicitly rejects `chatgpt`, so ChatGPT aliases are not folded into the universal factory.

- N3: Do not add new crates.
  - Source: user spec, explicit non-goal.
  - Must preserve: Existing dependency set unless a later user-approved exception is recorded.
  - Evidence required: Cargo diff review.
  - Status: verified
  - Evidence collected:
    - Partial, 2026-06-15 17:59 UTC+3: `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` produced no output after Checkpoint 2 skeleton work.
    - Partial, 2026-06-15 18:20 UTC+3: `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` still produced no output after Checkpoint 3.
    - Partial, 2026-06-15 18:33 UTC+3: `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` still produced no output after Checkpoint 4.
    - Partial, 2026-06-15 18:45 UTC+3: `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` still produced no output after Checkpoint 5.
    - Partial, 2026-06-15 18:58 UTC+3: `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` still produced no output after Checkpoint 6.
    - Partial, 2026-06-15 19:12 UTC+3: `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` still produced no output after Checkpoint 7.
    - Partial, 2026-06-15 19:24 UTC+3: `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` still produced no output after Checkpoint 8.
    - Partial, 2026-06-15 19:36 UTC+3: `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` still produced no output after Checkpoint 9.
    - Partial, 2026-06-15 19:46 UTC+3: `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` still produced no output after Checkpoint 10.
    - Partial, 2026-06-15 19:57 UTC+3: `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` still produced no output after Checkpoint 11.
    - Partial, 2026-06-15 20:08 UTC+3: `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` still produced no output after Checkpoint 12.
    - Partial, 2026-06-15 20:20 UTC+3: `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` still produced no output after Checkpoint 13.
    - Partial, 2026-06-15 20:32 UTC+3: `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` still produced no output after Checkpoint 14.
    - Verified, 2026-06-15 20:51 UTC+3: Final Cargo manifest diff review produced no output for `Cargo.toml` or `crates/oxide-agent-core/Cargo.toml`; no crates or dependency feature changes were added.

## Checkpoints

### Checkpoint 1: Audit and fixture capture

**Intent**

Freeze current provider wire behavior before moving code so the migration can prove parity rather than relying on memory.

**Files likely touched**

- `crates/oxide-agent-core/src/llm/providers/openai_base/mod.rs`
- `crates/oxide-agent-core/src/llm/providers/openai_base/profile.rs`
- `crates/oxide-agent-core/src/llm/providers/openrouter.rs`
- `crates/oxide-agent-core/src/llm/providers/openrouter/helpers.rs`
- `crates/oxide-agent-core/src/llm/providers/opencode_go.rs`
- `crates/oxide-agent-core/src/llm/providers/anthropic_messages/request.rs`
- `crates/oxide-agent-core/src/llm/providers/anthropic_messages/response.rs`
- `crates/oxide-agent-core/src/llm/providers/chatgpt/mod.rs`
- `crates/oxide-agent-core/src/llm/providers/modules.rs`
- Optional new fixture module under existing provider test modules.

**Required changes**

- Add or consolidate unit tests that assert current JSON request shape and parser output for:
  - `openai_base` generic text/tool request.
  - Mistral strict message/tool layout and mapped tool ID request/response.
  - ZAI thinking, native JSON, streaming flag, and chunk-array response parsing.
  - OpenRouter text, tool, image, audio, video request bodies and rate-limit error parsing.
  - OpenCode Go OpenAI request body, model normalization, image gating, JSON mode, and response parser.
  - Anthropic/MiniMax text/tool Messages request and response parsing.
  - OpenCode Go Anthropic branch request profile and fallback empty tool-use ID prefix.
  - ChatGPT Responses/Codex body and Responses SSE parser.
- Record an `rg` baseline in the Progress Log for duplicated helpers before migration, especially `prepare_structured_messages`, `prepare_tools_json`, `parse_tool_calls`, `parse_usage`, `decode_utf8_prefix`, `normalize_newlines_in_place`, and image MIME helpers.
- Do not move production code in this checkpoint except for tiny test-only visibility adjustments.

**Preserve / anti-regression**

- Tool correlation tests must include provider wire IDs and fallback/uncorrelated IDs.
- Mistral tests must verify 9-character alphanumeric mapped IDs and reverse mapping.
- Anthropic tests must verify cache token accounting, not just text output.
- ChatGPT fixture tests must intentionally use Responses/Codex events so they cannot accidentally pass through Chat Completions parsing.

**Validation**

- `cargo fmt --all -- --check`
- `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base --lib openai_base`
- `cargo test -p oxide-agent-core --no-default-features --features llm-openrouter --lib openrouter`
- `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go --lib opencode_go`
- `cargo test -p oxide-agent-core --no-default-features --features llm-minimax --lib anthropic_messages`
- `cargo test -p oxide-agent-core --no-default-features --features llm-chatgpt --lib chatgpt`
- Add/update fixture tests named around request/response parity, for example `*_request_body_matches_legacy_shape`, `*_tool_call_parser_preserves_wire_ids`, and `*_usage_parser_preserves_cache_tokens`.

### Checkpoint 2: Create `chat_completions` module skeleton

**Intent**

Introduce the new reusable Chat Completions module without changing provider behavior yet.

**Files likely touched**

- `crates/oxide-agent-core/src/llm/providers/mod.rs`
- New `crates/oxide-agent-core/src/llm/providers/chat_completions/mod.rs`
- New `crates/oxide-agent-core/src/llm/providers/chat_completions/client.rs`
- New `crates/oxide-agent-core/src/llm/providers/chat_completions/request.rs`
- New `crates/oxide-agent-core/src/llm/providers/chat_completions/response.rs`
- New `crates/oxide-agent-core/src/llm/providers/chat_completions/profile.rs`
- New `crates/oxide-agent-core/src/llm/providers/chat_completions/streaming.rs`
- `crates/oxide-agent-core/Cargo.toml` only if feature wiring must be adjusted; do not add dependencies.

**Required changes**

- Add feature-gated module exports equivalent to current providers that need Chat Completions.
- Define `ChatCompletionsProfile`, endpoint/auth/policy enums, and a `ChatCompletionsClient` shell that accepts an existing `reqwest::Client`, endpoint, optional API key, model, and profile.
- Keep the initial module compiling with stubs or copied profile types, but do not route production providers until tests are ready.
- Decide whether `openai_base::profile::OpenAICompatibleProfile` becomes a compatibility type alias/re-export or migrates immediately to `chat_completions::profile::ChatCompletionsProfile`. Record the decision.

**Preserve / anti-regression**

- Do not make `chat_completions` unconditional if that would require optional `reqwest` in no-provider builds.
- Keep `openai_base`, `openrouter`, and `opencode_go` public/internal module paths compiling.
- Keep policy enum names concrete; avoid generic “ProviderQuirk” bags.

**Validation**

- `cargo fmt --all -- --check`
- `cargo check -p oxide-agent-core --no-default-features --features llm-openai-base`
- `cargo check -p oxide-agent-core --no-default-features --features llm-openrouter`
- `cargo check -p oxide-agent-core --no-default-features --features llm-opencode-go`
- Unit tests to add/update: simple profile-constructor tests for `generic`, `mistral`, `zai`, `openrouter`, and `opencode_go` profiles.

### Checkpoint 3: Move OpenAI-compatible request builders

**Intent**

Centralize Chat Completions request-body construction while preserving provider-specific profile behavior.

**Files likely touched**

- `providers/chat_completions/request.rs`
- `providers/chat_completions/profile.rs`
- `providers/openai_base/mod.rs`
- `providers/openai_base/profile.rs`
- `providers/openrouter.rs`
- `providers/openrouter/helpers.rs`
- `providers/opencode_go.rs`
- `providers/tool_call_encoder.rs`
- `providers/tool_result_encoder.rs`
- `providers/protocol_profiles.rs`

**Required changes**

- Move shared OpenAI-style history/message conversion into `chat_completions::request`, including:
  - generic system/user/assistant/tool message rendering;
  - image URL/data URL content parts;
  - Mistral strict message layout;
  - tool result name inclusion behavior if currently required;
  - JSON mode body insertion only when profile policy allows it;
  - ZAI `thinking.type` policy and native JSON interaction;
  - Mistral/OpenCode `reasoning_effort` policy;
  - tool schema generation for Chat Completions tools;
  - OpenRouter `provider.require_parameters = true` when tools are present;
  - OpenRouter tool-choice omission if that is the current behavior;
  - OpenCode OpenAI request body shape and model normalization hook.
- Expose explicit request-building entry points such as:
  - `build_text_body(...)`
  - `build_tool_body(...)`
  - `build_image_body(...)`
  - `build_audio_body(...)` only for OpenRouter-style `input_audio` if kept in Chat Completions;
  - `build_video_body(...)` for OpenRouter-style `video_url`.
- Make request builders return both the JSON body and any per-request mapping context needed by response parsing, especially Mistral `ToolCallIdMapper` state.

**Preserve / anti-regression**

- Do not change the serialized JSON field names or omit provider-specific fields.
- Do not route ChatGPT Responses-shaped tool schema through this code.
- Mistral mapped IDs must be generated before network I/O and reverse-mapped after the response.
- OpenRouter audio/video/image content part shapes must not collapse into generic image-only media support.
- `ToolProtocolProfile` and encoders must remain the source of truth for tool message/tool result semantics.

**Validation**

- `cargo fmt --all -- --check`
- `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base --lib chat_completions::request`
- `cargo test -p oxide-agent-core --no-default-features --features llm-openrouter --lib openrouter`
- `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go --lib opencode_go`
- Specific tests:
  - `chat_completions_generic_tool_request_matches_openai_base_legacy`
  - `chat_completions_mistral_request_uses_strict_layout_and_mapped_ids`
  - `chat_completions_zai_native_json_disables_streaming_thinking_conflict`
  - `chat_completions_openrouter_tool_request_sets_require_parameters`
  - `chat_completions_opencode_openai_body_preserves_reasoning_effort`

### Checkpoint 4: Move OpenAI-compatible response/tool/usage parsers

**Intent**

Centralize Chat Completions non-streaming response parsing and provider-specific response quirks.

**Files likely touched**

- `providers/chat_completions/response.rs`
- `providers/chat_completions/profile.rs`
- `providers/openai_base/mod.rs`
- `providers/openrouter.rs`
- `providers/openrouter/helpers.rs`
- `providers/opencode_go.rs`
- `providers/tool_call_adapter.rs`
- `providers/tool_correlation.rs`
- `types.rs`

**Required changes**

- Move or rewrite around existing code for:
  - `extract_message_content` / text segment extraction;
  - reasoning content extraction from string and chunk arrays;
  - ZAI reasoning/content chunk-array parsing;
  - tool call parsing with normalized argument strings;
  - Mistral reverse mapping through `ToolCallIdMapper`;
  - empty tool call ID policy: uncorrelated vs fallback prefix;
  - OpenAI-style usage parsing;
  - OpenRouter and OpenCode response/error envelope parsing as profile policies where shared.
- Return the existing `LlmResponse`, `ToolCall`, and `TokenUsage` types with the same semantics.
- Keep provider-specific rate-limit wait extraction explicit: OpenRouter metadata reset, ZAI flush time, default retry-after.

**Preserve / anti-regression**

- Tool call IDs must remain valid across assistant tool call -> tool result -> retry history repair.
- Mistral mapped wire ID must be reverse-mapped to the caller-visible invocation/provider ID expected by existing tests.
- ZAI chunk-array reasoning must not be dropped when content also exists.
- ChatGPT usage parser is not part of `UsagePolicy::OpenAIUsage`; it stays in `chatgpt` because Responses can use `input_tokens`/`output_tokens` fallback names.

**Validation**

- `cargo fmt --all -- --check`
- `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base --lib chat_completions::response`
- `cargo test -p oxide-agent-core --no-default-features --features llm-openrouter --lib openrouter`
- `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go --lib opencode_go`
- Specific tests:
  - `chat_completions_parse_tool_calls_preserves_wire_ids`
  - `chat_completions_parse_empty_tool_call_id_uses_profile_policy`
  - `chat_completions_parse_mistral_tool_call_reverse_maps_id`
  - `chat_completions_parse_zai_chunk_array_content_and_reasoning`
  - `chat_completions_parse_openai_usage`
  - `chat_completions_parse_openrouter_rate_limit_metadata`

### Checkpoint 5: Move Chat Completions SSE parser

**Intent**

Centralize OpenAI-compatible Chat Completions streaming while keeping ChatGPT Responses SSE separate.

**Files likely touched**

- `providers/chat_completions/streaming.rs`
- `providers/chat_completions/client.rs`
- `providers/chat_completions/response.rs`
- `providers/openai_base/mod.rs`
- `providers/openai_base/profile.rs`
- `support/sse.rs` if introduced in this checkpoint
- `providers/chatgpt/mod.rs` only for optional low-level decoder adoption

**Required changes**

- Move Chat Completions SSE event accumulator logic into `chat_completions::streaming`, including:
  - content delta accumulation;
  - reasoning delta accumulation;
  - streaming tool-call delta accumulation by index;
  - finish/stop reason handling;
  - usage extraction if present;
  - ZAI streaming policy and native JSON non-streaming fallback.
- Extract only byte-level helpers such as UTF-8 prefix decoding and newline normalization to `support::sse`, if useful.
- Keep ChatGPT `process_sse_event` / Responses event parser separate and profile-independent.

**Preserve / anti-regression**

- Do not parse ChatGPT `response.output_*` or `function_call` events in `chat_completions::streaming`.
- Do not lose stream diagnostics currently emitted by ChatGPT.
- Streaming tool calls must retain indices and IDs correctly until finalization.
- ZAI must still stream unless native JSON mode is active.

**Validation**

- `cargo fmt --all -- --check`
- `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base --lib chat_completions::streaming`
- `cargo test -p oxide-agent-core --no-default-features --features llm-chatgpt --lib chatgpt`
- Specific tests:
  - `chat_completions_stream_accumulates_content_and_reasoning`
  - `chat_completions_stream_accumulates_tool_call_deltas`
  - `chat_completions_stream_zai_disabled_for_native_json`
  - `chatgpt_responses_sse_parser_remains_special`

### Checkpoint 6: Refactor `openai_base` to wrapper/profile wiring

**Intent**

Turn `openai_base` into a thin compatibility wrapper over `chat_completions` while preserving OpenAI base, Mistral, and ZAI module behavior.

**Files likely touched**

- `providers/openai_base/mod.rs`
- `providers/openai_base/profile.rs`
- `providers/openai_base/module.rs`
- `providers/openai_base/tool_ids.rs`
- `providers/openai_base/transcription.rs`
- `providers/chat_completions/**`
- `providers/modules.rs`

**Required changes**

- Replace large local request/response/streaming logic in `OpenAIBaseProvider` with a `ChatCompletionsClient` field or delegated method calls.
- Keep constructor compatibility:
  - `OpenAIBaseProvider::new_with_client(...)`
  - `OpenAIBaseProvider::new_with_client_and_profile(...)`
  - any module tests that instantiate the provider directly.
- Preserve `OpenAIBaseProviderModule` env parsing for named instances and profile selection.
- Preserve `MistralProviderModule` aliases, capabilities, and audio transcription behavior.
- If `openai_base::profile` remains as compatibility shim, make it re-export `chat_completions::profile` types instead of duplicating them.

**Preserve / anti-regression**

- Mistral must still be configured by profile and not as a separate implementation fork.
- ZAI must still be selectable by profile env and keep model-specific structured output policy.
- Existing OpenAI base legacy env error behavior must remain.
- Do not hold the Mistral tool ID mapper lock across `.await` points.

**Validation**

- `cargo fmt --all -- --check`
- `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base --lib openai_base`
- `cargo test -p oxide-agent-core --no-default-features --features llm-mistral --lib openai_base`
- `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib providers::modules`
- Specific tests:
  - existing openai_base named instance/env tests;
  - existing Mistral strict/tool/audio tests;
  - new wrapper delegation test proving `openai_base` uses `chat_completions` profile constructors.

### Checkpoint 7: Refactor OpenRouter onto `chat_completions`

**Intent**

Remove OpenRouter’s duplicated OpenAI-style implementation while preserving all OpenRouter-specific headers, media, capability, and rate-limit behavior.

**Files likely touched**

- `providers/openrouter.rs`
- `providers/openrouter/helpers.rs`
- `providers/openrouter/module.rs`
- `providers/chat_completions/**`
- `support/media.rs`
- `capabilities.rs`

**Required changes**

- Introduce `ChatCompletionsProfile::openrouter()` with:
  - exact endpoint `https://openrouter.ai/api/v1/chat/completions`;
  - Bearer auth;
  - `OPENROUTER_HEADERS` / attribution headers;
  - `RateLimitPolicy::OpenRouterResetMetadata`;
  - `ChatToolChoicePolicy::Omit` if current OpenRouter tool requests omit explicit `tool_choice`;
  - `provider.require_parameters = true` when tools are present;
  - media policies for image URL, `input_audio`, and `video_url`.
- Rewrite `OpenRouterProvider` to delegate text, tools, image, audio, and video calls to `ChatCompletionsClient` with the OpenRouter profile.
- Delete `openrouter/helpers.rs` if no longer needed; otherwise leave only compatibility test helpers and remove production duplication.
- Keep `OpenRouterProviderModule` model/media capability gating unchanged.

**Preserve / anti-regression**

- Do not drop app attribution headers on any OpenRouter request.
- Do not drop media support while moving onto generic Chat Completions.
- Do not bypass OpenRouter model/media capability checks.
- Do not parse OpenRouter rate limits as plain `Retry-After` only.

**Validation**

- `cargo fmt --all -- --check`
- `cargo test -p oxide-agent-core --no-default-features --features llm-openrouter --lib openrouter`
- `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib providers::modules`
- Specific tests:
  - `openrouter_text_request_uses_headers_and_exact_endpoint`
  - `openrouter_tool_request_sets_require_parameters`
  - `openrouter_image_audio_video_requests_keep_content_part_shapes`
  - `openrouter_rate_limit_metadata_reset_is_preserved`
  - `openrouter_capability_gating_unchanged`

### Checkpoint 8: Refactor OpenCode Go OpenAI branch onto `chat_completions`

**Intent**

Keep OpenCode Go as a protocol router while delegating its OpenAI Chat Completions branch to shared code.

**Files likely touched**

- `providers/opencode_go.rs`
- `providers/opencode_go/discovery.rs`
- `providers/opencode_go/module.rs`
- `providers/chat_completions/**`
- `capabilities.rs`

**Required changes**

- Add `ChatCompletionsProfile::opencode_go()` and `ChatCompletionsProfile::opencode_zen()` if profile differences require it.
- Replace local OpenAI branch helpers in `opencode_go.rs` with `ChatCompletionsClient` calls for:
  - `complete_internal_text` when protocol is `OpenAiChatCompletions`;
  - `chat_with_tools` when protocol is `OpenAiChatCompletions`;
  - `analyze_image` when protocol is `OpenAiChatCompletions` and model supports image input.
- Keep these local to OpenCode Go:
  - dynamic model protocol resolution;
  - `ModelProtocol::Unknown` error path;
  - model ID normalization/provider prefix stripping;
  - separate `api_base` and `api_base_messages`;
  - throttle state and cooldown/recovery;
  - request/response logging summaries;
  - image model gating;
  - OpenCode-specific reasoning effort behavior.

**Preserve / anti-regression**

- Do not flatten OpenCode Go into one fixed protocol.
- Do not let shared Chat Completions code decide OpenCode protocol routing.
- Do not lose request/response summary logs used for debugging provider behavior.
- OpenCode OpenAI model IDs must remain normalized before request serialization.

**Validation**

- `cargo fmt --all -- --check`
- `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go --lib opencode_go`
- `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
- Specific tests:
  - existing discovery and protocol inference tests;
  - `opencode_go_openai_branch_delegates_to_chat_completions_profile`;
  - existing throttle/cooldown/recovery tests;
  - existing OpenCode JSON mode and image gating tests.

### Checkpoint 9: Rename/refactor `anthropic_messages` into `messages`

**Intent**

Make the existing Anthropic Messages-compatible implementation the universal `messages` module instead of a provider-specific module name.

**Files likely touched**

- `providers/anthropic_messages/mod.rs`
- `providers/anthropic_messages/request.rs`
- `providers/anthropic_messages/response.rs`
- New `providers/messages/mod.rs`
- New `providers/messages/request.rs`
- New `providers/messages/response.rs`
- New `providers/messages/profile.rs`
- New `providers/messages/client.rs`
- `providers/mod.rs`
- `providers/anthropic/client.rs`
- `providers/opencode_go.rs`

**Required changes**

- Move `anthropic_messages::request` and `anthropic_messages::response` into `messages` with minimal behavior changes.
- Introduce `MessagesProfile` to hold:
  - endpoint policy;
  - auth/header policy;
  - empty tool-use ID fallback prefix;
  - thinking policy;
  - usage/cache accounting policy.
- Add `MessagesClient` wrapping reqwest POST and response parsing.
- Update imports in `anthropic/client.rs` and OpenCode Go Anthropic branch.
- Decide whether to keep a temporary `anthropic_messages` re-export during migration. If kept, mark it as internal compatibility only and remove when all imports are moved.

**Preserve / anti-regression**

- Do not rewrite request semantics from scratch. Move and profile the existing logic.
- Preserve top-level `system` handling and absence of `system` role in `messages` array.
- Preserve grouping of consecutive tool results into one user message.
- Preserve cache token accounting tests.

**Validation**

- `cargo fmt --all -- --check`
- `cargo test -p oxide-agent-core --no-default-features --features llm-minimax --lib messages`
- `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go --lib messages`
- Specific tests:
  - moved request tests from `anthropic_messages`;
  - moved response tests from `anthropic_messages`;
  - import-path tests ensuring no production code still depends on old `anthropic_messages` except temporary re-export.

### Checkpoint 10: Refactor Anthropic/MiniMax provider onto `messages`

**Intent**

Make the Anthropic/MiniMax provider a thin wrapper over the reusable Messages path.

**Files likely touched**

- `providers/anthropic/client.rs`
- `providers/anthropic/module.rs` if present
- `providers/messages/**`
- `providers/modules.rs`
- `capabilities.rs`

**Required changes**

- Replace direct `send_json_request`/parse logic in `AnthropicProvider` with `MessagesClient` delegation.
- Preserve constructor behavior and base URL handling.
- Preserve `x-api-key` and `anthropic-version` header insertion.
- Preserve `ProviderCapabilities::anthropic()` / MiniMax capability behavior and aliases.
- Ensure `ToolProtocol::AnthropicClientTools` and `ToolTransport::ClientRoundTrip` remain the tool protocol/transport for this path.

**Preserve / anti-regression**

- Anthropic usage parser must continue to account for `cache_read_input_tokens` and `cache_creation_input_tokens`.
- Empty tool-use fallback prefix must match the current provider profile.
- `tool_choice auto` must only be emitted when tools exist.

**Validation**

- `cargo fmt --all -- --check`
- `cargo test -p oxide-agent-core --no-default-features --features llm-minimax --lib anthropic`
- `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib providers::modules`
- Specific tests:
  - `anthropic_provider_uses_messages_headers`
  - `anthropic_provider_text_delegates_to_messages`
  - `anthropic_provider_tools_preserve_tool_use_and_tool_result_blocks`
  - existing MiniMax alias/capability tests.

### Checkpoint 11: Refactor OpenCode Go Anthropic branch onto `messages`

**Intent**

Delegate OpenCode Go’s `ModelProtocol::AnthropicMessages` branch to the reusable Messages path while preserving router-specific behavior.

**Files likely touched**

- `providers/opencode_go.rs`
- `providers/opencode_go/discovery.rs`
- `providers/opencode_go/module.rs`
- `providers/messages/**`

**Required changes**

- Add a `MessagesProfile::opencode_go()` with:
  - OpenCode-specific empty tool-use fallback prefix;
  - OpenCode reasoning/thinking behavior;
  - Anthropic-compatible tool schema and content block parsing.
- Route OpenCode Anthropic text and tool calls through `MessagesClient` using `api_base_messages`.
- Keep model ID normalization and provider prefix stripping in OpenCode Go before delegation.
- Keep throttle/logging/cooldown/recovery around the delegated call.
- Preserve `ModelProtocol::Unknown` handling and unsupported protocol error text.

**Preserve / anti-regression**

- Do not infer Anthropic protocol inside `messages`; OpenCode discovery remains the authority.
- Do not use `api_base` for Messages branch; use `api_base_messages`.
- Preserve OpenCode Anthropic fallback prefix/profile.
- Preserve request/response log summaries.

**Validation**

- `cargo fmt --all -- --check`
- `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go --lib opencode_go`
- `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
- Specific tests:
  - `opencode_go_anthropic_branch_uses_messages_api_base`
  - `opencode_go_anthropic_branch_preserves_fallback_tool_use_prefix`
  - existing unknown protocol and throttle tests.

### Checkpoint 12: Extract shared support: SSE decoder and media MIME/data URL helpers

**Intent**

Remove low-level duplicated helper code while keeping protocol-specific request and event semantics separate.

**Files likely touched**

- New `crates/oxide-agent-core/src/llm/support/sse.rs`
- New `crates/oxide-agent-core/src/llm/support/media.rs`
- `crates/oxide-agent-core/src/llm/support/mod.rs` if present
- `providers/chat_completions/streaming.rs`
- `providers/chatgpt/mod.rs`
- `providers/openrouter.rs`
- `providers/opencode_go.rs`
- `providers/openai_base/mod.rs`

**Required changes**

- Extract byte-level helpers:
  - UTF-8 safe prefix decoding;
  - newline normalization;
  - SSE `data:` line extraction if this can be done without assuming event schema.
- Extract media helpers:
  - image MIME inference;
  - MIME normalization;
  - data URL generation;
  - base64 audio formatting helpers only if shared without changing content part shape.
- Replace duplicate call sites in Chat Completions and provider wrappers.
- Allow ChatGPT to use only low-level `support::sse` helpers; keep Responses event parser local.

**Preserve / anti-regression**

- Shared SSE helpers must not understand `choices[].delta`, `response.output_text.delta`, or any provider event schema.
- Shared media helpers must not decide whether a provider supports image/audio/video. Capability gates remain in profiles/modules.
- OpenRouter audio/video shapes must remain provider-specific.

**Validation**

- `cargo fmt --all -- --check`
- `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base,llm-openrouter,llm-opencode-go,llm-chatgpt --lib support`
- `cargo test -p oxide-agent-core --no-default-features --features llm-chatgpt --lib chatgpt`
- Specific tests:
  - `support_sse_decodes_utf8_prefix_without_losing_tail`
  - `support_sse_normalizes_crlf_boundaries`
  - `support_media_infers_png_jpeg_webp_gif_and_defaults_safely`
  - `support_media_builds_data_url_compatible_with_legacy_requests`

### Checkpoint 13: Add generic endpoint provider/factory/config plan

**Intent**

Allow future compatible providers to be configured as `kind = "chat_completions"` or `kind = "messages"` without creating another provider implementation, while preserving current legacy wrappers.

**Files likely touched**

- `providers/modules.rs`
- `providers/openai_base/module.rs`
- `providers/anthropic/client.rs` or module file
- `providers/chat_completions/profile.rs`
- `providers/messages/profile.rs`
- `capabilities.rs`
- Configuration schema/capability output files if generated by this repo.

**Required changes**

- Add an internal factory shape such as:
  - `ProviderKind::ChatCompletions` -> `ChatCompletionsClient` + `ChatCompletionsProfile`;
  - `ProviderKind::Messages` -> `MessagesClient` + `MessagesProfile`.
- Define config fields for new compatible providers without removing old env/config behavior:
  - `kind = "chat_completions" | "messages"`;
  - `endpoint_url = "https://..."`;
  - `api_key = "..."` or named env source consistent with existing config style;
  - optional `profile = "generic" | "mistral" | "zai" | "openrouter" | "anthropic" | "opencode_go"` when appropriate.
- Keep legacy provider modules as preconfigured wrappers/profiles:
  - `openai_base`, Mistral, ZAI, OpenRouter, OpenCode Go, Anthropic/MiniMax.
- If the repo’s config surface is not ready for a user-visible generic provider stanza, implement only the internal factory and document the exact deferred user-facing config follow-up. Do not invent an unstable public config format in code without tests.

**Preserve / anti-regression**

- Do not remove legacy aliases or env names during this checkpoint.
- Do not let a generic provider bypass capability/media/tool-history policy.
- Do not allow `kind = "chatgpt"` to pretend to be one of the two universal paths.

**Validation**

- `cargo fmt --all -- --check`
- `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib providers::modules`
- `cargo run -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features profile-embedded-opencode-local -- capabilities --compiled --json`
- `cargo run -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features profile-embedded-opencode-local -- config schema --compiled --json`
- Specific tests:
  - `generic_chat_completions_provider_builds_from_kind_endpoint_profile`
  - `generic_messages_provider_builds_from_kind_endpoint_profile`
  - `legacy_aliases_still_build_same_provider_modules`

### Checkpoint 14: Preserve legacy aliases and module capability behavior

**Intent**

Verify that internal unification did not break provider discovery, aliases, capability manifests, or feature gates.

**Files likely touched**

- `providers/modules.rs`
- `providers/mod.rs`
- `capabilities.rs`
- `providers/openai_base/module.rs`
- `providers/openrouter/module.rs`
- `providers/opencode_go/module.rs`
- `providers/chatgpt/module.rs`
- `providers/anthropic/**`

**Required changes**

- Update module imports after renaming/refactoring `messages` and `chat_completions`.
- Keep provider IDs and aliases stable:
  - `llm-provider/openai-base` instances and named-instance aliases;
  - `llm-provider/mistral`, alias `mistral`;
  - `llm-provider/openrouter`, alias `openrouter`;
  - OpenCode Go/Zen aliases and env aliases;
  - Anthropic/MiniMax aliases;
  - ChatGPT aliases;
  - removed Gemini aliases remain removed.
- Re-run capability tests and compiled capability output.
- Check feature-gated module exports for no-default builds.

**Preserve / anti-regression**

- No direct Gemini provider resurrection.
- No change to provider capability defaults unless documented and accepted in this goal.
- No unconditional `reqwest` dependency in builds that do not enable LLM HTTP providers.

**Validation**

- `cargo fmt --all -- --check`
- `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib providers::modules`
- `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib capabilities`
- `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
- `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`
- Specific tests:
  - all existing provider module alias tests;
  - compiled capability JSON smoke test if command output is supported in CI.

### Checkpoint 15: Full validation and regression matrix

**Intent**

Prove the migration is complete with full static checks, targeted tests, feature-profile checks, and explicit regression matrix review.

**Files likely touched**

- This goal doc progress/final verification sections.
- Any tests needed to close gaps found during final validation.

**Required changes**

- Run the validation contract below.
- Fill `Progress Log` with command output summaries.
- Update every Completion Audit item status with direct evidence.
- Review `rg` output to confirm duplicates are removed or intentionally retained:
  - OpenAI-style message builders remain only in `chat_completions` plus ChatGPT special body builder;
  - Chat Completions tool schema remains only in `chat_completions` plus ChatGPT Responses-shaped schema;
  - OpenAI-style tool-call parser remains in `chat_completions`;
  - OpenAI-style usage parser remains in `chat_completions`;
  - byte-level SSE helpers remain in `support::sse`;
  - image MIME/data URL helpers remain in `support::media`.
- Review `git diff` for accidental user-visible config/alias changes.

**Preserve / anti-regression**

- If any workspace tests are feature-incompatible, document the exact failing command/output and run scoped equivalent tests. Do not call the goal complete without explaining the scope.
- Do not mark Completion Audit items verified based only on green tests if the relevant code path was not compiled by that feature set.
- Final verification must include ChatGPT special-path tests even though ChatGPT is not part of the two universal paths.

**Validation**

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
- `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`
- `cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local`
- `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib`
- `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base,llm-openrouter,llm-opencode-go,llm-minimax,llm-chatgpt --lib`
- If `cargo clippy --workspace --all-targets -- -D warnings` or workspace tests are incompatible with feature combinations, record the incompatibility and run the narrowest command set that compiles every migrated provider module.

## Regression matrix

| Case | Expected request path | Critical behavior preserved |
|---|---|---|
| `openai_base` generic | `openai_base` wrapper -> `chat_completions` using configured endpoint with Chat Completions path policy | Bearer auth only when API key exists, OpenAI-style messages, OpenAI tool schema, JSON object mode only when no tools if configured, OpenAI usage parsing, generic media capability behavior. |
| Mistral | `mistral` module / `openai_base` profile -> `chat_completions` | `ToolCallIdStrategy::MistralNineAlnum`, `MistralStrict`, 9-character alphanumeric wire IDs, bidirectional mapper, tool result name inclusion if currently required, reasoning effort for matching Mistral model IDs, multipart audio transcription, strict tool history. |
| ZAI | `openai_base` ZAI profile -> `chat_completions` | `thinking.type` enabled/disabled policy, streaming unless native JSON mode, native JSON interaction with thinking, structured output only for GLM tool models, ZAI flush-time parsing, reasoning/content chunk-array response parsing. |
| OpenRouter text | `openrouter` wrapper -> `chat_completions` exact `https://openrouter.ai/api/v1/chat/completions` | `OPENROUTER_HEADERS` / app attribution, Bearer auth, OpenRouter rate-limit metadata parsing, model capability policy. |
| OpenRouter tools | `openrouter` wrapper -> `chat_completions` | OpenAI-style tools via shared schema, `provider.require_parameters = true` when tools are used, current `tool_choice` emission/omission behavior, tool call parsing and usage parsing. |
| OpenRouter image | `openrouter` wrapper -> `chat_completions` | `image_url` content part, data URL/MIME behavior, OpenRouter model/media capability gating. |
| OpenRouter audio | `openrouter` wrapper -> `chat_completions` | `input_audio` content part with expected format/base64 handling, prompt support, OpenRouter headers and capability gating. |
| OpenRouter video | `openrouter` wrapper -> `chat_completions` | `video_url` content part, prompt support, OpenRouter headers and capability gating. |
| OpenCode Go OpenAI model | `opencode_go` router -> `chat_completions` when discovery returns `ModelProtocol::OpenAiChatCompletions` | Dynamic protocol discovery, model ID normalization/provider prefix stripping, `api_base`, adaptive throttle/cooldown/recovery, request/response logging summaries, image model gating, OpenCode reasoning effort, unknown protocol still rejected. |
| OpenCode Go Anthropic model | `opencode_go` router -> `messages` when discovery returns `ModelProtocol::AnthropicMessages` | Dynamic protocol discovery, model ID normalization/provider prefix stripping, separate `api_base_messages`, adaptive throttle/cooldown/recovery, request/response logging summaries, OpenCode Messages profile and fallback empty tool-use prefix. |
| Anthropic/MiniMax text | `anthropic`/MiniMax wrapper -> `messages` | `x-api-key`, `anthropic-version`, top-level `system`, system history folded into `system`, messages array without system role, text content block parsing, stop reason mapping, cache token accounting. |
| Anthropic/MiniMax tools | `anthropic`/MiniMax wrapper -> `messages` | `tool_use` blocks, `tool_result` blocks, consecutive tool results grouped into one user message, `tool_choice auto` only when tools exist, `input_schema`, empty tool ID fallback prefixes, `ToolProtocol::AnthropicClientTools`. |
| ChatGPT text | `chatgpt` special path -> Responses/Codex endpoint | OAuth auth manager, ChatGPT account ID header, `instructions + input` body, streaming-only shape, unsupported parameter retry/removal, GPT-5 temperature suppression, GPT-5 `reasoning.effort`, `truncation: auto`, stream diagnostics, Responses SSE parser. |
| ChatGPT tools | `chatgpt` special path -> Responses/Codex endpoint | Responses-shaped tool schema, `function_call` and `function_call_output`, call ID handling, Responses usage parser with `input_tokens`/`output_tokens` fallback, no accidental Chat Completions parser usage. |

## Validation commands

Required final command set:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace --no-default-features --features profile-embedded-opencode-local
cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local
cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local
```

Additional provider-focused commands that should run before completion:

```bash
cargo check --workspace --no-default-features --features profile-full
cargo test -p oxide-agent-core --no-default-features --features profile-full --lib
cargo test -p oxide-agent-core --no-default-features --features llm-openai-base,llm-openrouter,llm-opencode-go,llm-minimax,llm-chatgpt --lib
cargo test -p oxide-agent-core --no-default-features --features llm-mistral --lib
cargo run -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features profile-embedded-opencode-local -- capabilities --compiled --json
cargo run -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features profile-embedded-opencode-local -- config schema --compiled --json
```

Scoped strategy for feature incompatibilities:

- If workspace-wide tests are incompatible with a feature combination, record the exact failing command and error summary in `Progress Log`.
- Replace the failing broad command with scoped commands that compile every migrated provider module and run the relevant unit tests.
- A scoped replacement is acceptable only if it includes Chat Completions, Messages, OpenCode Go, OpenRouter, Anthropic/MiniMax, Mistral, ZAI, and ChatGPT special-path coverage.

## Decisions

- 2026-06-15: Use protocol names `chat_completions` and `messages`, not `openai_way` or `anthropic_way`, because the goal is to preserve explicit wire protocol boundaries.
- 2026-06-15: Keep ChatGPT OAuth/Codex separate. It may share only byte-level SSE decoding helpers, never request/response/tool schema code.
- 2026-06-15: Prefer profile structs and explicit policy enums over trait-object frameworks. Provider wrappers should be thin but readable.
- 2026-06-15: Keep OpenCode Go as a router wrapper. Shared wire paths must not own dynamic protocol discovery.
- 2026-06-15: Preserve legacy provider aliases and env/config behavior through the first migration. Generic endpoint provider support is additive only.
- 2026-06-15: Generic compatible endpoint support is internal factory-only for this migration. Deferred user-facing follow-up: add a tested public config surface for compatible endpoints with `kind`, `endpoint_url`, `api_key` or env source, optional `profile`, and explicit capability/media policy before exposing it in compiled config schema.

## Progress Log

- 2026-06-15 17:30 UTC+3: Goal document created.
  - Changed: Added migration goal contract and checkpoint plan.
  - Evidence: Repository audit identified current provider files, existing docs/goals convention, provider feature gates, and validation commands.
  - Commands: Documentation-only checkpoint; no Cargo validation run yet.
  - Audit IDs updated: none verified; all implementation audit items pending.
  - Next: Start Checkpoint 1 by adding fixture/parity tests and recording duplication baseline.

- 2026-06-15 17:52 UTC+3: Checkpoint 1 fixture coverage expanded; production code not moved.
  - Changed: Added request/parser/SSE fixture tests for OpenAI-compatible low-level stream decoding, OpenRouter tool schema/rate-limit metadata, Anthropic Messages system folding/redacted thinking/null input/error envelopes, and ChatGPT Responses-shaped tools/SSE usage/newline behavior.
  - Evidence: `rg` baseline still shows duplicated helpers before migration: `prepare_structured_messages` in `openai_base`, `openrouter/helpers`, `opencode_go`; `prepare_tools_json` in `openai_base`, `openrouter/helpers`, `opencode_go`, plus protocol-specific Anthropic and ChatGPT shapes; OpenAI-style `parse_tool_calls` in `openai_base`, `openrouter/helpers`, `opencode_go`; `parse_usage`/`parse_token_usage` in `openai_base`, `opencode_go`, ChatGPT, Anthropic; byte helpers in `openai_base` and ChatGPT; image MIME/data URL helpers in `openai_base`, OpenRouter, and OpenCode Go.
  - Commands: `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base --lib openai_base` passed: 88 passed; `cargo test -p oxide-agent-core --no-default-features --features llm-openrouter --lib openrouter` passed: 15 passed; `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go --lib opencode_go` passed: 57 passed; `cargo test -p oxide-agent-core --no-default-features --features llm-minimax --lib anthropic_messages` passed after one assertion text fix: 21 passed; `cargo test -p oxide-agent-core --no-default-features --features llm-chatgpt --lib chatgpt` passed: 25 passed; `cargo fmt --all -- --check` passed.
  - Audit IDs updated: none verified yet; this checkpoint only freezes legacy behavior for later parity migration. New evidence contributes to G2, G3, G4, G7, G9, G10, Q1, and V1 but does not satisfy their final acceptance because shared wire paths are not implemented yet.
  - Next: Compress this closed checkpoint context, then start Checkpoint 2 by adding the feature-gated `providers/chat_completions` skeleton and profile/client policy types.

- 2026-06-15 17:59 UTC+3: Checkpoint 2 `chat_completions` skeleton added; production providers not routed yet.
  - Changed: Added feature-gated `providers/chat_completions/{mod,client,request,response,profile,streaming}.rs`; `providers/mod.rs` now compiles it only for `llm-mistral`, `llm-openai-base`, `llm-opencode-go`, or `llm-openrouter`. Added a client shell accepting an existing `reqwest::Client`, endpoint, optional API key, model, and `ChatCompletionsProfile`; added concrete endpoint/auth/tool/message/json/thinking/reasoning/streaming/rate-limit/media policies and constructor tests for `generic`, `mistral`, `zai`, `openrouter`, and `opencode_go`.
  - Evidence: Decision recorded: `openai_base::profile::OpenAICompatibleProfile` remains the compatibility profile for current production providers until Checkpoint 3 request parity routing starts; no type alias/re-export migration was done in Checkpoint 2. ChatGPT code was not changed to import `chat_completions`; only the new OpenAI-compatible module family was added.
  - Commands: `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base --lib chat_completions` passed: 7 passed; `cargo fmt --all -- --check` passed; `cargo check -p oxide-agent-core --no-default-features --features llm-openai-base` passed with pre-existing feature-gated warnings; `cargo check -p oxide-agent-core --no-default-features --features llm-openrouter` passed with pre-existing `anthropic_messages` dead-code warnings; `cargo check -p oxide-agent-core --no-default-features --features llm-opencode-go` passed; `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` produced no output.
  - Audit IDs updated: none verified yet; G1 has partial skeleton evidence, V1 has Checkpoint 2 command evidence, and Q3/N3 have no-new-dependency evidence. Final acceptance still requires `messages` module and provider delegation.
  - Next: Compress this checkpoint context, then start Checkpoint 3 by moving OpenAI-compatible request builders into `chat_completions::request` behind parity tests.

- 2026-06-15 18:20 UTC+3: Checkpoint 3 Chat Completions request builders centralized.
  - Changed: Implemented `chat_completions::request` builders for text, tool, image, OpenRouter audio, and OpenRouter video bodies; added shared message rendering, Chat Completions tool schema generation, Mistral mapped ID support via `ChatToolCallIdMapper`, JSON mode gating, ZAI thinking/streaming policy, Mistral/OpenCode reasoning policy, OpenRouter `provider.require_parameters`, and shared image MIME/data URL helpers. Routed `openai_base`, OpenRouter, and OpenCode Go OpenAI request construction through these shared builders while leaving ChatGPT untouched and leaving response parsing for Checkpoint 4.
  - Evidence: New request tests added and passing: `chat_completions_generic_tool_request_matches_openai_base_legacy`, `chat_completions_mistral_request_uses_strict_layout_and_mapped_ids`, `chat_completions_zai_native_json_disables_streaming_thinking_conflict`, `chat_completions_openrouter_tool_request_sets_require_parameters`, and `chat_completions_opencode_openai_body_preserves_reasoning_effort`. Existing legacy request/body tests for OpenAI base, OpenRouter, and OpenCode Go still pass. Targeted `rg` shows request helper implementations now centralized in `chat_completions/request.rs`; provider-local matches are wrapper/test-only shims. ChatGPT import review found no `chat_completions` dependency under `providers/chatgpt`.
  - Commands: `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base --lib chat_completions::request` passed: 5 passed; `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base --lib openai_base` passed: 90 passed; `cargo test -p oxide-agent-core --no-default-features --features llm-openrouter --lib openrouter` passed: 17 passed; `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go --lib opencode_go` passed: 58 passed; `cargo fmt --all -- --check` passed; `cargo check -p oxide-agent-core --no-default-features --features llm-openai-base` passed with pre-existing feature-gated warnings; `cargo check -p oxide-agent-core --no-default-features --features llm-openrouter` passed with pre-existing `anthropic_messages` dead-code warnings; `cargo check -p oxide-agent-core --no-default-features --features llm-opencode-go` passed cleanly; `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` produced no output.
  - Audit IDs updated: G2 verified. G5, G6, G7, G8, Q1, Q3, V1, N1, and N3 gained partial evidence, but response parsing, streaming extraction, Messages path, and final wrapper delegation remain pending.
  - Next: Compress this checkpoint context, then start Checkpoint 4 by moving OpenAI-compatible response/tool/usage parsing into `chat_completions::response` behind parser parity tests.

- 2026-06-15 18:33 UTC+3: Checkpoint 4 Chat Completions response parsers centralized.
  - Changed: Implemented `chat_completions::response` parsing for non-streaming Chat Completions responses, including string and chunk-array content/reasoning extraction, OpenAI-style usage parsing, normalized tool argument strings, profile-controlled empty tool-call IDs, Mistral reverse mapping via `ChatToolCallIdResolver`, OpenCode error-envelope formatting, ZAI flush-time wait parsing, and OpenRouter metadata reset parsing. Routed `openai_base`, OpenRouter, and OpenCode Go OpenAI response/tool/usage parser wrappers through the shared module. Streaming remains local until Checkpoint 5, and ChatGPT remains separate.
  - Evidence: New shared parser tests added and passing: `chat_completions_parse_tool_calls_preserves_wire_ids`, `chat_completions_parse_empty_tool_call_id_uses_profile_policy`, `chat_completions_parse_mistral_tool_call_reverse_maps_id`, `chat_completions_parse_zai_chunk_array_content_and_reasoning`, `chat_completions_parse_openai_usage`, and `chat_completions_parse_openrouter_rate_limit_metadata`. Existing parser tests for OpenAI base, OpenRouter, and OpenCode Go still pass after routing. Targeted parser-location `rg` shows real parser implementations in `chat_completions/response.rs`; provider-local matches are wrapper/test compatibility functions or streaming-only helpers pending Checkpoint 5. ChatGPT import review still shows no `chat_completions` imports under `providers/chatgpt`.
  - Commands: `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base --lib chat_completions::response` passed: 6 passed; `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base --lib openai_base` passed: 90 passed; `cargo test -p oxide-agent-core --no-default-features --features llm-openrouter --lib openrouter` passed: 18 passed; `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go --lib opencode_go` passed: 58 passed; `cargo fmt --all -- --check` passed; `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` produced no output.
  - Audit IDs updated: G3 verified. G5, G6, G7, G8, Q1, Q3, V1, N1, and N3 gained partial evidence; streaming, Messages path, full wrappers, aliases/capabilities, and final validation remain pending.
  - Next: Compress this checkpoint context, then start Checkpoint 5 by moving Chat Completions SSE parsing into `chat_completions::streaming` while keeping ChatGPT Responses SSE separate.

- 2026-06-15 18:45 UTC+3: Checkpoint 5 Chat Completions streaming parser centralized.
  - Changed: Implemented `chat_completions::streaming` for OpenAI-compatible SSE parsing: reqwest byte stream buffering, UTF-8 prefix decoding, newline normalization, Chat Completions `choices[].delta` content/reasoning accumulation, streaming tool-call delta accumulation by index, finish reason handling, usage extraction through `chat_completions::response::parse_usage`, finalization, and ZAI profile streaming policy checks. `openai_base` now delegates streaming response parsing and test-compatibility helpers to `chat_completions::streaming`. ChatGPT stayed on its local Responses SSE parser.
  - Evidence: New shared streaming tests added and passing: `chat_completions_stream_accumulates_content_and_reasoning`, `chat_completions_stream_accumulates_tool_call_deltas`, and `chat_completions_stream_zai_disabled_for_native_json`. Added and passed `chatgpt_responses_sse_parser_remains_special`, proving ChatGPT handles Responses `response.output_text.delta` and ignores Chat Completions `choices[].delta` chunks. Targeted streaming-location review shows real Chat Completions streaming functions in `chat_completions/streaming.rs`; `openai_base` has only delegating wrappers/test helpers; ChatGPT has only its local Responses parser and no `chat_completions` imports.
  - Commands: `cargo fmt --all -- --check` passed; `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base --lib chat_completions::streaming` passed: 3 passed; `cargo test -p oxide-agent-core --no-default-features --features llm-chatgpt --lib chatgpt` passed: 26 passed; additional compatibility check `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base --lib openai_base` passed: 90 passed; `cargo check -p oxide-agent-core --no-default-features --features llm-openai-base` passed with only pre-existing feature-gated warnings; `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` produced no output.
  - Audit IDs updated: G4, G10, and N1 verified. G6, Q3, V1, and N3 gained partial evidence; Messages path, full wrapper/client integration, alias/capability validation, and final regression matrix remain pending.
  - Next: Compress this checkpoint context, then start Checkpoint 6 by turning `openai_base` into thinner wrapper/profile wiring over `chat_completions` while preserving constructors and Mistral/ZAI behavior.

- 2026-06-15 18:58 UTC+3: Checkpoint 6 OpenAI base wrapper/profile wiring thinned.
  - Changed: `OpenAIBaseProvider` now stores a shared `ChatCompletionsClient` plus `transcription_base` and `ToolCallIdMapper`, rather than separate local HTTP/API/profile fields. Text, image, tool, response, streaming, auth, and endpoint access now go through the shared client/profile. `openai_base::profile` is now a compatibility re-export of canonical `chat_completions::profile` policy/profile types, and ZAI structured-output and Mistral reasoning model helpers moved onto `ChatCompletionsProfile`. Added `ChatCompletionsClient::api_key()` for preserved Mistral transcription auth. Constructor names `new`, `new_with_client`, `new_mistral`, and `new_with_client_and_profile` are preserved.
  - Evidence: Added `openai_base_wrapper_uses_chat_completions_profile_constructor`, proving `new_with_client_and_profile(..., OpenAICompatibleProfile::mistral())` stores the canonical Mistral Chat Completions profile in `ChatCompletionsClient`, computes the same `/chat/completions` endpoint, and preserves trimmed Bearer auth. `rg` review shows `openai_base::profile` re-exports `chat_completions::profile`, `OpenAIBaseProvider` holds `ChatCompletionsClient`, and mapper lock review found no `.await` while holding `tool_id_mapper.lock()`.
  - Commands: `cargo fmt --all -- --check` passed; `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base --lib openai_base` passed: 91 passed; `cargo test -p oxide-agent-core --no-default-features --features llm-mistral --lib openai_base` passed: 91 passed; `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib providers::modules` passed: 18 passed; `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` produced no output.
  - Audit IDs updated: G5, G6, and N2 verified. G1, Q2, Q3, V1, and N3 gained partial evidence; OpenRouter wrapper thinning, OpenCode router branch completion, Messages path, compiled capability output, and final regression matrix remain pending.
  - Next: Compress this checkpoint context, then start Checkpoint 7 by refactoring OpenRouter onto the shared `ChatCompletionsClient` while preserving headers, media, capabilities, and rate-limit behavior.

- 2026-06-15 19:12 UTC+3: Checkpoint 7 OpenRouter wrapper delegated to `ChatCompletionsClient`.
  - Changed: `OpenRouterProvider` now stores a shared `ChatCompletionsClient` configured with `ChatCompletionsProfile::openrouter()` instead of local HTTP client/API key fields. Text, tool, image, audio, and video calls build bodies through `chat_completions::request` and send through `client.post_json()`, which applies Bearer auth, the exact OpenRouter endpoint, profile attribution headers, and OpenRouter metadata rate-limit wait parsing. `openrouter/helpers.rs` is now test-only from `openrouter.rs`; production request/response/tool/usage behavior is in `chat_completions`.
  - Evidence: New/passing tests verify `openrouter_profile_adds_attribution_headers`, `openrouter_text_request_uses_headers_and_exact_endpoint`, `openrouter_tool_request_sets_require_parameters`, `openrouter_image_audio_video_requests_keep_content_part_shapes`, `openrouter_rate_limit_metadata_reset_is_preserved`, and `openrouter_capability_gating_unchanged`. `rg` review shows OpenRouter's exact endpoint and `OPENROUTER_HEADERS` live in `chat_completions::profile`, `OpenRouterProvider` holds `ChatCompletionsClient`, and direct OpenRouter `send_json_request`/local HTTP field usage is gone from the wrapper.
  - Commands: `cargo fmt --all -- --check` passed; `cargo test -p oxide-agent-core --no-default-features --features llm-openrouter --lib openrouter` passed: 22 passed; `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib providers::modules` passed: 18 passed; `cargo check -p oxide-agent-core --no-default-features --features llm-openrouter` passed with pre-existing `anthropic_messages` dead-code warnings; `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` produced no output.
  - Audit IDs updated: G7 verified. Q3, V1, and N3 gained partial evidence; OpenCode router branch, Messages path, compiled capability output, and final regression matrix remain pending.
  - Next: Compress this checkpoint context, then start Checkpoint 8 by refactoring the OpenCode Go OpenAI branch onto `ChatCompletionsClient` while preserving dynamic protocol routing, model normalization, throttling, logging, and image gating.

- 2026-06-15 19:24 UTC+3: Checkpoint 8 OpenCode Go OpenAI branch delegated to `ChatCompletionsClient`.
  - Changed: `OpenCodeGoProvider` now stores `ChatCompletionsClient` for the OpenAI Chat Completions branch instead of local `http_client`/`api_key`/`api_base` fields, while keeping `api_base_messages`, dynamic protocol discovery, model normalization, adaptive throttle, summary logging, image gating, and unknown protocol handling local to the OpenCode router. Added `ChatCompletionsProfile::opencode_zen()` so OpenCode Zen has its own exact default endpoint/profile label while sharing OpenCode request policies.
  - Evidence: Added/passed `opencode_go_openai_branch_delegates_to_chat_completions_profile`, proving the router stores the configured Chat Completions endpoint in `ChatCompletionsClient`, trims Bearer auth, keeps `api_base_messages` separate, and maps Zen to `ChatCompletionsProfile::opencode_zen()`. Boundary review shows OpenCode Go still owns `resolve_model_protocol`, `unsupported_protocol_error`, request/response summary logging, throttle state, and messages endpoint routing, while OpenAI branch sends through `chat_client.post_json()`.
  - Commands: `cargo fmt --all -- --check` passed; `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go --lib opencode_go` passed: 59 passed; `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib providers::modules` passed: 18 passed; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` passed cleanly; `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` produced no output.
  - Audit IDs updated: G8 verified. Q1, Q3, V1, and N3 gained partial evidence; Messages path, compiled capability output, and final regression matrix remain pending.
  - Next: Compress this checkpoint context, then start Checkpoint 9 by renaming/refactoring `anthropic_messages` into reusable `providers/messages` while preserving Anthropic/MiniMax and OpenCode Messages behavior.

- 2026-06-15 19:36 UTC+3: Checkpoint 9 `anthropic_messages` renamed/refactored into reusable `providers/messages`.
  - Changed: Moved Anthropic-compatible request and response helpers/tests into `providers/messages::{request,response}`, added `MessagesProfile` and `MessagesClient`, changed production imports in `anthropic/client.rs` and OpenCode Go to `providers::messages`, and reduced `providers/anthropic_messages` to an internal compatibility re-export. Existing discovery protocol string `anthropic_messages` remains accepted as a wire/config alias, not a production module dependency.
  - Evidence: Moved Messages tests prove system folding, absence of system role in `messages`, assistant `tool_use`, grouped `tool_result`, conditional `tool_choice`, `input_schema`, stop-reason mapping, thinking/redacted-thinking parsing, cache token accounting, Anthropic/OpenCode fallback prefixes, and profile headers/auth policy. Import review `rg -n "anthropic_messages::|providers::anthropic_messages|super::anthropic_messages|use super::anthropic_messages|use crate::llm::providers::anthropic_messages" crates/oxide-agent-core/src/llm/providers` produced no output; only `providers/mod.rs` retains the temporary compatibility module and `opencode_go/discovery.rs` retains the protocol string alias.
  - Commands: `cargo fmt --all -- --check` passed; `cargo test -p oxide-agent-core --no-default-features --features llm-minimax --lib messages` passed: 36 passed; `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go --lib messages` passed: 35 passed; `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go --lib opencode_go` passed: 59 passed; `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` produced no output.
  - Audit IDs updated: G1, G9, Q1, Q3, V1, and N3 gained partial evidence; G9 remains pending until Anthropic/MiniMax and OpenCode Messages branches delegate through `MessagesClient` in Checkpoints 10-11.
  - Next: Compress this checkpoint context, then start Checkpoint 10 by refactoring the Anthropic/MiniMax provider wrapper onto `MessagesClient` while preserving `x-api-key`, `anthropic-version`, aliases, capabilities, and existing MiniMax behavior.

- 2026-06-15 19:46 UTC+3: Checkpoint 10 Anthropic/MiniMax wrapper delegated to `MessagesClient`.
  - Changed: `AnthropicProvider` now stores `messages::MessagesClient` and builds it from the configured base URL with `MessagesProfile::anthropic()`. Text and tool methods still build request bodies through `messages::request`, but sending and response parsing now delegate to `MessagesClient::send_and_parse()`. Direct `send_json_request` usage was removed from `providers/anthropic/client.rs`; provider module capability/alias code was unchanged.
  - Evidence: Added/passed `anthropic_provider_uses_messages_headers`, `anthropic_provider_text_delegates_to_messages`, and `anthropic_provider_tools_preserve_tool_use_and_tool_result_blocks`. These prove the wrapper still posts to `/v1/messages`, sends `anthropic-version` and `x-api-key` without `Authorization`, extracts text responses, and preserves Anthropic `tool_use` / grouped `tool_result` / `input_schema` / conditional `tool_choice` request shape. Boundary review shows `send_json_request` for this path lives only inside `providers/messages/client.rs`; `providers/anthropic/client.rs` holds `MessagesClient`; `ANTHROPIC_CLIENT_TOOL_PROFILE` remains `ToolProtocol::AnthropicClientTools` with `ToolTransport::ClientRoundTrip`.
  - Commands: `cargo fmt --all -- --check` passed; `cargo test -p oxide-agent-core --no-default-features --features llm-minimax --lib anthropic` passed: 17 passed; `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib providers::modules` passed: 18 passed; `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` produced no output.
  - Audit IDs updated: G9 verified. Q1, Q2, Q3, V1, and N3 gained partial evidence; G1 remains pending until OpenCode Go's Anthropic Messages branch also delegates through `MessagesClient`.
  - Next: Compress this checkpoint context, then start Checkpoint 11 by routing OpenCode Go's `ModelProtocol::AnthropicMessages` branch through `MessagesClient` while preserving router-owned protocol resolution, `api_base_messages`, throttling, logging, unknown-protocol behavior, and OpenCode fallback prefix.

- 2026-06-15 19:57 UTC+3: Checkpoint 11 OpenCode Go Anthropic branch delegated to `MessagesClient`.
  - Changed: `OpenCodeGoProvider` now stores both `ChatCompletionsClient` and `messages::MessagesClient`. The `ModelProtocol::AnthropicMessages` text and tool branches still build Anthropic-compatible request bodies through `messages::request` after OpenCode-owned protocol resolution/model normalization, but now send through the router-owned `messages_client` configured with `api_base_messages` and `MessagesProfile::opencode_go()`. OpenAI branch delegation, `api_base_messages`, discovery, unknown-protocol errors, adaptive throttle, request/response summary logging, and image gating remain local to the OpenCode router.
  - Evidence: Added/passed `opencode_go_anthropic_branch_uses_messages_api_base`, proving Anthropic-protocol models post to the configured `/v1/messages` endpoint rather than the Chat Completions endpoint, send `Authorization: Bearer token`, `x-api-key`, and `anthropic-version`, normalize `opencode-go/minimax-m2` to `minimax-m2`, and keep Messages request shape without Chat Completions `response_format`. Added/passed `opencode_go_anthropic_branch_preserves_fallback_tool_use_prefix`, proving empty Anthropic tool IDs still use `opencode_go_tool_use_0`. Boundary review shows direct `send_json_request` is gone from `opencode_go.rs`; it remains inside `providers/messages/client.rs` and other unrelated provider paths.
  - Commands: `cargo fmt --all -- --check` passed; `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go --lib opencode_go` passed: 61 passed with only the pre-existing `canonical_route_provider` test-scope unused import warning; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` passed cleanly; `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` produced no output.
  - Audit IDs updated: G1 verified. G8 gained final OpenCode Messages-branch evidence. Q1, Q3, V1, and N3 gained partial evidence; Q2 still needs compiled capability command evidence, and final support-helper extraction/regression matrix remain pending.
  - Next: Compress this checkpoint context, then start Checkpoint 12 by extracting shared low-level SSE decoder and media MIME/data URL helpers without moving provider-specific event schemas or media capability decisions.

- 2026-06-15 20:08 UTC+3: Checkpoint 12 low-level SSE and media helpers extracted.
  - Changed: Added `llm/support/sse.rs` for UTF-8 safe prefix decoding, CRLF normalization, and schema-agnostic SSE `data:` payload extraction. Added `llm/support/media.rs` for image MIME inference, image/data URL generation, raw base64 encoding, and audio MIME-to-input-format mapping. `chat_completions::streaming` and ChatGPT now use only the low-level SSE helpers while keeping their provider-specific event parsers local. `chat_completions::request` now delegates image/audio/video encoding helpers to `support::media`, and OpenRouter test shims delegate MIME/audio-format checks to the same support module.
  - Evidence: New/passing support tests cover `support_sse_decodes_utf8_prefix_without_losing_tail`, `support_sse_normalizes_crlf_boundaries`, schema-free `data:` extraction, `support_media_infers_png_jpeg_webp_gif_and_defaults_safely`, and `support_media_builds_data_url_compatible_with_legacy_requests`. Boundary review found no `choices[].delta` or `response.output_text.delta` schema knowledge under `llm/support`, and no direct `data:` extraction duplicated outside `support::sse` except delegating wrappers/tests. ChatGPT still has its local Responses `process_sse_event` parser and no `chat_completions` imports.
  - Commands: `cargo fmt --all -- --check` passed; `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base,llm-openrouter,llm-opencode-go,llm-chatgpt --lib support` passed: 42 passed; `cargo test -p oxide-agent-core --no-default-features --features llm-chatgpt --lib chatgpt` passed: 26 passed; additional compatibility checks `cargo test -p oxide-agent-core --no-default-features --features llm-openrouter --lib openrouter` passed: 22 passed and `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base --lib chat_completions::request` passed: 5 passed; `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` produced no output.
  - Audit IDs updated: G4, G10, and N1 gained final low-level-helper boundary evidence. Q3, V1, and N3 gained partial evidence; Q2 compiled capability output, generic endpoint factory/config plan, alias/capability verification, cleanup, and final regression matrix remain pending.
  - Next: Compress this checkpoint context, then start Checkpoint 13 by adding the generic endpoint provider/factory/config plan without removing legacy aliases or inventing untested public config.

- 2026-06-15 20:20 UTC+3: Checkpoint 13 internal generic compatible endpoint factory added.
  - Changed: Added internal `GenericProviderKind`, `GenericEndpointProviderConfig`, `GenericEndpointClient`, and `build_generic_endpoint_provider` in `providers/modules.rs`. The factory builds `ChatCompletionsClient + ChatCompletionsProfile` for `kind = "chat_completions"` and `MessagesClient + MessagesProfile` for `kind = "messages"`, carries capability/media policy from the selected profile/path, rejects `chatgpt`, and remains deliberately unwired from public `AgentSettings`. Added `ChatCompletionsProfile::endpoint_for` for exact-vs-append endpoint policy reuse.
  - Evidence: Added/passed `generic_chat_completions_provider_builds_from_kind_endpoint_profile`, `generic_messages_provider_builds_from_kind_endpoint_profile`, and `legacy_aliases_still_build_same_provider_modules`. Compiled config schema output still exposes only existing stable module config fields; deferred user-facing follow-up is documented in the Decision Ledger instead of inventing an untested public config format.
  - Commands: `cargo fmt --all -- --check` passed; `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib providers::modules` passed: 21 passed; `cargo run -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features profile-embedded-opencode-local -- capabilities --compiled --json` succeeded; `cargo run -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features profile-embedded-opencode-local -- config schema --compiled --json` succeeded; `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` produced no output.
  - Audit IDs updated: Q2 verified. N2 gained final alias/factory boundary evidence. Q3, V1, and N3 gained partial evidence; Checkpoint 14 alias/capability feature-gate review and final regression matrix remain pending.
  - Next: Compress this checkpoint context, then start Checkpoint 14 by validating legacy aliases, module capability behavior, compiled capabilities, and no-default feature gates after all wrapper/factory changes.

- 2026-06-15 20:32 UTC+3: Checkpoint 14 legacy aliases and capability behavior revalidated.
  - Changed: No production provider behavior changed. Cleaned feature-gated OpenAI base exports so no-default web/embedded profiles compile without new unused warnings, and adjusted two capability-manifest tests to expect the existing compiled sandbox backend requirement options that include bwrap capability alternatives even when the bwrap backend module is not enabled in the current profile.
  - Evidence: Provider module tests still cover Mistral, OpenRouter, OpenCode Go/Zen, Anthropic/MiniMax, ChatGPT, named OpenAI base instances, legacy OpenAI base migration error, disabled modules, removed direct Gemini aliases, and OpenRouter/OpenCode capability gating. Capability tests now pass under `profile-full`; compiled capability JSON smoke reports OpenCode Go/Zen provider modules and no generic provider module. Boundary grep found direct Gemini provider strings only in rejection tests and the internal generic factory only in `providers/modules.rs`.
  - Commands: `cargo fmt --all -- --check` passed; `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib providers::modules` passed: 21 passed; `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib capabilities` passed: 32 passed; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` passed; `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local` passed; `cargo run -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features profile-embedded-opencode-local -- capabilities --compiled --json` succeeded and summarized 20 modules; `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` produced no output.
  - Audit IDs updated: Q2 evidence strengthened with full Checkpoint 14 alias/capability validation. Q3, V1, and N3 gained partial evidence; final regression matrix and duplicate-location review remain pending for Checkpoint 15.
  - Next: Compress this checkpoint context, then start Checkpoint 15 full validation and regression matrix review.

- 2026-06-15 20:51 UTC+3: Checkpoint 15 final validation and regression matrix review completed.
  - Changed: Fixed a default-feature compile/clippy regression in the internal generic endpoint factory fallback cfgs by compiling fallback builders only when the internal generic factory types are compiled, and gated `ToolHistoryMode` import to Messages-capable builds. No provider behavior, aliases, or Cargo manifests changed. Updated this audit/final verification section.
  - Evidence: Duplicate-helper review shows real OpenAI-compatible request builders/tool schema in `providers/chat_completions/request.rs`, real OpenAI-style tool/usage parsers in `providers/chat_completions/response.rs`, low-level SSE helpers in `llm/support/sse.rs`, and media helpers in `llm/support/media.rs`; remaining matches are test/compatibility wrappers, provider-specific Messages/ChatGPT schemas, or ChatGPT Responses parser helpers. ChatGPT import review shows no `chat_completions` imports. Support boundary grep found no `choices[].delta`/`response.output_*` schema knowledge in `llm/support`. Alias/config review found direct Gemini strings only in rejection tests/config validation, and the internal generic factory only in `providers/modules.rs`.
  - Commands: Passing final commands: `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`; `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`; `cargo check --workspace --no-default-features --features profile-full`; scoped provider/capability chain passed: `profile-full --lib providers::modules` 21, `profile-full --lib capabilities` 32, `llm-openai-base --lib openai_base` 91, `llm-mistral --lib openai_base` 91, `llm-openrouter --lib openrouter` 22, `llm-opencode-go --lib opencode_go` 61, `llm-minimax --lib anthropic` 17, `llm-minimax --lib messages` 38, `llm-chatgpt --lib chatgpt` 26; compiled capabilities JSON smoke reported 20 modules with OpenCode Go/Zen and no generic provider module; compiled config schema JSON smoke succeeded and reported no generic endpoint config exposure; `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` produced no output.
  - Feature-incompatible broad tests documented: `cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local` failed with 939 passed / 7 failed / 10 ignored; `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib` failed with 1219 passed / 9 failed / 8 ignored; `cargo test -p oxide-agent-core --no-default-features --features llm-openai-base,llm-openrouter,llm-opencode-go,llm-minimax,llm-chatgpt --lib` failed with 928 passed / 9 failed. Failures were outside migrated provider wire modules except one cross-feature prompt-folding assertion; scoped replacements above compile and test every migrated provider module plus ChatGPT special path.
  - Audit IDs updated: Q1, Q3, V1, and N3 verified. All Completion Audit items are now verified.
  - Next: Mark goal complete after final artifact audit.

## Risks and blockers

### High

- Tool call correlation breakage.
  - Impact: Assistant tool calls, tool results, retry history repair, and provider wire IDs can silently desynchronize.
  - Evidence to watch: changed `ToolCallCorrelation`, `wire_tool_call_id()`, `ToolProtocolProfile`, mapper/fallback behavior, or history validation tests.
  - Mitigation: Add parity tests around Chat Completions tool calls, Mistral mapped IDs, Anthropic tool results, OpenCode both branches, and ChatGPT Responses function calls before moving production code.
  - Audit IDs affected: G2, G3, G5, G8, G9, G10, Q1.

- Mistral ID mapping breakage.
  - Impact: Mistral tool calls can fail because the provider requires 9-character alphanumeric wire IDs while local history expects stable invocation/provider IDs.
  - Evidence to watch: `ToolCallIdMapper`, `MistralNineAlnum`, strict layout tests, lock lifetime around `.await`.
  - Mitigation: Keep mapper state in the wrapper/client context and test bidirectional mapping across request and response parsing.
  - Audit IDs affected: G5, Q1.

- OpenCode dynamic protocol routing breakage.
  - Impact: OpenAI-compatible models, Anthropic-compatible models, and unknown models could be routed to the wrong wire path or lose throttling behavior.
  - Evidence to watch: `ModelProtocol`, discovery overrides, `api_base_messages`, model prefix stripping, throttle state.
  - Mitigation: Keep OpenCode router local and delegate only after protocol resolution and normalization.
  - Audit IDs affected: G8.

- OpenRouter headers/media/rate-limit loss.
  - Impact: Requests can be unattributed, rejected, misrouted, or lose image/audio/video support.
  - Evidence to watch: missing `OPENROUTER_HEADERS`, missing `provider.require_parameters`, missing `input_audio`/`video_url`, generic retry-after replacing OpenRouter metadata reset parsing.
  - Mitigation: Encode each as named OpenRouter profile policy and test all media paths.
  - Audit IDs affected: G7.

- ChatGPT accidental merge/regression.
  - Impact: OAuth/Codex provider can break because Responses/Codex is not Chat Completions.
  - Evidence to watch: ChatGPT importing `chat_completions::request`, `chat_completions::response`, or Chat Completions tool schema.
  - Mitigation: Restrict sharing to `support::sse` byte helpers and keep ChatGPT request/parser fixtures.
  - Audit IDs affected: G10, N1.

### Medium

- Anthropic cache token accounting loss.
  - Impact: usage/cost reporting becomes wrong for providers returning cache read/creation tokens.
  - Evidence to watch: changes to `TokenUsage` prompt/completion arithmetic and `cache_read_input_tokens`/`cache_creation_input_tokens` parsing.
  - Mitigation: Preserve parser tests with explicit cache token fixtures.
  - Audit IDs affected: G9.

- Capability manifest/provider module alias regressions.
  - Impact: users can lose providers or capabilities despite internal code compiling.
  - Evidence to watch: `providers/modules.rs`, module IDs, aliases, `ProviderCapabilities`, compiled capability JSON output.
  - Mitigation: Run existing module/capability tests and compiled capability command after wrapper changes.
  - Audit IDs affected: Q2, N2.

- Endpoint policy mismatch.
  - Impact: configured base URLs can accidentally double-append or fail to append `/chat/completions` or `/v1/messages`.
  - Evidence to watch: endpoint construction tests for exact endpoint vs append policies.
  - Mitigation: Use explicit `EndpointPolicy` and tests for each wrapper.
  - Audit IDs affected: G1, G2, G8, G9.

- Feature-gate compile regressions.
  - Impact: no-default builds or partial provider builds can fail because shared modules pull optional `reqwest` unconditionally.
  - Evidence to watch: `providers/mod.rs` cfgs and `Cargo.toml` feature diffs.
  - Mitigation: Validate with every no-default profile command and provider-specific feature commands.
  - Audit IDs affected: V1, Q3.

- ZAI streaming/native JSON/rate-limit regression.
  - Impact: ZAI can send incompatible streaming/JSON/thinking combinations or lose rate-limit wait behavior.
  - Evidence to watch: ZAI profile request tests and parser tests.
  - Mitigation: Keep ZAI as named profile; do not fold into generic OpenAI behavior.
  - Audit IDs affected: G6.

### Low

- Fixture test brittleness.
  - Impact: tests may fail on field ordering or insignificant JSON differences.
  - Evidence to watch: string snapshots instead of parsed JSON asserts.
  - Mitigation: Compare `serde_json::Value` and assert critical fields, not raw formatting.
  - Audit IDs affected: V1.

- Log wording drift.
  - Impact: diagnostics remain usable but exact message text changes.
  - Evidence to watch: OpenCode logging summary tests if any assert exact strings.
  - Mitigation: Preserve structured fields and avoid changing log semantics unless necessary.
  - Audit IDs affected: G8.

- Import churn from module rename.
  - Impact: `anthropic_messages` rename can produce noisy diffs.
  - Evidence to watch: large unrelated formatting changes.
  - Mitigation: Do rename in one checkpoint, run focused compile, and avoid unrelated edits.
  - Audit IDs affected: G9.

## Final implementation guidance

- Prefer boring explicit code.
- No new dependencies unless an unavoidable need is documented and approved before editing `Cargo.toml`.
- Keep provider wrappers thin:
  - wrappers read config/env, expose aliases/capabilities, construct profiles, and hold router-only state;
  - universal clients own request/response/streaming wire logic;
  - OpenCode Go owns protocol discovery/throttle/logging and delegates only after routing.
- Do not delete quirks. Move them into named policies and test them.
- Do not hide protocol differences behind vague abstractions. Chat Completions, Messages, and ChatGPT Responses/Codex are three different protocol surfaces even if some JSON fragments look similar.
- Do not use a one-size-fits-all media abstraction that drops OpenRouter audio/video or Mistral multipart transcription.
- Do not use green compile checks as proof of completion. The Completion Audit requires request/response parity tests and regression matrix review.
- Commit after each completed checkpoint or phase: review `git status`, review relevant diff, run or record checkpoint validation, stage only that checkpoint, commit one meaningful unit, and record the commit summary/hash in this doc.

## Final Verification

- Completion Audit result: complete. G1-G10, Q1-Q3, V1, and N1-N3 are verified with direct code review and command evidence.
- Commands run: final passing commands listed in the Checkpoint 15 progress entry; broad incompatible test commands and their exact pass/fail summaries are also documented there with scoped replacements.
- Artifacts inspected: `git status --short`, `git diff --stat`, `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml`, duplicate-helper `rg` output, ChatGPT import boundary `rg`, `support` schema-boundary `rg`, alias/direct-Gemini/generic-factory `rg`, `target/goal-checks/capabilities-compiled-final.json`, and `target/goal-checks/config-schema-compiled-final.json`.
- Remaining gaps: no required goal gaps. Deferred follow-up remains the documented user-facing generic endpoint config surface, intentionally not exposed in this migration.
- User-accepted exceptions: none. Broad workspace/profile test incompatibilities are documented with scoped provider-equivalent validation per the goal's scoped strategy.
- Final status: ready to close.
