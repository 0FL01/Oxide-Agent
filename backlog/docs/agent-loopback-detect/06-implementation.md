# Main Service Implementation

## Complete LoopDetectionService API

```rust
use sha2::{Sha256, Digest};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use chrono::Utc;

pub struct LoopDetectionService {
    config: Arc<Config>,
    prompt_id: String,
    disabled_for_session: bool,

    // Sub-detectors
    tool_detector: ToolCallDetector,
    content_detector: ContentLoopDetector,
    llm_detector: LlmLoopDetector,

    loop_detected: bool,
}

impl LoopDetectionService {
    pub fn new(config: Arc<Config>) -> Self {
        Self {
            config,
            prompt_id: String::new(),
            disabled_for_session: false,
            tool_detector: ToolCallDetector::new(),
            content_detector: ContentLoopDetector::new(),
            llm_detector: LlmLoopDetector::new(),
            loop_detected: false,
        }
    }

    /// Process stream event and check for loops
    pub fn add_and_check(&mut self, event: &StreamEvent) -> bool {
        if self.disabled_for_session {
            return false;
        }

        if self.loop_detected {
            return true;
        }

        match event {
            StreamEvent::ToolCallRequest { name, args } => {
                // Reset content tracking on tool calls
                self.content_detector.reset(false);
                self.loop_detected = self.tool_detector.check(name, args);
            }
            StreamEvent::Content(text) => {
                self.loop_detected = self.content_detector.check(text);
            }
            _ => {}
        }

        self.loop_detected
    }

    /// Called at start of each turn for LLM-based checks
    pub async fn turn_started(&mut self, signal: AbortSignal) -> Result<bool, Error> {
        if self.disabled_for_session {
            return Ok(false);
        }

        if let Some(detected) = self.llm_detector.check(signal).await? {
            if detected {
                self.loop_detected = true;
                return Ok(true);
            }
        }

        Ok(false)
    }

    pub fn reset(&mut self, prompt_id: String) {
        self.prompt_id = prompt_id;
        self.tool_detector.reset();
        self.content_detector.reset(true);
        self.llm_detector.reset();
        self.loop_detected = false;
    }

    pub fn disable_for_session(&mut self) {
        self.disabled_for_session = true;
    }
}
```

## Integration Example (Telegram Bot)

```rust
pub struct TelegramAgent {
    loop_detector: LoopDetectionService,
    // ... other fields
}

impl TelegramAgent {
    pub async fn handle_message(&mut self, msg: Message) -> Result<()> {
        // Start new turn
        let loop_detected = self.loop_detector.turn_started(AbortSignal::new()).await?;

        if loop_detected {
            self.send_reply("I seem to be stuck in a loop. Let me try again...").await?;
            self.loop_detector.disable_for_session();
            return self.handle_message(msg).await;  // Retry
        }

        // Generate response stream
        let mut stream = self.model.generate_stream(&msg).await?;

        while let Some(event) = stream.next().await {
            // Check for loops during streaming
            if self.loop_detector.add_and_check(&event) {
                self.send_reply("Detected repetitive pattern. Aborting...").await?;
                break;
            }

            // Process event normally
            match event {
                StreamEvent::Content(text) => {
                    self.update_typing_status(&text).await?;
                }
                StreamEvent::ToolCallRequest { name, args } => {
                    self.execute_tool(&name, &args).await?;
                }
                _ => {}
            }
        }

        Ok(())
    }
}
```

## Event Emitter Pattern

```rust
pub trait LoopDetectionHandler: Send + Sync {
    fn on_loop_detected(&self, event: LoopDetectedEvent);
}

pub struct LoopDetectionService {
    handlers: Vec<Arc<dyn LoopDetectionHandler>>,
    // ... other fields
}

impl LoopDetectionService {
    pub fn add_handler(&mut self, handler: Arc<dyn LoopDetectionHandler>) {
        self.handlers.push(handler);
    }

    fn notify_loop_detected(&self, loop_type: LoopType) {
        let event = LoopDetectedEvent {
            loop_type,
            prompt_id: self.prompt_id.clone(),
            confirmed_by_model: None,
            timestamp: Utc::now(),
        };

        for handler in &self.handlers {
            handler.on_loop_detected(event.clone());
        }
    }
}
```

## Example Handler for Logging

```rust
pub struct LoggingHandler;

impl LoopDetectionHandler for LoggingHandler {
    fn on_loop_detected(&self, event: LoopDetectedEvent) {
        log::warn!(
            "Loop detected: {:?} for prompt {} at {}",
            event.loop_type,
            event.prompt_id,
            event.timestamp
        );
    }
}
```

## Testing Utilities

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn create_tool_event(name: &str, args: &str) -> StreamEvent {
        StreamEvent::ToolCallRequest {
            name: name.to_string(),
            args: serde_json::from_str(args).unwrap(),
        }
    }

    fn create_content_event(text: &str) -> StreamEvent {
        StreamEvent::Content(text.to_string())
    }

    #[test]
    fn test_tool_call_loop_detection() {
        let config = Arc::new(Config::default());
        let mut detector = LoopDetectionService::new(config);
        detector.reset("test_prompt".to_string());

        let event = create_tool_event("test_tool", r#"{"param": "value"}"#);

        // Below threshold
        for _ in 0..4 {
            assert!(!detector.add_and_check(&event));
        }

        // At threshold
        assert!(detector.add_and_check(&event));
    }

    #[test]
    fn test_content_loop_detection() {
        let config = Arc::new(Config::default());
        let mut detector = LoopDetectionService::new(config);
        detector.reset("test_prompt".to_string());

        let repetitive = "I will repeat this. ".repeat(5);

        // Add repetitive content
        for _ in 0..10 {
            let event = create_content_event(&repetitive);
            if detector.add_and_check(&event) {
                break;
            }
        }

        assert!(detector.loop_detected);
    }
}
```

## Performance Considerations

1. **Content tracking** limits history to 5000 chars
2. **Hashing** uses SHA256, fast enough for streaming
3. **LLM checks** are throttled (3-15 turns)
4. **HashMap** lookups are O(1) average
5. Consider using `parking_lot` Mutex for contention

## Error Handling

```rust
pub enum LoopDetectionError {
    LlmQueryError(String),
    HistoryEmpty,
    InvalidResponse,
}

impl std::error::Error for LoopDetectionError {}
impl std::fmt::Display for LoopDetectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::LlmQueryError(msg) => write!(f, "LLM query failed: {}", msg),
            Self::HistoryEmpty => write!(f, "No history available"),
            Self::InvalidResponse => write!(f, "Invalid LLM response format"),
        }
    }
}
```
