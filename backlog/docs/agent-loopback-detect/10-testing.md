# Testing Strategy

## Unit Tests

### Tool Call Detection Tests

```rust
#[cfg(test)]
mod tool_call_tests {
    use super::*;

    fn create_detector() -> ToolCallDetector {
        ToolCallDetector::new()
    }

    fn create_tool(name: &str, args: &str) -> ToolCall {
        ToolCall {
            name: name.to_string(),
            args: serde_json::from_str(args).unwrap(),
        }
    }

    #[test]
    fn test_below_threshold() {
        let mut detector = create_detector();
        let tool = create_tool("test", r#"{"param": "value"}"#);

        for _ in 0..4 {
            assert!(!detector.check(&tool));
        }

        assert_eq!(detector.repetition_count(), 4);
    }

    #[test]
    fn test_at_threshold() {
        let mut detector = create_detector();
        let tool = create_tool("test", r#"{"param": "value"}"#);

        for _ in 0..4 {
            assert!(!detector.check(&tool));
        }

        assert!(detector.check(&tool));
    }

    #[test]
    fn test_different_tools() {
        let mut detector = create_detector();
        let tool1 = create_tool("tool1", r#"{"a": "1"}"#);
        let tool2 = create_tool("tool2", r#"{"b": "2"}"#);

        assert!(!detector.check(&tool1));
        assert!(!detector.check(&tool2));
        assert!(!detector.check(&tool1));
        assert!(!detector.check(&tool2));

        assert_eq!(detector.repetition_count(), 1);
    }

    #[test]
    fn test_different_args() {
        let mut detector = create_detector();
        let tool1 = create_tool("tool", r#"{"a": "1"}"#);
        let tool2 = create_tool("tool", r#"{"a": "2"}"#);

        assert!(!detector.check(&tool1));
        assert!(!detector.check(&tool2));

        assert_eq!(detector.repetition_count(), 1);
    }
}
```

### Content Detection Tests

````rust
#[cfg(test)]
mod content_tests {
    use super::*;

    fn create_detector() -> ContentLoopDetector {
        ContentLoopDetector::new()
    }

    #[test]
    fn test_random_content() {
        let mut detector = create_detector();

        for i in 0..1000 {
            let text = format!("random text {}", i);
            assert!(!detector.check(&text));
        }
    }

    #[test]
    fn test_repetitive_chunks() {
        let mut detector = create_detector();
        let repetitive = "I will repeat this. ".repeat(5);

        let mut detected = false;
        for _ in 0..15 {
            if detector.check(&repetitive) {
                detected = true;
                break;
            }
        }

        assert!(detected);
    }

    #[test]
    fn test_code_block_ignored() {
        let mut detector = create_detector();
        let code = "function test() { return true; }";

        for _ in 0..10 {
            assert!(!detector.check(&format!("```{}```", code)));
        }
    }

    #[test]
    fn test_history_truncation() {
        let mut detector = create_detector();

        // Add enough content to trigger truncation
        let long_text = "a".repeat(6000);
        detector.check(&long_text);

        assert!(detector.history().len() <= 5000);
    }

    #[test]
    fn test_far_apart_repetitions() {
        let mut detector = create_detector();
        let pattern = "repeat this";

        // Pattern far apart with filler
        let filler = "x".repeat(1000);

        for _ in 0..10 {
            assert!(!detector.check(&pattern));
            assert!(!detector.check(&filler));
        }
    }
}
````

### Integration Tests

```rust
#[cfg(test)]
mod integration_tests {
    use super::*;

    #[tokio::test]
    async fn test_full_detection_flow() {
        let config = Arc::new(LoopDetectionConfig::default());
        let mut detector = LoopDetectionService::new(config);
        detector.reset("test".to_string());

        // Tool call loop
        let tool_event = StreamEvent::ToolCallRequest {
            name: "test".to_string(),
            args: json!({"param": "value"}),
        };

        for _ in 0..4 {
            assert!(!detector.add_and_check(&tool_event));
        }

        assert!(detector.add_and_check(&tool_event));
    }

    #[tokio::test]
    async fn test_session_disable() {
        let config = Arc::new(LoopDetectionConfig::default());
        let mut detector = LoopDetectionService::new(config);
        detector.disable_for_session();

        let tool_event = StreamEvent::ToolCallRequest {
            name: "test".to_string(),
            args: json!({}),
        };

        for _ in 0..10 {
            assert!(!detector.add_and_check(&tool_event));
        }
    }
}
```

## Mock LLM for Testing

```rust
pub struct MockLlmDetector {
    responses: Vec<Result<LoopDetectionResponse, Error>>,
    call_count: usize,
}

impl MockLlmDetector {
    pub fn new(responses: Vec<Result<LoopDetectionResponse, Error>>) -> Self {
        Self {
            responses,
            call_count: 0,
        }
    }
}

#[async_trait]
impl LlmDetector for MockLlmDetector {
    async fn check(&mut self, _history: &[Message]) -> Result<bool, Error> {
        let response = self.responses[self.call_count].clone()?;
        self.call_count += 1;
        Ok(response.confidence >= 0.9)
    }
}

#[tokio::test]
async fn test_llm_detection() {
    let responses = vec![
        Ok(LoopDetectionResponse {
            analysis: "Not a loop".to_string(),
            confidence: 0.5,
        }),
        Ok(LoopDetectionResponse {
            analysis: "Loop detected".to_string(),
            confidence: 0.95,
        }),
    ];

    let mock = MockLlmDetector::new(responses);

    assert!(!mock.check(&[]).await.unwrap());
    assert!(mock.check(&[]).await.unwrap());
}
```

## Property-Based Tests

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn test_tool_call_threshold(count in 0..20usize) {
        let mut detector = ToolCallDetector::new();
        let tool = ToolCall::new("test", json!({}));

        for _ in 0..count {
            detector.check(&tool);
        }

        prop_assert_eq!(detector.loop_detected(), count >= 5);
    }

    #[test]
    fn test_content_detection_properties(
        text in "[a-z]{1,100}"
    ) {
        let mut detector = ContentLoopDetector::new();

        // Single text shouldn't trigger
        let result = detector.check(&text);
        prop_assert!(!result);
    }
}
```

## Performance Tests

```rust
#[cfg(test)]
mod performance_tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn test_tool_call_performance() {
        let mut detector = ToolCallDetector::new();
        let tool = ToolCall::new("test", json!({}));

        let start = Instant::now();
        for _ in 0..100000 {
            detector.check(&tool);
        }
        let duration = start.elapsed();

        println!("100k checks: {:?}", duration);
        assert!(duration.as_millis() < 100); // Should be fast
    }

    #[test]
    fn test_content_performance() {
        let mut detector = ContentLoopDetector::new();
        let text = "sample text for performance testing";

        let start = Instant::now();
        for _ in 0..10000 {
            detector.check(text);
        }
        let duration = start.elapsed();

        println!("10k content checks: {:?}", duration);
    }
}
```

## Test Fixtures

```rust
pub struct TestFixtures;

impl TestFixtures {
    pub fn simple_tool_loop() -> Vec<StreamEvent> {
        let tool = StreamEvent::ToolCallRequest {
            name: "test".to_string(),
            args: json!({"value": 1}),
        };

        vec![tool; 6]
    }

    pub fn content_loop() -> Vec<StreamEvent> {
        let text = "I will repeat this text multiple times. ".repeat(5);
        vec![StreamEvent::Content(text); 12]
    }

    pub fn mixed_events() -> Vec<StreamEvent> {
        vec![
            StreamEvent::Content("Starting work".to_string()),
            StreamEvent::ToolCallRequest {
                name: "read_file".to_string(),
                args: json!({"path": "test.txt"}),
            },
            StreamEvent::Content("Processing".to_string()),
            StreamEvent::ToolCallRequest {
                name: "read_file".to_string(),
                args: json!({"path": "test.txt"}),
            },
        ]
    }
}

#[test]
fn test_with_fixtures() {
    let mut detector = LoopDetectionService::new(Arc::new(Config::default()));

    for event in TestFixtures::simple_tool_loop() {
        if detector.add_and_check(&event) {
            break;
        }
    }

    assert!(detector.loop_detected());
}
```

## Test Utilities

```rust
#[cfg(test)]
pub mod test_utils {
    use super::*;

    pub fn assert_tool_call_count(count: usize) {
        let mut detector = ToolCallDetector::new();
        let tool = ToolCall::new("test", json!({}));

        for _ in 0..count {
            detector.check(&tool);
        }

        assert_eq!(detector.repetition_count(), count);
        assert_eq!(detector.loop_detected(), count >= 5);
    }

    pub async fn run_with_timeout<F, T>(
        future: F,
        duration: Duration,
    ) -> Result<T, String>
    where
        F: Future<Output = T>,
    {
        match tokio::time::timeout(duration, future).await {
            Ok(result) => Ok(result),
            Err(_) => Err("Timeout".to_string()),
        }
    }
}
```
