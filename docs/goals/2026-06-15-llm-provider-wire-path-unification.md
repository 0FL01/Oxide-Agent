# Goal: unify LLM providers into two reqwest wire paths

Date started: 2026-06-15
Status: active
Codex goal: `/goal Implement docs/goals/2026-06-15-llm-provider-wire-path-unification.md until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update the doc after each meaningful verification, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.`
Source spec: user-provided LLM provider unification spec, plus repo-local goal documentation rules
Goal doc owner: Codex
Last updated: 2026-06-15 17:30 UTC+3

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
  - Status: pending
  - Evidence collected:

- G2: Eliminate OpenAI-compatible request-building duplication.
  - Source: user spec, current duplication list.
  - Requirement: OpenAI-style message building, tool schema generation, JSON mode, thinking/reasoning request policies, and media request parts are centralized in `chat_completions::request` with profile policies.
  - Acceptance: `openai_base`, `openrouter`, and OpenCode Go OpenAI branch no longer maintain separate OpenAI-style message/tool request builders except small wrapper glue.
  - Evidence required: targeted `rg` review and unit tests for generic, Mistral, ZAI, OpenRouter, and OpenCode request bodies.
  - Status: pending
  - Evidence collected:

- G3: Eliminate OpenAI-compatible response/tool/usage parsing duplication.
  - Source: user spec, current duplication list.
  - Requirement: Tool call parsing, usage parsing, reasoning/content chunk parsing, and error/rate-limit parsing policy are centralized in `chat_completions::response`.
  - Acceptance: `openai_base`, `openrouter`, and OpenCode Go OpenAI branch parse responses through shared code with profile-specific policies.
  - Evidence required: parser unit tests for tool calls, empty IDs, Mistral mapped IDs, ZAI chunk arrays, OpenRouter usage, OpenCode usage, and error envelopes.
  - Status: pending
  - Evidence collected:

- G4: Centralize Chat Completions streaming without merging ChatGPT Responses SSE.
  - Source: user spec, ChatGPT exclusion.
  - Requirement: Chat Completions SSE parsing moves to `chat_completions::streaming`; low-level byte decoding may move to `support::sse`; ChatGPT keeps its Responses event parser.
  - Acceptance: ZAI/generic Chat Completions stream tests pass and ChatGPT Responses stream tests still use ChatGPT-specific event handling.
  - Evidence required: streaming unit tests and code review proving ChatGPT event parser remains separate.
  - Status: pending
  - Evidence collected:

- G5: Preserve Mistral behavior.
  - Source: user spec, Mistral anti-regression list.
  - Requirement: Preserve `MistralNineAlnum`, `MistralStrict`, 9-character alphanumeric IDs, bidirectional `ToolCallIdMapper`, tool result name inclusion if currently required, reasoning effort model matching, multipart audio transcription, and strict tool history behavior.
  - Acceptance: Existing Mistral tests pass and new fixture tests prove request/response parity.
  - Evidence required: Mistral-specific unit tests plus `cargo test -p oxide-agent-core --no-default-features --features llm-mistral --lib` or broader equivalent.
  - Status: pending
  - Evidence collected:

- G6: Preserve ZAI behavior.
  - Source: user spec, ZAI anti-regression list.
  - Requirement: Preserve `thinking.type` enabled/disabled policy, streaming unless native JSON mode, native JSON mode interaction with thinking, GLM model-gated structured output, ZAI flush-time rate-limit parsing, and reasoning/content chunk-array parsing.
  - Acceptance: ZAI profile tests pass with request and parser fixtures.
  - Evidence required: ZAI request, response, streaming, and rate-limit unit tests.
  - Status: pending
  - Evidence collected:

- G7: Preserve OpenRouter behavior.
  - Source: user spec, OpenRouter anti-regression list.
  - Requirement: Preserve app attribution headers, Bearer auth, exact endpoint, `provider.require_parameters = true` when tools are used, OpenRouter rate-limit metadata parsing, image/audio/video content parts, and model/media capability gating.
  - Acceptance: OpenRouter wrapper delegates to `chat_completions` and all current OpenRouter tests pass with new profile tests.
  - Evidence required: request-body tests for text/tools/image/audio/video, rate-limit parser tests, capability tests, and feature-gated compile.
  - Status: pending
  - Evidence collected:

- G8: Preserve OpenCode Go dynamic routing.
  - Source: user spec, OpenCode Go anti-regression list.
  - Requirement: Preserve dynamic model protocol discovery, `OpenAiChatCompletions`, `AnthropicMessages`, `Unknown`, model ID normalization, provider prefix stripping, separate `api_base` and `api_base_messages`, adaptive throttle, cooldown/recovery, request/response logging summaries, image model gating, OpenCode reasoning effort, and Anthropic fallback prefix/profile.
  - Acceptance: `opencode_go` remains a router wrapper and its OpenAI/Anthropic branches delegate to shared paths without flattening into one fixed protocol.
  - Evidence required: discovery tests, protocol routing tests, throttle tests, request tests for both branches, and profile-feature compile commands.
  - Status: pending
  - Evidence collected:

- G9: Preserve Anthropic/MiniMax Messages behavior.
  - Source: user spec, Anthropic/MiniMax anti-regression list.
  - Requirement: Preserve top-level `system`, folding history system messages into system field, messages array without system role, assistant content blocks, `tool_use`, `tool_result`, grouping consecutive tool results into one user message, `tool_choice auto` only when tools exist, `input_schema`, stop reason mapping, thinking/redacted thinking parsing, cache token accounting, empty tool ID fallback prefixes, `x-api-key`, and `anthropic-version`.
  - Acceptance: `messages` is the reusable module and `anthropic`/MiniMax wrapper delegates to it.
  - Evidence required: request/response unit tests moved from `anthropic_messages`, new wrapper integration tests, and feature-gated compile.
  - Status: pending
  - Evidence collected:

- G10: Preserve ChatGPT special provider behavior.
  - Source: user spec, ChatGPT exclusion.
  - Requirement: ChatGPT remains separate with OAuth auth manager, account ID header, unsupported parameter retry/removal, Responses/Codex body, GPT-5 temperature suppression, GPT-5 reasoning effort, `truncation: auto`, stream diagnostics, Responses SSE parser, and `function_call` call ID handling.
  - Acceptance: ChatGPT code does not depend on `chat_completions` request/response/profile; only optional `support::sse` low-level decoder usage is allowed.
  - Evidence required: ChatGPT request/parser tests and code review of imports.
  - Status: pending
  - Evidence collected:

- Q1: Preserve shared tool abstractions and correlation integrity.
  - Source: user spec, shared abstractions list.
  - Requirement: Do not bypass `ToolProtocolProfile`, `ProviderToolCallAdapter`, `ProviderToolCallEncoder`, `ProviderToolResultEncoder`, `ToolCorrelationNormalizer`, `ToolProtocol`, `ToolTransport`, or `ToolCallCorrelation`.
  - Acceptance: Assistant tool calls, tool results, retry history repair, and provider wire ID mapping remain equivalent or stricter.
  - Evidence required: tool history/correlation tests covering Chat Completions, Mistral mapped IDs, Anthropic tool results, OpenCode branches, and ChatGPT special path.
  - Status: pending
  - Evidence collected:

- Q2: Preserve aliases, module IDs, env behavior, and capability manifests.
  - Source: user spec, non-goals and registry behavior.
  - Requirement: Initial migration must not remove legacy provider aliases or module capability behavior.
  - Acceptance: Existing provider module tests pass; compiled capability output remains compatible except for documented internal module path names.
  - Evidence required: provider module tests and capability command output.
  - Status: pending
  - Evidence collected:

- Q3: No new crates and no unnecessary user-visible behavior changes.
  - Source: user spec, non-goals.
  - Requirement: The migration uses existing dependencies and preserves external request behavior.
  - Acceptance: `Cargo.toml` has no new dependencies and fixture tests prove request/response parity.
  - Evidence required: `git diff -- crates/oxide-agent-core/Cargo.toml Cargo.toml` review and fixture tests.
  - Status: pending
  - Evidence collected:

- V1: Required validation commands pass or feature incompatibilities are documented with scoped alternatives.
  - Source: user spec, validation commands.
  - Requirement: Run the validation contract commands below.
  - Acceptance: Commands pass, or any feature-incompatible workspace tests are narrowed with exact command output and documented rationale.
  - Evidence required: command output summaries in Progress Log and Final Verification.
  - Status: pending
  - Evidence collected:

- N1: Do not rewrite ChatGPT OAuth/Codex into the two paths.
  - Source: user spec, explicit exclusion.
  - Must preserve: Separate `providers/chatgpt` path.
  - Evidence required: code review and ChatGPT tests.
  - Status: pending
  - Evidence collected:

- N2: Do not remove legacy provider aliases during initial migration.
  - Source: user spec, explicit non-goal.
  - Must preserve: Existing provider IDs, aliases, and env names.
  - Evidence required: provider module tests.
  - Status: pending
  - Evidence collected:

- N3: Do not add new crates.
  - Source: user spec, explicit non-goal.
  - Must preserve: Existing dependency set unless a later user-approved exception is recorded.
  - Evidence required: Cargo diff review.
  - Status: pending
  - Evidence collected:

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

## Progress Log

- 2026-06-15 17:30 UTC+3: Goal document created.
  - Changed: Added migration goal contract and checkpoint plan.
  - Evidence: Repository audit identified current provider files, existing docs/goals convention, provider feature gates, and validation commands.
  - Commands: Documentation-only checkpoint; no Cargo validation run yet.
  - Audit IDs updated: none verified; all implementation audit items pending.
  - Next: Start Checkpoint 1 by adding fixture/parity tests and recording duplication baseline.

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

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
