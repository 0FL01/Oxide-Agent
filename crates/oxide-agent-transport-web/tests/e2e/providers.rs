//! Scripted LLM providers used in E2E tests.

use async_trait::async_trait;
use oxide_agent_core::llm::{ChatResponse, ChatWithToolsRequest, LlmError, LlmProvider, Message};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};

/// Scripted ZAI provider that returns pre-programmed responses in sequence.
/// Logs each model_id call for verification.
#[derive(Clone)]
pub struct SequencedZaiProvider {
    responses: Arc<Mutex<VecDeque<ChatResponse>>>,
    model_log: Arc<Mutex<Vec<String>>>,
}

impl SequencedZaiProvider {
    pub fn new(responses: Vec<ChatResponse>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(VecDeque::from(responses))),
            model_log: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub async fn model_log(&self) -> Vec<String> {
        self.model_log.lock().await.clone()
    }
}

#[async_trait]
impl LlmProvider for SequencedZaiProvider {
    async fn chat_completion(
        &self,
        _system_prompt: &str,
        _history: &[Message],
        _user_message: &str,
        _model_id: &str,
        _max_tokens: u32,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown(
            "chat_completion is not used by this provider".to_string(),
        ))
    }

    async fn chat_with_tools<'a>(
        &self,
        request: ChatWithToolsRequest<'a>,
    ) -> Result<ChatResponse, LlmError> {
        self.model_log
            .lock()
            .await
            .push(request.model_id.to_string());

        self.responses
            .lock()
            .await
            .pop_front()
            .ok_or_else(|| LlmError::ApiError("No scripted ZAI response available".to_string()))
    }

    async fn transcribe_audio(
        &self,
        _audio_bytes: Vec<u8>,
        _mime_type: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown("transcribe not implemented".to_string()))
    }

    async fn analyze_image(
        &self,
        _image_bytes: Vec<u8>,
        _text_prompt: &str,
        _system_prompt: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown(
            "analyze_image not implemented".to_string(),
        ))
    }
}

/// Scripted narrator provider that can be made to hang on a specific call
/// and released programmatically via `.release()`.
#[derive(Clone)]
pub struct ControlledNarratorProvider {
    hang_on_call: Option<usize>,
    call_count: Arc<AtomicUsize>,
    released: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl ControlledNarratorProvider {
    pub fn new(hang_on_call: Option<usize>) -> Self {
        Self {
            hang_on_call,
            call_count: Arc::new(AtomicUsize::new(0)),
            released: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(Notify::new()),
        }
    }

    pub fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }

    pub fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
}

#[async_trait]
impl LlmProvider for ControlledNarratorProvider {
    async fn chat_completion(
        &self,
        _system_prompt: &str,
        _history: &[Message],
        _user_message: &str,
        _model_id: &str,
        _max_tokens: u32,
    ) -> Result<String, LlmError> {
        let call_index = self.call_count.fetch_add(1, Ordering::SeqCst) + 1;
        if self.hang_on_call == Some(call_index) && !self.released.load(Ordering::SeqCst) {
            self.notify.notified().await;
        }

        Ok(
            r#"{"headline":"Delegating work","content":"Tracking delegated work progress."}"#
                .to_string(),
        )
    }

    async fn chat_with_tools<'a>(
        &self,
        _: ChatWithToolsRequest<'a>,
    ) -> Result<ChatResponse, LlmError> {
        Err(LlmError::Unknown(
            "chat_with_tools is not used by this provider".to_string(),
        ))
    }

    async fn transcribe_audio(
        &self,
        _audio_bytes: Vec<u8>,
        _mime_type: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown("transcribe not implemented".to_string()))
    }

    async fn analyze_image(
        &self,
        _image_bytes: Vec<u8>,
        _text_prompt: &str,
        _system_prompt: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown(
            "analyze_image not implemented".to_string(),
        ))
    }
}
