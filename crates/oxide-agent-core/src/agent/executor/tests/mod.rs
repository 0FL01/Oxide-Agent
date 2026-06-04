// Allow clone_on_ref_ptr in tests due to trait object coercion requirements
#![allow(clippy::clone_on_ref_ptr)]

mod basics;
mod registry;
mod resume;

pub(super) use super::policy_hooks::PolicyControlledHook;
pub(super) use super::{AgentExecutor, AgentUserInput};
pub(super) use crate::agent::hooks::{Hook, HookContext, HookEvent, HookResult};
pub(super) use crate::agent::profile::HookAccessPolicy;
pub(super) use crate::agent::providers::TodoList;
#[cfg(feature = "manager-control-plane")]
pub(super) use crate::agent::providers::{
    ForumTopicActionResult, ForumTopicCreateRequest, ForumTopicCreateResult, ForumTopicEditRequest,
    ForumTopicEditResult, ForumTopicThreadRequest, ManagerTopicLifecycle,
};
pub(super) use crate::agent::session::{AgentSession, PendingUserInput, UserInputKind};
pub(super) use crate::config::AgentSettings;
pub(super) use crate::llm::LlmClient;
#[cfg(feature = "manager-control-plane")]
pub(super) use crate::storage::MockStorageProvider;
#[cfg(feature = "manager-control-plane")]
pub(super) use anyhow::bail;
pub(super) use anyhow::Result;
pub(super) use std::sync::{Arc, Mutex as StdMutex};
pub(super) use tokio::sync::Mutex;

#[cfg(feature = "manager-control-plane")]
pub(super) struct RecordingTopicLifecycle {
    create_calls: StdMutex<Vec<ForumTopicCreateRequest>>,
}

#[cfg(feature = "manager-control-plane")]
impl RecordingTopicLifecycle {
    pub(super) fn new() -> Self {
        Self {
            create_calls: StdMutex::new(Vec::new()),
        }
    }
}

#[cfg(feature = "manager-control-plane")]
#[async_trait::async_trait]
impl ManagerTopicLifecycle for RecordingTopicLifecycle {
    async fn forum_topic_create(
        &self,
        request: ForumTopicCreateRequest,
    ) -> Result<ForumTopicCreateResult> {
        if let Ok(mut calls) = self.create_calls.lock() {
            calls.push(request.clone());
        }
        Ok(ForumTopicCreateResult {
            chat_id: request.chat_id.unwrap_or(-100_555),
            thread_id: 313,
            name: request.name,
            icon_color: request.icon_color.unwrap_or(9_367_192),
            icon_custom_emoji_id: request.icon_custom_emoji_id,
        })
    }

    async fn forum_topic_edit(
        &self,
        _request: ForumTopicEditRequest,
    ) -> Result<ForumTopicEditResult> {
        bail!("forum_topic_edit is not used by this test lifecycle")
    }

    async fn forum_topic_close(
        &self,
        _request: ForumTopicThreadRequest,
    ) -> Result<ForumTopicActionResult> {
        bail!("forum_topic_close is not used by this test lifecycle")
    }

    async fn forum_topic_reopen(
        &self,
        _request: ForumTopicThreadRequest,
    ) -> Result<ForumTopicActionResult> {
        bail!("forum_topic_reopen is not used by this test lifecycle")
    }

    async fn forum_topic_delete(
        &self,
        _request: ForumTopicThreadRequest,
    ) -> Result<ForumTopicActionResult> {
        bail!("forum_topic_delete is not used by this test lifecycle")
    }
}

pub(super) fn build_executor() -> AgentExecutor {
    let settings = Arc::new(crate::config::AgentSettings::default());
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    AgentExecutor::new(llm, session, settings)
}

pub(super) fn build_executor_with_timeout(agent_timeout_secs: u64) -> AgentExecutor {
    let settings = Arc::new(AgentSettings {
        agent_timeout_secs: Some(agent_timeout_secs),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    AgentExecutor::new(llm, session, settings)
}

pub(super) fn build_executor_with_mock_response(response_text: &'static str) -> AgentExecutor {
    let settings = Arc::new(crate::config::AgentSettings {
        agent_model_id: Some("deepseek-v4-flash".to_string()),
        agent_model_provider: Some("opencode-go".to_string()),
        ..crate::config::AgentSettings::default()
    });
    let mut provider = crate::llm::MockLlmProvider::new();
    provider.expect_chat_with_tools().return_once(move |_| {
        Ok(crate::llm::ChatResponse {
            content: Some(response_text.to_string()),
            tool_calls: Vec::new(),
            finish_reason: "stop".to_string(),
            reasoning_content: None,
            usage: None,
        })
    });
    provider
        .expect_complete_internal_text()
        .returning(|_, _, _, _, _| {
            Err(crate::llm::LlmError::Unknown("Not implemented".to_string()))
        });
    provider
        .expect_transcribe_audio()
        .returning(|_, _, _| Err(crate::llm::LlmError::Unknown("Not implemented".to_string())));
    provider
        .expect_analyze_image()
        .returning(|_, _, _, _| Err(crate::llm::LlmError::Unknown("Not implemented".to_string())));
    let mut llm = LlmClient::new(settings.as_ref());
    llm.register_provider("opencode-go".to_string(), Arc::new(provider));
    let session = AgentSession::new(9_i64.into());
    AgentExecutor::new(Arc::new(llm), session, settings)
}

pub(super) struct BlockingTestHook;

impl Hook for BlockingTestHook {
    fn name(&self) -> &'static str {
        "search_budget"
    }

    fn handle(&self, _event: &HookEvent, _context: &HookContext) -> HookResult {
        HookResult::Block {
            reason: "test hook blocked".to_string(),
        }
    }
}
