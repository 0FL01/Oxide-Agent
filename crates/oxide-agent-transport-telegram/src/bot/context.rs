use crate::bot::{thread_peer_key_from_spec, TelegramThreadKind, TelegramThreadSpec};
use anyhow::Result;
use oxide_agent_core::sandbox::SandboxScope;
use oxide_agent_core::storage::{
    generate_chat_uuid, StorageProvider, UserConfig, UserContextConfig,
};
use std::sync::Arc;
use teloxide::types::ChatId;

fn should_use_legacy_fallback(thread_spec: TelegramThreadSpec) -> bool {
    matches!(thread_spec.kind, TelegramThreadKind::Dm)
}

fn context_entry_mut<'a>(
    config: &'a mut UserConfig,
    context_key: &str,
    chat_id: ChatId,
    thread_spec: TelegramThreadSpec,
) -> &'a mut UserContextConfig {
    let entry = config.contexts.entry(context_key.to_string()).or_default();
    entry.chat_id = Some(chat_id.0);
    entry.thread_id = thread_spec
        .thread_id
        .map(|thread_id| i64::from(thread_id.0 .0));
    entry
}

#[must_use]
pub(crate) fn storage_context_key(chat_id: ChatId, thread_spec: TelegramThreadSpec) -> String {
    thread_peer_key_from_spec(chat_id, thread_spec)
}

#[must_use]
pub(crate) fn scoped_chat_storage_id(context_key: &str, chat_uuid: &str) -> String {
    format!("{context_key}/{chat_uuid}")
}

#[must_use]
pub(crate) fn sandbox_scope(
    user_id: i64,
    chat_id: ChatId,
    thread_spec: TelegramThreadSpec,
) -> SandboxScope {
    SandboxScope::new(user_id, storage_context_key(chat_id, thread_spec)).with_transport_metadata(
        Some(chat_id.0),
        thread_spec
            .thread_id
            .map(|thread_id| i64::from(thread_id.0 .0)),
    )
}

#[must_use]
pub(crate) fn current_context_state_from_config(
    config: &UserConfig,
    context_key: &str,
    thread_spec: TelegramThreadSpec,
) -> Option<String> {
    config
        .contexts
        .get(context_key)
        .and_then(|context| context.state.clone())
        .or_else(|| {
            should_use_legacy_fallback(thread_spec)
                .then(|| config.state.clone())
                .flatten()
        })
}

pub(crate) async fn current_context_state(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    chat_id: ChatId,
    thread_spec: TelegramThreadSpec,
) -> Result<Option<String>> {
    let config = storage.get_user_config(user_id).await?;
    Ok(current_context_state_from_config(
        &config,
        &storage_context_key(chat_id, thread_spec),
        thread_spec,
    ))
}

pub(crate) async fn set_current_context_state(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    chat_id: ChatId,
    thread_spec: TelegramThreadSpec,
    state: Option<&str>,
) -> Result<()> {
    let mut config = storage.get_user_config(user_id).await?;
    let context_key = storage_context_key(chat_id, thread_spec);
    let context = context_entry_mut(&mut config, &context_key, chat_id, thread_spec);
    context.state = state.map(str::to_string);

    if should_use_legacy_fallback(thread_spec) {
        config.state = context.state.clone();
    }

    storage.update_user_config(user_id, config).await?;
    Ok(())
}

pub(crate) async fn ensure_current_chat_uuid(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    chat_id: ChatId,
    thread_spec: TelegramThreadSpec,
) -> Result<String> {
    let mut config = storage.get_user_config(user_id).await?;
    let context_key = storage_context_key(chat_id, thread_spec);

    if let Some(chat_uuid) = config
        .contexts
        .get(&context_key)
        .and_then(|context| context.current_chat_uuid.clone())
    {
        return Ok(chat_uuid);
    }

    if should_use_legacy_fallback(thread_spec) {
        if let Some(chat_uuid) = config.current_chat_uuid.clone() {
            let context = context_entry_mut(&mut config, &context_key, chat_id, thread_spec);
            context.current_chat_uuid = Some(chat_uuid.clone());
            storage.update_user_config(user_id, config).await?;
            return Ok(chat_uuid);
        }
    }

    let chat_uuid = generate_chat_uuid();
    let context = context_entry_mut(&mut config, &context_key, chat_id, thread_spec);
    context.current_chat_uuid = Some(chat_uuid.clone());

    if should_use_legacy_fallback(thread_spec) {
        config.current_chat_uuid = Some(chat_uuid.clone());
    }

    storage.update_user_config(user_id, config).await?;
    Ok(chat_uuid)
}

pub(crate) async fn reset_current_chat_uuid(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    chat_id: ChatId,
    thread_spec: TelegramThreadSpec,
) -> Result<String> {
    let mut config = storage.get_user_config(user_id).await?;
    let context_key = storage_context_key(chat_id, thread_spec);
    let chat_uuid = generate_chat_uuid();
    let context = context_entry_mut(&mut config, &context_key, chat_id, thread_spec);
    context.current_chat_uuid = Some(chat_uuid.clone());

    if should_use_legacy_fallback(thread_spec) {
        config.current_chat_uuid = Some(chat_uuid.clone());
    }

    storage.update_user_config(user_id, config).await?;
    Ok(chat_uuid)
}

#[cfg(test)]
mod tests {
    use super::{
        current_context_state_from_config, reset_current_chat_uuid, sandbox_scope,
        scoped_chat_storage_id, storage_context_key,
    };
    use crate::bot::resolve_thread_spec_from_context;
    use async_trait::async_trait;
    use oxide_agent_core::agent::AgentMemory;
    use oxide_agent_core::storage::{
        AgentProfileRecord, AppendAuditEventOptions, AuditEventRecord, Message, StorageError,
        StorageProvider, TopicBindingRecord, UpsertAgentProfileOptions, UpsertTopicBindingOptions,
        UserConfig, UserContextConfig,
    };
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use teloxide::types::{ChatId, MessageId, ThreadId};

    #[derive(Default)]
    struct ConfigStorage {
        config: Mutex<UserConfig>,
    }

    #[async_trait]
    impl StorageProvider for ConfigStorage {
        async fn get_user_config(&self, _user_id: i64) -> Result<UserConfig, StorageError> {
            self.config
                .lock()
                .map(|config| config.clone())
                .map_err(|_| StorageError::Config("config mutex poisoned".to_string()))
        }

        async fn update_user_config(
            &self,
            _user_id: i64,
            config: UserConfig,
        ) -> Result<(), StorageError> {
            let mut guard = self
                .config
                .lock()
                .map_err(|_| StorageError::Config("config mutex poisoned".to_string()))?;
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

        async fn update_user_state(
            &self,
            _user_id: i64,
            _state: String,
        ) -> Result<(), StorageError> {
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
        ) -> Result<Vec<Message>, StorageError> {
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
        ) -> Result<Vec<Message>, StorageError> {
            Ok(Vec::new())
        }

        async fn clear_chat_history_for_chat(
            &self,
            _user_id: i64,
            _chat_uuid: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn save_agent_memory(
            &self,
            _user_id: i64,
            _memory: &AgentMemory,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn load_agent_memory(
            &self,
            _user_id: i64,
        ) -> Result<Option<AgentMemory>, StorageError> {
            Ok(None)
        }

        async fn clear_agent_memory(&self, _user_id: i64) -> Result<(), StorageError> {
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
                "not needed in context tests".to_string(),
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
                "not needed in context tests".to_string(),
            ))
        }

        async fn delete_topic_binding(
            &self,
            _user_id: i64,
            _topic_id: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn append_audit_event(
            &self,
            options: AppendAuditEventOptions,
        ) -> Result<AuditEventRecord, StorageError> {
            Ok(AuditEventRecord {
                schema_version: 1,
                version: 1,
                event_id: "evt-1".to_string(),
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                action: options.action,
                payload: options.payload,
                created_at: 0,
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

    #[test]
    fn storage_context_key_uses_chat_and_thread() {
        let spec = resolve_thread_spec_from_context(true, true, Some(ThreadId(MessageId(42))));
        assert_eq!(storage_context_key(ChatId(-1001), spec), "-1001:42");
    }

    #[test]
    fn scoped_chat_storage_id_nests_uuid_under_context() {
        assert_eq!(
            scoped_chat_storage_id("-1001:42", "chat-1"),
            "-1001:42/chat-1"
        );
    }

    #[test]
    fn sandbox_scope_reuses_topic_context_key() {
        let spec = resolve_thread_spec_from_context(true, true, Some(ThreadId(MessageId(42))));
        let scope = sandbox_scope(77, ChatId(-1001), spec);

        assert_eq!(scope.namespace(), "-1001:42");
        assert_eq!(scope.chat_id(), Some(-1001));
        assert_eq!(scope.thread_id(), Some(42));
    }

    #[test]
    fn forum_context_state_does_not_fall_back_to_legacy_global_state() {
        let mut contexts = HashMap::new();
        contexts.insert(
            "-1001:42".to_string(),
            UserContextConfig {
                state: Some("agent_mode".to_string()),
                current_chat_uuid: None,
                chat_id: Some(-1001),
                thread_id: Some(42),
            },
        );
        let config = UserConfig {
            state: Some("chat_mode".to_string()),
            contexts,
            ..UserConfig::default()
        };
        let spec = resolve_thread_spec_from_context(true, true, Some(ThreadId(MessageId(99))));

        assert_eq!(
            current_context_state_from_config(&config, "-1001:99", spec),
            None
        );
    }

    #[tokio::test]
    async fn reset_current_chat_uuid_only_touches_requested_context() {
        let storage: Arc<dyn StorageProvider> = Arc::new(ConfigStorage {
            config: Mutex::new(UserConfig {
                contexts: HashMap::from([
                    (
                        "-1001:42".to_string(),
                        UserContextConfig {
                            state: Some("chat_mode".to_string()),
                            current_chat_uuid: Some("chat-a".to_string()),
                            chat_id: Some(-1001),
                            thread_id: Some(42),
                        },
                    ),
                    (
                        "-1001:77".to_string(),
                        UserContextConfig {
                            state: Some("chat_mode".to_string()),
                            current_chat_uuid: Some("chat-b".to_string()),
                            chat_id: Some(-1001),
                            thread_id: Some(77),
                        },
                    ),
                ]),
                ..UserConfig::default()
            }),
        });
        let thread_spec =
            resolve_thread_spec_from_context(true, true, Some(ThreadId(MessageId(42))));
        let new_uuid = reset_current_chat_uuid(&storage, 7, ChatId(-1001), thread_spec)
            .await
            .expect("reset must succeed");

        let saved = storage
            .get_user_config(7)
            .await
            .expect("config load must succeed");
        assert_ne!(new_uuid, "chat-a");
        assert_eq!(
            saved
                .contexts
                .get("-1001:42")
                .and_then(|context| context.current_chat_uuid.as_deref()),
            Some(new_uuid.as_str())
        );
        assert_eq!(
            saved
                .contexts
                .get("-1001:77")
                .and_then(|context| context.current_chat_uuid.as_deref()),
            Some("chat-b")
        );
    }
}
