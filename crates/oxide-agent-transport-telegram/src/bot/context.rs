use crate::bot::{TelegramThreadKind, TelegramThreadSpec, thread_peer_key_from_spec};
use anyhow::Result;
use oxide_agent_core::sandbox::SandboxScope;
use oxide_agent_core::storage::{StorageProvider, UserContextConfig, generate_flow_id};
use std::sync::Arc;
use teloxide::types::ChatId;

fn should_mirror_dm_global_state(thread_spec: TelegramThreadSpec) -> bool {
    matches!(thread_spec.kind, TelegramThreadKind::Dm)
}

#[must_use]
pub(crate) fn storage_context_key(chat_id: ChatId, thread_spec: TelegramThreadSpec) -> String {
    thread_peer_key_from_spec(chat_id, thread_spec)
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
            .map(|thread_id| i64::from(thread_id.0.0)),
    )
}

#[must_use]
pub(crate) fn resolved_context_state(
    context: Option<&UserContextConfig>,
    global_state: Option<String>,
    thread_spec: TelegramThreadSpec,
) -> Option<String> {
    context
        .and_then(|context| context.state.clone())
        .or_else(|| {
            should_mirror_dm_global_state(thread_spec)
                .then_some(global_state)
                .flatten()
        })
}

pub(crate) async fn current_context_state(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    chat_id: ChatId,
    thread_spec: TelegramThreadSpec,
) -> Result<Option<String>> {
    let context_key = storage_context_key(chat_id, thread_spec);
    let context = storage.get_user_context(user_id, &context_key).await?;
    let global_state = if should_mirror_dm_global_state(thread_spec) {
        storage.get_user_state(user_id).await?
    } else {
        None
    };
    Ok(resolved_context_state(
        context.as_ref(),
        global_state,
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
    let context_key = storage_context_key(chat_id, thread_spec);
    storage
        .set_context_state(
            user_id,
            &context_key,
            state.map(str::to_string),
            chat_id.0,
            thread_spec
                .thread_id
                .map(|thread_id| i64::from(thread_id.0.0)),
            should_mirror_dm_global_state(thread_spec),
        )
        .await?;
    Ok(())
}

pub(crate) async fn ensure_current_agent_flow_id(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    chat_id: ChatId,
    thread_spec: TelegramThreadSpec,
) -> Result<(String, bool)> {
    let context_key = storage_context_key(chat_id, thread_spec);
    storage
        .ensure_context_agent_flow(
            user_id,
            &context_key,
            generate_flow_id(),
            chat_id.0,
            thread_spec
                .thread_id
                .map(|thread_id| i64::from(thread_id.0.0)),
        )
        .await
        .map_err(Into::into)
}

pub(crate) async fn set_current_agent_flow_id(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    chat_id: ChatId,
    thread_spec: TelegramThreadSpec,
    flow_id: String,
) -> Result<()> {
    let context_key = storage_context_key(chat_id, thread_spec);
    storage
        .set_context_agent_flow(
            user_id,
            &context_key,
            flow_id,
            chat_id.0,
            thread_spec
                .thread_id
                .map(|thread_id| i64::from(thread_id.0.0)),
        )
        .await?;
    Ok(())
}

pub(crate) async fn reset_current_agent_flow_id(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    chat_id: ChatId,
    thread_spec: TelegramThreadSpec,
) -> Result<String> {
    let flow_id = generate_flow_id();
    set_current_agent_flow_id(storage, user_id, chat_id, thread_spec, flow_id.clone()).await?;
    Ok(flow_id)
}

#[cfg(test)]
mod tests {
    use super::{
        ensure_current_agent_flow_id, reset_current_agent_flow_id, resolved_context_state,
        sandbox_scope, storage_context_key,
    };
    use crate::bot::resolve_thread_spec_from_context;
    use async_trait::async_trait;
    use oxide_agent_core::agent::AgentMemory;
    use oxide_agent_core::storage::{
        AgentProfileRecord, AppendAuditEventOptions, AuditEventRecord, StorageError,
        StorageProvider, TopicBindingRecord, UpsertAgentProfileOptions, UpsertTopicBindingOptions,
        UserContextConfig,
    };
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use teloxide::types::{ChatId, MessageId, ThreadId};

    #[derive(Default)]
    struct ConfigState {
        state: Option<String>,
        contexts: HashMap<String, UserContextConfig>,
    }

    #[derive(Default)]
    struct ConfigStorage {
        config: Mutex<ConfigState>,
    }

    #[async_trait]
    impl StorageProvider for ConfigStorage {
        async fn get_user_context(
            &self,
            _user_id: i64,
            context_key: &str,
        ) -> Result<Option<UserContextConfig>, StorageError> {
            self.config
                .lock()
                .map(|config| config.contexts.get(context_key).cloned())
                .map_err(|_| StorageError::Config("config mutex poisoned".to_string()))
        }

        async fn set_context_state(
            &self,
            _user_id: i64,
            context_key: &str,
            state: Option<String>,
            chat_id: i64,
            thread_id: Option<i64>,
            mirror_global_state: bool,
        ) -> Result<(), StorageError> {
            let mut config = self
                .config
                .lock()
                .map_err(|_| StorageError::Config("config mutex poisoned".to_string()))?;
            let context = config.contexts.entry(context_key.to_string()).or_default();
            context.state = state.clone();
            context.chat_id = Some(chat_id);
            context.thread_id = thread_id;
            if mirror_global_state {
                config.state = state;
            }
            Ok(())
        }

        async fn ensure_context_agent_flow(
            &self,
            _user_id: i64,
            context_key: &str,
            new_flow_id: String,
            chat_id: i64,
            thread_id: Option<i64>,
        ) -> Result<(String, bool), StorageError> {
            let mut config = self
                .config
                .lock()
                .map_err(|_| StorageError::Config("config mutex poisoned".to_string()))?;
            let context = config.contexts.entry(context_key.to_string()).or_default();
            if let Some(flow_id) = context.current_agent_flow_id.clone() {
                return Ok((flow_id, false));
            }
            context.current_agent_flow_id = Some(new_flow_id.clone());
            context.chat_id = Some(chat_id);
            context.thread_id = thread_id;
            Ok((new_flow_id, true))
        }

        async fn set_context_agent_flow(
            &self,
            _user_id: i64,
            context_key: &str,
            flow_id: String,
            chat_id: i64,
            thread_id: Option<i64>,
        ) -> Result<(), StorageError> {
            let mut config = self
                .config
                .lock()
                .map_err(|_| StorageError::Config("config mutex poisoned".to_string()))?;
            let context = config.contexts.entry(context_key.to_string()).or_default();
            context.current_agent_flow_id = Some(flow_id);
            context.chat_id = Some(chat_id);
            context.thread_id = thread_id;
            Ok(())
        }

        async fn get_user_state(&self, _user_id: i64) -> Result<Option<String>, StorageError> {
            self.config
                .lock()
                .map(|config| config.state.clone())
                .map_err(|_| StorageError::Config("config mutex poisoned".to_string()))
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
    fn sandbox_scope_reuses_topic_context_key() {
        let spec = resolve_thread_spec_from_context(true, true, Some(ThreadId(MessageId(42))));
        let scope = sandbox_scope(77, ChatId(-1001), spec);

        assert_eq!(scope.namespace(), "-1001:42");
        assert_eq!(scope.chat_id(), Some(-1001));
        assert_eq!(scope.thread_id(), Some(42));
    }

    #[test]
    fn forum_context_state_does_not_read_dm_global_state() {
        let spec = resolve_thread_spec_from_context(true, true, Some(ThreadId(MessageId(99))));

        assert_eq!(
            resolved_context_state(None, Some("dm_state".to_string()), spec),
            None
        );
    }

    #[tokio::test]
    async fn ensure_current_agent_flow_id_only_touches_requested_context() {
        let storage: Arc<dyn StorageProvider> = Arc::new(ConfigStorage {
            config: Mutex::new(ConfigState {
                state: None,
                contexts: HashMap::from([(
                    "-1001:77".to_string(),
                    UserContextConfig {
                        state: Some("agent_mode".to_string()),
                        current_agent_flow_id: Some("flow-b".to_string()),
                        chat_id: Some(-1001),
                        thread_id: Some(77),
                        forum_topic_name: None,
                        forum_topic_icon_color: None,
                        forum_topic_icon_custom_emoji_id: None,
                        forum_topic_closed: false,
                    },
                )]),
            }),
        });
        let thread_spec =
            resolve_thread_spec_from_context(true, true, Some(ThreadId(MessageId(42))));
        let (flow_id, created) =
            ensure_current_agent_flow_id(&storage, 7, ChatId(-1001), thread_spec)
                .await
                .expect("ensure must succeed");

        let saved = storage
            .get_user_context(7, "-1001:42")
            .await
            .expect("context load must succeed")
            .expect("requested context must exist");
        let other = storage
            .get_user_context(7, "-1001:77")
            .await
            .expect("context load must succeed")
            .expect("other context must exist");
        assert!(created);
        assert_eq!(
            saved.current_agent_flow_id.as_deref(),
            Some(flow_id.as_str())
        );
        assert_eq!(other.current_agent_flow_id.as_deref(), Some("flow-b"));
    }

    #[tokio::test]
    async fn reset_current_agent_flow_id_only_touches_requested_context() {
        let storage: Arc<dyn StorageProvider> = Arc::new(ConfigStorage {
            config: Mutex::new(ConfigState {
                state: None,
                contexts: HashMap::from([
                    (
                        "-1001:42".to_string(),
                        UserContextConfig {
                            state: Some("agent_mode".to_string()),
                            current_agent_flow_id: Some("flow-a".to_string()),
                            chat_id: Some(-1001),
                            thread_id: Some(42),
                            forum_topic_name: None,
                            forum_topic_icon_color: None,
                            forum_topic_icon_custom_emoji_id: None,
                            forum_topic_closed: false,
                        },
                    ),
                    (
                        "-1001:77".to_string(),
                        UserContextConfig {
                            state: Some("agent_mode".to_string()),
                            current_agent_flow_id: Some("flow-b".to_string()),
                            chat_id: Some(-1001),
                            thread_id: Some(77),
                            forum_topic_name: None,
                            forum_topic_icon_color: None,
                            forum_topic_icon_custom_emoji_id: None,
                            forum_topic_closed: false,
                        },
                    ),
                ]),
            }),
        });
        let thread_spec =
            resolve_thread_spec_from_context(true, true, Some(ThreadId(MessageId(42))));
        let new_flow_id = reset_current_agent_flow_id(&storage, 7, ChatId(-1001), thread_spec)
            .await
            .expect("reset must succeed");

        let saved = storage
            .get_user_context(7, "-1001:42")
            .await
            .expect("context load must succeed")
            .expect("requested context must exist");
        let other = storage
            .get_user_context(7, "-1001:77")
            .await
            .expect("context load must succeed")
            .expect("other context must exist");
        assert_ne!(new_flow_id, "flow-a");
        assert_eq!(
            saved.current_agent_flow_id.as_deref(),
            Some(new_flow_id.as_str())
        );
        assert_eq!(other.current_agent_flow_id.as_deref(), Some("flow-b"));
    }
}
