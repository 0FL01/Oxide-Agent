use async_trait::async_trait;
use oxide_agent_core::agent::AgentMemory;
use oxide_agent_core::config::AgentSettings;
use oxide_agent_core::storage::{
    AgentProfileRecord, AppendAuditEventOptions, AuditEventRecord, Message as StoredMessage,
    OptionalMetadataPatch, StorageError, StorageProvider, TopicBindingKind, TopicBindingRecord,
    UpsertAgentProfileOptions, UpsertTopicBindingOptions, UserConfig,
};
use oxide_agent_transport_telegram::bot::thread::{
    build_outbound_thread_params, resolve_thread_spec_from_context,
};
use oxide_agent_transport_telegram::bot::topic_route::{
    resolve_topic_route, resolve_topic_route_decision, TopicRouteContext,
};
use oxide_agent_transport_telegram::config::{
    BotSettings, TelegramSettings, TelegramTopicSettings,
};
use std::collections::HashMap;
use std::sync::Mutex;
use teloxide::prelude::Bot;
use teloxide::types::{
    Chat, ChatId, ChatKind, ChatPublic, MediaKind, MediaText, Message, MessageCommon, MessageId,
    MessageKind, PublicChatKind, PublicChatSupergroup, ThreadId, User, UserId,
};

fn topic(
    chat_id: i64,
    thread_id: Option<i32>,
    enabled: bool,
    require_mention: bool,
    system_prompt: Option<&str>,
) -> TelegramTopicSettings {
    TelegramTopicSettings {
        chat_id,
        thread_id,
        agent_id: None,
        enabled,
        require_mention,
        skills: Vec::new(),
        system_prompt: system_prompt.map(str::to_string),
    }
}

#[derive(Default)]
struct TestStorage {
    bindings: Mutex<HashMap<(i64, String), TopicBindingRecord>>,
    profiles: Mutex<HashMap<(i64, String), AgentProfileRecord>>,
}

impl TestStorage {
    fn key(user_id: i64, id: String) -> (i64, String) {
        (user_id, id)
    }

    fn lock_error() -> StorageError {
        StorageError::Config("test storage lock poisoned".to_string())
    }

    fn with_profile(
        self,
        user_id: i64,
        agent_id: &str,
        profile_json: &str,
    ) -> Result<Self, StorageError> {
        let profile = profile_json.parse().map_err(StorageError::Json)?;
        let mut profiles = self.profiles.lock().map_err(|_| Self::lock_error())?;
        profiles.insert(
            Self::key(user_id, agent_id.to_string()),
            AgentProfileRecord {
                schema_version: 1,
                version: 1,
                user_id,
                agent_id: agent_id.to_string(),
                profile,
                created_at: 10,
                updated_at: 10,
            },
        );
        drop(profiles);
        Ok(self)
    }
}

#[async_trait]
impl StorageProvider for TestStorage {
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
        let profiles = self.profiles.lock().map_err(|_| Self::lock_error())?;
        Ok(profiles.get(&Self::key(user_id, agent_id)).cloned())
    }

    async fn upsert_agent_profile(
        &self,
        options: UpsertAgentProfileOptions,
    ) -> Result<AgentProfileRecord, StorageError> {
        let mut profiles = self.profiles.lock().map_err(|_| Self::lock_error())?;
        let record = AgentProfileRecord {
            schema_version: 1,
            version: 1,
            user_id: options.user_id,
            agent_id: options.agent_id,
            profile: options.profile,
            created_at: 100,
            updated_at: 100,
        };
        profiles.insert(
            Self::key(record.user_id, record.agent_id.clone()),
            record.clone(),
        );
        Ok(record)
    }

    async fn delete_agent_profile(
        &self,
        user_id: i64,
        agent_id: String,
    ) -> Result<(), StorageError> {
        let mut profiles = self.profiles.lock().map_err(|_| Self::lock_error())?;
        profiles.remove(&Self::key(user_id, agent_id));
        Ok(())
    }

    async fn get_topic_binding(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicBindingRecord>, StorageError> {
        let bindings = self.bindings.lock().map_err(|_| Self::lock_error())?;
        Ok(bindings.get(&Self::key(user_id, topic_id)).cloned())
    }

    async fn upsert_topic_binding(
        &self,
        options: UpsertTopicBindingOptions,
    ) -> Result<TopicBindingRecord, StorageError> {
        let mut bindings = self.bindings.lock().map_err(|_| Self::lock_error())?;
        let key = Self::key(options.user_id, options.topic_id.clone());
        let existing = bindings.get(&key).cloned();
        let now = options.last_activity_at.unwrap_or(100);
        let mut record = existing.unwrap_or(TopicBindingRecord {
            schema_version: 1,
            version: 0,
            user_id: options.user_id,
            topic_id: options.topic_id.clone(),
            agent_id: options.agent_id.clone(),
            binding_kind: TopicBindingKind::Manual,
            chat_id: None,
            thread_id: None,
            expires_at: None,
            last_activity_at: None,
            created_at: now,
            updated_at: now,
        });

        record.version += 1;
        record.agent_id = options.agent_id;
        if let Some(binding_kind) = options.binding_kind {
            record.binding_kind = binding_kind;
        }
        record.chat_id = options.chat_id.apply(record.chat_id);
        record.thread_id = options.thread_id.apply(record.thread_id);
        record.expires_at = options.expires_at.apply(record.expires_at);
        record.last_activity_at = options.last_activity_at.or(record.last_activity_at);
        record.updated_at = now;
        bindings.insert(key, record.clone());

        Ok(record)
    }

    async fn delete_topic_binding(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError> {
        let mut bindings = self.bindings.lock().map_err(|_| Self::lock_error())?;
        bindings.remove(&Self::key(user_id, topic_id));
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

fn forum_text_message(chat_id: i64, thread_id: i32, text: &str) -> Message {
    Message {
        id: MessageId(71),
        thread_id: Some(ThreadId(MessageId(thread_id))),
        from: Some(User {
            id: UserId(5007),
            is_bot: false,
            first_name: "tester".to_string(),
            last_name: None,
            username: None,
            language_code: None,
            is_premium: false,
            added_to_attachment_menu: false,
        }),
        sender_chat: None,
        date: std::time::SystemTime::UNIX_EPOCH.into(),
        chat: Chat {
            id: ChatId(chat_id),
            kind: ChatKind::Public(ChatPublic {
                title: Some("ops".to_string()),
                kind: PublicChatKind::Supergroup(PublicChatSupergroup {
                    username: None,
                    is_forum: true,
                }),
            }),
        },
        is_topic_message: true,
        via_bot: None,
        sender_business_bot: None,
        kind: MessageKind::Common(MessageCommon {
            author_signature: None,
            paid_star_count: None,
            effect_id: None,
            forward_origin: None,
            reply_to_message: None,
            external_reply: None,
            quote: None,
            reply_to_story: None,
            sender_boost_count: None,
            edit_date: None,
            media_kind: MediaKind::Text(MediaText {
                text: text.to_string(),
                entities: Vec::new(),
                link_preview_options: None,
            }),
            reply_markup: None,
            is_automatic_forward: false,
            has_protected_content: false,
            is_from_offline: false,
            business_connection_id: None,
        }),
    }
}

#[test]
fn topic_routing_resolves_topic_settings_and_default_fallback() {
    let settings = TelegramSettings {
        telegram_token: "dummy".to_string(),
        allowed_users_str: None,
        agent_allowed_users_str: None,
        manager_allowed_users_str: None,
        topic_configs: vec![
            topic(-100_123, Some(42), false, true, Some("support-only")),
            topic(-100_123, Some(7), true, true, Some("mention-required")),
            topic(-100_123, None, true, false, None),
        ],
    };

    let blocked_context = TopicRouteContext {
        text: Some("ping @bot"),
        caption: None,
        reply_to_bot: false,
    };
    let disabled_topic = settings.resolve_topic_config(-100_123, Some(42));
    let disabled_decision =
        resolve_topic_route_decision(disabled_topic, &blocked_context, Some("bot"));
    assert!(!disabled_decision.allows_processing());
    assert!(disabled_decision.require_mention);
    assert_eq!(
        disabled_decision.system_prompt_override.as_deref(),
        Some("support-only")
    );

    let no_mention_context = TopicRouteContext {
        text: Some("regular message"),
        caption: None,
        reply_to_bot: false,
    };
    let mention_topic = settings.resolve_topic_config(-100_123, Some(7));
    let mention_decision =
        resolve_topic_route_decision(mention_topic, &no_mention_context, Some("bot"));
    assert!(!mention_decision.allows_processing());
    assert!(mention_decision.require_mention);
    assert!(!mention_decision.mention_satisfied);
    assert_eq!(
        mention_decision.system_prompt_override.as_deref(),
        Some("mention-required")
    );

    let chat_default = settings.resolve_topic_config(-100_123, None);
    let chat_default_decision =
        resolve_topic_route_decision(chat_default, &no_mention_context, Some("bot"));
    assert!(chat_default_decision.allows_processing());
    assert!(!chat_default_decision.require_mention);
    assert_eq!(chat_default_decision.system_prompt_override, None);

    let unknown_chat = settings.resolve_topic_config(-200_999, Some(42));
    let fallback_decision =
        resolve_topic_route_decision(unknown_chat, &no_mention_context, Some("bot"));
    assert!(fallback_decision.allows_processing());
    assert!(!fallback_decision.require_mention);
    assert!(fallback_decision.mention_satisfied);
    assert_eq!(fallback_decision.system_prompt_override, None);
}

#[test]
fn topic_route_and_thread_context_regression_preserves_non_general_topic_replies() {
    let settings = TelegramSettings {
        telegram_token: "dummy".to_string(),
        allowed_users_str: None,
        agent_allowed_users_str: None,
        manager_allowed_users_str: None,
        topic_configs: vec![topic(-100_123, Some(42), true, true, Some("topic-prompt"))],
    };

    let reply_context = TopicRouteContext {
        text: Some("regular message"),
        caption: None,
        reply_to_bot: true,
    };
    let reply_route = resolve_topic_route_decision(
        settings.resolve_topic_config(-100_123, Some(42)),
        &reply_context,
        Some("oxide_agent"),
    );
    assert!(reply_route.allows_processing());
    assert!(reply_route.require_mention);
    assert!(reply_route.mention_satisfied);
    assert_eq!(
        reply_route.system_prompt_override.as_deref(),
        Some("topic-prompt")
    );

    let forum_topic_spec =
        resolve_thread_spec_from_context(true, true, Some(ThreadId(MessageId(42))));
    let forum_topic_params = build_outbound_thread_params(forum_topic_spec);
    assert_eq!(
        forum_topic_params.message_thread_id,
        Some(ThreadId(MessageId(42)))
    );

    let mention_context = TopicRouteContext {
        text: Some("ping @oxide_agent"),
        caption: None,
        reply_to_bot: false,
    };
    let mention_route = resolve_topic_route_decision(
        settings.resolve_topic_config(-100_123, Some(42)),
        &mention_context,
        Some("oxide_agent"),
    );
    assert!(mention_route.allows_processing());
    assert!(mention_route.mention_satisfied);

    let general_topic_spec =
        resolve_thread_spec_from_context(true, true, Some(ThreadId(MessageId(1))));
    let general_topic_params = build_outbound_thread_params(general_topic_spec);
    assert_eq!(general_topic_params.message_thread_id, None);
}

#[tokio::test]
async fn resolve_topic_route_prefers_dynamic_binding_over_static_topic_config() -> anyhow::Result<()>
{
    let user_id = 7;
    let storage = TestStorage::default().with_profile(
        user_id,
        "dynamic-agent",
        r#"{"systemPrompt":"dynamic prompt from profile"}"#,
    )?;

    storage
        .upsert_topic_binding(UpsertTopicBindingOptions {
            user_id,
            topic_id: "-1001:313".to_string(),
            agent_id: "dynamic-agent".to_string(),
            binding_kind: Some(TopicBindingKind::Runtime),
            chat_id: OptionalMetadataPatch::Set(-1001),
            thread_id: OptionalMetadataPatch::Set(313),
            expires_at: OptionalMetadataPatch::Keep,
            last_activity_at: Some(100),
        })
        .await?;

    let settings = BotSettings::new(
        AgentSettings::default(),
        TelegramSettings {
            telegram_token: "test-token".to_string(),
            allowed_users_str: None,
            agent_allowed_users_str: None,
            manager_allowed_users_str: None,
            topic_configs: vec![TelegramTopicSettings {
                chat_id: -1001,
                thread_id: Some(313),
                agent_id: Some("static-agent".to_string()),
                enabled: false,
                require_mention: true,
                skills: Vec::new(),
                system_prompt: Some("static prompt".to_string()),
            }],
        },
    );

    let message = forum_text_message(-1001, 313, "hello without mention");
    let bot = Bot::new("123456:TEST_TOKEN");

    let decision = resolve_topic_route(&bot, &storage, user_id, &settings, &message).await;

    assert!(decision.allows_processing());
    assert_eq!(decision.agent_id.as_deref(), Some("dynamic-agent"));
    assert_eq!(
        decision.system_prompt_override.as_deref(),
        Some("dynamic prompt from profile")
    );
    assert_eq!(
        decision.dynamic_binding_topic_id.as_deref(),
        Some("-1001:313")
    );

    Ok(())
}
