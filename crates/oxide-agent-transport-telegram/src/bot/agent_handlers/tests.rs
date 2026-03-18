use super::{
    agent_mode_session_keys, assemble_text_batch, cancel_status_reply_markup,
    cleanup_abandoned_empty_flow, clear_pending_cancel_confirmation, clear_pending_cancel_message,
    derive_agent_mode_session_id, ensure_session_exists, manager_control_plane_enabled,
    manager_default_chat_id, merge_prompt_instructions, parse_agent_callback_action,
    parse_agent_control_command, pending_cancel_confirmation, pending_cancel_message,
    remember_pending_cancel_confirmation, remember_pending_cancel_message,
    remove_sessions_with_compat, resolve_execution_profile, select_existing_session_id,
    session_manager_control_plane_enabled, should_create_fresh_flow_on_detach,
    should_merge_text_batch, take_pending_cancel_confirmation, take_pending_cancel_message,
    use_inline_flow_controls, AgentCallbackAction, AgentControlCommand, BatchedTextTaskContext,
    EnsureSessionContext, PendingTextInputBatch, PendingTextInputPart, SessionTransportContext,
    AGENT_TEXT_INPUT_SPLIT_THRESHOLD_CHARS, SESSION_REGISTRY,
};
use crate::bot::views::{
    AGENT_CALLBACK_CANCEL_TASK, AGENT_CALLBACK_CONFIRM_CANCEL_NO, AGENT_CALLBACK_CONFIRM_CANCEL_YES,
};
use crate::bot::{general_forum_topic_id, resolve_thread_spec_from_context};
use crate::config::{BotSettings, TelegramSettings};
use async_trait::async_trait;
use oxide_agent_core::agent::SessionId;
use oxide_agent_core::agent::{AgentMemory, AgentSession};
use oxide_agent_core::config::AgentSettings;
use oxide_agent_core::llm::LlmClient;
use oxide_agent_core::sandbox::SandboxScope;
use oxide_agent_core::storage::{
    AgentFlowRecord, AgentProfileRecord, AppendAuditEventOptions, AuditEventRecord, Message,
    StorageError, StorageProvider, TopicAgentsMdRecord, TopicBindingRecord,
    UpsertAgentProfileOptions, UpsertTopicBindingOptions, UserConfig,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use teloxide::types::{ChatId, MessageId, ReplyMarkup, ThreadId};
use teloxide::Bot;

fn test_batch() -> PendingTextInputBatch {
    PendingTextInputBatch::new(
        BatchedTextTaskContext {
            bot: Bot::new("TOKEN"),
            chat_id: ChatId(1),
            session_id: SessionId::from(7_i64),
            user_id: 1,
            storage: Arc::new(NoopStorage::default()),
            context_key: "ctx".to_string(),
            agent_flow_id: "flow".to_string(),
            message_thread_id: None,
            use_inline_progress_controls: false,
            use_inline_flow_controls: false,
        },
        MessageId(10),
        "x".repeat(AGENT_TEXT_INPUT_SPLIT_THRESHOLD_CHARS),
        Instant::now(),
    )
}

#[test]
fn merges_sequential_large_text_parts() {
    let batch = test_batch();

    assert!(should_merge_text_batch(
        &batch,
        MessageId(11),
        "tail",
        batch.updated_at + Duration::from_millis(200),
    ));
}

#[test]
fn does_not_merge_short_independent_messages() {
    let batch = PendingTextInputBatch {
        ctx: test_batch().ctx,
        parts: vec![PendingTextInputPart {
            message_id: MessageId(10),
            text: "short note".to_string(),
        }],
        revision: 1,
        updated_at: Instant::now(),
    };

    assert!(!should_merge_text_batch(
        &batch,
        MessageId(11),
        "another short note",
        batch.updated_at + Duration::from_millis(200),
    ));
}

#[test]
fn assembles_text_batch_in_order_without_extra_separators() {
    let parts = vec![
        PendingTextInputPart {
            message_id: MessageId(10),
            text: "first".to_string(),
        },
        PendingTextInputPart {
            message_id: MessageId(11),
            text: "second".to_string(),
        },
    ];

    assert_eq!(assemble_text_batch(&parts), "firstsecond");
}

#[derive(Default)]
struct NoopStorage {
    flow_memory: Option<AgentMemory>,
    agent_profile: Option<serde_json::Value>,
    topic_context: Option<String>,
    topic_agents_md: Option<String>,
    fail_flow_memory_lookup: bool,
    cleared_flows: Arc<Mutex<Vec<(String, String)>>>,
}

impl NoopStorage {
    fn with_flow_memory(flow_memory: Option<AgentMemory>) -> Self {
        Self {
            flow_memory,
            agent_profile: None,
            topic_context: None,
            topic_agents_md: None,
            fail_flow_memory_lookup: false,
            cleared_flows: Arc::default(),
        }
    }

    fn with_agent_profile_and_topic_context(
        agent_profile: serde_json::Value,
        topic_context: &str,
    ) -> Self {
        Self {
            flow_memory: None,
            agent_profile: Some(agent_profile),
            topic_context: Some(topic_context.to_string()),
            topic_agents_md: None,
            fail_flow_memory_lookup: false,
            cleared_flows: Arc::default(),
        }
    }

    fn with_topic_agents_md(topic_agents_md: &str) -> Self {
        Self {
            flow_memory: None,
            agent_profile: None,
            topic_context: None,
            topic_agents_md: Some(topic_agents_md.to_string()),
            fail_flow_memory_lookup: false,
            cleared_flows: Arc::default(),
        }
    }

    fn with_failed_flow_memory_lookup() -> Self {
        Self {
            flow_memory: None,
            agent_profile: None,
            topic_context: None,
            topic_agents_md: None,
            fail_flow_memory_lookup: true,
            cleared_flows: Arc::default(),
        }
    }
}

#[async_trait]
impl StorageProvider for NoopStorage {
    async fn get_user_config(&self, _user_id: i64) -> Result<UserConfig, StorageError> {
        Ok(UserConfig::default())
    }

    async fn update_user_config(
        &self,
        _user_id: i64,
        _config: UserConfig,
    ) -> Result<(), StorageError> {
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
        _memory: &oxide_agent_core::agent::AgentMemory,
    ) -> Result<(), StorageError> {
        Ok(())
    }

    async fn load_agent_memory(
        &self,
        _user_id: i64,
    ) -> Result<Option<oxide_agent_core::agent::AgentMemory>, StorageError> {
        Ok(None)
    }

    async fn clear_agent_memory(&self, _user_id: i64) -> Result<(), StorageError> {
        Ok(())
    }

    async fn clear_agent_memory_for_flow(
        &self,
        _user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<(), StorageError> {
        self.cleared_flows
            .lock()
            .expect("cleared_flows mutex poisoned")
            .push((context_key, flow_id));
        Ok(())
    }

    async fn load_agent_memory_for_flow(
        &self,
        _user_id: i64,
        _context_key: String,
        _flow_id: String,
    ) -> Result<Option<AgentMemory>, StorageError> {
        if self.fail_flow_memory_lookup {
            return Err(StorageError::Config(
                "flow memory lookup failed".to_string(),
            ));
        }

        Ok(self.flow_memory.clone())
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

    async fn clear_all_context(&self, _user_id: i64) -> Result<(), StorageError> {
        Ok(())
    }

    async fn check_connection(&self) -> Result<(), String> {
        Ok(())
    }

    async fn get_agent_profile(
        &self,
        user_id: i64,
        agent_id: String,
    ) -> Result<Option<AgentProfileRecord>, StorageError> {
        Ok(self
            .agent_profile
            .clone()
            .map(|profile| AgentProfileRecord {
                schema_version: 1,
                version: 1,
                user_id,
                agent_id,
                profile,
                created_at: 0,
                updated_at: 0,
            }))
    }

    async fn upsert_agent_profile(
        &self,
        _options: UpsertAgentProfileOptions,
    ) -> Result<AgentProfileRecord, StorageError> {
        Err(StorageError::Config("not needed in tests".to_string()))
    }

    async fn delete_agent_profile(
        &self,
        _user_id: i64,
        _agent_id: String,
    ) -> Result<(), StorageError> {
        Ok(())
    }

    async fn get_topic_context(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<oxide_agent_core::storage::TopicContextRecord>, StorageError> {
        Ok(self.topic_context.clone().map(|context| {
            oxide_agent_core::storage::TopicContextRecord {
                schema_version: 1,
                version: 1,
                user_id,
                topic_id,
                context,
                created_at: 0,
                updated_at: 0,
            }
        }))
    }

    async fn upsert_topic_context(
        &self,
        _options: oxide_agent_core::storage::UpsertTopicContextOptions,
    ) -> Result<oxide_agent_core::storage::TopicContextRecord, StorageError> {
        Err(StorageError::Config("not needed in tests".to_string()))
    }

    async fn delete_topic_context(
        &self,
        _user_id: i64,
        _topic_id: String,
    ) -> Result<(), StorageError> {
        Ok(())
    }

    async fn get_topic_agents_md(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicAgentsMdRecord>, StorageError> {
        Ok(self
            .topic_agents_md
            .clone()
            .map(|agents_md| TopicAgentsMdRecord {
                schema_version: 1,
                version: 1,
                user_id,
                topic_id,
                agents_md,
                created_at: 0,
                updated_at: 0,
            }))
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
        Err(StorageError::Config("not needed in tests".to_string()))
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
        _options: AppendAuditEventOptions,
    ) -> Result<AuditEventRecord, StorageError> {
        Err(StorageError::Config("not needed in tests".to_string()))
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

fn test_settings(manager_users: Option<&str>) -> Arc<BotSettings> {
    Arc::new(BotSettings::new(
        AgentSettings::default(),
        TelegramSettings {
            telegram_token: "dummy".to_string(),
            allowed_users_str: None,
            agent_allowed_users_str: Some("77 88".to_string()),
            manager_allowed_users_str: manager_users.map(str::to_string),
            topic_configs: Vec::new(),
        },
    ))
}

fn test_llm(settings: &Arc<BotSettings>) -> Arc<LlmClient> {
    Arc::new(LlmClient::new(settings.agent.as_ref()))
}

fn test_sandbox_scope(user_id: i64, context_key: &str) -> SandboxScope {
    SandboxScope::new(user_id, context_key.to_string())
}

#[test]
fn control_commands_are_recognized_for_topic_gate_bypass() {
    assert_eq!(
        parse_agent_control_command(Some("❌ Cancel Task")),
        Some(AgentControlCommand::CancelTask)
    );
    assert_eq!(
        parse_agent_control_command(Some("🗑 Clear Memory")),
        Some(AgentControlCommand::ClearMemory)
    );
    assert_eq!(
        parse_agent_control_command(Some("🔄 Recreate Container")),
        Some(AgentControlCommand::RecreateContainer)
    );
    assert_eq!(
        parse_agent_control_command(Some("⬅️ Exit Agent Mode")),
        Some(AgentControlCommand::ExitAgentMode)
    );
    assert_eq!(
        parse_agent_control_command(Some("/c")),
        Some(AgentControlCommand::ShowControls)
    );
}

#[test]
fn non_control_messages_do_not_bypass_topic_gate() {
    assert_eq!(parse_agent_control_command(Some("please help")), None);
    assert_eq!(parse_agent_control_command(Some("user@example.com")), None);
    assert_eq!(parse_agent_control_command(None), None);
}

#[test]
fn inline_cancel_callbacks_are_recognized() {
    assert_eq!(
        parse_agent_callback_action(AGENT_CALLBACK_CANCEL_TASK),
        Some(AgentCallbackAction::StartCancelTaskConfirmation)
    );
    assert_eq!(
        parse_agent_callback_action(AGENT_CALLBACK_CONFIRM_CANCEL_YES),
        Some(AgentCallbackAction::ResolveCancelTaskConfirmation(true))
    );
    assert_eq!(
        parse_agent_callback_action(AGENT_CALLBACK_CONFIRM_CANCEL_NO),
        Some(AgentCallbackAction::ResolveCancelTaskConfirmation(false))
    );
}

#[test]
fn ssh_approval_callbacks_are_recognized() {
    assert_eq!(
        parse_agent_callback_action("agent:ssh:approve:req-1"),
        Some(AgentCallbackAction::ApproveSsh("req-1".to_string()))
    );
    assert_eq!(
        parse_agent_callback_action("agent:ssh:reject:req-1"),
        Some(AgentCallbackAction::RejectSsh("req-1".to_string()))
    );
}

#[test]
fn session_id_derivation_is_stable_without_thread() {
    let user_id = 12345;
    let first = derive_agent_mode_session_id(user_id, ChatId(-1001), None, "flow-a");
    let second = derive_agent_mode_session_id(user_id, ChatId(-1001), None, "flow-a");

    assert_eq!(first, second);
}

#[test]
fn session_id_derivation_is_stable_for_same_thread() {
    let user_id = 12345;
    let thread_id = Some(ThreadId(MessageId(42)));
    let first = derive_agent_mode_session_id(user_id, ChatId(-1001), thread_id, "flow-a");
    let second = derive_agent_mode_session_id(user_id, ChatId(-1001), thread_id, "flow-a");

    assert_eq!(first, second);
}

#[test]
fn session_id_derivation_differs_for_different_threads() {
    let user_id = 12345;
    let first = derive_agent_mode_session_id(
        user_id,
        ChatId(-1001),
        Some(ThreadId(MessageId(42))),
        "flow-a",
    );
    let second = derive_agent_mode_session_id(
        user_id,
        ChatId(-1001),
        Some(ThreadId(MessageId(43))),
        "flow-a",
    );

    assert_ne!(first, second);
}

#[test]
fn session_id_derivation_differs_for_different_flows() {
    let user_id = 12345;
    let thread_id = Some(ThreadId(MessageId(42)));
    let first = derive_agent_mode_session_id(user_id, ChatId(-1001), thread_id, "flow-a");
    let second = derive_agent_mode_session_id(user_id, ChatId(-1001), thread_id, "flow-b");

    assert_ne!(first, second);
}

#[test]
fn existing_session_selection_prefers_primary_key() {
    let keys = agent_mode_session_keys(
        12345,
        ChatId(-1001),
        Some(ThreadId(MessageId(42))),
        "flow-a",
    );
    let selected = select_existing_session_id(keys, true, true);

    assert_eq!(selected, Some(keys.primary));
}

#[test]
fn existing_session_selection_falls_back_to_legacy_key() {
    let keys = agent_mode_session_keys(
        12345,
        ChatId(-1001),
        Some(ThreadId(MessageId(42))),
        "flow-a",
    );
    let selected = select_existing_session_id(keys, false, true);

    assert_eq!(selected, Some(keys.legacy));
}

#[test]
fn cancel_status_reply_markup_uses_flow_controls_in_topics() {
    let thread_id = ThreadId(MessageId(42));
    let markup = cancel_status_reply_markup(
        resolve_thread_spec_from_context(true, true, Some(thread_id)),
        "flow-a",
    );

    let ReplyMarkup::InlineKeyboard(keyboard) = markup else {
        panic!("topic cancel status should use inline keyboard");
    };

    assert_eq!(keyboard.inline_keyboard.len(), 1);
    assert_eq!(keyboard.inline_keyboard[0].len(), 2);
}

#[test]
fn cancel_status_reply_markup_uses_flow_controls_in_private_chats() {
    let markup = cancel_status_reply_markup(
        resolve_thread_spec_from_context(false, false, None),
        "flow-a",
    );

    let ReplyMarkup::InlineKeyboard(keyboard) = markup else {
        panic!("private chat cancel status should use inline keyboard");
    };

    assert_eq!(keyboard.inline_keyboard.len(), 1);
    assert_eq!(keyboard.inline_keyboard[0].len(), 2);
}

#[test]
fn inline_flow_controls_cover_forums_and_private_chats_only() {
    assert!(use_inline_flow_controls(resolve_thread_spec_from_context(
        false, false, None
    )));
    assert!(use_inline_flow_controls(resolve_thread_spec_from_context(
        true, true, None
    )));
    assert!(!use_inline_flow_controls(resolve_thread_spec_from_context(
        true, false, None
    )));
}

#[tokio::test]
async fn pending_cancel_message_round_trip_clears_entry() {
    let session_id = SessionId::from(777_i64);

    clear_pending_cancel_message(session_id).await;
    remember_pending_cancel_message(session_id, MessageId(55)).await;

    assert_eq!(
        pending_cancel_message(session_id).await,
        Some(MessageId(55))
    );
    assert_eq!(
        take_pending_cancel_message(session_id).await,
        Some(MessageId(55))
    );
    assert_eq!(pending_cancel_message(session_id).await, None);
}

#[tokio::test]
async fn pending_cancel_confirmation_round_trip_clears_entry() {
    let session_id = SessionId::from(778_i64);

    clear_pending_cancel_confirmation(session_id).await;
    remember_pending_cancel_confirmation(session_id, MessageId(56)).await;

    assert_eq!(
        pending_cancel_confirmation(session_id).await,
        Some(MessageId(56))
    );
    assert_eq!(
        take_pending_cancel_confirmation(session_id).await,
        Some(MessageId(56))
    );
    assert_eq!(pending_cancel_confirmation(session_id).await, None);
}

#[test]
fn manager_default_chat_id_is_available_in_general_forum_topic() {
    let spec = resolve_thread_spec_from_context(true, true, None);
    assert_eq!(
        manager_default_chat_id(ChatId(-100_123), spec),
        Some(ChatId(-100_123))
    );
}

#[test]
fn manager_default_chat_id_is_not_available_outside_forum_context() {
    let spec = resolve_thread_spec_from_context(true, false, None);
    assert_eq!(manager_default_chat_id(ChatId(-100_123), spec), None);
}

#[test]
fn manager_control_plane_access_requires_dedicated_allowlist_entry() {
    let settings = BotSettings::new(
        AgentSettings::default(),
        TelegramSettings {
            telegram_token: "dummy".to_string(),
            allowed_users_str: None,
            agent_allowed_users_str: Some("77 88".to_string()),
            manager_allowed_users_str: Some("88".to_string()),
            topic_configs: Vec::new(),
        },
    );

    let general_spec = resolve_thread_spec_from_context(true, true, None);

    assert!(!manager_control_plane_enabled(&settings, 77, general_spec));
    assert!(manager_control_plane_enabled(&settings, 88, general_spec));
}

#[test]
fn manager_control_plane_access_is_disabled_in_direct_messages() {
    let settings = BotSettings::new(
        AgentSettings::default(),
        TelegramSettings {
            telegram_token: "dummy".to_string(),
            allowed_users_str: None,
            agent_allowed_users_str: Some("88".to_string()),
            manager_allowed_users_str: Some("88".to_string()),
            topic_configs: Vec::new(),
        },
    );

    let dm_spec = resolve_thread_spec_from_context(false, false, None);

    assert!(!manager_control_plane_enabled(&settings, 88, dm_spec));
}

#[test]
fn manager_control_plane_access_is_disabled_in_non_forum_groups() {
    let settings = BotSettings::new(
        AgentSettings::default(),
        TelegramSettings {
            telegram_token: "dummy".to_string(),
            allowed_users_str: None,
            agent_allowed_users_str: Some("88".to_string()),
            manager_allowed_users_str: Some("88".to_string()),
            topic_configs: Vec::new(),
        },
    );

    let group_spec = resolve_thread_spec_from_context(true, false, None);

    assert!(!manager_control_plane_enabled(&settings, 88, group_spec));
}

#[test]
fn manager_control_plane_access_disabled_when_allowlist_is_empty() {
    let settings = BotSettings::new(
        AgentSettings::default(),
        TelegramSettings {
            telegram_token: "dummy".to_string(),
            allowed_users_str: None,
            agent_allowed_users_str: Some("77".to_string()),
            manager_allowed_users_str: None,
            topic_configs: Vec::new(),
        },
    );

    let general_spec = resolve_thread_spec_from_context(true, true, None);

    assert!(!manager_control_plane_enabled(&settings, 77, general_spec));
}

#[test]
fn manager_control_plane_gating_disables_tools_inside_created_topics() {
    let settings = BotSettings::new(
        AgentSettings::default(),
        TelegramSettings {
            telegram_token: "dummy".to_string(),
            allowed_users_str: None,
            agent_allowed_users_str: Some("77 88".to_string()),
            manager_allowed_users_str: Some("88".to_string()),
            topic_configs: Vec::new(),
        },
    );

    let thread_keys = agent_mode_session_keys(
        77,
        ChatId(-100_123),
        Some(ThreadId(MessageId(42))),
        "flow-a",
    );
    let general_spec = resolve_thread_spec_from_context(true, true, None);
    let created_topic_spec =
        resolve_thread_spec_from_context(true, true, Some(ThreadId(MessageId(42))));

    assert_ne!(thread_keys.primary, thread_keys.legacy);
    assert!(manager_control_plane_enabled(&settings, 88, general_spec));
    assert!(!manager_control_plane_enabled(
        &settings,
        88,
        created_topic_spec
    ));
    assert!(!manager_control_plane_enabled(&settings, 77, general_spec));
}

#[tokio::test]
async fn threaded_transport_session_disables_manager_tools_inside_created_topics() {
    let bot = Bot::new("token");
    let chat_id = ChatId(-100_123);
    let storage: Arc<dyn StorageProvider> = Arc::new(NoopStorage::default());
    let manager_settings = test_settings(Some("88"));
    let llm = test_llm(&manager_settings);

    let general_thread = general_forum_topic_id();
    let blocked_thread = ThreadId(MessageId(43));
    let allowed_keys = agent_mode_session_keys(88, chat_id, Some(general_thread), "flow-a");
    let blocked_keys = agent_mode_session_keys(77, chat_id, Some(blocked_thread), "flow-a");

    remove_sessions_with_compat(allowed_keys).await;
    remove_sessions_with_compat(blocked_keys).await;

    let allowed_session = ensure_session_exists(EnsureSessionContext {
        session_keys: allowed_keys,
        context_key: "allowed".to_string(),
        agent_flow_id: "flow-a".to_string(),
        agent_flow_created: false,
        sandbox_scope: test_sandbox_scope(88, "allowed"),
        user_id: 88,
        bot: &bot,
        transport_ctx: SessionTransportContext {
            manager_default_chat_id: Some(chat_id),
            thread_spec: resolve_thread_spec_from_context(true, true, Some(general_thread)),
        },
        llm: &llm,
        storage: &storage,
        settings: &manager_settings,
    })
    .await;
    let blocked_session = ensure_session_exists(EnsureSessionContext {
        session_keys: blocked_keys,
        context_key: "blocked".to_string(),
        agent_flow_id: "flow-a".to_string(),
        agent_flow_created: false,
        sandbox_scope: test_sandbox_scope(77, "blocked"),
        user_id: 77,
        bot: &bot,
        transport_ctx: SessionTransportContext {
            manager_default_chat_id: Some(chat_id),
            thread_spec: resolve_thread_spec_from_context(true, true, Some(blocked_thread)),
        },
        llm: &llm,
        storage: &storage,
        settings: &manager_settings,
    })
    .await;

    assert_eq!(allowed_session, allowed_keys.primary);
    assert_eq!(blocked_session, blocked_keys.primary);
    assert_eq!(
        session_manager_control_plane_enabled(allowed_session).await,
        Some(true)
    );
    assert_eq!(
        session_manager_control_plane_enabled(blocked_session).await,
        Some(false)
    );

    remove_sessions_with_compat(allowed_keys).await;
    remove_sessions_with_compat(blocked_keys).await;
}

#[tokio::test]
async fn threaded_transport_session_keeps_manager_tools_disabled_for_allowlisted_created_topic() {
    let bot = Bot::new("token");
    let chat_id = ChatId(-100_123);
    let thread_id = ThreadId(MessageId(42));
    let storage: Arc<dyn StorageProvider> = Arc::new(NoopStorage::default());
    let manager_settings = test_settings(Some("88"));
    let llm = test_llm(&manager_settings);
    let keys = agent_mode_session_keys(88, chat_id, Some(thread_id), "flow-a");

    remove_sessions_with_compat(keys).await;

    let session_id = ensure_session_exists(EnsureSessionContext {
        session_keys: keys,
        context_key: "topic-a".to_string(),
        agent_flow_id: "flow-a".to_string(),
        agent_flow_created: false,
        sandbox_scope: test_sandbox_scope(88, "topic-a"),
        user_id: 88,
        bot: &bot,
        transport_ctx: SessionTransportContext {
            manager_default_chat_id: Some(chat_id),
            thread_spec: resolve_thread_spec_from_context(true, true, Some(thread_id)),
        },
        llm: &llm,
        storage: &storage,
        settings: &manager_settings,
    })
    .await;

    assert_eq!(
        session_manager_control_plane_enabled(session_id).await,
        Some(false)
    );

    remove_sessions_with_compat(keys).await;
}

#[tokio::test]
async fn new_flow_injects_topic_agents_md_once() {
    let bot = Bot::new("token");
    let chat_id = ChatId(-100_123);
    let thread_id = general_forum_topic_id();
    let storage: Arc<dyn StorageProvider> = Arc::new(NoopStorage::with_topic_agents_md(
        "# Topic AGENTS\nUse deploy checklist.",
    ));
    let settings = test_settings(None);
    let llm = test_llm(&settings);
    let keys = agent_mode_session_keys(77, chat_id, Some(thread_id), "flow-agents");

    remove_sessions_with_compat(keys).await;

    let session_id = ensure_session_exists(EnsureSessionContext {
        session_keys: keys,
        context_key: "topic-a".to_string(),
        agent_flow_id: "flow-agents".to_string(),
        agent_flow_created: true,
        sandbox_scope: test_sandbox_scope(77, "topic-a"),
        user_id: 77,
        bot: &bot,
        transport_ctx: SessionTransportContext {
            manager_default_chat_id: Some(chat_id),
            thread_spec: resolve_thread_spec_from_context(true, true, Some(thread_id)),
        },
        llm: &llm,
        storage: &storage,
        settings: &settings,
    })
    .await;

    let executor = SESSION_REGISTRY
        .get(&session_id)
        .await
        .expect("session must exist");
    let executor = executor.read().await;
    let memory = &executor.session().memory;
    let pinned_count = memory
        .get_messages()
        .iter()
        .filter(|message| message.content.starts_with("[TOPIC_AGENTS_MD]\n"))
        .count();

    assert!(memory.has_topic_agents_md());
    assert_eq!(pinned_count, 1);

    remove_sessions_with_compat(keys).await;
}

#[tokio::test]
async fn restored_flow_does_not_duplicate_topic_agents_md() {
    let bot = Bot::new("token");
    let chat_id = ChatId(-100_123);
    let thread_id = general_forum_topic_id();
    let mut flow_memory = AgentMemory::new(10_000);
    flow_memory.add_message(
        oxide_agent_core::agent::memory::AgentMessage::topic_agents_md(
            "# Topic AGENTS\nUse deploy checklist.",
        ),
    );
    let storage: Arc<dyn StorageProvider> = Arc::new(NoopStorage {
        flow_memory: Some(flow_memory),
        agent_profile: None,
        topic_context: None,
        topic_agents_md: Some("# Topic AGENTS\nUse deploy checklist.".to_string()),
        fail_flow_memory_lookup: false,
        cleared_flows: Arc::default(),
    });
    let settings = test_settings(None);
    let llm = test_llm(&settings);
    let keys = agent_mode_session_keys(77, chat_id, Some(thread_id), "flow-existing");

    remove_sessions_with_compat(keys).await;

    let session_id = ensure_session_exists(EnsureSessionContext {
        session_keys: keys,
        context_key: "topic-a".to_string(),
        agent_flow_id: "flow-existing".to_string(),
        agent_flow_created: false,
        sandbox_scope: test_sandbox_scope(77, "topic-a"),
        user_id: 77,
        bot: &bot,
        transport_ctx: SessionTransportContext {
            manager_default_chat_id: Some(chat_id),
            thread_spec: resolve_thread_spec_from_context(true, true, Some(thread_id)),
        },
        llm: &llm,
        storage: &storage,
        settings: &settings,
    })
    .await;

    let executor = SESSION_REGISTRY
        .get(&session_id)
        .await
        .expect("session must exist");
    let executor = executor.read().await;
    let pinned_count = executor
        .session()
        .memory
        .get_messages()
        .iter()
        .filter(|message| message.content.starts_with("[TOPIC_AGENTS_MD]\n"))
        .count();

    assert_eq!(pinned_count, 1);

    remove_sessions_with_compat(keys).await;
}

#[tokio::test]
async fn threaded_transport_session_recreates_primary_when_manager_rbac_changes() {
    let flow_id = "flow-rbac-refresh";
    let bot = Bot::new("token");
    let chat_id = ChatId(-100_123);
    let thread_id = general_forum_topic_id();
    let storage: Arc<dyn StorageProvider> = Arc::new(NoopStorage::default());
    let allowed_settings = test_settings(Some("77"));
    let restricted_settings = test_settings(None);
    let llm = test_llm(&allowed_settings);
    let keys = agent_mode_session_keys(77, chat_id, Some(thread_id), flow_id);

    remove_sessions_with_compat(keys).await;

    let first_session = ensure_session_exists(EnsureSessionContext {
        session_keys: keys,
        context_key: "topic-a".to_string(),
        agent_flow_id: flow_id.to_string(),
        agent_flow_created: false,
        sandbox_scope: test_sandbox_scope(77, "topic-a"),
        user_id: 77,
        bot: &bot,
        transport_ctx: SessionTransportContext {
            manager_default_chat_id: Some(chat_id),
            thread_spec: resolve_thread_spec_from_context(true, true, Some(thread_id)),
        },
        llm: &llm,
        storage: &storage,
        settings: &allowed_settings,
    })
    .await;
    assert_eq!(first_session, keys.primary);
    assert_eq!(
        session_manager_control_plane_enabled(first_session).await,
        Some(true)
    );

    let second_session = ensure_session_exists(EnsureSessionContext {
        session_keys: keys,
        context_key: "topic-a".to_string(),
        agent_flow_id: flow_id.to_string(),
        agent_flow_created: false,
        sandbox_scope: test_sandbox_scope(77, "topic-a"),
        user_id: 77,
        bot: &bot,
        transport_ctx: SessionTransportContext {
            manager_default_chat_id: Some(chat_id),
            thread_spec: resolve_thread_spec_from_context(true, true, Some(thread_id)),
        },
        llm: &llm,
        storage: &storage,
        settings: &restricted_settings,
    })
    .await;
    assert_eq!(second_session, keys.primary);
    assert_eq!(
        session_manager_control_plane_enabled(second_session).await,
        Some(false)
    );

    remove_sessions_with_compat(keys).await;
}

#[tokio::test]
async fn threaded_transport_session_defers_rbac_refresh_while_running_then_refreshes_after_complete(
) {
    let flow_id = "flow-rbac-running";
    let bot = Bot::new("token");
    let chat_id = ChatId(-100_123);
    let thread_id = general_forum_topic_id();
    let storage: Arc<dyn StorageProvider> = Arc::new(NoopStorage::default());
    let allowed_settings = test_settings(Some("77"));
    let restricted_settings = test_settings(None);
    let llm = test_llm(&allowed_settings);
    let keys = agent_mode_session_keys(77, chat_id, Some(thread_id), flow_id);

    remove_sessions_with_compat(keys).await;

    let first_session = ensure_session_exists(EnsureSessionContext {
        session_keys: keys,
        context_key: "topic-a".to_string(),
        agent_flow_id: flow_id.to_string(),
        agent_flow_created: false,
        sandbox_scope: test_sandbox_scope(77, "topic-a"),
        user_id: 77,
        bot: &bot,
        transport_ctx: SessionTransportContext {
            manager_default_chat_id: Some(chat_id),
            thread_spec: resolve_thread_spec_from_context(true, true, Some(thread_id)),
        },
        llm: &llm,
        storage: &storage,
        settings: &allowed_settings,
    })
    .await;
    assert_eq!(first_session, keys.primary);
    assert_eq!(
        session_manager_control_plane_enabled(first_session).await,
        Some(true)
    );

    let marked_running = SESSION_REGISTRY
        .with_executor_mut(&first_session, |executor| {
            Box::pin(async move {
                executor.session_mut().start_task();
            })
        })
        .await;
    assert!(marked_running.is_ok());

    let second_session = ensure_session_exists(EnsureSessionContext {
        session_keys: keys,
        context_key: "topic-a".to_string(),
        agent_flow_id: flow_id.to_string(),
        agent_flow_created: false,
        sandbox_scope: test_sandbox_scope(77, "topic-a"),
        user_id: 77,
        bot: &bot,
        transport_ctx: SessionTransportContext {
            manager_default_chat_id: Some(chat_id),
            thread_spec: resolve_thread_spec_from_context(true, true, Some(thread_id)),
        },
        llm: &llm,
        storage: &storage,
        settings: &restricted_settings,
    })
    .await;
    assert_eq!(second_session, first_session);
    assert_eq!(
        session_manager_control_plane_enabled(second_session).await,
        Some(true)
    );

    let marked_completed = SESSION_REGISTRY
        .with_executor_mut(&first_session, |executor| {
            Box::pin(async move {
                executor.session_mut().complete();
            })
        })
        .await;
    assert!(marked_completed.is_ok());

    let third_session = ensure_session_exists(EnsureSessionContext {
        session_keys: keys,
        context_key: "topic-a".to_string(),
        agent_flow_id: flow_id.to_string(),
        agent_flow_created: false,
        sandbox_scope: test_sandbox_scope(77, "topic-a"),
        user_id: 77,
        bot: &bot,
        transport_ctx: SessionTransportContext {
            manager_default_chat_id: Some(chat_id),
            thread_spec: resolve_thread_spec_from_context(true, true, Some(thread_id)),
        },
        llm: &llm,
        storage: &storage,
        settings: &restricted_settings,
    })
    .await;
    assert_eq!(third_session, first_session);
    assert_eq!(
        session_manager_control_plane_enabled(third_session).await,
        Some(false)
    );

    remove_sessions_with_compat(keys).await;
}

#[tokio::test]
async fn threaded_transport_session_does_not_bypass_rbac_via_legacy_fallback() {
    let bot = Bot::new("token");
    let chat_id = ChatId(-100_123);
    let thread_id = ThreadId(MessageId(52));
    let storage: Arc<dyn StorageProvider> = Arc::new(NoopStorage::default());
    let legacy_manager_settings = test_settings(Some("77"));
    let llm = test_llm(&legacy_manager_settings);
    let keys = agent_mode_session_keys(77, chat_id, Some(thread_id), "flow-a");

    remove_sessions_with_compat(keys).await;

    let legacy_executor = oxide_agent_core::agent::AgentExecutor::new(
        llm.clone(),
        AgentSession::new(keys.legacy),
        legacy_manager_settings.agent.clone(),
    )
    .with_manager_control_plane(storage.clone(), 77);
    SESSION_REGISTRY.insert(keys.legacy, legacy_executor).await;

    let restricted_settings = test_settings(None);
    let resolved_session = ensure_session_exists(EnsureSessionContext {
        session_keys: keys,
        context_key: "topic-a".to_string(),
        agent_flow_id: "flow-a".to_string(),
        agent_flow_created: false,
        sandbox_scope: test_sandbox_scope(77, "topic-a"),
        user_id: 77,
        bot: &bot,
        transport_ctx: SessionTransportContext {
            manager_default_chat_id: Some(chat_id),
            thread_spec: resolve_thread_spec_from_context(true, true, Some(thread_id)),
        },
        llm: &llm,
        storage: &storage,
        settings: &restricted_settings,
    })
    .await;

    assert_eq!(resolved_session, keys.primary);
    assert_eq!(
        session_manager_control_plane_enabled(resolved_session).await,
        Some(false)
    );

    remove_sessions_with_compat(keys).await;
}

#[tokio::test]
async fn threaded_transport_session_migrates_idle_legacy_session_to_primary_scope() {
    let bot = Bot::new("token");
    let chat_id = ChatId(-100_123);
    let thread_id = ThreadId(MessageId(53));
    let storage: Arc<dyn StorageProvider> = Arc::new(NoopStorage::default());
    let settings = test_settings(None);
    let llm = test_llm(&settings);
    let keys = agent_mode_session_keys(77, chat_id, Some(thread_id), "flow-a");

    remove_sessions_with_compat(keys).await;

    let legacy_executor = oxide_agent_core::agent::AgentExecutor::new(
        llm.clone(),
        AgentSession::new(keys.legacy),
        settings.agent.clone(),
    );
    SESSION_REGISTRY.insert(keys.legacy, legacy_executor).await;

    let resolved_session = ensure_session_exists(EnsureSessionContext {
        session_keys: keys,
        context_key: "topic-a".to_string(),
        agent_flow_id: "flow-a".to_string(),
        agent_flow_created: false,
        sandbox_scope: test_sandbox_scope(77, "topic-a"),
        user_id: 77,
        bot: &bot,
        transport_ctx: SessionTransportContext {
            manager_default_chat_id: Some(chat_id),
            thread_spec: resolve_thread_spec_from_context(true, true, Some(thread_id)),
        },
        llm: &llm,
        storage: &storage,
        settings: &settings,
    })
    .await;

    assert_eq!(resolved_session, keys.primary);
    assert!(SESSION_REGISTRY.contains(&keys.primary).await);
    assert!(!SESSION_REGISTRY.contains(&keys.legacy).await);

    remove_sessions_with_compat(keys).await;
}

#[tokio::test]
async fn detach_creates_new_flow_only_when_current_flow_has_saved_memory() {
    let storage: Arc<dyn StorageProvider> =
        Arc::new(NoopStorage::with_flow_memory(Some(AgentMemory::new(1024))));

    assert!(should_create_fresh_flow_on_detach(&storage, 77, "-100123:42", "flow-a").await);
}

#[tokio::test]
async fn detach_reuses_current_flow_when_current_flow_has_no_saved_memory() {
    let storage: Arc<dyn StorageProvider> = Arc::new(NoopStorage::with_flow_memory(None));

    assert!(!should_create_fresh_flow_on_detach(&storage, 77, "-100123:42", "flow-a").await);
}

#[tokio::test]
async fn detach_falls_back_to_reset_when_memory_lookup_fails() {
    let storage: Arc<dyn StorageProvider> = Arc::new(NoopStorage::with_failed_flow_memory_lookup());

    assert!(should_create_fresh_flow_on_detach(&storage, 77, "-100123:42", "flow-a").await);
}

#[tokio::test]
async fn attach_cleanup_deletes_abandoned_empty_flow_record() {
    let storage = NoopStorage::with_flow_memory(None);
    let cleared_flows = storage.cleared_flows.clone();
    let storage: Arc<dyn StorageProvider> = Arc::new(storage);

    cleanup_abandoned_empty_flow(&storage, 77, "-100123:42", "flow-empty").await;

    let cleared_flows = cleared_flows
        .lock()
        .expect("cleared_flows mutex poisoned")
        .clone();
    assert_eq!(
        cleared_flows,
        vec![("-100123:42".to_string(), "flow-empty".to_string())]
    );
}

#[tokio::test]
async fn attach_cleanup_keeps_non_empty_flow_record() {
    let storage = NoopStorage::with_flow_memory(Some(AgentMemory::new(1024)));
    let cleared_flows = storage.cleared_flows.clone();
    let storage: Arc<dyn StorageProvider> = Arc::new(storage);

    cleanup_abandoned_empty_flow(&storage, 77, "-100123:42", "flow-a").await;

    assert!(cleared_flows
        .lock()
        .expect("cleared_flows mutex poisoned")
        .is_empty());
}

#[tokio::test]
async fn attach_cleanup_skips_delete_when_memory_lookup_fails() {
    let storage = NoopStorage::with_failed_flow_memory_lookup();
    let cleared_flows = storage.cleared_flows.clone();
    let storage: Arc<dyn StorageProvider> = Arc::new(storage);

    cleanup_abandoned_empty_flow(&storage, 77, "-100123:42", "flow-a").await;

    assert!(cleared_flows
        .lock()
        .expect("cleared_flows mutex poisoned")
        .is_empty());
}

#[test]
fn merge_prompt_instructions_deduplicates_identical_sections() {
    let merged = merge_prompt_instructions(Some("infra rules"), Some("infra rules"));

    assert_eq!(merged.as_deref(), Some("infra rules"));
}

#[tokio::test]
async fn resolve_execution_profile_loads_profile_policy_for_static_topic_agent() {
    let storage: Arc<dyn StorageProvider> =
        Arc::new(NoopStorage::with_agent_profile_and_topic_context(
            serde_json::json!({
                "systemPrompt": "act as infra agent",
                "allowedTools": ["todos_write", "execute_command"],
                "blockedTools": ["delegate_to_sub_agent"],
                "disabledHooks": ["search_budget"]
            }),
            "persistent topic runbook",
        ));
    let route = crate::bot::topic_route::TopicRouteDecision {
        enabled: true,
        require_mention: false,
        mention_satisfied: true,
        system_prompt_override: Some("topic-specific note".to_string()),
        agent_id: Some("infra-agent".to_string()),
        dynamic_binding_topic_id: None,
    };

    let profile = resolve_execution_profile(&storage, 77, "topic-a", &route, false).await;

    assert_eq!(profile.agent_id(), Some("infra-agent"));
    let prompt = profile
        .prompt_instructions()
        .expect("profile prompt must be resolved");
    assert!(prompt.contains("Profile instructions:"));
    assert!(prompt.contains("Topic instructions:"));
    assert!(prompt.contains("Persistent topic context:"));
    assert!(prompt.contains("persistent topic runbook"));
    assert!(profile.tool_policy().allows("todos_write"));
    assert!(profile.tool_policy().allows("reminder_schedule"));
    assert!(!profile.tool_policy().allows("delegate_to_sub_agent"));
    assert!(!profile.tool_policy().allows("file_write"));
    assert!(!profile.hook_policy().allows("search_budget"));
}

#[tokio::test]
async fn resolve_execution_profile_applies_manager_default_blocklist() {
    let storage: Arc<dyn StorageProvider> = Arc::new(NoopStorage::default());
    let route = crate::bot::topic_route::TopicRouteDecision {
        enabled: true,
        require_mention: false,
        mention_satisfied: true,
        system_prompt_override: None,
        agent_id: None,
        dynamic_binding_topic_id: None,
    };

    let profile = resolve_execution_profile(&storage, 77, "manager-topic", &route, true).await;

    assert!(profile.tool_policy().allows("execute_command"));
    assert!(!profile.tool_policy().allows("delegate_to_sub_agent"));
    assert!(!profile.tool_policy().allows("ytdlp_get_video_metadata"));
    assert!(!profile.tool_policy().allows("ytdlp_download_video"));
}

#[tokio::test]
async fn resolve_execution_profile_keeps_manager_tools_available_with_profile_allowlist() {
    let storage: Arc<dyn StorageProvider> =
        Arc::new(NoopStorage::with_agent_profile_and_topic_context(
            serde_json::json!({
                "systemPrompt": "act as control plane agent",
                "allowedTools": ["execute_command"],
            }),
            "manager topic context",
        ));
    let route = crate::bot::topic_route::TopicRouteDecision {
        enabled: true,
        require_mention: false,
        mention_satisfied: true,
        system_prompt_override: None,
        agent_id: Some("control-plane".to_string()),
        dynamic_binding_topic_id: None,
    };

    let profile = resolve_execution_profile(&storage, 77, "manager-topic", &route, true).await;

    assert!(profile.tool_policy().allows("execute_command"));
    assert!(profile.tool_policy().allows("topic_agents_md_upsert"));
    assert!(profile.tool_policy().allows("topic_agents_md_get"));
    assert!(profile.tool_policy().allows("forum_topic_list"));
    assert!(!profile.tool_policy().allows("delegate_to_sub_agent"));
}
