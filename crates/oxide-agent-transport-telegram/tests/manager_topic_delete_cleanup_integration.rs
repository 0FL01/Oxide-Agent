use async_trait::async_trait;
use oxide_agent_core::agent::providers::{
    ForumTopicActionResult, ForumTopicCreateRequest, ForumTopicCreateResult, ForumTopicEditRequest,
    ForumTopicEditResult, ForumTopicThreadRequest, ManagerControlPlaneProvider,
    ManagerTopicLifecycle, ManagerTopicSandboxCleanup,
};
use oxide_agent_core::agent::{AgentMemory, ToolProvider};
use oxide_agent_core::storage::{
    AgentFlowRecord, AgentProfileRecord, AppendAuditEventOptions, AuditEventRecord,
    Message as StoredMessage, StorageError, StorageProvider, TopicBindingRecord,
    UpsertAgentProfileOptions, UpsertTopicBindingOptions, UserConfig, UserContextConfig,
};
use oxide_agent_transport_telegram::bot::thread::{
    resolve_thread_spec_from_context, thread_peer_key_from_spec,
};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Mutex;
use teloxide::types::{ChatId, MessageId, ThreadId};

#[derive(Default)]
struct IntegrationStorage {
    user_config: Mutex<UserConfig>,
    cleared_histories: Mutex<Vec<String>>,
    cleared_memories: Mutex<Vec<String>>,
    deleted_bindings: Mutex<Vec<String>>,
    audit_events: Mutex<Vec<AppendAuditEventOptions>>,
}

impl IntegrationStorage {
    fn lock_error() -> StorageError {
        StorageError::Config("integration storage lock poisoned".to_string())
    }

    fn with_topic_context(context_key: &str) -> Self {
        Self {
            user_config: Mutex::new(UserConfig {
                contexts: HashMap::from([(
                    context_key.to_string(),
                    UserContextConfig {
                        state: Some("agent_mode".to_string()),
                        current_chat_uuid: Some("chat-1".to_string()),
                        current_agent_flow_id: Some("flow-1".to_string()),
                        chat_id: Some(-100_123),
                        thread_id: Some(77),
                        forum_topic_name: Some("Topic 77".to_string()),
                        forum_topic_icon_color: Some(7_322_096),
                        forum_topic_icon_custom_emoji_id: None,
                        forum_topic_closed: false,
                    },
                )]),
                ..UserConfig::default()
            }),
            ..Self::default()
        }
    }
}

#[async_trait]
impl StorageProvider for IntegrationStorage {
    async fn get_user_config(&self, _user_id: i64) -> Result<UserConfig, StorageError> {
        self.user_config
            .lock()
            .map(|config| config.clone())
            .map_err(|_| Self::lock_error())
    }

    async fn update_user_config(
        &self,
        _user_id: i64,
        config: UserConfig,
    ) -> Result<(), StorageError> {
        let mut guard = self.user_config.lock().map_err(|_| Self::lock_error())?;
        *guard = config;
        Ok(())
    }

    async fn update_user_prompt(
        &self,
        _user_id: i64,
        _system_prompt: String,
    ) -> Result<(), StorageError> {
        Ok(())
    }

    async fn get_user_prompt(&self, _user_id: i64) -> Result<Option<String>, StorageError> {
        Ok(None)
    }

    async fn update_user_model(
        &self,
        _user_id: i64,
        _model_name: String,
    ) -> Result<(), StorageError> {
        Ok(())
    }

    async fn get_user_model(&self, _user_id: i64) -> Result<Option<String>, StorageError> {
        Ok(None)
    }

    async fn update_user_state(&self, _user_id: i64, _state: String) -> Result<(), StorageError> {
        Ok(())
    }

    async fn get_user_state(&self, _user_id: i64) -> Result<Option<String>, StorageError> {
        Ok(None)
    }

    async fn save_message(
        &self,
        _user_id: i64,
        _role: String,
        _content: String,
    ) -> Result<(), StorageError> {
        Ok(())
    }

    async fn get_chat_history(
        &self,
        _user_id: i64,
        _limit: usize,
    ) -> Result<Vec<StoredMessage>, StorageError> {
        Ok(Vec::new())
    }

    async fn clear_chat_history(&self, _user_id: i64) -> Result<(), StorageError> {
        Ok(())
    }

    async fn save_message_for_chat(
        &self,
        _user_id: i64,
        _chat_uuid: String,
        _role: String,
        _content: String,
    ) -> Result<(), StorageError> {
        Ok(())
    }

    async fn get_chat_history_for_chat(
        &self,
        _user_id: i64,
        _chat_uuid: String,
        _limit: usize,
    ) -> Result<Vec<StoredMessage>, StorageError> {
        Ok(Vec::new())
    }

    async fn clear_chat_history_for_chat(
        &self,
        _user_id: i64,
        _chat_uuid: String,
    ) -> Result<(), StorageError> {
        Ok(())
    }

    async fn clear_chat_history_for_context(
        &self,
        _user_id: i64,
        context_key: String,
    ) -> Result<(), StorageError> {
        self.cleared_histories
            .lock()
            .map_err(|_| Self::lock_error())?
            .push(context_key);
        Ok(())
    }

    async fn save_agent_memory(
        &self,
        _user_id: i64,
        _memory: &AgentMemory,
    ) -> Result<(), StorageError> {
        Ok(())
    }

    async fn load_agent_memory(&self, _user_id: i64) -> Result<Option<AgentMemory>, StorageError> {
        Ok(None)
    }

    async fn clear_agent_memory(&self, _user_id: i64) -> Result<(), StorageError> {
        Ok(())
    }

    async fn get_agent_flow_record(
        &self,
        _user_id: i64,
        _context_key: String,
        _flow_id: String,
    ) -> Result<Option<AgentFlowRecord>, StorageError> {
        Ok(None)
    }

    async fn upsert_agent_flow_record(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<AgentFlowRecord, StorageError> {
        Ok(AgentFlowRecord {
            schema_version: 1,
            user_id,
            context_key,
            flow_id,
            created_at: 0,
            updated_at: 0,
        })
    }

    async fn clear_agent_memory_for_context(
        &self,
        _user_id: i64,
        context_key: String,
    ) -> Result<(), StorageError> {
        self.cleared_memories
            .lock()
            .map_err(|_| Self::lock_error())?
            .push(context_key);
        Ok(())
    }

    async fn clear_all_context(&self, _user_id: i64) -> Result<(), StorageError> {
        Ok(())
    }

    async fn check_connection(&self) -> Result<(), String> {
        Ok(())
    }

    async fn get_agent_profile(
        &self,
        _user_id: i64,
        _agent_id: String,
    ) -> Result<Option<AgentProfileRecord>, StorageError> {
        Ok(None)
    }

    async fn upsert_agent_profile(
        &self,
        _options: UpsertAgentProfileOptions,
    ) -> Result<AgentProfileRecord, StorageError> {
        Err(StorageError::Config(
            "not needed in integration test".to_string(),
        ))
    }

    async fn delete_agent_profile(
        &self,
        _user_id: i64,
        _agent_id: String,
    ) -> Result<(), StorageError> {
        Ok(())
    }

    async fn get_topic_binding(
        &self,
        _user_id: i64,
        _topic_id: String,
    ) -> Result<Option<TopicBindingRecord>, StorageError> {
        Ok(None)
    }

    async fn upsert_topic_binding(
        &self,
        _options: UpsertTopicBindingOptions,
    ) -> Result<TopicBindingRecord, StorageError> {
        Err(StorageError::Config(
            "not needed in integration test".to_string(),
        ))
    }

    async fn delete_topic_binding(
        &self,
        _user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError> {
        self.deleted_bindings
            .lock()
            .map_err(|_| Self::lock_error())?
            .push(topic_id);
        Ok(())
    }

    async fn append_audit_event(
        &self,
        options: AppendAuditEventOptions,
    ) -> Result<AuditEventRecord, StorageError> {
        self.audit_events
            .lock()
            .map_err(|_| Self::lock_error())?
            .push(options.clone());
        Ok(AuditEventRecord {
            schema_version: 1,
            version: 1,
            event_id: "evt-1".to_string(),
            user_id: options.user_id,
            topic_id: options.topic_id,
            agent_id: options.agent_id,
            action: options.action,
            payload: options.payload,
            created_at: 100,
        })
    }

    async fn list_audit_events(
        &self,
        _user_id: i64,
        _limit: usize,
    ) -> Result<Vec<AuditEventRecord>, StorageError> {
        Ok(Vec::new())
    }

    async fn list_audit_events_page(
        &self,
        _user_id: i64,
        _before_version: Option<u64>,
        _limit: usize,
    ) -> Result<Vec<AuditEventRecord>, StorageError> {
        Ok(Vec::new())
    }
}

#[derive(Default)]
struct RecordingLifecycle {
    deleted_topics: Mutex<Vec<ForumTopicThreadRequest>>,
}

#[async_trait]
impl ManagerTopicLifecycle for RecordingLifecycle {
    async fn forum_topic_create(
        &self,
        _request: ForumTopicCreateRequest,
    ) -> anyhow::Result<ForumTopicCreateResult> {
        anyhow::bail!("not used in this integration test")
    }

    async fn forum_topic_edit(
        &self,
        _request: ForumTopicEditRequest,
    ) -> anyhow::Result<ForumTopicEditResult> {
        anyhow::bail!("not used in this integration test")
    }

    async fn forum_topic_close(
        &self,
        _request: ForumTopicThreadRequest,
    ) -> anyhow::Result<ForumTopicActionResult> {
        anyhow::bail!("not used in this integration test")
    }

    async fn forum_topic_reopen(
        &self,
        _request: ForumTopicThreadRequest,
    ) -> anyhow::Result<ForumTopicActionResult> {
        anyhow::bail!("not used in this integration test")
    }

    async fn forum_topic_delete(
        &self,
        request: ForumTopicThreadRequest,
    ) -> anyhow::Result<ForumTopicActionResult> {
        self.deleted_topics
            .lock()
            .expect("mutex poisoned")
            .push(request.clone());
        Ok(ForumTopicActionResult {
            chat_id: request
                .chat_id
                .expect("chat id is required for integration test"),
            thread_id: request.thread_id,
        })
    }
}

#[derive(Default)]
struct RecordingSandboxCleanup {
    calls: Mutex<Vec<(i64, i64, i64)>>,
}

#[async_trait]
impl ManagerTopicSandboxCleanup for RecordingSandboxCleanup {
    async fn cleanup_topic_sandbox(
        &self,
        user_id: i64,
        topic: &ForumTopicActionResult,
    ) -> anyhow::Result<()> {
        self.calls
            .lock()
            .expect("mutex poisoned")
            .push((user_id, topic.chat_id, topic.thread_id));
        Ok(())
    }
}

#[tokio::test]
async fn manager_forum_topic_delete_cleans_transport_topic_scope() -> anyhow::Result<()> {
    let user_id = 77;
    let spec = resolve_thread_spec_from_context(true, true, Some(ThreadId(MessageId(77))));
    let context_key = thread_peer_key_from_spec(ChatId(-100_123), spec);
    let storage = std::sync::Arc::new(IntegrationStorage::with_topic_context(&context_key));
    let lifecycle = std::sync::Arc::new(RecordingLifecycle::default());
    let sandbox_cleanup = std::sync::Arc::new(RecordingSandboxCleanup::default());
    let provider = ManagerControlPlaneProvider::new(storage.clone(), user_id)
        .with_topic_lifecycle(lifecycle.clone())
        .with_topic_sandbox_cleanup(sandbox_cleanup.clone());

    let response = provider
        .execute(
            "forum_topic_delete",
            r#"{"chat_id":-100123,"thread_id":77}"#,
            None,
            None,
        )
        .await?;

    let parsed: serde_json::Value = serde_json::from_str(&response)?;
    assert_eq!(parsed["ok"], json!(true));
    assert_eq!(parsed["cleanup"]["context_key"], json!(context_key));
    assert_eq!(parsed["cleanup"]["deleted_container"], json!(true));
    assert_eq!(
        parsed["cleanup"]["deleted_topic_binding_keys"],
        json!(["-100123:77", "77"])
    );

    assert_eq!(
        lifecycle
            .deleted_topics
            .lock()
            .expect("mutex poisoned")
            .as_slice(),
        &[ForumTopicThreadRequest {
            chat_id: Some(-100_123),
            thread_id: 77,
        }]
    );
    assert_eq!(
        sandbox_cleanup
            .calls
            .lock()
            .expect("mutex poisoned")
            .as_slice(),
        &[(user_id, -100_123, 77)]
    );
    assert_eq!(
        storage
            .cleared_histories
            .lock()
            .expect("mutex poisoned")
            .as_slice(),
        std::slice::from_ref(&context_key)
    );
    assert_eq!(
        storage
            .cleared_memories
            .lock()
            .expect("mutex poisoned")
            .as_slice(),
        std::slice::from_ref(&context_key)
    );
    assert_eq!(
        storage
            .deleted_bindings
            .lock()
            .expect("mutex poisoned")
            .as_slice(),
        &[context_key.clone(), "77".to_string()]
    );
    assert!(!storage
        .get_user_config(user_id)
        .await?
        .contexts
        .contains_key(&context_key));
    assert_eq!(
        storage.audit_events.lock().expect("mutex poisoned").len(),
        1
    );

    Ok(())
}
