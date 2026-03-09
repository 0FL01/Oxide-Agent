use oxide_agent_transport_telegram::bot::thread::{
    build_outbound_thread_params, resolve_thread_spec_from_context,
};
use oxide_agent_transport_telegram::bot::topic_route::{
    resolve_topic_route_decision, TopicRouteContext,
};
use oxide_agent_transport_telegram::config::{TelegramSettings, TelegramTopicSettings};
use teloxide::types::{MessageId, ThreadId};

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

#[test]
fn topic_routing_resolves_topic_settings_and_default_fallback() {
    let settings = TelegramSettings {
        telegram_token: "dummy".to_string(),
        allowed_users_str: None,
        agent_allowed_users_str: None,
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
