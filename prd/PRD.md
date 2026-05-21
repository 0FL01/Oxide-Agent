# PRD: OpenCode Go provider for Oxide Agent Agent Mode

**Status:** implementation-ready context pack for an LLM coding agent  
**Date:** 2026-05-20  
**Repository inspected:** `Oxide-Agent-dev.zip`  
**Primary target:** provider `OpenCode Go`, model `deepseek-v4-flash` / DeepSeek V4 Flash  
**Primary outcome:** reliable Agent Mode support with native tool calls and structured-output flow.

---

## 0. TL;DR for the coding agent

Add a new LLM provider named `opencode-go` that calls OpenCode Go’s OpenAI-compatible Chat Completions endpoint:

```text
https://opencode.ai/zen/go/v1/chat/completions
```

The main model id should be stored in Oxide as raw `deepseek-v4-flash`, not as `opencode-go/deepseek-v4-flash`, because Oxide already separates `provider` and `model_id`. Add defensive normalization that strips `opencode-go/` if a user copies the OpenCode CLI-style id.

The highest-risk part is not the HTTP request itself. The highest-risk part is the exact interaction between:

- OpenAI-compatible `tools[].function.name` schema;
- provider wire tool-call ids vs Oxide internal invocation ids;
- structured JSON output mode;
- existing route failover and history-repair behavior.

Do **not** blindly copy the ZAI SDK tool serializer. The request body for tools must contain this exact shape:

```json
{
  "type": "function",
  "function": {
    "name": "tool_name",
    "description": "...",
    "parameters": { "type": "object", "properties": {} }
  }
}
```

Use native OpenAI-style tool calls when tools are present. Only add `response_format: {"type":"json_object"}` when `json_mode == true` **and** there are no tools in the request. This mirrors the safe pattern already used by ZAI.

---

## 1. External RECON snapshot

### 1.1 OpenCode Go facts that matter

OpenCode Go is documented as a low-cost subscription provider, currently in beta, intended to work “like any other provider” and usable with different coding agents. The public plan messaging says `$5` for the first month and `$10/month` afterwards.

The docs list DeepSeek V4 Flash in the OpenCode Go model lineup with model id:

```text
deepseek-v4-flash
```

The relevant OpenCode Go endpoint is:

```text
POST https://opencode.ai/zen/go/v1/chat/completions
```

The docs also expose a models endpoint:

```text
GET https://opencode.ai/zen/go/v1/models
```

Use the Chat Completions endpoint for MVP. The models endpoint can be a later diagnostic feature; do not block this provider on dynamic model discovery.

OpenCode Go docs show OpenCode CLI-style model ids as `opencode-go/<model-id>`, for example `opencode-go/kimi-k2.6`. In Oxide, provider and model are separate fields, so the canonical Oxide config should be:

```dotenv
AGENT_MODEL_PROVIDER="opencode-go"
AGENT_MODEL_ID="deepseek-v4-flash"
```

Do not require users to put the provider prefix inside `AGENT_MODEL_ID`.

### 1.2 DeepSeek V4 Flash facts that matter

DeepSeek’s official API docs list:

```text
deepseek-v4-flash
deepseek-v4-pro
```

The docs say DeepSeek V4 models expose OpenAI-compatible Chat Completions, support `JSON Output`, support `Tool Calls`, support a `1M` context length, and support max output up to `384K` tokens.

Treat those capabilities as model capability claims, not as an instruction to immediately push Oxide’s internal context budget to one million tokens. Oxide currently defaults the main agent context budget to `200_000` tokens and sub-agent to `64_000`; keep those defaults unless there is a separate compaction/token-budget PR.

### 1.3 Live/negative ecosystem signal

A public GitHub issue against another agent stack reported an `opencode-go/deepseek-v4-flash` failure on tool-enabled requests where the provider rejected the payload with:

```text
tools[0].function: missing field `name`
```

Do not treat that issue as authoritative OpenCode Go behavior. Treat it as a useful regression test: our provider must prove that every outgoing tool payload includes `tools[n].function.name`.

---

## 2. Existing Oxide architecture RECON

### 2.1 Provider location

LLM providers live here:

```text
crates/oxide-agent-core/src/llm/providers/
```

Current provider files/directories include:

```text
chatgpt/
gemini/
groq.rs
minimax/
mistral/
nvidia.rs
openrouter.rs
openrouter/helpers.rs
zai.rs
zai/sdk.rs
protocol_profiles.rs
tool_call_adapter.rs
tool_call_encoder.rs
tool_result_encoder.rs
tool_correlation.rs
```

New provider should be added as either:

```text
crates/oxide-agent-core/src/llm/providers/opencode_go.rs
```

or, if tests/helpers grow:

```text
crates/oxide-agent-core/src/llm/providers/opencode_go/mod.rs
crates/oxide-agent-core/src/llm/providers/opencode_go/helpers.rs
```

For this project’s scale, prefer the single-file module first unless it becomes messy.

### 2.2 Provider trait

Provider contract is in:

```text
crates/oxide-agent-core/src/llm/provider.rs
```

The method that matters for Agent Mode is:

```rust
async fn chat_with_tools<'a>(
    &self,
    request: ChatWithToolsRequest<'a>,
) -> Result<ChatResponse, LlmError>
```

The default implementation returns “Tool calling not supported”, so `OpenCodeGoProvider` must implement it explicitly.

### 2.3 Core LLM types

Core request/response and tool correlation types are in:

```text
crates/oxide-agent-core/src/llm/types.rs
```

Important types:

```rust
ToolDefinition {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

ToolCall {
    id: String,
    tool_call_correlation: Option<ToolCallCorrelation>,
    function: ToolCallFunction,
    is_recovered: bool,
}

ToolCallFunction {
    name: String,
    arguments: String,
}

ChatResponse {
    content: Option<String>,
    tool_calls: Vec<ToolCall>,
    finish_reason: String,
    reasoning_content: Option<String>,
    usage: Option<TokenUsage>,
}

ChatWithToolsRequest<'a> {
    system_prompt: &'a str,
    messages: &'a [Message],
    tools: &'a [ToolDefinition],
    model_id: &'a str,
    max_tokens: u32,
    temperature: Option<f32>,
    json_mode: bool,
}
```

`ToolCallCorrelation` is the guardrail that preserves the difference between Oxide’s stable internal invocation id and the provider’s opaque wire id. For OpenAI-style providers, always use `CHAT_LIKE_TOOL_PROFILE` from:

```text
crates/oxide-agent-core/src/llm/providers/protocol_profiles.rs
```

### 2.4 Closest implementation template

Closest provider template is OpenRouter:

```text
crates/oxide-agent-core/src/llm/providers/openrouter.rs
crates/oxide-agent-core/src/llm/providers/openrouter/helpers.rs
```

Useful patterns from OpenRouter:

- raw `reqwest` JSON request instead of relying on SDK magic;
- `prepare_structured_messages` for OpenAI-chat-style message history;
- `prepare_tools_json` with correct nested `function` shape;
- `parse_tool_calls` that accepts string or object `function.arguments`;
- `CHAT_LIKE_TOOL_PROFILE.inbound_provider_tool_call(...)` for provider wire ids.

Do not reuse OpenRouter’s hardcoded attribution headers. OpenCode Go does not document `HTTP-Referer`, `X-Title`, or `X-OpenRouter-Title` as needed.

### 2.5 Structured-output runner behavior

Structured output schema is built in:

```text
crates/oxide-agent-core/src/agent/prompt/composer.rs
```

The required schema is:

```json
{
  "thought": "Brief description of the solution and step",
  "tool_call": {
    "name": "tool_name",
    "arguments": {}
  },
  "final_answer": "Final answer to the user",
  "awaiting_user_input": {
    "kind": "text|url|file|url_or_file",
    "prompt": "Question or request for the user"
  }
}
```

Exactly one of `tool_call`, `final_answer`, or `awaiting_user_input` must be non-null.

The parser is in:

```text
crates/oxide-agent-core/src/agent/structured_output.rs
```

The agent runner uses this sequence in:

```text
crates/oxide-agent-core/src/agent/runner/execution.rs
```

Current behavior:

1. If provider returns native `response.tool_calls`, runner executes those directly.
2. If no native tool calls and route requires structured output, runner parses `response.content` as the mandatory JSON schema.
3. If structured-output parsing fails repeatedly, runner injects repair guidance and eventually fails.

This is why OpenCode Go should support both native tool calls and JSON mode, but should not force JSON mode when tools are present.

### 2.6 Capabilities gate

Capabilities are declared in:

```text
crates/oxide-agent-core/src/llm/capabilities.rs
```

Current providers declare whether they support:

- tool calling;
- structured output;
- strict vs best-effort tool history.

Recommended OpenCode Go capability:

```rust
"opencode-go" | "opencode_go" => ProviderCapabilities::new(
    ToolHistoryMode::Strict,
    true,
    true,
)
```

Rationale:

- endpoint is OpenAI-chat-compatible;
- OpenAI-style providers often reject orphaned/mismatched tool history;
- the user explicitly cares about agent-mode quality;
- strict validation triggers local repair before bad history reaches the provider.

Add model-specific structured-output logic for DeepSeek V4 ids so future OpenCode Go models do not automatically get overclaimed:

```rust
fn opencode_go_supports_structured_output(model_id: &str) -> bool {
    matches!(
        normalize_opencode_go_model_id(model_id).as_str(),
        "deepseek-v4-flash" | "deepseek-v4-pro"
    )
}
```

If implementing this model-specific override, provider-level `supports_structured_output` can be true only as a default, then overridden for OpenCode Go. Prefer explicit model allowlist.

### 2.7 Registration point

Provider registration is in:

```text
crates/oxide-agent-core/src/llm/client.rs
```

`LlmClient::new` registers providers based on keys from `AgentSettings`. Add OpenCode Go there.

Careful: the current code passes the shared `http_client` by clone to most providers, then moves it into OpenRouter. If adding OpenCode Go after OpenRouter, you will hit a moved-value problem. Register OpenCode Go before OpenRouter or pass `http_client.clone()` everywhere and keep ownership clear.

### 2.8 Config loading point

Config struct is in:

```text
crates/oxide-agent-core/src/config.rs
```

Add:

```rust
pub const OPENCODE_GO_CHAT_TEMPERATURE: f32 = 0.7;

#[serde(default = "default_opencode_go_api_base")]
pub opencode_go_api_base: String,

pub opencode_go_api_key: Option<String>,
```

Default:

```rust
fn default_opencode_go_api_base() -> String {
    "https://opencode.ai/zen/go/v1/chat/completions".to_string()
}
```

Important current mine: `AgentSettings::new()` currently hard-fails if `ZAI_API_KEY` is missing. If the goal is to let users run only OpenCode Go, this unconditional requirement must be relaxed or replaced with route-aware validation. Otherwise the provider will be implemented correctly but unusable without a dummy ZAI key.

Minimal route-aware replacement:

- do not require `ZAI_API_KEY` unconditionally;
- validate that every configured primary route has its provider credential available, or at least validate the active chat/agent routes;
- preserve clear error messages if no configured LLM provider is available.

If that refactor feels too broad for the first PR, document that `ZAI_API_KEY` is still required by current app startup. But that is a bad user experience for a new paid OpenCode Go plan.

---

## 3. Product goal

Enable Oxide Agent to run Agent Mode primarily on:

```text
provider: opencode-go
model: deepseek-v4-flash
```

The implementation should be good enough for real tool loops:

- read/search files;
- call sandbox tools;
- call web/search providers;
- run multiple tool iterations;
- produce final answers through structured-output path when appropriate;
- fail over cleanly when OpenCode Go rate-limits or rejects a request.

---

## 4. Non-goals

Do not implement these in the first PR:

- streaming/SSE support for OpenCode Go;
- dynamic model list syncing into config;
- OpenCode Zen balance management;
- multimodal image/audio/video support;
- provider-specific thinking controls unless OpenCode Go docs explicitly confirm the exact request schema;
- broad LLM provider abstraction refactor;
- rewriting the agent runner.

---

## 5. User-facing config target

Add these env vars to `.env.example`, README, and Russian README.

### 5.1 Provider credentials

```dotenv
OPENCODE_GO_API_KEY="..."
OPENCODE_GO_API_BASE="https://opencode.ai/zen/go/v1/chat/completions"
```

`OPENCODE_GO_API_BASE` should be optional because there is a sane default.

### 5.2 Main agent route

Recommended initial direct route:

```dotenv
AGENT_MODEL_ID="deepseek-v4-flash"
AGENT_MODEL_PROVIDER="opencode-go"
AGENT_MODEL_MAX_OUTPUT_TOKENS=32000
AGENT_MODEL_CONTEXT_WINDOW_TOKENS=200000
AGENT_MODEL_TEMPERATURE=0.7
```

Why not max out at `384000` output tokens or `1000000` context tokens immediately:

- Oxide compaction, token estimates, tool transcript sizes, and Telegram UX are tuned for lower budgets;
- huge completions can hide agent loops and burn subscription budget;
- `deepseek-v4-flash` can support large windows, but application-level defaults should be conservative.

### 5.3 Sub-agent route

```dotenv
SUB_AGENT_MODEL_ID="deepseek-v4-flash"
SUB_AGENT_MODEL_PROVIDER="opencode-go"
SUB_AGENT_MAX_OUTPUT_TOKENS=16000
SUB_AGENT_CONTEXT_WINDOW_TOKENS=64000
```

### 5.4 Weighted failover route example

```dotenv
AGENT_MODEL_ROUTES__0__ID="deepseek-v4-flash"
AGENT_MODEL_ROUTES__0__PROVIDER="opencode-go"
AGENT_MODEL_ROUTES__0__MAX_OUTPUT_TOKENS=32000
AGENT_MODEL_ROUTES__0__CONTEXT_WINDOW_TOKENS=200000
AGENT_MODEL_ROUTES__0__WEIGHT=10

AGENT_MODEL_ROUTES__1__ID="MiniMax-M2.7"
AGENT_MODEL_ROUTES__1__PROVIDER="minimax"
AGENT_MODEL_ROUTES__1__MAX_OUTPUT_TOKENS=64000
AGENT_MODEL_ROUTES__1__CONTEXT_WINDOW_TOKENS=200000
AGENT_MODEL_ROUTES__1__WEIGHT=2

AGENT_MODEL_ROUTES__2__ID="glm-4.7"
AGENT_MODEL_ROUTES__2__PROVIDER="zai"
AGENT_MODEL_ROUTES__2__MAX_OUTPUT_TOKENS=64000
AGENT_MODEL_ROUTES__2__CONTEXT_WINDOW_TOKENS=200000
AGENT_MODEL_ROUTES__2__WEIGHT=1
```

Keep OpenCode Go primary. Use weighted fallback only for outage/rate-limit resilience.

---

## 6. Implementation blueprint

### 6.1 Add provider module export

Edit:

```text
crates/oxide-agent-core/src/llm/providers/mod.rs
```

Add:

```rust
#[allow(missing_docs)]
pub mod opencode_go;

pub use opencode_go::OpenCodeGoProvider;
```

### 6.2 Add provider config

Edit:

```text
crates/oxide-agent-core/src/config.rs
```

Add near provider defaults:

```rust
/// Default temperature used for OpenCode Go chat completions.
pub const OPENCODE_GO_CHAT_TEMPERATURE: f32 = 0.7;
```

Add to `AgentSettings`:

```rust
/// OpenCode Go API key.
pub opencode_go_api_key: Option<String>,

/// OpenCode Go Chat Completions endpoint.
#[serde(default = "default_opencode_go_api_base")]
pub opencode_go_api_base: String,
```

Add default function:

```rust
fn default_opencode_go_api_base() -> String {
    "https://opencode.ai/zen/go/v1/chat/completions".to_string()
}
```

Add tests that env deserialization works:

```rust
OPENCODE_GO_API_KEY=dummy
OPENCODE_GO_API_BASE=https://example.test/v1/chat/completions
```

### 6.3 Register provider

Edit:

```text
crates/oxide-agent-core/src/llm/client.rs
```

Add in `LlmClient::new`:

```rust
if let Some(api_key) = settings.opencode_go_api_key.as_ref() {
    Self::insert_provider(
        &mut providers,
        "opencode-go",
        Arc::new(providers::OpenCodeGoProvider::new_with_client(
            api_key.clone(),
            settings.opencode_go_api_base.clone(),
            http_client.clone(),
        )),
    );

    // Optional alias: protects users who type OPENCODE_GO style provider name.
    Self::insert_provider(
        &mut providers,
        "opencode_go",
        Arc::new(providers::OpenCodeGoProvider::new_with_client(
            api_key.clone(),
            settings.opencode_go_api_base.clone(),
            http_client.clone(),
        )),
    );
}
```

If you dislike two provider instances, create one `Arc` and insert it twice:

```rust
let provider = Arc::new(providers::OpenCodeGoProvider::new_with_client(...));
Self::insert_provider(&mut providers, "opencode-go", provider.clone());
Self::insert_provider(&mut providers, "opencode_go", provider);
```

### 6.4 Add capabilities

Edit:

```text
crates/oxide-agent-core/src/llm/capabilities.rs
```

Add provider branch:

```rust
"opencode-go" | "opencode_go" => {
    ProviderCapabilities::new(ToolHistoryMode::Strict, true, true)
}
```

Then add model-specific override in `provider_capabilities_for_model`:

```rust
} else if is_opencode_go_provider(&model_info.provider) {
    capabilities.supports_structured_output =
        opencode_go_supports_structured_output(&model_info.id);
}
```

Recommended helper:

```rust
fn is_opencode_go_provider(provider: &str) -> bool {
    matches!(provider.trim().to_ascii_lowercase().as_str(), "opencode-go" | "opencode_go")
}

fn normalize_opencode_go_model_id(model_id: &str) -> String {
    model_id
        .trim()
        .strip_prefix("opencode-go/")
        .unwrap_or(model_id.trim())
        .to_ascii_lowercase()
}

fn opencode_go_supports_structured_output(model_id: &str) -> bool {
    matches!(
        normalize_opencode_go_model_id(model_id).as_str(),
        "deepseek-v4-flash" | "deepseek-v4-pro"
    )
}
```

Add tests:

- `opencode_go_capabilities_enable_strict_tools`
- `opencode_go_deepseek_v4_flash_supports_structured_output`
- `opencode_go_unknown_model_does_not_overclaim_structured_output`
- `opencode_go_prefixed_model_id_is_normalized`

### 6.5 Do not add media capabilities

Leave `provider_media_capabilities` unchanged for OpenCode Go, or explicitly map it to false:

```rust
"opencode-go" | "opencode_go" => MediaCapabilities::new(false, false, false),
```

DeepSeek V4 Flash via OpenCode Go is being added as a text/tool agent model. Do not route image/audio/video understanding to it in this PR.

---

## 7. Provider implementation details

### 7.1 New provider skeleton

Create:

```text
crates/oxide-agent-core/src/llm/providers/opencode_go.rs
```

Suggested skeleton:

```rust
use crate::config::OPENCODE_GO_CHAT_TEMPERATURE;
use crate::llm::providers::protocol_profiles::CHAT_LIKE_TOOL_PROFILE;
use crate::llm::support::http::{create_http_client, send_json_request};
use crate::llm::{
    ChatResponse, ChatWithToolsRequest, LlmError, LlmProvider, Message, TokenUsage,
    ToolCall, ToolDefinition,
};
use async_trait::async_trait;
use reqwest::Client as HttpClient;
use serde_json::{json, Value};

pub struct OpenCodeGoProvider {
    http_client: HttpClient,
    api_key: String,
    api_base: String,
}

impl OpenCodeGoProvider {
    #[must_use]
    pub fn new(api_key: String, api_base: String) -> Self {
        Self {
            http_client: create_http_client(),
            api_key,
            api_base,
        }
    }

    #[must_use]
    pub fn new_with_client(api_key: String, api_base: String, http_client: HttpClient) -> Self {
        Self { http_client, api_key, api_base }
    }
}
```

Use `api_base` as the full Chat Completions endpoint, not as a `/v1` base. That avoids ambiguity about whether to append `/chat/completions`.

### 7.2 Model id normalization

Add:

```rust
fn normalize_model_id(model_id: &str) -> &str {
    model_id
        .trim()
        .strip_prefix("opencode-go/")
        .unwrap_or_else(|| model_id.trim())
}
```

If you need case-insensitive stripping, return `String` and lower only for comparison. Do not lower arbitrary model ids before sending; model ids can be case-sensitive in some providers. For DeepSeek ids lowercase is fine, but general helper should preserve user-supplied case after prefix stripping.

### 7.3 Message builder

Either copy the OpenRouter helper behavior or extract a tiny shared helper. For this PR, copying is acceptable and safer than refactoring all providers.

Required OpenAI-style message shapes:

System:

```json
{ "role": "system", "content": "..." }
```

User/assistant text:

```json
{ "role": "user", "content": "..." }
{ "role": "assistant", "content": "..." }
```

Assistant tool call history:

```json
{
  "role": "assistant",
  "content": "",
  "tool_calls": [
    {
      "id": "provider-or-runtime-wire-id",
      "type": "function",
      "function": {
        "name": "tool_name",
        "arguments": "{\"key\":\"value\"}"
      }
    }
  ]
}
```

Tool result history:

```json
{
  "role": "tool",
  "tool_call_id": "same-wire-id-as-assistant-tool-call",
  "content": "tool result text"
}
```

Use:

```rust
CHAT_LIKE_TOOL_PROFILE.encode_tool_call(tool_call)
CHAT_LIKE_TOOL_PROFILE.encode_tool_result(message)
```

Do not manually use `tool_call.id` for outbound history if a provider wire id exists. Use the profile encoder so provider ids are preserved.

### 7.4 Tool schema builder

Required:

```rust
fn prepare_tools_json(tools: &[ToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| json!({
            "type": "function",
            "function": {
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.parameters,
            }
        }))
        .collect()
}
```

Add a unit test that asserts:

```rust
assert_eq!(body["tools"][0]["function"]["name"], json!("read_file"));
```

This is not optional. It directly guards the reported ecosystem failure.

### 7.5 Tool request body

For `chat_with_tools`, build a body like:

```json
{
  "model": "deepseek-v4-flash",
  "messages": [],
  "max_tokens": 32000,
  "temperature": 0.7,
  "stream": false,
  "tools": [],
  "tool_choice": "auto",
  "parallel_tool_calls": true
}
```

Rules:

- include `tools` only when non-empty;
- include `tool_choice: "auto"` only when tools are non-empty;
- include `parallel_tool_calls: true` only when tools are non-empty;
- include `stream: false` explicitly;
- include `response_format` only when `json_mode && tools.is_empty()`.

Suggested helper:

```rust
fn should_use_native_json_mode(json_mode: bool, has_tools: bool) -> bool {
    json_mode && !has_tools
}
```

Then:

```rust
if should_use_native_json_mode(json_mode, !tools.is_empty()) {
    body["response_format"] = json!({ "type": "json_object" });
}
```

### 7.6 Why no `response_format` with tools

Agent Mode has two control paths:

- native provider `tool_calls`, which is best for OpenAI-compatible tools;
- structured JSON content with `tool_call`, which is a fallback/compatibility strategy for models that need strict JSON.

If `response_format` is forced while tools are present, some models will avoid native `tool_calls` and return JSON text instead, while others may reject the request or produce malformed tool JSON. For OpenCode Go + DeepSeek V4 Flash, native tool calls are a base requirement. Do not add friction.

### 7.7 Basic chat completion

Implement `chat_completion` too. It is used outside Agent Mode by other flows.

Request body:

```json
{
  "model": "deepseek-v4-flash",
  "messages": [
    { "role": "system", "content": "..." },
    { "role": "user", "content": "..." }
  ],
  "max_tokens": 32000,
  "temperature": 0.7,
  "stream": false
}
```

Return `choices[0].message.content`.

If content is missing but there are tool calls, that should only happen in `chat_with_tools`, not basic `chat_completion`.

### 7.8 Headers

Use:

```text
Authorization: Bearer <OPENCODE_GO_API_KEY>
Content-Type: application/json
User-Agent: Oxide-Agent/0.1.0
```

`send_json_request` already adds `User-Agent` and handles `Authorization`.

Do not log the API key. Do not include the full request body in `warn`/`info`; existing `trace!` behavior can include prompts and history when explicitly enabled, but avoid adding new high-level logs that leak prompts.

### 7.9 Error handling

Use shared:

```rust
send_json_request(&self.http_client, &self.api_base, &body, Some(&auth), &[]).await?
```

The shared helper already maps HTTP 429 to:

```rust
LlmError::RateLimit { wait_secs, message }
```

That lets the existing agent runner:

- retry with backoff;
- quarantine the route after persistent 429;
- emit `ProviderFailoverActivated` when routes are configured.

For non-429 provider errors, keep enough body text to diagnose schema failures. Do not over-sanitize away the `tools[0].function.name` clue.

### 7.10 Response parser

Parse:

```text
choices[0].message.content
choices[0].message.tool_calls
choices[0].finish_reason
usage.prompt_tokens
usage.completion_tokens
usage.total_tokens
```

`content` can be null when `tool_calls` are present.

Tool-call parser must accept both:

```json
"arguments": "{\"path\":\"file.txt\"}"
```

and:

```json
"arguments": { "path": "file.txt" }
```

If args are an object, serialize them back to a string for `ToolCallFunction.arguments`.

For provider wire id:

```rust
let wire_id = call.get("id").and_then(Value::as_str).filter(|id| !id.trim().is_empty());
```

If present:

```rust
CHAT_LIKE_TOOL_PROFILE.inbound_provider_tool_call(
    wire_id,
    None,
    name.to_string(),
    arguments,
)
```

If absent/empty:

```rust
CHAT_LIKE_TOOL_PROFILE.inbound_uncorrelated_tool_call(name.to_string(), arguments)
```

Do not set `is_recovered = true` for valid native tool calls. Recovery is for malformed text extraction, not normal provider responses.

### 7.11 Reasoning fields

DeepSeek V4 supports thinking/non-thinking modes, and some OpenAI-compatible providers expose `reasoning_content`. For MVP:

- parse `message.reasoning_content` if present;
- optionally parse `message.reasoning` if it is a string;
- never merge reasoning into `content`;
- never send raw reasoning as final user-visible text.

Suggested helper:

```rust
fn parse_reasoning_content(message: &Value) -> Option<String> {
    message
        .get("reasoning_content")
        .and_then(Value::as_str)
        .or_else(|| message.get("reasoning").and_then(Value::as_str))
        .map(ToString::to_string)
        .filter(|s| !s.trim().is_empty())
}
```

Do not add provider-specific `thinking` request parameters unless the OpenCode Go docs explicitly define the schema for this endpoint. The ZAI provider has a `thinking` body because that provider’s SDK/schema supports it; do not cargo-cult it.

---

## 8. Critical mines and how to defuse them

### Mine 1: wrong tool schema

Symptom:

```text
tools[0].function: missing field `name`
```

Cause:

- using a SDK type that serializes to the wrong shape;
- putting `name` at the wrong level;
- sending `function` as a string or incomplete object.

Defuse:

- build tool JSON manually;
- unit test exact request body;
- keep `tools[n].function.name` present for every tool.

### Mine 2: JSON mode fights native tool calls

Symptom:

- model returns JSON text instead of native tool calls;
- provider rejects combined `tools` + `response_format`;
- runner repeatedly fails structured parsing;
- agent stalls despite model being tool-capable.

Defuse:

- `response_format` only for `json_mode && tools.is_empty()`;
- keep native tool calls as first-class path when tools exist.

### Mine 3: model id prefix confusion

OpenCode docs show `opencode-go/deepseek-v4-flash` for OpenCode config. Oxide already has provider and model fields.

Defuse:

- docs tell users to set `provider=opencode-go`, `id=deepseek-v4-flash`;
- provider strips `opencode-go/` defensively before sending.

### Mine 4: unconditional ZAI startup requirement

Current `AgentSettings::new()` hard-fails without `ZAI_API_KEY`. That makes a new OpenCode Go-only setup fail before provider registration.

Defuse:

- replace hardcoded ZAI requirement with route-aware provider credential validation;
- or document the current requirement explicitly as temporary debt.

Recommended fix is route-aware validation.

### Mine 5: moved shared HTTP client

`LlmClient::new` currently moves `http_client` into OpenRouter at the end of provider registration.

Defuse:

- register OpenCode Go before OpenRouter with `http_client.clone()`;
- or clone for OpenRouter too.

### Mine 6: strict tool history

OpenAI-compatible providers usually require a complete assistant-tool-call / tool-result sequence.

Defuse:

- use `ToolHistoryMode::Strict` for OpenCode Go;
- rely on `support::history::validate_tool_history` before sending;
- preserve provider wire ids in assistant history and tool result messages.

### Mine 7: leaking reasoning/planning text

A negative ecosystem report described planning text leaking after fallback in another project.

Defuse in Oxide:

- do not copy `reasoning_content` into `content`;
- do not send raw reasoning as final answer;
- if provider returns `<think>...</think>` in content, add a small sanitizer test and strip it before final output only if observed in live testing;
- keep final-answer path using `final_answer`, not `thought`.

### Mine 8: overclaiming 1M context

DeepSeek V4 docs say 1M context. Oxide defaults main agent to 200K and sub-agent to 64K.

Defuse:

- do not alter global defaults in provider PR;
- let advanced users override `*_CONTEXT_WINDOW_TOKENS` manually;
- keep compaction safeguards unchanged.

### Mine 9: OpenCode Go limits are budget-like, not stable request counts

Docs show estimated request counts by model, but these can change as routing/pricing changes.

Defuse:

- do not hardcode request counts;
- treat 429 as provider signal;
- use existing failover/quarantine logic.

### Mine 10: broad refactor temptation

This repo is explicitly personal/small-scale and rejects over-engineering.

Defuse:

- add a focused provider;
- copy tiny OpenAI-compatible helpers if needed;
- avoid building a generic “all OpenAI-compatible providers” framework unless a second PR proves it is worth it.

---

## 9. Tests to add

### 9.1 Provider body tests

Add tests in `opencode_go.rs` or `opencode_go/tests.rs`.

Required test names:

```rust
build_tool_request_includes_openai_function_schema_name
build_tool_request_omits_response_format_when_tools_present
build_tool_request_sets_response_format_for_json_mode_without_tools
normalizes_opencode_go_prefixed_model_id
basic_chat_body_uses_non_streaming_chat_completions
```

Assertions:

```rust
assert_eq!(body["model"], json!("deepseek-v4-flash"));
assert_eq!(body["stream"], json!(false));
assert_eq!(body["tools"][0]["type"], json!("function"));
assert_eq!(body["tools"][0]["function"]["name"], json!("read_file"));
assert!(body.get("response_format").is_none()); // when tools present
```

For no-tool JSON mode:

```rust
assert_eq!(body["response_format"]["type"], json!("json_object"));
```

### 9.2 Response parser tests

Required test names:

```rust
parses_text_response_with_usage
parses_tool_calls_with_provider_wire_ids
parses_tool_calls_with_object_arguments
parses_null_content_with_tool_calls
rejects_empty_response_without_content_reasoning_or_tools
parses_reasoning_content_separately
```

### 9.3 Capabilities tests

Add to `llm/capabilities.rs` tests:

```rust
opencode_go_capabilities_use_strict_tool_history
opencode_go_deepseek_v4_flash_enables_structured_output
opencode_go_unknown_model_does_not_enable_structured_output
opencode_go_prefixed_model_id_capability_normalizes
opencode_go_has_no_media_capabilities
```

### 9.4 Client registration/config tests

Add tests around `AgentSettings` / `LlmClient`:

```rust
settings_load_opencode_go_api_key_and_base
llm_client_registers_opencode_go_when_key_present
llm_client_accepts_opencode_go_alias
```

If you fix the unconditional ZAI requirement:

```rust
settings_do_not_require_zai_key_when_active_routes_use_opencode_go
settings_error_when_active_provider_key_missing
```

### 9.5 Live smoke test, gated

Do not run live tests in normal CI. Add a manual/gated test or doc command:

```bash
OPENCODE_GO_API_KEY=... \
RUN_LLM_E2E_CHECKS=1 \
cargo test -p oxide-agent-core opencode_go_live_tool_call -- --ignored --nocapture
```

Live smoke should verify:

- simple no-tool JSON-mode response parses;
- one tool call is emitted for a forced fake tool;
- tool result continuation produces final answer.

If there is no existing live-test pattern, put this in docs/manual checklist instead of adding a flaky ignored test.

---

## 10. Manual cURL checks

### 10.1 Basic chat

```bash
curl -sS https://opencode.ai/zen/go/v1/chat/completions \
  -H "Authorization: Bearer $OPENCODE_GO_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "deepseek-v4-flash",
    "stream": false,
    "messages": [
      {"role":"system","content":"You answer briefly."},
      {"role":"user","content":"Say ok as JSON with field final_answer."}
    ],
    "response_format": {"type":"json_object"},
    "max_tokens": 512,
    "temperature": 0.7
  }' | jq .
```

### 10.2 Tool schema check

```bash
curl -sS https://opencode.ai/zen/go/v1/chat/completions \
  -H "Authorization: Bearer $OPENCODE_GO_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "deepseek-v4-flash",
    "stream": false,
    "messages": [
      {"role":"system","content":"Use tools when needed."},
      {"role":"user","content":"What is in /tmp/demo.txt? Use the tool."}
    ],
    "tools": [
      {
        "type": "function",
        "function": {
          "name": "read_file",
          "description": "Read a local file",
          "parameters": {
            "type": "object",
            "properties": {
              "path": {"type":"string"}
            },
            "required": ["path"]
          }
        }
      }
    ],
    "tool_choice": "auto",
    "parallel_tool_calls": true,
    "max_tokens": 1024,
    "temperature": 0.7
  }' | jq .
```

Expected: no `tools[0].function: missing field name` error. Ideally response includes `choices[0].message.tool_calls`.

---

## 11. Documentation edits

### 11.1 `.env.example`

Add provider key near other LLM keys:

```dotenv
# OpenCode Go / Zen subscription provider
OPENCODE_GO_API_KEY=
OPENCODE_GO_API_BASE=https://opencode.ai/zen/go/v1/chat/completions
```

Add commented agent route example:

```dotenv
# OpenCode Go DeepSeek V4 Flash agent route
# AGENT_MODEL_ID="deepseek-v4-flash"
# AGENT_MODEL_PROVIDER="opencode-go"
# AGENT_MODEL_MAX_OUTPUT_TOKENS=32000
# AGENT_MODEL_CONTEXT_WINDOW_TOKENS=200000
```

### 11.2 README.md

Add provider to provider list:

```text
OpenCode Go (`OPENCODE_GO_API_KEY`) — subscription OpenAI-compatible provider. Recommended model for Agent Mode: `deepseek-v4-flash` with provider `opencode-go`. Supports native tool calls and structured JSON for DeepSeek V4 routes.
```

### 11.3 README-ru.md

Russian equivalent:

```text
OpenCode Go (`OPENCODE_GO_API_KEY`) — подписочный OpenAI-compatible провайдер. Рекомендуемая модель для Agent Mode: `deepseek-v4-flash`, провайдер `opencode-go`. Для DeepSeek V4 маршрутов включены native tool calls и structured JSON.
```

### 11.4 AGENTS.md

Update stack/provider line to mention OpenCode Go after implementation.

---

## 12. Acceptance criteria

Implementation is complete when all are true:

- `opencode-go` provider is registered when `OPENCODE_GO_API_KEY` is set.
- `opencode_go` alias works or is intentionally rejected with docs explaining canonical `opencode-go`.
- `deepseek-v4-flash` route can be used as `AGENT_MODEL_PROVIDER=opencode-go` and `AGENT_MODEL_ID=deepseek-v4-flash`.
- Tool request body includes `tools[n].function.name` for every tool.
- Tool response parser preserves provider wire ids through `CHAT_LIKE_TOOL_PROFILE`.
- Native tool calls work in Agent Mode without forcing `response_format` when tools are present.
- No-tool structured-output request uses `response_format: {"type":"json_object"}`.
- Capabilities mark OpenCode Go + DeepSeek V4 Flash as tool-capable and structured-output-capable.
- Capabilities do not mark OpenCode Go as image/audio/video-capable.
- 429 responses map to `LlmError::RateLimit` and trigger existing retry/failover behavior.
- `cargo fmt --all` passes.
- `cargo clippy --workspace --all-targets -- -D warnings` passes.
- `cargo test -p oxide-agent-core` passes.
- Documentation includes env vars and route examples.

---

## 13. Suggested implementation order

1. Add config fields/defaults/tests.
2. Add `opencode_go.rs` provider with request builders and parser helpers as pure functions.
3. Add provider unit tests before wiring network calls.
4. Register provider in `providers/mod.rs` and `LlmClient::new`.
5. Add capabilities and tests.
6. Fix or document unconditional `ZAI_API_KEY` requirement.
7. Update `.env.example`, README, README-ru, AGENTS.md.
8. Run fmt/clippy/tests.
9. Do manual live smoke with real `OPENCODE_GO_API_KEY`.
10. Only after live smoke, consider enabling OpenCode Go in default route examples.

---

## 14. Ready-to-copy prompt for a coding LLM agent

Use this as the implementation prompt.

```text
You are working in the Oxide Agent Rust repository. Implement a focused new LLM provider for Agent Mode: OpenCode Go, with primary model DeepSeek V4 Flash (`deepseek-v4-flash`). Do not perform broad provider refactors.

Goal:
- Add provider key `opencode-go` using endpoint `https://opencode.ai/zen/go/v1/chat/completions`.
- Support high-quality native OpenAI-style tool calls.
- Support structured JSON output for no-tool structured-output requests.
- Preserve existing route failover, retry, history repair, and tool-call id correlation.

Files to inspect first:
- `crates/oxide-agent-core/src/llm/provider.rs`
- `crates/oxide-agent-core/src/llm/types.rs`
- `crates/oxide-agent-core/src/llm/client.rs`
- `crates/oxide-agent-core/src/llm/capabilities.rs`
- `crates/oxide-agent-core/src/llm/providers/openrouter.rs`
- `crates/oxide-agent-core/src/llm/providers/openrouter/helpers.rs`
- `crates/oxide-agent-core/src/llm/providers/protocol_profiles.rs`
- `crates/oxide-agent-core/src/llm/support/http.rs`
- `crates/oxide-agent-core/src/agent/prompt/composer.rs`
- `crates/oxide-agent-core/src/agent/runner/execution.rs`
- `crates/oxide-agent-core/src/config.rs`

Implementation constraints:
- Create `crates/oxide-agent-core/src/llm/providers/opencode_go.rs`.
- Export it from `llm/providers/mod.rs` as `OpenCodeGoProvider`.
- Add `OPENCODE_GO_API_KEY` and optional `OPENCODE_GO_API_BASE` config.
- Register provider in `LlmClient::new` when key is present.
- Use shared reqwest client via `new_with_client`.
- Use raw JSON request building, not ZAI SDK types.
- Use `CHAT_LIKE_TOOL_PROFILE` for assistant tool-call history and tool-result history.
- Tool schema must be OpenAI-compatible nested shape: `tools[n].function.name`, `description`, `parameters`.
- Strip `opencode-go/` prefix from model id before sending, but document raw `deepseek-v4-flash` as canonical Oxide config.
- Set `stream: false`.
- Add `tools`, `tool_choice: "auto"`, and `parallel_tool_calls: true` only when tools are non-empty.
- Add `response_format: {"type":"json_object"}` only when `json_mode == true` and there are no tools.
- Parse `choices[0].message.content`, `choices[0].message.tool_calls`, `finish_reason`, `usage`, and optional `reasoning_content`.
- Do not merge reasoning into content.
- Map empty content with no tool calls and no reasoning to an error.
- Keep provider media capabilities false.
- Use `ToolHistoryMode::Strict` for `opencode-go`.
- Add model-specific structured-output support for `deepseek-v4-flash` and `deepseek-v4-pro`.

Important mine:
- Current config hard-requires `ZAI_API_KEY`. If possible, replace this with route-aware provider credential validation so an OpenCode Go-only setup can boot. If this is too broad, clearly document the remaining startup dependency in README and tests.

Tests required:
- Request body includes `tools[0].function.name`.
- `response_format` is omitted when tools are present.
- `response_format` is present for no-tool JSON mode.
- Prefixed model id `opencode-go/deepseek-v4-flash` normalizes to `deepseek-v4-flash`.
- Parser handles tool calls with provider ids and object/string arguments.
- Capabilities mark OpenCode Go as strict, tool-capable, structured-capable for DeepSeek V4 Flash.
- OpenCode Go has no media capabilities.
- Client registers provider when `OPENCODE_GO_API_KEY` is configured.

Validation commands:
- `cargo fmt --all`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test -p oxide-agent-core`

Do not implement streaming, dynamic model discovery, multimodal support, or broad abstraction refactors in this PR.
```

---

## 15. Source links used for external RECON

- OpenCode Go product page: `https://opencode.ai/go`
- OpenCode Go docs: `https://opencode.ai/docs/opencode-go/`
- DeepSeek API docs: `https://api-docs.deepseek.com/`
- DeepSeek V4 announcement: `https://api-docs.deepseek.com/news/news251201`
- DeepSeek OpenCode integration docs: `https://api-docs.deepseek.com/guides/third_party_platforms/opencode`
- Negative ecosystem issue: `https://github.com/openclaw/openclaw/issues/71683`

