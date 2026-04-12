# Rust Dependencies

## Cargo.toml

```toml
[package]
name = "telegram-agent"
version = "0.1.0"
edition = "2021"

[dependencies]
# Core
tokio = { version = "1.35", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"

# Cryptography
sha2 = "0.10"

# Date/Time
chrono = { version = "0.4", features = ["serde"] }

# Telegram Bot
teloxide = { version = "0.12", features = ["macros"] }

# Async
futures = "0.3"
async-trait = "0.1"

# Logging
log = "0.4"
env_logger = "0.11"

# Error handling
anyhow = "1.0"
thiserror = "1.0"

# Config
config = "0.14"
toml = "0.8"

# Regular expressions (for content filtering)
regex = "1.10"

# HTTP client (for LLM calls)
reqwest = { version = "0.11", features = ["json"] }

# AI/LLM
genai = "0.1"  # Or your preferred AI library

# Utilities
parking_lot = "0.12"  # Faster Mutex
once_cell = "1.19"

[dev-dependencies]
tokio-test = "0.4"
mockall = "0.12"
```

## Minimal Dependencies

If you want minimal setup:

```toml
[dependencies]
tokio = { version = "1.35", features = ["rt-multi-thread", "sync"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
sha2 = "0.10"
log = "0.4"
anyhow = "1.0"
```

## Alternative AI Libraries

### Using OpenAI

```toml
[dependencies]
async-openai = "0.14"
```

```rust
use async_openai::Client as OpenAIClient;

async fn query_openai(&self, prompt: &str) -> Result<f64> {
    let client = OpenAIClient::new();
    let response = client.chat().create(&request).await?;

    Ok(serde_json::from_str::<LoopDetectionResponse>(
        &response.choices[0].message.content
    )?.confidence)
}
```

### Using Anthropic

```toml
[dependencies]
anthropic = "0.1"
```

```rust
use anthropic::Client as AnthropicClient;

async fn query_anthropic(&self, prompt: &str) -> Result<f64> {
    let client = AnthropicClient::from_env()?;
    let response = client.messages().create(&request).await?;

    Ok(serde_json::from_str::<LoopDetectionResponse>(
        &response.content[0].text
    )?.confidence)
}
```

## Optional Dependencies

### Telemetry

```toml
[dependencies]
opentelemetry = "0.21"
opentelemetry-jaeger = "0.20"
tracing-opentelemetry = "0.22"
```

### Metrics

```toml
[dependencies]
prometheus = "0.13"
```

```rust
use prometheus::{Counter, Histogram};

lazy_static! {
    static ref LOOP_DETECTED_COUNTER: Counter = register_counter!(
        "loop_detection_detected_total",
        "Total number of loops detected"
    ).unwrap();

    static ref LLM_CHECK_DURATION: Histogram = register_histogram!(
        "loop_detection_llm_check_duration_seconds",
        "Duration of LLM loop checks"
    ).unwrap();
}
```

### Testing

```toml
[dev-dependencies]
proptest = "1.4"
criterion = "0.5"
```

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn test_tool_call_loop_detection(count in 0..10usize) {
        let mut detector = ToolCallDetector::new();
        let tool = ToolCall::new("test", json!({}));

        for _ in 0..count {
            detector.check(&tool);
        }

        assert_eq!(detector.loop_detected(), count >= 5);
    }
}
```

## Feature Flags

```toml
[features]
default = ["telegram", "full"]

telegram = ["teloxide"]
full = ["telegram", "telemetry", "metrics"]

telemetry = ["opentelemetry", "opentelemetry-jaeger", "tracing-opentelemetry"]
metrics = ["prometheus"]

minimal = []
```

## Usage

```rust
// Minimal
cargo build --features minimal

// Full
cargo build --features full

// Telegram only
cargo build --features telegram
```

## Build Script (Optional)

If you need build-time configuration:

```rust
// build.rs
fn main() {
    println!("cargo:rerun-if-changed=config.toml");

    let config = std::fs::read_to_string("config.toml").unwrap();
    let parsed: toml::Value = toml::from_str(&config).unwrap();

    if let Some(true) = parsed.get("loop_detection")
        .and_then(|d| d.get("enabled"))
        .and_then(|v| v.as_bool())
    {
        println!("cargo:rustc-cfg=loop_detection_enabled");
    }
}
```

```rust
#[cfg(loop_detection_enabled)]
let detector = LoopDetectionService::new(config);
```
