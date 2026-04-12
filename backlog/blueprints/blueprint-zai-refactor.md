# Blueprint: ZAI (Zhipu AI) Refactoring to SDK (zai-rs)

**Status**: Approved
**Target SDK**: `zai-rs = "0.1.10"`
**Feature**: Full replacement of ZAI provider with native SDK support for GLM-4.7 (Thinking), GLM-4.5-air, and Vision.

> **CRITICAL ARCHITECTURAL CHANGE**:
> Complete migration from manual HTTP requests to `zai-rs` SDK.
> - **Native Tool Calling**: Replaces "JSON-in-text" prompting.
> - **Typed Models**: Uses `GLM4_7` (Main), `GLM4_5_air` (Sub), `GLM4_5v` (Vision).
> - **No Backward Compatibility**: Old `ZaiRequest` structures will be removed.

---

## Phase 1: Environment & Dependencies [ ]

**Goal**: Prepare environment and remove legacy code.

**Resource Context**:
- 📄 `Cargo.toml`
- 📄 `src/llm/providers/zai.rs`
- 📄 `src/llm/providers/zai/stream.rs`
- 📚 **Reference**: [Overview](backlog/docs/zai/overview.md) (SDK Installation)

**Steps**:
1. [ ] **Dependency Update**:
    - Run `cargo add zai-rs@0.1.10`.
    - Verify `tokio` compatibility (project uses 1.x, SDK uses 1.x).
2. [ ] **Legacy Cleanup**:
    - **Delete**: `src/llm/providers/zai/stream.rs` (Manual SSE parser is obsolete).
    - **Strip**: In `src/llm/providers/zai.rs`, remove all `struct ZaiRequest`, `struct ZaiMessage`, etc. Keep only the empty `impl LlmProvider for ZaiProvider` shell.

---

## Phase 2: Type Mapping & Model Factory [ ]

**Goal**: Implement strictly typed model selection and message conversion.

**Resource Context**:
- 📄 `src/llm/providers/zai.rs`
- 📚 **Crate**: `zai_rs::model::chat_models`, `zai_rs::model::chat_message_types`
- 📚 **Reference**: [Chat API](backlog/docs/zai/chat.md) (Message Types & Models)

**Steps**:
1. [ ] **Model Dispatch**:
    - In `chat_completion` / `chat_with_tools`, map string IDs to SDK structs:
        - `MainAgent` / `"glm-4"` -> `GLM4_7` (Supports `reasoning_content`).
        - `SubAgent` / `"glm-4-air"` -> `GLM4_5_air`.
        - `"glm-4v"` -> `GLM4_5v` (Vision only).
2. [ ] **Message Converter**:
    - Implement `fn convert_to_sdk_messages(messages: Vec<LlmMessage>) -> TextMessages`.
    - **System**: Map `Role::System` -> `TextMessage::system`.
    - **User**: Map `Role::User` -> `TextMessage::user`.
    - **Assistant**: Map `Role::Assistant` -> `TextMessage::assistant`.
    - **Tools**: If `Role::Tool` exists, convert to SDK's `ToolMessage` (or equivalent `Role::Tool` handling in SDK).

---

## Phase 3: Native Tool Integration [ ]

**Goal**: Enable native function calling, removing the need for system prompt hacks.

**Resource Context**:
- 📄 `src/llm/providers/zai.rs`
- 📚 **Crate**: `zai_rs::model::tools::Function`
- 📚 **Reference**: [Tools API](backlog/docs/zai/tools.md) (Function & ToolCall)

**Steps**:
1. [ ] **Tool Conversion**:
    - Implement `fn convert_tools(tools: Vec<LlmTool>) -> Vec<Function>`.
    - Use `Function::new(name, description, json_schema)`.
2. [ ] **Request Logic**:
    - In `chat_with_tools`:
        - Instantiate `ChatCompletion::new(model, messages, api_key)`.
        - Call `.set_tools(converted_tools)`.
        - **Critical**: Ensure `GLM4_5v` (Vision) does *not* receive tools (API limitation).
3. [ ] **System Prompt Update**:
    - (Action in `src/agent/prompt/`) Remove specific "You must output JSON" instructions for ZAI provider, as the SDK handles `ToolCall` objects natively.

---

## Phase 4: Streaming & Reasoning (Chain of Thought) [ ]

**Goal**: Implement streaming with native support for the "Thinking" process.

**Resource Context**:
- 📄 `src/llm/providers/zai.rs` (Logic moves here from stream.rs)
- 📚 **Crate**: `zai_rs::model::chat_stream_response`
- 📚 **Reference**: [Chat API](backlog/docs/zai/chat.md#streaming)

**Steps**:
1. [ ] **Stream Execution**:
    - Use `client.stream().await?`.
2. [ ] **Chunk Processing Loop**:
    - Iterate via `while let Some(chunk) = stream.next().await`:
    - **Reasoning**: Check `chunk.choices[0].delta.reasoning_content`.
        - If present, yield `LlmProviderStreamChunk::Reasoning(content)`.
        - *Note*: GLM-4.7 and Air models use this for Chain of Thought.
    - **Content**: Check `chunk.choices[0].delta.content`.
        - Yield `LlmProviderStreamChunk::Content(text)`.
    - **Tools**: Accumulate `tool_calls` from delta.
3. [ ] **Response Assembly**:
    - On stream end, return `LlmResponse` containing the full `content`, `reasoning_content`, and parsed `tool_calls`.

---

## Phase 5: Vision Implementation [ ]

**Goal**: Enable image analysis using `GLM4_5v`.

**Resource Context**:
- 📄 `src/llm/providers/zai.rs`
- 📚 **Crate**: `zai_rs::model::chat_message_types::VisionMessage`
- 📚 **Reference**: [Chat API](backlog/docs/zai/chat.md#vision-messages)
- 📚 **Reference**: [Files API](backlog/docs/zai/files.md) (Future integration)

**Steps**:
1. [ ] **Method Implementation**:
    - Implement `analyze_image(image: Vec<u8>)`.
    - **Force Model**: Always use `GLM4_5v` struct regardless of configured model ID.
2. [ ] **Payload Construction**:
    - Convert `Vec<u8>` to Base64.
    - Create `VisionRichContent::image(base64_string)`.
    - Wrap in `VisionMessage::user`.
3. [ ] **Execution**:
    - Use standard `ChatCompletion` (non-streaming usually sufficient for analysis) and return text description.

---

## Phase 6: Verification [ ]

**Goal**: Ensure type safety and absence of regressions.

**Steps**:
1. [ ] **Check**: `cargo check --package oxide-agent`.
2. [ ] **Lint**: `cargo clippy --package oxide-agent`.
3. [ ] **Manual Test**:
    - Trigger Main Agent (GLM-4.7) -> Verify `reasoning_content` appears in logs.
    - Trigger Vision -> Verify image description.
