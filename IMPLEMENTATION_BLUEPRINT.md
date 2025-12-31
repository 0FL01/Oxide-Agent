# Implementation Blueprint: Another Chat TG Bot (Python -> Rust)

This blueprint outlines the plan for porting the "Another Chat TG Bot" from Python to Rust. The goal is to achieve feature parity while ensuring type safety, performance, and maintainability.

## Phase 1: Project Setup & Configuration Logic

**Goal**: Initialize the Rust project, set up structured logging with sensitive data redaction, and implement strictly typed configuration loading.

**Resource Context**:
- 🔗 **References**:
    - `src/config.py` (Environment variables, Pydantic settings)
    - `src/utils.py` (Logging filters/redaction logic)
    - `.env` (Environment variable structure)
- 📄 **Target Files**:
    - `src/config.rs`
    - `src/main.rs`
    - `Cargo.toml`
- 📚 **Crate Documentation**:
    - `config::Config` (Settings loading)
    - `tracing_subscriber::filter` (Log filtering)
    - `dotenvy` (Env file loading)

**Steps**:
1. [x] **Setup**: Initialize Cargo workspace and add dependencies (`tokio`, `tracing`, `config`, `dotenvy`, `serde`, `thiserror`).
2. [x] **Context Analysis**: Analyze `src/config.py` to identify all required environment variables and their defaults (e.g., `ALLOWED_USERS`, `MODELS`).
3. [x] **Verify API**: Use `search_documentation_items` for `tracing_subscriber` to find how to implement custom field redaction (equivalent to Python's `SensitiveDataFilter`).
4. [x] **Implementation**: Implement `struct Settings` in `src/config.rs` using the `config` crate. Ensure all fields from Python's `Settings` class are present and correctly typed.
5. [x] **Implementation**: Implement logging setup in `src/main.rs` that mirrors the Python `TokenMaskingFormatter`.
6. [x] **QA**: Run `cargo check` and verify configuration loads correctly from a sample `.env` file.

## Phase 2: Storage Layer (R2/S3)

**Goal**: Port the Cloudflare R2 (S3-compatible) interaction logic, ensuring data compatibility with the existing JSON schema.

**Resource Context**:
- 🔗 **References**:
    - `src/database.py` (R2Storage class, key generation logic)
- 📄 **Target Files**:
    - `src/storage.rs`
- 📚 **Crate Documentation**:
    - `aws_sdk_s3::Client` (S3 Client)
    - `aws_sdk_s3::types::ByteStream` (Data streaming)
    - `serde_json` (JSON serialization)

**Steps**:
1. [x] **Context Analysis**: Analyze `src/database.py` to understand the key paths (`users/{id}/config.json`, `users/{id}/history.json`) and error handling (e.g., `NoSuchKey`).
2. [x] **Verify API**: Use `search_documentation_items` for `aws-sdk-s3` to confirm the method signatures for `get_object`, `put_object`, and `delete_object`.
3. [x] **Implementation**: Create `struct R2Storage` in `src/storage.rs`. Implement methods: `save_json`, `load_json`, `delete_object`.
4. [x] **Compatibility**: Ensure the JSON serialization uses `serde_json` to produce output compatible with existing Python-generated files.
5. [x] **QA**: Write integration tests (or use a mock S3 server) to verify full round-trip (save -> load) compatibility.

## Phase 3: LLM Client Abstraction

**Goal**: Implement a unified client to handle multiple LLM providers (Groq, Mistral, Gemini, OpenRouter), mirroring the Python logic including model-specific fallbacks.

**Resource Context**:
- 🔗 **References**:
    - `src/config.py` (Client initialization)
    - `src/handlers.py` (API calls, retry logic, `retry_with_model_fallback` decorator)
- 📄 **Target Files**:
    - `src/llm/mod.rs`
    - `src/llm/providers.rs`
- 📚 **Crate Documentation**:
    - `async_openai::Client` (OpenAI-compatible client)
    - `reqwest::multipart` (For file uploads to Gemini/Mistral)

**Steps**:
1. [x] **Context Analysis**: Study `src/handlers.py` to understand the exact JSON payloads sent to each provider (system prompts, chat history format) and the fallback logic.
2. [x] **Verify API**: Use `search_documentation_items` for `async-openai` to check compatibility with Groq and OpenRouter endpoints.
3. [x] **Verify API**: Check `reqwest` documentation for handling multipart requests required for Gemini audio/image uploads.
4. [x] **Implementation**: Define a `trait LlmProvider` with methods like `chat_completion`, `transcribe_audio`, `analyze_image`.
5. [x] **Implementation**: Implement the trait for `Groq`, `Mistral`, `Gemini` (via REST), and `OpenRouter`.
6. [x] **Logic Port**: Implement the retry and fallback logic (equivalent to Python's decorators) within the provider abstraction.

## Phase 4: Bot Handlers & Logic

**Goal**: Port the Telegram bot command and message handlers using `teloxide`.

**Resource Context**:
- � **References**:
    - `src/handlers.py` (Command logic: `/start`, `/clear`, text/voice/photo handling)
    - `src/utils.py` (Text formatting helpers)
- 📄 **Target Files**:
    - `src/bot/handlers.rs`
    - `src/bot/state.rs`
- 📚 **Crate Documentation**:
    - `teloxide::dispatching::UpdateHandler` (Handler definition)
    - `teloxide::types::InputFile` (Sending files)

**Steps**:
1. [ ] **Context Analysis**: Analyze `auth` decorator in `handlers.py` to replicate the `ALLOWED_USERS` check middleware.
2. [ ] **Verify API**: Use `search_documentation_items` for `teloxide` to understand the filter system (`dpt.filter(...)`) for routing updates.
3. [ ] **Implementation**: Implement command handlers: `start`, `clear`, `healthcheck`.
4. [ ] **Implementation**: Implement message handlers:
    - Text: Model switching, System prompt editing.
    - Voice: Transcription flow (using `LlmProvider`).
    - Photo: Vision analysis flow (using `LlmProvider`).
5. [ ] **Implementation**: Port `split_long_message` helper from `src/utils.py` to Rust to handle Telegram message length limits.

## Phase 5: Final Integration

**Goal**: Wire all components together in the main application loop and finalize dockerization.

**Resource Context**:
- � **References**:
    - `src/main.py` (Startup sequence, polling logic)
    - `Dockerfile` (Build process)
- 📄 **Target Files**:
    - `src/main.rs`
    - `Dockerfile`
- 📚 **Crate Documentation**:
    - `tokio::signal` (Graceful shutdown)

**Steps**:
1. [ ] **Implementation**: In `src/main.rs`, initialize `Settings`, `R2Storage`, and `LlmClient`.
2. [ ] **Implementation**: Configure the `teloxide` Dispatcher with the handlers from Phase 4.
3. [ ] **Implementation**: Add graceful shutdown handling.
4. [ ] **Dockerization**: Create a multi-stage `Dockerfile` (build vs runtime) to produce a minimal image.
5. [ ] **QA**: Perform full end-to-end testing (manual verification of all bot features).
