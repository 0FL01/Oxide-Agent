use crate::bot::{resolve_thread_spec, thread_peer_key, thread_peer_key_from_spec};
use crate::config::{BotSettings, TelegramTopicSettings};
use oxide_agent_core::storage::{
    binding_is_active, resolve_active_topic_binding, OptionalMetadataPatch, StorageProvider,
    TopicBindingRecord, UpsertTopicBindingOptions,
};
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};
use teloxide::prelude::*;
use teloxide::types::Message;
use tokio::sync::RwLock;
use tracing::warn;

static BOT_USERNAME_CACHE: LazyLock<RwLock<Option<String>>> = LazyLock::new(|| RwLock::new(None));

/// Inbound message context required for topic route checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicRouteContext<'a> {
    /// Incoming text payload (if present).
    pub text: Option<&'a str>,
    /// Incoming media caption payload (if present).
    pub caption: Option<&'a str>,
    /// Whether the message is a reply to a bot-authored message.
    pub reply_to_bot: bool,
}

/// Effective topic route decision for a single inbound message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicRouteDecision {
    /// Topic-level processing switch.
    pub enabled: bool,
    /// Whether mention/reply signal is required in this topic.
    pub require_mention: bool,
    /// Whether mention/reply requirement is satisfied by current message.
    pub mention_satisfied: bool,
    /// Optional topic-level system prompt override.
    pub system_prompt_override: Option<String>,
    /// Selected topic agent identifier (informational in current stage).
    pub agent_id: Option<String>,
    /// Dynamic binding topic identifier when route is resolved from storage.
    pub dynamic_binding_topic_id: Option<String>,
}

impl TopicRouteDecision {
    /// Returns true when inbound message should be processed.
    #[must_use]
    pub const fn allows_processing(&self) -> bool {
        self.enabled && (!self.require_mention || self.mention_satisfied)
    }

    /// Returns true when route should touch dynamic binding activity.
    #[must_use]
    pub const fn should_touch_dynamic_binding_activity(&self) -> bool {
        self.allows_processing() && self.dynamic_binding_topic_id.is_some()
    }
}

/// Resolve topic decision from Telegram settings and inbound message.
pub async fn resolve_topic_route(
    bot: &Bot,
    storage: &dyn StorageProvider,
    user_id: i64,
    settings: &BotSettings,
    message: &Message,
) -> TopicRouteDecision {
    let now = current_timestamp_unix_secs();
    if let Some(binding) = resolve_dynamic_topic_binding(storage, user_id, message, now).await {
        let profile_prompt =
            resolve_profile_system_prompt_override(storage, user_id, &binding).await;
        return dynamic_route_decision(&binding, profile_prompt);
    }

    let thread_id = resolve_thread_spec(message)
        .thread_id
        .map(|thread| thread.0 .0);
    let topic = settings
        .telegram
        .resolve_topic_config(message.chat.id.0, thread_id);
    let bot_username = cached_bot_username(bot).await;
    let context = TopicRouteContext {
        text: message.text(),
        caption: message.caption(),
        reply_to_bot: is_reply_to_bot(message),
    };

    resolve_topic_route_decision(topic, &context, bot_username.as_deref())
}

#[must_use]
fn dynamic_route_decision(
    binding: &TopicBindingRecord,
    profile_prompt: Option<String>,
) -> TopicRouteDecision {
    TopicRouteDecision {
        enabled: true,
        require_mention: false,
        mention_satisfied: true,
        system_prompt_override: profile_prompt,
        agent_id: Some(binding.agent_id.clone()),
        dynamic_binding_topic_id: Some(binding.topic_id.clone()),
    }
}

/// Resolve topic decision using pre-extracted context and optional topic config.
#[must_use]
pub fn resolve_topic_route_decision(
    topic: Option<&TelegramTopicSettings>,
    context: &TopicRouteContext<'_>,
    bot_username: Option<&str>,
) -> TopicRouteDecision {
    if let Some(topic_config) = topic {
        let mention_satisfied = if topic_config.require_mention {
            context.reply_to_bot
                || contains_bot_mention(context.text, bot_username)
                || contains_bot_mention(context.caption, bot_username)
        } else {
            true
        };

        return TopicRouteDecision {
            enabled: topic_config.enabled,
            require_mention: topic_config.require_mention,
            mention_satisfied,
            system_prompt_override: topic_config.system_prompt.clone(),
            agent_id: topic_config.agent_id.clone(),
            dynamic_binding_topic_id: None,
        };
    }

    TopicRouteDecision {
        enabled: true,
        require_mention: false,
        mention_satisfied: true,
        system_prompt_override: None,
        agent_id: None,
        dynamic_binding_topic_id: None,
    }
}

/// Touch dynamic binding activity timestamp for successful routed messages.
pub async fn touch_dynamic_binding_activity_if_needed(
    storage: &dyn StorageProvider,
    user_id: i64,
    decision: &TopicRouteDecision,
) {
    let Some(topic_id) = decision.dynamic_binding_topic_id.as_ref() else {
        return;
    };
    if !decision.should_touch_dynamic_binding_activity() {
        return;
    }

    let now = current_timestamp_unix_secs();
    let record = match storage.get_topic_binding(user_id, topic_id.clone()).await {
        Ok(record) => record,
        Err(error) => {
            warn!(
                error = %error,
                user_id,
                topic_id,
                "Failed to load topic binding for activity touch"
            );
            return;
        }
    };

    let Some(binding) = resolve_active_topic_binding(record, now) else {
        return;
    };
    let Some(options) = build_binding_activity_touch_options(binding, now) else {
        return;
    };

    if let Err(error) = storage.upsert_topic_binding(options).await {
        warn!(
            error = %error,
            user_id,
            topic_id,
            "Failed to upsert topic binding activity timestamp"
        );
    }
}

fn build_binding_activity_touch_options(
    binding: TopicBindingRecord,
    now: i64,
) -> Option<UpsertTopicBindingOptions> {
    if !binding_is_active(&binding, now) {
        return None;
    }

    Some(UpsertTopicBindingOptions {
        user_id: binding.user_id,
        topic_id: binding.topic_id,
        agent_id: binding.agent_id,
        binding_kind: Some(binding.binding_kind),
        chat_id: patch_optional_metadata(binding.chat_id),
        thread_id: patch_optional_metadata(binding.thread_id),
        expires_at: patch_optional_metadata(binding.expires_at),
        last_activity_at: Some(now),
    })
}

fn patch_optional_metadata(value: Option<i64>) -> OptionalMetadataPatch<i64> {
    match value {
        Some(inner) => OptionalMetadataPatch::Set(inner),
        None => OptionalMetadataPatch::Clear,
    }
}

async fn resolve_dynamic_topic_binding(
    storage: &dyn StorageProvider,
    user_id: i64,
    message: &Message,
    now: i64,
) -> Option<TopicBindingRecord> {
    for topic_id in topic_binding_lookup_keys(message) {
        let record = match storage.get_topic_binding(user_id, topic_id).await {
            Ok(record) => record,
            Err(error) => {
                warn!(
                    error = %error,
                    user_id,
                    "Failed to fetch topic binding during route resolution"
                );
                continue;
            }
        };

        if let Some(binding) = resolve_active_topic_binding(record, now) {
            return Some(binding);
        }
    }

    None
}

fn topic_binding_lookup_keys(message: &Message) -> Vec<String> {
    let spec = resolve_thread_spec(message);
    let primary = thread_peer_key_from_spec(message.chat.id, spec);
    let raw_key = thread_peer_key(message.chat.id, message.thread_id);
    let mut keys = vec![primary.clone()];

    if raw_key != primary {
        keys.push(raw_key);
    }

    if let Some(thread_id) = message.thread_id {
        let thread_key = thread_id.0 .0.to_string();
        if !keys.contains(&thread_key) {
            keys.push(thread_key);
        }
    }

    if let Some(thread_id) = spec.thread_id {
        let thread_key = thread_id.0 .0.to_string();
        if !keys.contains(&thread_key) {
            keys.push(thread_key);
        }
    }

    keys
}

async fn resolve_profile_system_prompt_override(
    storage: &dyn StorageProvider,
    user_id: i64,
    binding: &TopicBindingRecord,
) -> Option<String> {
    let profile = match storage
        .get_agent_profile(user_id, binding.agent_id.clone())
        .await
    {
        Ok(profile) => profile,
        Err(error) => {
            warn!(
                error = %error,
                user_id,
                agent_id = %binding.agent_id,
                "Failed to load agent profile for dynamic topic route"
            );
            return None;
        }
    };

    profile.and_then(|record| {
        let camel = record
            .profile
            .get("systemPrompt")
            .and_then(|inner| inner.as_str());
        let snake = record
            .profile
            .get("system_prompt")
            .and_then(|inner| inner.as_str());
        select_profile_system_prompt(camel, snake)
    })
}

fn select_profile_system_prompt(
    system_prompt_camel: Option<&str>,
    system_prompt_snake: Option<&str>,
) -> Option<String> {
    for value in [system_prompt_camel, system_prompt_snake]
        .into_iter()
        .flatten()
    {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    None
}

fn current_timestamp_unix_secs() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs() as i64,
        Err(_) => 0,
    }
}

fn is_reply_to_bot(message: &Message) -> bool {
    message
        .reply_to_message()
        .and_then(|reply| reply.from.as_ref())
        .is_some_and(|user| user.is_bot)
}

async fn cached_bot_username(bot: &Bot) -> Option<String> {
    {
        let cache = BOT_USERNAME_CACHE.read().await;
        if let Some(username) = cache.as_ref() {
            return Some(username.clone());
        }
    }

    let username = match bot.get_me().await {
        Ok(me) => me.user.username,
        Err(error) => {
            warn!(error = %error, "Failed to fetch bot username for topic mentions");
            None
        }
    };

    if let Some(name) = username.clone() {
        let mut cache = BOT_USERNAME_CACHE.write().await;
        if cache.is_none() {
            *cache = Some(name);
        }
    }

    username
}

fn contains_bot_mention(value: Option<&str>, bot_username: Option<&str>) -> bool {
    let Some(text) = value else {
        return false;
    };
    let Some(bot_username) = bot_username else {
        return false;
    };

    let lowered_text = text.to_ascii_lowercase();
    let mention = format!("@{}", bot_username.to_ascii_lowercase());
    let mut search_start = 0;

    while let Some(pos) = lowered_text[search_start..].find(&mention) {
        let start = search_start + pos;
        let end = start + mention.len();
        let prev_char = lowered_text[..start].chars().next_back();
        let next_char = lowered_text[end..].chars().next();

        let has_valid_prefix = prev_char.is_none_or(|ch| !is_mention_char(ch));
        let has_valid_suffix = next_char.is_none_or(|ch| !is_mention_char(ch));
        if has_valid_prefix && has_valid_suffix {
            return true;
        }

        search_start = start + 1;
    }

    false
}

const fn is_mention_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

#[cfg(test)]
mod tests {
    use super::{
        build_binding_activity_touch_options, dynamic_route_decision, resolve_active_topic_binding,
        resolve_topic_route_decision, select_profile_system_prompt, TopicRouteContext,
    };
    use crate::config::TelegramTopicSettings;
    use oxide_agent_core::storage::{TopicBindingKind, TopicBindingRecord};

    fn topic(
        enabled: bool,
        require_mention: bool,
        system_prompt: Option<&str>,
        agent_id: Option<&str>,
    ) -> TelegramTopicSettings {
        TelegramTopicSettings {
            chat_id: -10001,
            thread_id: Some(42),
            agent_id: agent_id.map(str::to_string),
            enabled,
            require_mention,
            skills: Vec::new(),
            system_prompt: system_prompt.map(str::to_string),
        }
    }

    #[test]
    fn route_defaults_to_enabled_without_topic() {
        let context = TopicRouteContext {
            text: None,
            caption: None,
            reply_to_bot: false,
        };
        let decision = resolve_topic_route_decision(None, &context, Some("oxide_bot"));

        assert!(decision.allows_processing());
        assert_eq!(decision.system_prompt_override, None);
        assert_eq!(decision.agent_id, None);
        assert_eq!(decision.dynamic_binding_topic_id, None);
    }

    #[test]
    fn route_blocks_when_topic_disabled() {
        let context = TopicRouteContext {
            text: Some("hello"),
            caption: None,
            reply_to_bot: false,
        };
        let cfg = topic(false, false, None, Some("support-agent"));
        let decision = resolve_topic_route_decision(Some(&cfg), &context, Some("oxide_bot"));

        assert!(!decision.allows_processing());
        assert_eq!(decision.agent_id.as_deref(), Some("support-agent"));
        assert_eq!(decision.dynamic_binding_topic_id, None);
    }

    #[test]
    fn route_requires_mention_and_accepts_text_mention() {
        let context = TopicRouteContext {
            text: Some("ping @oxide_bot"),
            caption: None,
            reply_to_bot: false,
        };
        let cfg = topic(true, true, Some("topic prompt"), None);
        let decision = resolve_topic_route_decision(Some(&cfg), &context, Some("oxide_bot"));

        assert!(decision.allows_processing());
        assert_eq!(
            decision.system_prompt_override.as_deref(),
            Some("topic prompt")
        );
        assert_eq!(decision.dynamic_binding_topic_id, None);
    }

    #[test]
    fn route_requires_mention_and_accepts_reply_to_bot() {
        let context = TopicRouteContext {
            text: Some("no mention here"),
            caption: None,
            reply_to_bot: true,
        };
        let cfg = topic(true, true, None, None);
        let decision = resolve_topic_route_decision(Some(&cfg), &context, Some("oxide_bot"));

        assert!(decision.allows_processing());
        assert_eq!(decision.dynamic_binding_topic_id, None);
    }

    #[test]
    fn route_requires_mention_and_blocks_without_signal() {
        let context = TopicRouteContext {
            text: Some("no mention"),
            caption: Some("still no mention"),
            reply_to_bot: false,
        };
        let cfg = topic(true, true, None, None);
        let decision = resolve_topic_route_decision(Some(&cfg), &context, Some("oxide_bot"));

        assert!(!decision.allows_processing());
        assert_eq!(decision.dynamic_binding_topic_id, None);
    }

    #[test]
    fn route_requires_explicit_bot_mention_not_generic_at() {
        let context = TopicRouteContext {
            text: Some("email me: user@example.com or ping @somebody"),
            caption: None,
            reply_to_bot: false,
        };
        let cfg = topic(true, true, None, None);
        let decision = resolve_topic_route_decision(Some(&cfg), &context, Some("oxide_bot"));

        assert!(!decision.allows_processing());
        assert_eq!(decision.dynamic_binding_topic_id, None);
    }

    #[test]
    fn route_accepts_caption_with_explicit_bot_mention() {
        let context = TopicRouteContext {
            text: None,
            caption: Some("Please review @oxide_bot."),
            reply_to_bot: false,
        };
        let cfg = topic(true, true, None, None);
        let decision = resolve_topic_route_decision(Some(&cfg), &context, Some("oxide_bot"));

        assert!(decision.allows_processing());
        assert_eq!(decision.dynamic_binding_topic_id, None);
    }

    fn topic_binding(
        topic_id: &str,
        agent_id: &str,
        expires_at: Option<i64>,
    ) -> TopicBindingRecord {
        TopicBindingRecord {
            schema_version: 1,
            version: 1,
            user_id: 7,
            topic_id: topic_id.to_string(),
            agent_id: agent_id.to_string(),
            binding_kind: TopicBindingKind::Runtime,
            chat_id: Some(-1001),
            thread_id: Some(42),
            expires_at,
            last_activity_at: Some(100),
            created_at: 10,
            updated_at: 11,
        }
    }

    #[test]
    fn dynamic_binding_route_ignores_static_topic_fields() {
        let binding = topic_binding("-1001:42", "dynamic-agent", None);
        let decision = dynamic_route_decision(&binding, None);

        assert!(decision.allows_processing());
        assert_eq!(decision.agent_id.as_deref(), Some("dynamic-agent"));
        assert_eq!(decision.system_prompt_override, None);
        assert_eq!(
            decision.dynamic_binding_topic_id.as_deref(),
            Some("-1001:42")
        );
    }

    #[test]
    fn expired_binding_falls_back_to_static_config() {
        let mut decision = resolve_topic_route_decision(
            Some(&topic(true, false, None, Some("static-agent"))),
            &TopicRouteContext {
                text: Some("hello"),
                caption: None,
                reply_to_bot: false,
            },
            Some("oxide_bot"),
        );
        let expired = topic_binding("-1001:42", "dynamic-agent", Some(5));
        let now = 6;

        if let Some(active_binding) = resolve_active_topic_binding(Some(expired), now) {
            decision.agent_id = Some(active_binding.agent_id);
            decision.dynamic_binding_topic_id = Some(active_binding.topic_id);
        }

        assert_eq!(decision.agent_id.as_deref(), Some("static-agent"));
        assert_eq!(decision.dynamic_binding_topic_id, None);
    }

    #[test]
    fn profile_system_prompt_from_bound_agent_profile_becomes_override() {
        let camel = select_profile_system_prompt(Some("  dynamic profile prompt  "), None);
        let snake = select_profile_system_prompt(None, Some("snake_case prompt"));
        let fallback = select_profile_system_prompt(Some("   "), Some("snake"));

        assert_eq!(camel.as_deref(), Some("dynamic profile prompt"));
        assert_eq!(snake.as_deref(), Some("snake_case prompt"));
        assert_eq!(fallback.as_deref(), Some("snake"));
    }

    #[test]
    fn activity_touch_path_only_occurs_for_active_dynamic_binding() {
        let active = topic_binding("-1001:42", "dynamic-agent", Some(50));
        let expired = topic_binding("-1001:42", "dynamic-agent", Some(50));

        let active_touch = build_binding_activity_touch_options(active, 40);
        let expired_touch = build_binding_activity_touch_options(expired, 60);

        assert!(active_touch.is_some());
        assert!(expired_touch.is_none());
    }
}
