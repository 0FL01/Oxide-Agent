# Data Structures

## Loop Types

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum LoopType {
    ConsecutiveIdenticalToolCalls,
    ChantingIdenticalSentences,
    LlmDetectedLoop,
}

#[derive(Debug)]
pub struct LoopDetectedEvent {
    pub loop_type: LoopType,
    pub prompt_id: String,
    pub confirmed_by_model: Option<String>,
    pub timestamp: DateTime<Utc>,
}
```

## Stream Events

```rust
#[derive(Debug, Clone)]
pub enum StreamEvent {
    ToolCallRequest { name: String, args: serde_json::Value },
    Content(String),
    LoopDetected,
    // ... other events
}
```

## Tool Call Tracker

```rust
pub struct ToolCallTracker {
    last_key: Option<String>,
    repetition_count: usize,
}

impl ToolCallTracker {
    pub fn new() -> Self {
        Self {
            last_key: None,
            repetition_count: 0,
        }
    }

    pub fn reset(&mut self) {
        self.last_key = None;
        self.repetition_count = 0;
    }
}
```

## Content Tracker

```rust
pub struct ContentTracker {
    history: String,
    content_stats: HashMap<String, Vec<usize>>,
    last_content_index: usize,
    in_code_block: bool,
}

impl ContentTracker {
    pub fn new() -> Self {
        Self {
            history: String::new(),
            content_stats: HashMap::new(),
            last_content_index: 0,
            in_code_block: false,
        }
    }

    pub fn reset(&mut self, clear_history: bool) {
        if clear_history {
            self.history.clear();
        }
        self.content_stats.clear();
        self.last_content_index = 0;
    }
}
```

## LLM Tracker

```rust
pub struct LlmTracker {
    turns_in_current_prompt: usize,
    llm_check_interval: usize,
    last_check_turn: usize,
}

impl LlmTracker {
    pub fn new() -> Self {
        Self {
            turns_in_current_prompt: 0,
            llm_check_interval: DEFAULT_LLM_CHECK_INTERVAL,
            last_check_turn: 0,
        }
    }

    pub fn reset(&mut self) {
        self.turns_in_current_prompt = 0;
        self.llm_check_interval = DEFAULT_LLM_CHECK_INTERVAL;
        self.last_check_turn = 0;
    }
}
```

## Main Loop Detection Service

```rust
pub struct LoopDetectionService {
    config: Arc<Config>,
    prompt_id: String,

    // Tool call tracking
    tool_call_tracker: ToolCallTracker,

    // Content tracking
    content_tracker: ContentTracker,

    // LLM tracking
    llm_tracker: LlmTracker,

    // State flags
    loop_detected: bool,
    disabled_for_session: bool,
}

impl LoopDetectionService {
    pub fn new(config: Arc<Config>) -> Self {
        Self {
            config,
            prompt_id: String::new(),
            tool_call_tracker: ToolCallTracker::new(),
            content_tracker: ContentTracker::new(),
            llm_tracker: LlmTracker::new(),
            loop_detected: false,
            disabled_for_session: false,
        }
    }

    pub fn reset(&mut self, prompt_id: String) {
        self.prompt_id = prompt_id;
        self.tool_call_tracker.reset();
        self.content_tracker.reset(true);
        self.llm_tracker.reset();
        self.loop_detected = false;
    }

    pub fn disable_for_session(&mut self) {
        self.disabled_for_session = true;
    }
}
```

## LLM Response Structures

```rust
#[derive(Debug, Deserialize)]
pub struct LoopDetectionResponse {
    pub unproductive_state_analysis: String,
    pub unproductive_state_confidence: f64,
}

#[derive(Debug, Deserialize)]
struct LoopDetectionSchema {
    #[serde(rename = "unproductive_state_analysis")]
    analysis: String,
    #[serde(rename = "unproductive_state_confidence")]
    confidence: f64,
}
```

## Message Types for LLM

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMessage {
    pub role: String,  // "user" or "model"
    pub parts: Vec<MessagePart>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessagePart {
    Text { text: String },
    FunctionCall { name: String, args: serde_json::Value },
    FunctionResponse { name: String, response: serde_json::Value },
}
```

## Helper for Message Inspection

```rust
pub fn is_function_call(msg: &ConversationMessage) -> bool {
    msg.role == "model" &&
    msg.parts.iter().all(|p| matches!(p, MessagePart::FunctionCall { .. }))
}

pub fn is_function_response(msg: &ConversationMessage) -> bool {
    msg.role == "user" &&
    msg.parts.iter().all(|p| matches!(p, MessagePart::FunctionResponse { .. }))
}
```

## Configuration

```rust
pub struct LoopDetectionConfig {
    pub tool_call_threshold: usize,
    pub content_chunk_size: usize,
    pub content_loop_threshold: usize,
    pub max_history_length: usize,
    pub llm_check_after_turns: usize,
    pub llm_check_history_count: usize,
    pub llm_confidence_threshold: f64,
    pub flash_model: String,
    pub pro_model: String,
}

impl Default for LoopDetectionConfig {
    fn default() -> Self {
        Self {
            tool_call_threshold: 5,
            content_chunk_size: 50,
            content_loop_threshold: 10,
            max_history_length: 5000,
            llm_check_after_turns: 30,
            llm_check_history_count: 20,
            llm_confidence_threshold: 0.9,
            flash_model: "gemini-2.5-flash".to_string(),
            pro_model: "gemini-2.5-pro".to_string(),
        }
    }
}
```
