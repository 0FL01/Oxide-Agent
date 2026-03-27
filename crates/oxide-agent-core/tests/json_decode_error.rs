//! Integration test for JSON decoding error detection in LLM calls
//!
//! This test verifies that:
//! 1. Invalid JSON responses from LLM providers result in `LlmError::JsonError`
//! 2. JSON errors ARE retried (transient network/proxy issues can cause them)
//! 3. The error message contains diagnostic information
//!
//! Related bug: JSON decoding errors should be detected and retried appropriately.
//! Example error: "JSON error: error decoding response body"

// Allow clone_on_ref_ptr in integration tests due to trait object coercion requirements
#![allow(clippy::clone_on_ref_ptr)]
//! This error comes from reqwest when `response.json().await` fails, typically when:
//! - Server returns HTML error page instead of JSON
//! - Server returns empty or malformed body
//! - Network interruption during streaming

use oxide_agent_core::config::AgentSettings;
use oxide_agent_core::llm::{
    ChatResponse, ChatWithToolsRequest, LlmClient, LlmError, LlmProvider, Message,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Mock provider that returns JSON decode errors with optional retry behavior
struct JsonDecodeRetryMock {
    call_count: Arc<AtomicUsize>,
    error_message: String,
    /// After this many calls, return success. If None, always fail.
    succeed_after: Option<usize>,
}

impl JsonDecodeRetryMock {
    fn new(error_message: impl Into<String>, succeed_after: Option<usize>) -> Self {
        Self {
            call_count: Arc::new(AtomicUsize::new(0)),
            error_message: error_message.into(),
            succeed_after,
        }
    }

    fn call_count(&self) -> Arc<AtomicUsize> {
        self.call_count.clone()
    }
}

#[async_trait::async_trait]
impl LlmProvider for JsonDecodeRetryMock {
    async fn chat_completion(
        &self,
        _system_prompt: &str,
        _history: &[Message],
        _user_message: &str,
        _model_id: &str,
        _max_tokens: u32,
    ) -> Result<String, LlmError> {
        unimplemented!()
    }

    async fn transcribe_audio(
        &self,
        _audio_bytes: Vec<u8>,
        _mime_type: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        unimplemented!()
    }

    async fn analyze_image(
        &self,
        _image_bytes: Vec<u8>,
        _text_prompt: &str,
        _system_prompt: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        unimplemented!()
    }

    async fn chat_with_tools<'a>(
        &self,
        _request: ChatWithToolsRequest<'a>,
    ) -> Result<ChatResponse, LlmError> {
        let count = self.call_count.fetch_add(1, Ordering::SeqCst);
        match self.succeed_after {
            Some(limit) if count >= limit => Ok(ChatResponse {
                content: Some("Success after retry".to_string()),
                tool_calls: vec![],
                finish_reason: "stop".to_string(),
                reasoning_content: None,
                usage: None,
            }),
            _ => Err(LlmError::JsonError(self.error_message.clone())),
        }
    }
}

/// Test that JSON decoding errors are detected and retried
#[tokio::test]
async fn test_json_decoding_error_retried_on_failure() {
    let settings = AgentSettings {
        chat_model_id: Some("test-model".to_string()),
        chat_model_provider: Some("mock-provider".to_string()),
        ..AgentSettings::default()
    };

    let mut client = LlmClient::new(&settings);
    let mock = Arc::new(JsonDecodeRetryMock::new(
        "error decoding response body",
        None,
    ));
    client.register_provider("mock-provider".to_string(), mock.clone());

    let result = client
        .chat_with_tools("sys", &[], &[], "test-model", false)
        .await;

    // Verify the error is a JsonError
    assert!(
        matches!(result, Err(LlmError::JsonError(_))),
        "Expected JsonError, got: {:?}",
        result
    );

    // Verify the error message contains "error decoding response body"
    let error_msg = result.expect_err("Expected an error").to_string();
    assert!(
        error_msg.contains("error decoding response body"),
        "Error message should contain 'error decoding response body', got: {}",
        error_msg
    );

    // Verify multiple calls were made (JSON errors ARE retried now!)
    // With MAX_RETRIES = 5, we expect 5 failed attempts
    let call_count = mock.call_count().load(Ordering::SeqCst);
    assert_eq!(
        call_count, 5,
        "JSON errors should be retried (expected 5 attempts)"
    );
}

/// Test that JSON decoding errors eventually succeed after retry
#[tokio::test]
async fn test_json_decoding_error_succeeds_after_retry() {
    let settings = AgentSettings {
        chat_model_id: Some("test-model".to_string()),
        chat_model_provider: Some("mock-provider".to_string()),
        ..AgentSettings::default()
    };

    let mut client = LlmClient::new(&settings);
    // Succeed after 2 attempts (first fails, second succeeds)
    let mock = Arc::new(JsonDecodeRetryMock::new(
        "error decoding response body",
        Some(1),
    ));
    client.register_provider("mock-provider".to_string(), mock.clone());

    let result = client
        .chat_with_tools("sys", &[], &[], "test-model", false)
        .await;

    // Verify the request eventually succeeded
    assert!(
        result.is_ok(),
        "Expected success after retry, got: {:?}",
        result
    );

    let response = result.expect("Expected successful response");
    assert_eq!(
        response.content,
        Some("Success after retry".to_string()),
        "Expected success message"
    );

    // Verify 2 calls were made (1 failed + 1 succeeded)
    let call_count = mock.call_count().load(Ordering::SeqCst);
    assert_eq!(
        call_count, 2,
        "Expected 2 attempts (1 failed + 1 succeeded)"
    );
}

/// Test that JSON errors contain diagnostic information
#[tokio::test]
async fn test_json_error_contains_diagnostics() {
    // Create a mock that returns a specific JSON error with context
    let specific_error = "expected value at line 1 column 1";
    // We can't easily customize the mock per-call, so we'll verify the error format
    // by checking that JsonError contains the expected message format
    let error = LlmError::JsonError(specific_error.to_string());
    let error_string = error.to_string();

    assert!(
        error_string.starts_with("JSON error:"),
        "JSON error should start with 'JSON error:', got: {}",
        error_string
    );

    assert!(
        error_string.contains(specific_error),
        "JSON error should contain the specific error message, got: {}",
        error_string
    );
}

/// Test that distinguishes JSON errors from other error types
#[tokio::test]
async fn test_json_error_distinguished_from_network_error() {
    // Verify that JSON errors are different from Network errors
    let json_error = LlmError::JsonError("error decoding response body".to_string());
    let network_error = LlmError::NetworkError("connection refused".to_string());
    let api_error = LlmError::ApiError("500 Internal Server Error".to_string());

    // All should have different Display representations
    let json_str = json_error.to_string();
    let network_str = network_error.to_string();
    let api_str = api_error.to_string();

    assert!(
        json_str.starts_with("JSON error:"),
        "JSON error should start with 'JSON error:', got: {}",
        json_str
    );
    assert!(
        network_str.starts_with("Network error:"),
        "Network error should start with 'Network error:', got: {}",
        network_str
    );
    assert!(
        api_str.starts_with("API error:"),
        "API error should start with 'API error:', got: {}",
        api_str
    );

    // Now JSON errors ARE retried (like NetworkError)
    // Both have retry behavior via get_retry_delay
}
