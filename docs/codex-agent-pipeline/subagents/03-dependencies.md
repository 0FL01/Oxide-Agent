# 3. Dependencies

```toml
[package]
name = "agent-framework"
version = "0.1.0"
edition = "2024"

[dependencies]
tokio = { version = "1", features = ["full"] }
tokio-util = "0.7"
async-trait = "0.1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
uuid = { version = "1", features = ["v4", "serde"] }
futures = "0.3"
tracing = "0.1"
anyhow = "1"
```
