use oxide_agent_core::config::AgentSettings;
use oxide_agent_core::llm::{
    ChatResponse, LlmClient, LlmError, LlmProvider, Message, ToolDefinition,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct SuccessMock;

#[async_trait::async_trait]
impl LlmProvider for SuccessMock {
    async fn chat_completion(
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

    async fn chat_with_tools(
        &self,
        _system_prompt: &str,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _model_id: &str,
        _max_tokens: u32,
        _json_mode: bool,
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
        chat_model_id: Some("test-model".to_string()),
        chat_model_provider: Some("mock-provider".to_string()),
        ..AgentSettings::default()
    };

    let mut client = LlmClient::new(&settings);
    client.register_provider("mock-provider".to_string(), Arc::new(SuccessMock));

    let response = client
        .chat_completion("sys", &[], "user", "test-model")
        .await
        .expect("Should succeed");
    assert_eq!(response, "Mock Response");
}

struct RetrySuccessMock {
    call_count: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl LlmProvider for RetrySuccessMock {
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

    async fn chat_with_tools(
        &self,
        _system_prompt: &str,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _model_id: &str,
        _max_tokens: u32,
        _json_mode: bool,
    ) -> Result<ChatResponse, LlmError> {
        let count = self.call_count.fetch_add(1, Ordering::SeqCst);
        if count == 0 {
            Err(LlmError::ApiError("500 Internal Server Error".to_string()))
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
        chat_model_id: Some("test-model".to_string()),
        chat_model_provider: Some("mock-provider".to_string()),
        ..AgentSettings::default()
    };

    let mut client = LlmClient::new(&settings);
    client.register_provider(
        "mock-provider".to_string(),
        Arc::new(RetrySuccessMock {
            call_count: call_count.clone(),
        }),
    );

    let response = client
        .chat_with_tools("sys", &[], &[], "test-model", false)
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

    async fn chat_with_tools(
        &self,
        _system_prompt: &str,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _model_id: &str,
        _max_tokens: u32,
        _json_mode: bool,
    ) -> Result<ChatResponse, LlmError> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        Err(LlmError::ApiError("500 Internal Server Error".to_string()))
    }
}

#[tokio::test]
async fn test_retry_logic_failure() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let settings = AgentSettings {
        chat_model_id: Some("test-model".to_string()),
        chat_model_provider: Some("mock-provider".to_string()),
        ..AgentSettings::default()
    };

    let mut client = LlmClient::new(&settings);
    client.register_provider(
        "mock-provider".to_string(),
        Arc::new(AlwaysFailMock {
            call_count: call_count.clone(),
        }),
    );

    let result = client
        .chat_with_tools("sys", &[], &[], "test-model", false)
        .await;
    assert!(result.is_err());
    assert_eq!(call_count.load(Ordering::SeqCst), 5); // MAX_RETRIES is 5
}
