# Tools & Function Calling

**Source**: `zai_rs::model::tools`

## Defining Functions

Use the `Function` struct to define tools that the model can call.

```rust
use zai_rs::model::tools::Function;
use serde_json::json;

let weather_tool = Function::new(
    "get_weather",
    "Get current weather for a location",
    json!({
        "type": "object",
        "properties": {
            "location": { "type": "string", "description": "City name" },
            "unit": { "type": "string", "enum": ["celsius", "fahrenheit"] }
        },
        "required": ["location"]
    })
);
```

## Using Tools in Chat

Pass the tools to the `ChatCompletion` request.

```rust
// Note: Check specific API on ChatCompletion for adding tools.
// Usually involves a method like `.set_tools(vec![...])` or similar.
```

## Handling Tool Calls

The model response will contain `ToolCall` objects if it decides to call a function.

```rust
use zai_rs::model::chat_message_types::ToolCall;

// In the response processing logic:
if let Some(tool_calls) = response.choices[0].message.tool_calls {
    for call in tool_calls {
        println!("Function: {}", call.function.name);
        println!("Args: {}", call.function.arguments);
    }
}
```

## Web Search & Retrieval

The crate also supports `WebSearch` and `Retrieval` tools.

```rust
use zai_rs::model::tools::{WebSearch, Retrieval};

// Configure web search
let web_search = WebSearch {
    enable: true,
    search_query: Some("Rust lang".to_string()),
    ..Default::default()
};
```
