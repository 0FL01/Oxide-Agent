// Allow clone_on_ref_ptr in integration tests due to trait object coercion requirements
#![allow(clippy::clone_on_ref_ptr)]
#![cfg(feature = "llm-opencode-go")]

use oxide_agent_core::config::AgentSettings;
use oxide_agent_core::llm::{
    ChatResponse, ChatWithToolsRequest, LlmClient, LlmError, LlmProvider, Message,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

struct SuccessMock;

#[async_trait::async_trait]
impl LlmProvider for SuccessMock {
    async fn complete_internal_text(
        &self,
        _system_prompt: &str,
        _history: &[Message],
        _user_message: &str,
        _model_id: &str,
        _max_tokens: u32,
    ) -> Result<String, LlmError> {
        Ok("Mock Response".to_string())
    }

    async fn transcribe_audio(
        &self,
        _audio_bytes: Vec<u8>,
        _mime_type: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown("Not implemented".to_string()))
    }

    async fn analyze_image(
        &self,
        _image_bytes: Vec<u8>,
        _text_prompt: &str,
        _system_prompt: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown("Not implemented".to_string()))
    }

    async fn chat_with_tools<'a>(
        &self,
        _request: ChatWithToolsRequest<'a>,
    ) -> Result<ChatResponse, LlmError> {
        Ok(ChatResponse {
            content: Some("Success".to_string()),
            tool_calls: vec![],
            finish_reason: "stop".to_string(),
            reasoning_content: None,
            usage: None,
        })
    }
}

#[tokio::test]
async fn test_client_uses_registered_provider() {
    let settings = AgentSettings {
        agent_model_id: Some("test-model".to_string()),
        agent_model_provider: Some("opencode-go".to_string()),
        ..AgentSettings::default()
    };

    let mut client = LlmClient::new(&settings);
    client.register_provider("opencode-go".to_string(), Arc::new(SuccessMock));

    let response = client
        .chat_with_tools("sys", "", &[], &[], "test-model", false)
        .await
        .expect("Should succeed");
    assert_eq!(response.content.as_deref(), Some("Success"));
}

struct RetrySuccessMock {
    call_count: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl LlmProvider for RetrySuccessMock {
    async fn complete_internal_text(
        &self,
        _system_prompt: &str,
        _history: &[Message],
        _user_message: &str,
        _model_id: &str,
        _max_tokens: u32,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown(
            "unexpected internal text call in retry test".to_string(),
        ))
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
        if count == 0 {
            Err(LlmError::api_error_status(500, "500 Internal Server Error"))
        } else {
            Ok(ChatResponse {
                content: Some("Success".to_string()),
                tool_calls: vec![],
                finish_reason: "stop".to_string(),
                reasoning_content: None,
                usage: None,
            })
        }
    }
}

#[tokio::test]
async fn test_retry_logic_eventual_success() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let settings = AgentSettings {
        agent_model_id: Some("test-model".to_string()),
        agent_model_provider: Some("opencode-go".to_string()),
        ..AgentSettings::default()
    };

    let mut client = LlmClient::new(&settings);
    client.register_provider(
        "opencode-go".to_string(),
        Arc::new(RetrySuccessMock {
            call_count: call_count.clone(),
        }),
    );

    let response = client
        .chat_with_tools("sys", "", &[], &[], "test-model", false)
        .await
        .expect("Should eventually succeed");
    assert_eq!(response.content.expect("Should have content"), "Success");
    assert_eq!(call_count.load(Ordering::SeqCst), 2);
}

struct AlwaysFailMock {
    call_count: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl LlmProvider for AlwaysFailMock {
    async fn complete_internal_text(
        &self,
        _system_prompt: &str,
        _history: &[Message],
        _user_message: &str,
        _model_id: &str,
        _max_tokens: u32,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown(
            "unexpected internal text call in failure test".to_string(),
        ))
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
        self.call_count.fetch_add(1, Ordering::SeqCst);
        Err(LlmError::api_error_status(500, "500 Internal Server Error"))
    }
}

#[tokio::test(start_paused = true)]
async fn test_retry_logic_failure() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let settings = AgentSettings {
        agent_model_id: Some("test-model".to_string()),
        agent_model_provider: Some("opencode-go".to_string()),
        ..AgentSettings::default()
    };

    let mut client = LlmClient::new(&settings);
    client.register_provider(
        "opencode-go".to_string(),
        Arc::new(AlwaysFailMock {
            call_count: call_count.clone(),
        }),
    );

    let handle = tokio::spawn(async move {
        client
            .chat_with_tools("sys", "", &[], &[], "test-model", false)
            .await
    });

    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_secs(301)).await;
    tokio::task::yield_now().await;

    let result = handle.await.expect("retry task panicked");
    assert!(result.is_err());
    assert_eq!(call_count.load(Ordering::SeqCst), LlmClient::MAX_RETRIES);
}
