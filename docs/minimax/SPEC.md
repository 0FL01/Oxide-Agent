# MiniMax Provider — Implementation SPEC

## Overview

MiniMax OpenAI-compatible API with **native tool calling** and structured output via terminal tool pattern.

## API Configuration

| Parameter | Value |
|-----------|-------|
| Base URL | `https://api.minimax.io/v1` |
| Endpoint | `/chat/completions` |
| Client | `async-openai` with `byot` feature |

## Supported Models

| Model | Description |
|-------|-------------|
| `MiniMax-M2.7` | Main reasoning model |
| `MiniMax-M2.7-highspeed` | Faster variant |
| `MiniMax-M2.5` | Mid-tier |
| `MiniMax-M2.5-highspeed` | Faster variant |
| `MiniMax-M2.1` | Lower tier |
| `MiniMax-M2.1-highspeed` | Faster variant |

---

## Architecture

### Native Tool Calling

MiniMax supports OpenAI-compatible tool calling via `tools` array:

```json
{
  "model": "MiniMax-M2.7",
  "messages": [...],
  "tools": [
    {
      "type": "function",
      "function": {
        "name": "get_weather",
        "description": "Get current weather for a city",
        "parameters": {
          "type": "object",
          "properties": {
            "location": {
              "type": "string",
              "description": "City name"
            }
          },
          "required": ["location"]
        }
      }
    }
  ],
  "temperature": 1.0,
  "max_tokens": 4096,
  "reasoning_split": true
}
```

**Important:** Use `tools`, NOT deprecated `function_call`.

### Response Parsing

Extract tool calls from response:

```rust
let message = &response["choices"][0]["message"];

// Tool calls
let tool_calls: Vec<ToolCall> = message["tool_calls"]
    .as_array()
    .map(|arr| {
        arr.iter()
            .filter_map(|tc| serde_json::from_value(tc.clone()).ok())
            .collect()
    })
    .unwrap_or_default();

// Text content
let content = message["content"].as_str().map(String::from);

// Reasoning (when reasoning_split=true)
let reasoning = message["reasoning_details"].as_str().map(String::from);
```

### ToolCall Structure

```rust
pub struct ToolCall {
    pub id: String,
    pub function: ToolCallFunction,
}

pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,  // JSON string, parse with serde_json::from_str
}
```

### Tool Result Messages

```json
{
  "role": "tool",
  "tool_call_id": "call_abc123",
  "content": "{\"temperature\": 21, \"condition\": \"Sunny\"}"
}
```

---

## Structured Output

### Terminal Tool Pattern

For structured output, use a **terminal tool** that the model must call to return final results:

```rust
const RETURN_FINAL_ANSWER_TOOL: &str = "return_final_answer";
```

Tool definition:

```json
{
  "type": "function",
  "function": {
    "name": "return_final_answer",
    "description": "Return the final structured answer when the task is complete",
    "parameters": {
      "type": "object",
      "properties": {
        "answer": { "type": "string" },
        "confidence": { "type": "number" },
        "used_tools": {
          "type": "array",
          "items": { "type": "string" }
        }
      },
      "required": ["answer", "confidence", "used_tools"]
    }
  }
}
```

### Agent Loop

```
User Message
    ↓
Send chat_with_tools request
    ↓
┌─────────────────────────────────┐
│ Response contains:              │
│   - content (optional)          │
│   - tool_calls (array)          │
│   - reasoning_details (optional)│
└─────────────────────────────────┘
    ↓
If tool_calls not empty:
    ↓
For each tool_call:
    ├─ If name == "return_final_answer":
    │     → Parse arguments → FinalAnswer struct
    │     → Done
    │
    └─ Else:
          → Execute tool
          → Add tool result message to history
          → Loop: send next request
```

### Why Not `response_format`?

MiniMax's OpenAI-compatible docs do **not** document `response_format` for M2.x models. Only `MiniMax-Text-01` supports it on native API. Terminal tool pattern is the safe, production-ready approach.

---

## Message History Contract

MiniMax requires **full assistant message** preserved in history:

```rust
// Always push the complete assistant message
messages.push(assistant.clone());  // includes tool_calls, reasoning_details

// Tool results as separate messages
messages.push(json!({
    "role": "tool",
    "tool_call_id": tool_call_id,
    "content": serde_json::to_string(&tool_result)?
}));
```

---

## Implementation Checklist

### Provider Structure

```rust
use crate::llm::{
    ChatResponse, ChatWithToolsRequest, LlmError, LlmProvider, Message,
    ToolCall, ToolDefinition, TokenUsage,
};
use async_openai::{config::OpenAIConfig, Client};
use async_trait::async_trait;
use reqwest::Client as HttpClient;
use serde::Deserialize;
use serde_json::{json, Value};

pub struct MiniMaxProvider {
    client: Client<OpenAIConfig>,
    http_client: HttpClient,
    api_key: String,
}

impl MiniMaxProvider {
    pub fn new(api_key: String) -> Self {
        let config = OpenAIConfig::new()
            .with_api_key(api_key.clone())
            .with_api_base("https://api.minimax.io/v1");
        Self {
            client: Client::with_config(config),
            http_client: http_utils::create_http_client(),
            api_key,
        }
    }
}
```

### Build Request Body

```rust
fn build_tool_chat_body(
    system_prompt: &str,
    history: &[Message],
    tools: &[ToolDefinition],
    model_id: &str,
    max_tokens: u32,
) -> Value {
    let messages = Self::prepare_messages(system_prompt, history);

    let tools_json: Vec<Value> = tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters
                }
            })
        })
        .collect();

    json!({
        "model": model_id,
        "messages": messages,
        "tools": tools_json,
        "temperature": 1.0,
        "max_tokens": max_tokens,
        "reasoning_split": true
    })
}
```

### Implement `LlmProvider`

```rust
#[async_trait]
impl LlmProvider for MiniMaxProvider {
    async fn chat_with_tools(
        &self,
        request: ChatWithToolsRequest<'_>,
    ) -> Result<ChatResponse, LlmError> {
        let body = Self::build_tool_chat_body(
            request.system_prompt,
            request.messages,
            request.tools,
            request.model_id,
            request.max_tokens,
        );

        let response = self.send_request(body).await?;
        Self::parse_response(response)
    }
}
```

### Parse Response

```rust
fn parse_response(response: Value) -> Result<ChatResponse, LlmError> {
    let choice = response
        .get("choices")
        .and_then(|c| c.get(0))
        .ok_or_else(|| LlmError::ApiError("Missing choices".to_string()))?;

    let message = choice
        .get("message")
        .ok_or_else(|| LlmError::ApiError("Missing message".to_string()))?;

    let content = message
        .get("content")
        .and_then(|c| c.as_str())
        .map(String::from);

    let tool_calls: Vec<ToolCall> = message
        .get("tool_calls")
        .and_then(|tc| tc.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|tc| serde_json::from_value(tc.clone()).ok())
                .collect()
        })
        .unwrap_or_default();

    let reasoning = message
        .get("reasoning_details")
        .and_then(|r| r.as_str())
        .map(String::from);

    let finish_reason = choice
        .get("finish_reason")
        .and_then(|fr| fr.as_str())
        .unwrap_or("stop")
        .to_string();

    Ok(ChatResponse {
        content,
        tool_calls,
        finish_reason,
        reasoning_content: reasoning,
        usage: Self::parse_usage(&response),
    })
}
```

---

## Constraints

| Parameter | Value |
|-----------|-------|
| `temperature` | `(0.0, 1.0]` |
| `n` | only `1` |
| `presence_penalty` | ignored |
| `frequency_penalty` | ignored |
| `logit_bias` | ignored |
| `function_call` | **not supported** (use `tools`) |
| Image/audio input | not supported |

---

## Registration

Add to `LlmClient`:

```rust
// llm/mod.rs
pub struct LlmClient {
    // ...
    minimax: Option<providers::MiniMaxProvider>,
}

// Constructor
impl LlmClient {
    pub fn new(settings: &crate::config::AgentSettings) -> Self {
        Self {
            // ...
            minimax: settings.minimax_api_key.as_ref().map(|k| {
                providers::MiniMaxProvider::new(k.clone())
            }),
            // ...
        }
    }
}

// is_provider_available
if name.eq_ignore_ascii_case("minimax") {
    return self.minimax.is_some();
}

// get_provider
"minimax" => self.minimax.as_ref().map(|p| p as &dyn LlmProvider),
```

Add to `providers/mod.rs`:

```rust
pub mod minimax;
pub use minimax::MiniMaxProvider;
```

---

## Error Handling

```rust
async fn send_request(&self, body: Value) -> Result<Value, LlmError> {
    let url = "https://api.minimax.io/v1/chat/completions";

    let response = self
        .http_client
        .post(url)
        .header("Authorization", format!("Bearer {}", self.api_key))
        .json(&body)
        .send()
        .await
        .map_err(|e| LlmError::NetworkError(e.to_string()))?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let wait_secs = parse_retry_after(response.headers());
            return Err(LlmError::RateLimit {
                wait_secs,
                message: error_text,
            });
        }

        return Err(LlmError::ApiError(format!(
            "MiniMax API error: {status} - {error_text}"
        )));
    }

    response
        .json::<Value>()
        .await
        .map_err(|e| LlmError::JsonError(e.to_string()))
}
```
