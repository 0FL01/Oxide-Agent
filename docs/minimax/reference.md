# MiniMax API Reference

## Base URL

```
https://api.minimax.io/v1
```

## Authentication

Header: `Authorization: Bearer {api_key}`

## Request Format

### Headers

```
Content-Type: application/json
Authorization: Bearer {MINIMAX_API_KEY}
```

### Body Schema

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `model` | string | Yes | Model ID (e.g., `MiniMax-M2.7`) |
| `messages` | array | Yes | Array of message objects |
| `tools` | array | No | Array of tool definitions |
| `temperature` | number | No | Range: `(0.0, 1.0]`, default: `1.0` |
| `max_tokens` | integer | No | Maximum output tokens |
| `reasoning_split` | boolean | No | Enable reasoning extraction |

### Messages

```json
{
  "role": "system|user|assistant|tool",
  "content": "string",
  "tool_call_id": "string (for tool role)",
  "tool_calls": [
    {
      "id": "call_abc123",
      "type": "function",
      "function": {
        "name": "tool_name",
        "arguments": "{\"arg1\": \"value\"}"
      }
    }
  ]
}
```

### Tool Definition

```json
{
  "type": "function",
  "function": {
    "name": "string",
    "description": "string",
    "parameters": {
      "type": "object",
      "properties": {...},
      "required": [...]
    }
  }
}
```

## Response Format

### Success

```json
{
  "id": "chatcmpl-xxx",
  "object": "chat.completion",
  "created": 1234567890,
  "model": "MiniMax-M2.7",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "content": "text response",
        "tool_calls": [...],
        "reasoning_details": "thinking trace"
      },
      "finish_reason": "stop|tool_calls"
    }
  ],
  "usage": {
    "prompt_tokens": 100,
    "completion_tokens": 50,
    "total_tokens": 150
  }
}
```

### Error

```json
{
  "error": {
    "message": "Error description",
    "type": "invalid_request_error",
    "code": "invalid_api_key"
  }
}
```

## Status Codes

| Code | Meaning |
|------|---------|
| 200 | Success |
| 400 | Bad request |
| 401 | Invalid API key |
| 429 | Rate limit exceeded |
| 500 | Internal error |

## Rate Limit Headers

- `Retry-After`: Seconds to wait
- `X-RateLimit-Limit`: Request limit
- `X-RateLimit-Remaining`: Remaining requests

## Supported Parameters Summary

| Parameter | Supported | Notes |
|-----------|-----------|-------|
| `model` | Yes | |
| `messages` | Yes | |
| `tools` | Yes | Use instead of `function_call` |
| `tool_choice` | ? | Not documented |
| `temperature` | Yes | `(0.0, 1.0]` |
| `max_tokens` | Yes | |
| `reasoning_split` | Yes | Provider-specific |
| `n` | No | Only `1` |
| `presence_penalty` | No | Ignored |
| `frequency_penalty` | No | Ignored |
| `logit_bias` | No | Ignored |
| `response_format` | No | Only `MiniMax-Text-01` |

## Client Code

```rust
use async_openai::{config::OpenAIConfig, Client};

let config = OpenAIConfig::new()
    .with_api_key(api_key)
    .with_api_base("https://api.minimax.io/v1");

let client = Client::with_config(config);
```

Requires: `async-openai = { version = "0.33", features = ["byot"] }`
