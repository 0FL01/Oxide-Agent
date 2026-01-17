# Chat Completion

**Source**: `zai_rs::model::chat` & `zai_rs::model::chat_models`

## Models

Models are defined as structs in `zai_rs::model::chat_models`.

| Model Struct | Description | Capabilities |
| :--- | :--- | :--- |
| `GLM4_7` | Advanced reasoning | Text, Thinking |
| `GLM4_6` | Advanced reasoning | Text, Thinking |
| `GLM4_5_air` | Fast & Efficient reasoning | Text, Thinking |
| `GLM4_5_flash` | Fast & Efficient | Text, Thinking |
| `GLM4_5v` | Vision-enabled | Text, Vision |
| `GLM4_voice` | Voice-enabled | Text, Voice |

## Message Types

Messages are defined in `zai_rs::model::chat_message_types`.

### Text Messages
```rust
use zai_rs::model::chat_message_types::{TextMessage, TextMessages};

// Single message
let msg = TextMessage::user("Hello");

// Collection (Builder pattern)
let msgs = TextMessages::new(TextMessage::system("You are a helper."))
    .add_message(TextMessage::user("Count to 10."));
```

### Vision Messages
```rust
use zai_rs::model::chat_message_types::{VisionMessage, VisionRichContent};

let image = VisionRichContent::image("https://example.com/cat.jpg");
let msg = VisionMessage::user(image);
```

## Request Configuration

The `ChatCompletion` struct is the main entry point.

```rust
use zai_rs::model::chat::data::ChatCompletion;

let mut request = ChatCompletion::new(model, messages, api_key);

// Optional configuration (if available via builder methods - check docs for specific setters)
// request.set_temperature(0.7); 
// request.set_top_p(0.9);
```

## Streaming

To use streaming, ensure you handle the response as a stream.

```rust
// Assuming the client supports a stream method or returns a streamable response
// Check `zai_rs::model::chat_stream_response` for details.
```
