// Allow clone_on_ref_ptr in tests due to trait object coercion requirements
#![allow(clippy::clone_on_ref_ptr)]

mod basics;
mod manager;
mod persistent_memory;
mod registry;
mod resume;

pub(super) use super::policy_hooks::PolicyControlledHook;
pub(super) use super::AgentExecutor;
pub(super) use crate::agent::hooks::{Hook, HookContext, HookEvent, HookResult};
pub(super) use crate::agent::persistent_memory::{
    MemoryClassificationDecision, MemoryTaskClassifier, PersistentMemoryCoordinator,
    PostRunMemoryWriter, PostRunMemoryWriterInput, ValidatedPostRunEpisode,
    ValidatedPostRunMemoryWrite,
};
pub(super) use crate::agent::profile::HookAccessPolicy;
pub(super) use crate::agent::providers::TodoList;
pub(super) use crate::agent::providers::{
    ForumTopicActionResult, ForumTopicCreateRequest, ForumTopicCreateResult, ForumTopicEditRequest,
    ForumTopicEditResult, ForumTopicThreadRequest, ManagerTopicLifecycle,
};
pub(super) use crate::agent::session::{AgentSession, PendingUserInput, UserInputKind};
pub(super) use crate::config::AgentSettings;
pub(super) use crate::llm::LlmClient;
pub(super) use crate::storage::{
    AppendAuditEventOptions, AuditEventRecord, MockStorageProvider, TopicBindingKind,
    TopicBindingRecord,
};
pub(super) use anyhow::{bail, Result};
pub(super) use mockall::predicate::eq;
pub(super) use oxide_agent_memory::{CleanupStatus, InMemoryMemoryRepository, MemoryRepository};
pub(super) use serde_json::json;
pub(super) use std::sync::{Arc, Mutex as StdMutex};
pub(super) use tokio::sync::Mutex;

pub(super) struct RecordingTopicLifecycle {
    create_calls: StdMutex<Vec<ForumTopicCreateRequest>>,
}

pub(super) struct StubMemoryTaskClassifier {
    result: StdMutex<Option<Result<MemoryClassificationDecision>>>,
}

pub(super) struct StubPostRunMemoryWriter;

impl StubMemoryTaskClassifier {
    pub(super) fn success(decision: MemoryClassificationDecision) -> Self {
        Self {
            result: StdMutex::new(Some(Ok(decision))),
        }
    }

    pub(super) fn failure(error: anyhow::Error) -> Self {
        Self {
            result: StdMutex::new(Some(Err(error))),
        }
    }
}

#[async_trait::async_trait]
impl MemoryTaskClassifier for StubMemoryTaskClassifier {
    async fn classify(&self, _task: &str) -> Result<MemoryClassificationDecision> {
        self.result
            .lock()
            .expect("stub classifier mutex poisoned")
            .take()
            .expect("stub classifier must only be called once")
    }
}

#[async_trait::async_trait]
impl PostRunMemoryWriter for StubPostRunMemoryWriter {
    async fn write(
        &self,
        input: &PostRunMemoryWriterInput<'_>,
    ) -> Result<ValidatedPostRunMemoryWrite> {
        Ok(ValidatedPostRunMemoryWrite {
            thread_short_summary: Some(format!("{} summary", input.task)),
            episode: ValidatedPostRunEpisode {
                summary: format!("Completed task: {}", input.task),
                outcome: oxide_agent_memory::EpisodeOutcome::Success,
                failures: Vec::new(),
                importance: 0.9,
            },
            memories: Vec::new(),
        })
    }
}

impl RecordingTopicLifecycle {
    pub(super) fn new() -> Self {
        Self {
            create_calls: StdMutex::new(Vec::new()),
        }
    }

    pub(super) fn create_calls(&self) -> Vec<ForumTopicCreateRequest> {
        match self.create_calls.lock() {
            Ok(calls) => calls.clone(),
            Err(_) => Vec::new(),
        }
    }
}

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
        agent_model_id: Some("mock-model".to_string()),
        agent_model_provider: Some("mock".to_string()),
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
        .expect_chat_completion()
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
    llm.register_provider("mock".to_string(), Arc::new(provider));
    let session = AgentSession::new(9_i64.into());
    AgentExecutor::new(Arc::new(llm), session, settings)
}

pub(super) fn verbose_turn(label: &str, idx: usize, repeat: usize) -> String {
    let repeated = std::iter::repeat_n(label, repeat)
        .collect::<Vec<_>>()
        .join(" ");
    format!("{label} turn {idx}: {repeated}")
}

pub(super) fn build_audit_record(options: AppendAuditEventOptions) -> AuditEventRecord {
    AuditEventRecord {
        schema_version: 1,
        version: 1,
        event_id: "evt-1".to_string(),
        user_id: options.user_id,
        topic_id: options.topic_id,
        agent_id: options.agent_id,
        action: options.action,
        payload: options.payload,
        created_at: 100,
    }
}

pub(super) struct BlockingTestHook;

impl Hook for BlockingTestHook {
    fn name(&self) -> &'static str {
        "workload_distributor"
    }

    fn handle(&self, _event: &HookEvent, _context: &HookContext) -> HookResult {
        HookResult::Block {
            reason: "test hook blocked".to_string(),
        }
    }
}
