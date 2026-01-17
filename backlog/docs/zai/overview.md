# ZAI-RS Documentation

**Crate**: `zai-rs`
**Version**: `0.1.10`
**Source**: [docs.rs/zai-rs](https://docs.rs/zai-rs/0.1.10/zai_rs/index.html)
**Description**: A type-safe Rust SDK for the Zhipu AI (BigModel) APIs.

## Installation

```toml
[dependencies]
zai-rs = "0.1.10"
tokio = { version = "1", features = ["full"] }
```

## Quick Start

```rust
use zai_rs::model::chat_models::*;
use zai_rs::model::chat_message_types::*;
use zai_rs::model::chat::data::ChatCompletion;
use zai_rs::client::http::HttpClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("ZHIPU_API_KEY")?;
    
    // 1. Select Model
    let model = GLM4_7 {};
    
    // 2. Create Messages
    let messages = TextMessage::user("Hello, who are you?");
    
    // 3. Create Client & Request
    let mut client = ChatCompletion::new(model, messages, api_key);
    
    // 4. Execute
    let response = client.post().await?;
    
    println!("Response: {:?}", response);
    Ok(())
}
```

## Key Modules

- **`client`**: HTTP client configuration and error handling.
- **`model`**: Core data models for Chat, Images, Tools, etc.
- **`file`**: File management (upload, list, delete).

## Features

- **Type Safety**: Compile-time checks for model capabilities.
- **Async/Await**: Built on `tokio`.
- **Streaming**: SSE support for real-time responses.
- **Multimodal**: Support for Text, Vision, and Voice models.
