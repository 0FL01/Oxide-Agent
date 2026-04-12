# Context Compaction in Codex-RS

## Overview

Context compaction is a mechanism for managing the limited context window of language models in codex-rs. When the conversation history becomes too large, the system replaces old messages with a concise summary while preserving key information.

## Quick Start

```rust
// Manual compaction trigger
let session: Arc<Session> = ...;
Codex::compact(&session, "subscription-id".to_string()).await;

// Configuration (config.toml)
[model]
auto_compact_token_limit = 100000  // Trigger compaction at 100k tokens
compact_prompt = "Summarize the conversation for continuation..."
```

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                      Session                                 │
│  ┌────────────────────────────────────────────────────┐    │
│  │              ContextManager                         │    │
│  │  - history: Vec<ResponseItem>                     │    │
│  │  - token_info: TokenUsageInfo                     │    │
│  │  - reference_context_item: Option<TurnContextItem>│    │
│  └────────────────────────────────────────────────────┘    │
│                                                             │
│  ┌────────────────────────────────────────────────────┐    │
│  │           TurnContext                              │    │
│  │  - model_info: ModelInfo                           │    │
│  │  - auto_compact_token_limit: Option<i64>          │    │
│  │  - compact_prompt: Option<String>                  │    │
│  └────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────┘
                            │
                            │ Triggers:
                            │ 1. Pre-turn (total_tokens >= limit)
                            │ 2. Mid-turn (token_limit_reached && needs_follow_up)
                            │ 3. Model switch (old_context > new_context)
                            │ 4. Manual (thread/compact request)
                            ▼
        ┌───────────────────────────────┐
        │ should_use_remote_compact_task│
        │ (true for OpenAI)             │
        └───────────────────────────────┘
                    │           │
           true     │           │   false
                    ▼           ▼
    ┌──────────────────┐   ┌──────────────────┐
    │ compact_remote   │   │ compact (local)  │
    │                  │   │                  │
    │ API: /v1/        │   │ Local stream     │
    │ responses/compact│   │ request to model │
    └──────────────────┘   └──────────────────┘
                    │           │
                    └─────┬─────┘
                          ▼
              ┌───────────────────────┐
              │ replace_compacted_    │
              │ history()             │
              └───────────────────────┘
                          │
                          ▼
              ┌───────────────────────┐
              │ New history:          │
              │ - Initial Context     │
              │ - User Messages       │
              │ - Summary Prefix +   │
              │   Assistant Output    │
              └───────────────────────┘
```

## Data Types Reference

### Core Types

#### `ResponseItem`

Main enum representing all items in conversation history.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseItem {
    /// Standard message (user, assistant, developer)
    Message {
        id: Option<String>,
        role: String,  // "user", "assistant", "developer"
        content: Vec<ContentItem>,
        end_turn: Option<bool>,
        phase: Option<MessagePhase>,
    },

    /// Model reasoning/thinking
    Reasoning {
        id: String,
        summary: Vec<ReasoningItemReasoningSummary>,
        content: Option<Vec<ReasoningItemContent>>,
        encrypted_content: Option<String>,
    },

    /// Local shell command execution
    LocalShellCall {
        id: Option<String>,
        call_id: Option<String>,
        status: LocalShellStatus,
        action: LocalShellAction,
    },

    /// Function call from model
    FunctionCall {
        id: Option<String>,
        name: String,
        arguments: String,  // JSON string
        call_id: String,
    },

    /// Function call result
    FunctionCallOutput {
        call_id: String,
        output: FunctionCallOutputPayload,
    },

    /// Custom tool call
    CustomToolCall {
        id: Option<String>,
        status: Option<String>,
        call_id: String,
        name: String,
        input: String,
    },

    /// Custom tool output
    CustomToolCallOutput {
        call_id: String,
        output: FunctionCallOutputPayload,
    },

    /// Web search call
    WebSearchCall {
        id: Option<String>,
        status: Option<String>,
        action: Option<WebSearchAction>,
    },

    /// Image generation
    ImageGenerationCall {
        id: String,
        status: String,
        revised_prompt: Option<String>,
        result: String,
    },

    /// For undo functionality
    GhostSnapshot {
        ghost_commit: GhostCommit,
    },

    /// Compaction marker (encrypted summary)
    Compaction {
        encrypted_content: String,
    },

    /// Fallback
    Other,
}
```

#### `CompactedItem`

Result of compaction operation.

```rust
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CompactedItem {
    /// Generated summary text (empty for remote compaction)
    pub message: String,
    /// New compacted history to replace old
    pub replacement_history: Option<Vec<ResponseItem>>,
}
```

#### `ContextCompactionItem`

UI tracking item for compaction process.

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ContextCompactionItem {
    pub id: String,  // UUID
}

impl ContextCompactionItem {
    pub fn new() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
        }
    }
}
```

#### `InitialContextInjection`

Controls context injection behavior.

```rust
pub(crate) enum InitialContextInjection {
    /// Mid-turn: insert before last user message (model training requirement)
    BeforeLastUserMessage,
    /// Pre-turn: context will be injected after compaction
    DoNotInject,
}
```

### Supporting Types

#### `TurnContext`

Context for a single turn.

```rust
pub(crate) struct TurnContext {
    pub model_info: ModelInfo,
    pub personality: Personality,
    pub truncation_policy: TruncationPolicy,
    pub auto_compact_token_limit: Option<i64>,
    pub compact_prompt: Option<String>,
    pub reference_context_item: Option<TurnContextItem>,
    pub session_telemetry: SessionTelemetry,
    // ... other fields
}
```

#### `ModelInfo`

Model configuration and capabilities.

```rust
pub struct ModelInfo {
    pub slug: String,
    pub provider: ModelProviderInfo,
    pub context_window: i64,
    pub supports_reasoning_summaries: bool,
    pub auto_compact_token_limit: Option<i64>,
    pub input_modalities: Vec<InputModality>,
    pub output_modalities: Vec<OutputModality>,
}
```

## Configuration

### Config File (config.toml)

```toml
[model]
# Trigger compaction when token usage exceeds this limit
auto_compact_token_limit = 100000

# Custom prompt for compaction (optional)
compact_prompt = """
Summarize the key points of this conversation for continuation:
- Current task and progress
- Important decisions made
- Pending items
- Relevant context
"""

# Context window size
context_window = 200000
```

### Environment Variables

```bash
# Override auto-compact limit
CODEX_AUTO_COMPACT_TOKEN_LIMIT=100000

# Override context window
CODEX_CONTEXT_WINDOW=200000
```

### Runtime Configuration

```rust
// Programmatic configuration
let mut config = Config::default();
config.model_auto_compact_token_limit = Some(100000);
config.model_compact_prompt = Some("Custom summary prompt...".to_string());
```

## Key Components

| Component             | Path                                      | Purpose                                            |
| --------------------- | ----------------------------------------- | -------------------------------------------------- |
| **Local Compaction**  | `core/src/compact.rs`                     | Local compression via stream request to model      |
| **Remote Compaction** | `core/src/compact_remote.rs`              | Remote compression via `/v1/responses/compact` API |
| **Task Wrapper**      | `core/src/tasks/compact.rs`               | CompactTask wrapper for task runner                |
| **Integration**       | `core/src/codex.rs`                       | Session integration, triggers                      |
| **Tests**             | `app-server/tests/suite/v2/compaction.rs` | E2E tests                                          |

## Constants

```rust
// Maximum tokens for user messages in compacted history
pub const COMPACT_USER_MESSAGE_MAX_TOKENS: usize = 20_000;

// Prefix for summary messages (from templates/compact/summary_prefix.md)
pub const SUMMARY_PREFIX: &str = "Another language model started to solve this problem...";
```

## Triggers for Compaction

### 1. Pre-turn Compaction

**Location**: `codex.rs:run_pre_sampling_compact()`

Occurs before starting a new turn when `total_usage_tokens >= auto_compact_token_limit`.

```rust
if run_pre_sampling_compact(&sess, &turn_context)
    .await
    .is_err()
{
    error!("Failed to run pre-sampling compact");
    return None;
}
```

**Conditions**:

- Auto-compact: `total_usage_tokens >= auto_compact_token_limit`
- Model switch: Previous model's context > new model's context

### 2. Mid-turn Compaction

**Location**: `codex.rs:5498-5510`

Occurs during a turn when token limit is reached and follow-up is needed.

```rust
if token_limit_reached && needs_follow_up {
    if run_auto_compact(
        &sess,
        &turn_context,
        InitialContextInjection::BeforeLastUserMessage,  // Key difference!
    )
    .await
    .is_err()
    {
        return None;
    }
    continue;  // Retry sampling after compaction
}
```

**Key difference**: Uses `InitialContextInjection::BeforeLastUserMessage` - initial context must be injected **before** the last user message.

### 3. Manual Compaction

**Location**: `codex.rs:4665`

Triggered via API or user request.

```rust
pub async fn compact(sess: &Arc<Session>, sub_id: String) {
    let turn_context = sess.new_default_turn_with_sub_id(sub_id).await;

    sess.spawn_task(
        Arc::clone(&turn_context),
        vec![UserInput::Text {
            text: turn_context.compact_prompt().to_string(),
            text_elements: Vec::new(),
        }],
        CompactTask,
    )
    .await;
}
```

## Algorithm

### Original History (too large)

```
├─ [Old User Messages...]
├─ [Tool Calls & Outputs...]
├─ [Recent User Messages]
└─ [Recent Assistant Messages]
```

### After COMPACTION

```
├─ [Initial Context] (permissions, env)
├─ [Selected User Messages] (last ~20k tokens)
└─ [Summary Prefix + Assistant Summary]
```

### Selection Between Local and Remote

```rust
pub(crate) fn should_use_remote_compact_task(provider: &ModelProviderInfo) -> bool {
    provider.is_openai()  // Only for OpenAI
}
```

## API Reference

### Public Methods

#### `Codex::compact()`

Manual compaction trigger.

```rust
impl Codex {
    /// Trigger manual compaction for a session
    pub async fn compact(sess: &Arc<Session>, sub_id: String) {
        // Implementation
    }
}
```

**Parameters:**

- `sess: &Arc<Session>` - Session to compact
- `sub_id: String` - Subscription ID for event streaming

**Events Emitted:**

- `ContextCompactionStarted` - When compaction begins
- `ContextCompactionCompleted` - When compaction finishes
- `Warning` - With accuracy degradation warning

#### `Session::replace_compacted_history()`

Replace history with compacted version.

```rust
impl Session {
    pub(crate) async fn replace_compacted_history(
        &self,
        new_history: Vec<ResponseItem>,
        reference_context_item: Option<TurnContextItem>,
        compacted_item: CompactedItem,
    ) {
        // Implementation
    }
}
```

#### `Session::recompute_token_usage()`

Recalculate token usage after compaction.

```rust
impl Session {
    pub(crate) async fn recompute_token_usage(&self, turn_context: &TurnContext) {
        // Implementation
    }
}
```

### Internal Functions

#### `run_compact_task_inner()`

Core local compaction logic.

```rust
async fn run_compact_task_inner(
    sess: Arc<Session>,
    turn_context: Arc<TurnContext>,
    input: Vec<UserInput>,
    initial_context_injection: InitialContextInjection,
) -> CodexResult<()>
```

#### `run_remote_compact_task_inner_impl()`

Core remote compaction logic.

```rust
async fn run_remote_compact_task_inner_impl(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    initial_context_injection: InitialContextInjection,
) -> CodexResult<()>
```

#### `build_compacted_history()`

Build compacted history from components.

```rust
pub(crate) fn build_compacted_history(
    initial_context: Vec<ResponseItem>,
    user_messages: &[String],
    summary_text: &str,
) -> Vec<ResponseItem>
```

#### `insert_initial_context_before_last_real_user_or_summary()`

Insert context at correct position.

```rust
pub(crate) fn insert_initial_context_before_last_real_user_or_summary(
    compacted_history: Vec<ResponseItem>,
    initial_context: Vec<ResponseItem>,
) -> Vec<ResponseItem>
```

## Local Compaction Implementation

### Core Function

```rust
async fn run_compact_task_inner(
    sess: Arc<Session>,
    turn_context: Arc<TurnContext>,
    input: Vec<UserInput>,
    initial_context_injection: InitialContextInjection,
) -> CodexResult<()> {
    // 1. Create ContextCompactionItem for UI tracking
    let compaction_item = TurnItem::ContextCompaction(ContextCompactionItem::new());
    sess.emit_turn_item_started(&turn_context, &compaction_item).await;

    // 2. Form input: user input + current history
    let mut history = sess.clone_history().await;
    history.record_items(
        &[initial_input_for_turn.into()],
        turn_context.truncation_policy,
    );

    // 3. Launch LLM request with retry logic
    let mut client_session = sess.services.model_client.new_session();
    let mut retries = 0;

    loop {
        let turn_input = history.clone().for_prompt(&turn_context.model_info.input_modalities);
        let prompt = Prompt {
            input: turn_input,
            base_instructions: sess.get_base_instructions().await,
            personality: turn_context.personality,
            ..Default::default()
        };

        match drain_to_completed(&sess, &turn_context, &mut client_session, &prompt).await {
            Ok(()) => break,  // Success
            Err(CodexErr::ContextWindowExceeded) if turn_input.len() > 1 => {
                // Trim from beginning to preserve cache (prefix-based)
                history.remove_first_item();
                truncated_count += 1;
                continue;
            }
            Err(e) if retries < max_retries => {
                // Backoff and retry
                retries += 1;
                tokio::time::sleep(backoff(retries)).await;
                continue;
            }
            Err(e) => return Err(e),
        }
    }

    // 4. Collect data for new history
    let history_snapshot = sess.clone_history().await;
    let history_items = history_snapshot.raw_items();

    // Last assistant message + summary prefix
    let summary_suffix = get_last_assistant_message_from_turn(history_items).unwrap_or_default();
    let summary_text = format!("{SUMMARY_PREFIX}\n{summary_suffix}");

    // Collect user messages (filtered: exclude summary messages)
    let user_messages = collect_user_messages(history_items);

    // 5. Build compacted history
    let mut new_history = build_compacted_history(Vec::new(), &user_messages, &summary_text);

    // 6. Insert initial context if needed (mid-turn case)
    if matches!(initial_context_injection, InitialContextInjection::BeforeLastUserMessage) {
        let initial_context = sess.build_initial_context(turn_context.as_ref()).await;
        new_history = insert_initial_context_before_last_real_user_or_summary(new_history, initial_context);
    }

    // 7. Save ghost snapshots (for /undo)
    let ghost_snapshots: Vec<ResponseItem> = history_items
        .iter()
        .filter(|item| matches!(item, ResponseItem::GhostSnapshot { .. }))
        .cloned()
        .collect();
    new_history.extend(ghost_snapshots);

    // 8. Replace history
    let reference_context_item = match initial_context_injection {
        InitialContextInjection::DoNotInject => None,
        InitialContextInjection::BeforeLastUserMessage => Some(turn_context.to_turn_context_item()),
    };

    let compacted_item = CompactedItem {
        message: summary_text.clone(),
        replacement_history: Some(new_history.clone()),
    };

    sess.replace_compacted_history(new_history, reference_context_item, compacted_item).await;
    sess.recompute_token_usage(&turn_context).await;

    // 9. Send warning to user
    let warning = EventMsg::Warning(WarningEvent {
        message: "Heads up: Long threads and multiple compactions can cause the model to be less accurate. Start a new thread when possible to keep threads small and targeted.".to_string(),
    });
    sess.send_event(&turn_context, warning).await;

    sess.emit_turn_item_completed(&turn_context, compaction_item).await;
    Ok(())
}
```

### Building Compacted History with Limits

```rust
fn build_compacted_history_with_limit(
    mut history: Vec<ResponseItem>,
    user_messages: &[String],
    summary_text: &str,
    max_tokens: usize,
) -> Vec<ResponseItem> {
    // 1. Select user messages in reverse order (newest first)
    let mut selected_messages: Vec<String> = Vec::new();
    if max_tokens > 0 {
        let mut remaining = max_tokens;
        for message in user_messages.iter().rev() {
            if remaining == 0 {
                break;
            }
            let tokens = approx_token_count(message);
            if tokens <= remaining {
                selected_messages.push(message.clone());
                remaining = remaining.saturating_sub(tokens);
            } else {
                // Truncate if message is too long
                let truncated = truncate_text(message, TruncationPolicy::Tokens(remaining));
                selected_messages.push(truncated);
                break;
            }
        }
        selected_messages.reverse();  // Restore order
    }

    // 2. Add selected user messages
    for message in &selected_messages {
        history.push(ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText { text: message.clone() }],
            end_turn: None,
            phase: None,
        });
    }

    // 3. Add summary as last user message
    let summary_text = if summary_text.is_empty() {
        "(no summary available)".to_string()
    } else {
        summary_text.to_string()
    };

    history.push(ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputText { text: summary_text }],
        end_turn: None,
        phase: None,
    });

    history
}
```

### Inserting Initial Context

```rust
pub(crate) fn insert_initial_context_before_last_real_user_or_summary(
    mut compacted_history: Vec<ResponseItem>,
    initial_context: Vec<ResponseItem>,
) -> Vec<ResponseItem> {
    let mut last_user_or_summary_index = None;
    let mut last_real_user_index = None;

    // Find insertion point (iterate in reverse)
    for (i, item) in compacted_history.iter().enumerate().rev() {
        let Some(TurnItem::UserMessage(user)) = crate::event_mapping::parse_turn_item(item) else {
            continue;
        };
        last_user_or_summary_index.get_or_insert(i);
        if !is_summary_message(&user.message()) {
            last_real_user_index = Some(i);
            break;
        }
    }

    let last_compaction_index = compacted_history
        .iter()
        .enumerate()
        .rev()
        .find_map(|(i, item)| matches!(item, ResponseItem::Compaction { .. }).then_some(i));

    let insertion_index = last_real_user_index
        .or(last_user_or_summary_index)
        .or(last_compaction_index);

    // Insert at computed index or append if none found
    if let Some(insertion_index) = insertion_index {
        compacted_history.splice(insertion_index..insertion_index, initial_context);
    } else {
        compacted_history.extend(initial_context);
    }

    compacted_history
}
```

## Remote Compaction Implementation

### Core Function

```rust
pub(crate) async fn run_inline_remote_auto_compact_task(
    sess: Arc<Session>,
    turn_context: Arc<TurnContext>,
    initial_context_injection: InitialContextInjection,
) -> CodexResult<()> {
    run_remote_compact_task_inner(&sess, &turn_context, initial_context_injection).await?;
    Ok(())
}

async fn run_remote_compact_task_inner_impl(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    initial_context_injection: InitialContextInjection,
) -> CodexResult<()> {
    let compaction_item = TurnItem::ContextCompaction(ContextCompactionItem::new());
    sess.emit_turn_item_started(turn_context, &compaction_item).await;

    // 1. Clone history and trim if needed
    let mut history = sess.clone_history().await;
    let base_instructions = sess.get_base_instructions().await;

    let deleted_items = trim_function_call_history_to_fit_context_window(
        &mut history,
        turn_context.as_ref(),
        &base_instructions,
    );

    // 2. Save ghost snapshots
    let ghost_snapshots: Vec<ResponseItem> = history
        .raw_items()
        .iter()
        .filter(|item| matches!(item, ResponseItem::GhostSnapshot { .. }))
        .cloned()
        .collect();

    // 3. Form prompt and call remote API
    let prompt = Prompt {
        input: history.for_prompt(&turn_context.model_info.input_modalities),
        tools: vec![],
        parallel_tool_calls: false,
        base_instructions,
        personality: turn_context.personality,
        output_schema: None,
    };

    let mut new_history = sess
        .services
        .model_client
        .compact_conversation_history(
            &prompt,
            &turn_context.model_info,
            &turn_context.session_telemetry,
        )
        .await?;

    // 4. Post-processing: filtering and inserting initial context
    new_history = process_compacted_history(
        sess.as_ref(),
        turn_context.as_ref(),
        new_history,
        initial_context_injection,
    )
    .await;

    if !ghost_snapshots.is_empty() {
        new_history.extend(ghost_snapshots);
    }

    // 5. Replace history (same as local)
    let reference_context_item = match initial_context_injection {
        InitialContextInjection::DoNotInject => None,
        InitialContextInjection::BeforeLastUserMessage => Some(turn_context.to_turn_context_item()),
    };

    let compacted_item = CompactedItem {
        message: String::new(),  // Remote doesn't return message text
        replacement_history: Some(new_history.clone()),
    };

    sess.replace_compacted_history(new_history, reference_context_item, compacted_item).await;
    sess.recompute_token_usage(turn_context).await;

    sess.emit_turn_item_completed(turn_context, compaction_item).await;
    Ok(())
}
```

### Post-Processing Remote Result

```rust
pub(crate) async fn process_compacted_history(
    sess: &Session,
    turn_context: &TurnContext,
    mut compacted_history: Vec<ResponseItem>,
    initial_context_injection: InitialContextInjection,
) -> Vec<ResponseItem> {
    // 1. Get initial context if needed
    let initial_context = if matches!(
        initial_context_injection,
        InitialContextInjection::BeforeLastUserMessage
    ) {
        sess.build_initial_context(turn_context).await
    } else {
        Vec::new()
    };

    // 2. Filter elements (remove unwanted from remote)
    compacted_history.retain(should_keep_compacted_history_item);

    // 3. Insert initial context
    insert_initial_context_before_last_real_user_or_summary(compacted_history, initial_context)
}

fn should_keep_compacted_history_item(item: &ResponseItem) -> bool {
    match item {
        // Remove developer messages (stale/duplicated instructions)
        ResponseItem::Message { role, .. } if role == "developer" => false,

        // Remove non-user-content user messages (session prefix/instruction wrappers)
        ResponseItem::Message { role, .. } if role == "user" => {
            matches!(
                crate::event_mapping::parse_turn_item(item),
                Some(TurnItem::UserMessage(_))
            )
        }

        // Keep assistant messages (future remote models may emit them)
        ResponseItem::Message { role, .. } if role == "assistant" => true,

        // Remove other Message types
        ResponseItem::Message { .. } => false,

        // Keep Compaction items
        ResponseItem::Compaction { .. } => true,

        // Remove everything else
        ResponseItem::Reasoning { .. }
        | ResponseItem::LocalShellCall { .. }
        | ResponseItem::FunctionCall { .. }
        | ResponseItem::FunctionCallOutput { .. }
        | ResponseItem::CustomToolCall { .. }
        | ResponseItem::CustomToolCallOutput { .. }
        | ResponseItem::WebSearchCall { .. }
        | ResponseItem::ImageGenerationCall { .. }
        | ResponseItem::GhostSnapshot { .. }
        | ResponseItem::Other => false,
    }
}
```

## Comparison: Local vs Remote Compaction

| Feature                       | Local Compaction                               | Remote Compaction                                                           |
| ----------------------------- | ---------------------------------------------- | --------------------------------------------------------------------------- |
| **Trigger**                   | `should_use_remote_compact_task()` = false     | `should_use_remote_compact_task()` = true                                   |
| **Implementation**            | `compact::run_compact_task_inner()`            | `compact_remote::run_remote_compact_task_inner_impl()`                      |
| **API**                       | Standard `/responses` stream                   | Special `/responses/compact` (unary)                                        |
| **Execution**                 | LLM receives full prompt and generates summary | Model receives history and returns compacted transcript                     |
| **Result**                    | `summary_text` + `user_messages`               | Full `Vec<ResponseItem>` from model                                         |
| **Trimming**                  | During retries (remove_first_item)             | `trim_function_call_history_to_fit_context_window()`                        |
| **Filtering**                 | Collects only user messages + summary          | `should_keep_compacted_history_item()` (filters developer, system messages) |
| **Initial Context Injection** | Both supported                                 | Both supported                                                              |
| **Error Handling**            | Retry with backoff for stream errors           | Logs failure with detailed breakdown                                        |
| **Warning**                   | "Heads up: Long threads..." warning            | No warning                                                                  |
| **CompactItem.message**       | Filled with summary text                       | Empty (`String::new()`)                                                     |
| **Ghost Snapshots**           | Preserved from history                         | Preserved from history                                                      |

## Usage Examples

### Basic Manual Compaction

```rust
use codex_core::{Codex, Session};
use std::sync::Arc;

async fn compact_session(session: Arc<Session>) {
    // Trigger manual compaction
    Codex::compact(&session, "my-subscription".to_string()).await;

    // Compaction events will be emitted:
    // - ContextCompactionStarted
    // - ContextCompactionCompleted
    // - Warning (optional)
}
```

### Handling Compaction Events

```rust
use codex_core::protocol::{EventMsg, ThreadItem};

fn handle_event(event: EventMsg) {
    match event {
        EventMsg::ContextCompactionStarted { thread_id, item } => {
            println!("Compaction started for thread {}", thread_id);
            if let ThreadItem::ContextCompaction { id } = item {
                println!("Compaction ID: {}", id);
            }
        }
        EventMsg::ContextCompactionCompleted { thread_id, item } => {
            println!("Compaction completed for thread {}", thread_id);
        }
        EventMsg::Warning { message } => {
            println!("Warning: {}", message);
        }
        _ => {}
    }
}
```

### Custom Compaction Prompt

```rust
use codex_core::Config;

fn create_config_with_custom_prompt() -> Config {
    let mut config = Config::default();

    config.model_compact_prompt = Some(r#"
Summarize this conversation for continuation by another AI assistant:

CONTEXT:
- Project: {project_name}
- Current task: {task_description}

REQUIRED SECTIONS:
1. PROGRESS: What has been accomplished
2. DECISIONS: Key choices made and why
3. BLOCKERS: Any issues or constraints
4. NEXT STEPS: Clear actionable items

Be concise but include all technical details needed to continue.
"#.to_string());

    config
}
```

### Checking Token Usage

```rust
use codex_core::Session;

async fn check_and_compact_if_needed(session: Arc<Session>) {
    let token_info = session.get_token_usage().await;

    if let Some(limit) = session.config().model_auto_compact_token_limit {
        let usage = token_info.total_token_usage.tokens_in_context_window();

        if usage >= limit {
            println!("Token usage {} exceeds limit {}, compacting...", usage, limit);
            Codex::compact(&session, "auto-compact".to_string()).await;
        }
    }
}
```

## Test Examples

### Local Auto-Compaction Test

```rust
#[tokio::test]
async fn auto_compaction_local_emits_started_and_completed_items() -> Result<()> {
    // Setup
    let server = responses::start_mock_server().await;

    // Mock responses: first two turns consume tokens, third is compaction
    let sse1 = responses::sse(vec![
        responses::ev_assistant_message("m1", "FIRST_REPLY"),
        responses::ev_completed_with_tokens("r1", 70_000),  // 70k tokens
    ]);
    let sse2 = responses::sse(vec![
        responses::ev_assistant_message("m2", "SECOND_REPLY"),
        responses::ev_completed_with_tokens("r2", 330_000),  // 330k tokens
    ]);
    // Compaction request
    let sse3 = responses::sse(vec![
        responses::ev_assistant_message("m3", "LOCAL_SUMMARY"),
        responses::ev_completed_with_tokens("r3", 200),
    ]);
    // Final reply after compaction
    let sse4 = responses::sse(vec![
        responses::ev_assistant_message("m4", "FINAL_REPLY"),
        responses::ev_completed_with_tokens("r4", 120),
    ]);
    responses::mount_sse_sequence(&server, vec![sse1, sse2, sse3, sse4]).await;

    // Config with AUTO_COMPACT_LIMIT = 1_000
    write_mock_responses_config_toml(
        codex_home.path(),
        &server.uri(),
        &BTreeMap::default(),
        AUTO_COMPACT_LIMIT,  // 1_000 tokens
        None,  // use_local_compaction (not OpenAI)
        "mock_provider",
        COMPACT_PROMPT,
    )?;

    // Start thread and send turns
    let thread_id = start_thread(&mut mcp).await?;
    for message in ["first", "second", "third"] {
        send_turn_and_wait(&mut mcp, &thread_id, message).await?;
    }

    // Verify compaction started/completed events
    let started = wait_for_context_compaction_started(&mut mcp).await?;
    let completed = wait_for_context_compaction_completed(&mut mcp).await?;

    assert_eq!(started.thread_id, thread_id);
    assert_eq!(completed.thread_id, thread_id);

    // Check IDs match
    let ThreadItem::ContextCompaction { id: started_id } = started.item else {
        unreachable!("started item should be context compaction");
    };
    let ThreadItem::ContextCompaction { id: completed_id } = completed.item else {
        unreachable!("completed item should be context compaction");
    };
    assert_eq!(started_id, completed_id);
}
```

### Remote Auto-Compaction Test

```rust
#[tokio::test]
async fn auto_compaction_remote_emits_started_and_completed_items() -> Result<()> {
    // Setup
    let server = responses::start_mock_server().await;

    // Responses for normal turns
    let sse1 = responses::sse(vec![
        responses::ev_assistant_message("m1", "FIRST_REPLY"),
        responses::ev_completed_with_tokens("r1", 70_000),
    ]);
    let sse2 = responses::sse(vec![
        responses::ev_assistant_message("m2", "SECOND_REPLY"),
        responses::ev_completed_with_tokens("r2", 330_000),
    ]);
    let sse3 = responses::sse(vec![
        responses::ev_assistant_message("m3", "FINAL_REPLY"),
        responses::ev_completed_with_tokens("r3", 120),
    ]);
    let responses_log = responses::mount_sse_sequence(&server, vec![sse1, sse2, sse3]).await;

    // Mock compact endpoint response
    let compacted_history = vec![
        ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText {
                text: "REMOTE_COMPACT_SUMMARY".to_string(),
            }],
            end_turn: None,
            phase: None,
        },
        ResponseItem::Compaction {
            encrypted_content: "ENCRYPTED_COMPACTION_SUMMARY".to_string(),
        },
    ];
    let compact_mock = responses::mount_compact_json_once(
        &server,
        serde_json::json!({ "output": compacted_history }),
    )
    .await;

    // Config with remote compaction enabled
    write_mock_responses_config_toml(
        codex_home.path(),
        &server.uri(),
        &BTreeMap::default(),
        REMOTE_AUTO_COMPACT_LIMIT,  // 200_000
        Some(true),  // is_openai = true
        "openai",
        COMPACT_PROMPT,
    )?;

    // Setup OpenAI auth
    write_chatgpt_auth(
        codex_home.path(),
        ChatGptAuthFixture::new("access-chatgpt").plan_type("pro"),
        AuthCredentialsStoreMode::File,
    )?;

    // Start thread and send turns
    let thread_id = start_thread(&mut mcp).await?;
    for message in ["first", "second", "third"] {
        send_turn_and_wait(&mut mcp, &thread_id, message).await?;
    }

    // Verify compaction events
    let started = wait_for_context_compaction_started(&mut mcp).await?;
    let completed = wait_for_context_compaction_completed(&mut mcp).await?;

    assert_eq!(started.thread_id, thread_id);
    assert_eq!(completed.thread_id, thread_id);
    assert_eq!(started_id, completed_id);

    // Verify compact endpoint was called
    let compact_requests = compact_mock.requests();
    assert_eq!(compact_requests.len(), 1);
    assert_eq!(compact_requests[0].path(), "/v1/responses/compact");

    // Verify normal responses were called 3 times
    let response_requests = responses_log.requests();
    assert_eq!(response_requests.len(), 3);
}
```

### Unit Test: Context Insertion

```rust
#[test]
fn insert_initial_context_before_last_real_user_or_summary_keeps_summary_last() {
    // Test: initial context should be inserted before summary, not after
    let compacted_history = vec![
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "older user".to_string(),
            }],
            end_turn: None,
            phase: None,
        },
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "latest user".to_string(),
            }],
            end_turn: None,
            phase: None,
        },
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: format!("{SUMMARY_PREFIX}\nsummary text"),
            }],
            end_turn: None,
            phase: None,
        },
    ];

    let initial_context = vec![ResponseItem::Message {
        id: None,
        role: "developer".to_string(),
        content: vec![ContentItem::InputText {
            text: "fresh permissions".to_string(),
        }],
        end_turn: None,
        phase: None,
    }];

    let refreshed = insert_initial_context_before_last_real_user_or_summary(
        compacted_history,
        initial_context,
    );

    // Expected: context before "latest user", summary stays last
    let expected = vec![
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "older user".to_string(),
            }],
            end_turn: None,
            phase: None,
        },
        ResponseItem::Message {
            id: None,
            role: "developer".to_string(),
            content: vec![ContentItem::InputText {
                text: "fresh permissions".to_string(),
            }],
            end_turn: None,
            phase: None,
        },
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "latest user".to_string(),
            }],
            end_turn: None,
            phase: None,
        },
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: format!("{SUMMARY_PREFIX}\nsummary text"),
            }],
            end_turn: None,
            phase: None,
        },
    ];
    assert_eq!(refreshed, expected);
}
```

### Unit Test: Token Limit Truncation

```rust
#[test]
fn build_token_limited_compacted_history_truncates_overlong_user_messages() {
    let max_tokens = 16;
    let big = "word ".repeat(200);

    let history = super::build_compacted_history_with_limit(
        Vec::new(),
        std::slice::from_ref(&big),
        "SUMMARY",
        max_tokens,
    );

    assert_eq!(history.len(), 2);  // truncated message + summary

    // Verify truncation marker
    let truncated_message = &history[0];
    let ResponseItem::Message { role, content, .. } = truncated_message else {
        panic!("expected Message");
    };
    assert_eq!(role, "user");

    let truncated_text = content_items_to_text(content).unwrap_or_default();
    assert!(
        truncated_text.contains("tokens truncated"),
        "expected truncation marker in truncated user message"
    );
    assert!(
        !truncated_text.contains(&big),
        "truncated user message should not include the full oversized user text"
    );
}
```

## Final History Structure After Compaction

### Pre-turn Case (InitialContextInjection::DoNotInject)

```
┌─────────────────────────────────────────────────┐
│ 1. [Selected User Message 1]                    │  (oldest, may be truncated)
│ 2. [Selected User Message 2]                    │
│ 3. [Selected User Message 3]                    │
│ ...                                            │
│ N. [Selected User Message K]                    │  (newest, within 20k tokens)
│ N+1. [Summary Prefix + Assistant Output]       │  ← Last item
│ N+2. [Ghost Snapshots] (for /undo)            │
└─────────────────────────────────────────────────┘
```

### Mid-turn Case (InitialContextInjection::BeforeLastUserMessage)

```
┌─────────────────────────────────────────────────┐
│ 1. [Selected User Message 1]                    │
│ ...                                            │
│ M-1. [Selected User Message M-1]               │
│ M.   [Initial Context - Developer Instructions] │  ← Injected
│ M+1. [Initial Context - Environment Context]   │  ← Injected
│ M+2. [Initial Context - Model Switch Info]     │  ← Injected (if needed)
│ M+3. [Selected User Message M]                 │  ← Last real user message
│ M+4. [Summary Prefix + Assistant Output]       │  ← Last item
│ M+5. [Ghost Snapshots]                         │
└─────────────────────────────────────────────────┘
```

### Initial Context Includes

- Developer instructions (permissions, sandbox policy)
- Environment context (cwd, shell, timezone)
- Model switch info (if model changed)
- User instructions (from AGENTS.md)

### Summary Prefix (from `templates/compact/summary_prefix.md`)

```
Another language model started to solve this problem and produced a summary of its thinking process.
You also have access to the state of the tools that were used by that language model.
Use this to build on the work that has already been done and avoid duplicating work.
Here is the summary produced by the other language model, use the information in this summary
to assist with your own analysis:
```

### Summary Content (generated by model from `templates/compact/prompt.md`)

```
- Current progress and key decisions made
- Important context, constraints, or user preferences
- What remains to be done (clear next steps)
- Any critical data, examples, or references needed to continue
```

## Integration Guide

### Adding Compaction to Your Application

1. **Configuration**: Set `auto_compact_token_limit` in config
2. **Event Handling**: Listen for `ContextCompactionStarted`/`Completed`
3. **Manual Trigger**: Call `Codex::compact()` when needed
4. **User Feedback**: Display warnings about accuracy degradation

### Best Practices

1. **Start New Threads**: After multiple compactions, start fresh threads
2. **Monitor Token Usage**: Track `TokenUsageInfo` to predict compaction
3. **Custom Prompts**: Tailor `compact_prompt` to your domain
4. **Test Thoroughly**: Verify behavior with both local and remote compaction

### Common Pitfalls

1. **Lost Context**: Compaction removes detailed tool call history
2. **Accuracy Drop**: Multiple compactions degrade model performance
3. **Timing**: Mid-turn compaction can interrupt user experience
4. **Ghost Snapshots**: Ensure undo functionality still works

## Important Considerations

1. **Token Limit**: User messages are limited to ~20,000 tokens (`COMPACT_USER_MESSAGE_MAX_TOKENS`)

2. **Retry Logic**: On `ContextWindowExceeded`, messages are removed from the beginning (prefix-based) to preserve cache

3. **Ghost Snapshots**: Preserved to support `/undo` after compaction

4. **Warning**: User receives a warning about accuracy degradation with frequent compactions

5. **Summary Prefix**: Hardcoded in `templates/compact/summary_prefix.md`

6. **Context Window Training**: Model is trained to see the compaction summary as the last item in history after mid-turn compaction

7. **Prefix-based Trimming**: When trimming for context window, older messages are removed first to preserve cache hits on newer messages
