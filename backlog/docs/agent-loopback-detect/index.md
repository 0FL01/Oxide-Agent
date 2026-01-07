# Loop Detection Implementation Documentation

## Index

### [01-architecture.md](01-architecture.md)

Overview of the multi-layered loop detection system architecture. Describes the
three detection strategies (tool call, content, LLM-based), integration points,
state management, and reset conditions.

### [02-tool-call-detection.md](02-tool-call-detection.md)

Implementation details for detecting consecutive identical tool calls. Includes
algorithm, step-by-step logic, Rust code examples, and edge cases. Threshold: 5
identical calls with SHA256 hashing.

### [03-content-detection.md](03-content-detection.md)

Streaming text analysis for repetitive patterns. Covers sliding window
algorithm, chunk hashing (50 chars), threshold (10 repetitions), code block
exclusion, and Rust implementation with regex patterns.

### [04-llm-detection.md](04-llm-detection.md)

LLM-based periodic loop detection. Dual-model verification (Flash → Pro), system
prompt, JSON schema, adaptive check intervals (5-15 turns), confidence threshold
(0.9), and history preparation.

### [05-data-structures.md](05-data-structures.md)

All Rust data structures and types. Includes `LoopType` enum, `StreamEvent`
enum, tracker structs (ToolCall, Content, LLM), main service struct, LLM
response structures, and configuration types.

### [06-implementation.md](06-implementation.md)

Complete `LoopDetectionService` implementation with API. Includes main service
methods, integration example for Telegram bot, event emitter pattern, logging
handler, testing utilities, performance considerations, and error handling.

### [07-configuration.md](07-configuration.md)

Configuration management. Model selection (Flash/Pro), threshold values, timing
parameters, content analysis settings, environment variables, TOML configuration
file format, and Telegram-specific configuration.

### [08-telegram-integration.md](08-telegram-integration.md)

Telegram bot specific integration. Core pattern, command handlers
(disable/enable), progress updates, retry logic, notification templates, user
session management, and Telegram event handling.

### [09-dependencies.md](09-dependencies.md)

Complete Cargo.toml dependencies. Core dependencies (tokio, serde, sha2),
optional AI libraries (OpenAI, Anthropic), feature flags, optional
telemetry/metrics, and build script examples.

### [10-testing.md](10-testing.md)

Testing strategy and examples. Unit tests for tool call and content detection,
integration tests, mock LLM for testing, property-based tests with proptest,
performance benchmarks, test fixtures, and test utilities.
