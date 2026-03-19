use super::*;

#[test]
fn user_chat_history_key_uses_chat_uuid_namespace() {
    let key = user_chat_history_key(42, "chat-123");
    assert_eq!(key, "users/42/chats/chat-123/history.json");
}

#[test]
fn legacy_user_history_key_stays_unchanged() {
    let key = user_history_key(42);
    assert_eq!(key, "users/42/history.json");
}

#[test]
fn user_chat_history_key_isolated_by_user_and_chat_uuid() {
    let key_a = user_chat_history_key(1, "chat-a");
    let key_b = user_chat_history_key(1, "chat-b");
    let key_c = user_chat_history_key(2, "chat-a");

    assert_ne!(key_a, key_b);
    assert_ne!(key_a, key_c);
    assert_ne!(key_b, key_c);
}

#[test]
fn user_context_agent_memory_key_uses_topic_namespace() {
    let key = user_context_agent_memory_key(42, "-1001:77");
    assert_eq!(key, "users/42/topics/-1001:77/agent_memory.json");
}

#[test]
fn user_context_agent_flows_prefix_uses_topic_namespace() {
    let prefix = user_context_agent_flows_prefix(42, "-1001:77");
    assert_eq!(prefix, "users/42/topics/-1001:77/flows/");
}

#[test]
fn user_context_agent_flow_key_uses_flow_namespace() {
    let key = user_context_agent_flow_key(42, "-1001:77", "flow-123");
    assert_eq!(key, "users/42/topics/-1001:77/flows/flow-123/meta.json");
}

#[test]
fn user_context_agent_flow_memory_key_uses_flow_namespace() {
    let key = user_context_agent_flow_memory_key(42, "-1001:77", "flow-123");
    assert_eq!(key, "users/42/topics/-1001:77/flows/flow-123/memory.json");
}

#[test]
fn user_context_chat_history_prefix_uses_topic_namespace() {
    let prefix = user_context_chat_history_prefix(42, "-1001:77");
    assert_eq!(prefix, "users/42/chats/-1001:77/");
}

#[test]
fn generate_chat_uuid_returns_v4_uuid() {
    let chat_uuid = generate_chat_uuid();
    let parsed = Uuid::parse_str(&chat_uuid);
    assert!(parsed.is_ok());
    let version = parsed.map(|uuid| uuid.get_version_num());
    assert_eq!(version, Ok(4));
}

#[test]
fn user_config_deserializes_without_current_chat_uuid() {
    let json = r#"{
            "system_prompt": "You are helpful",
            "model_name": "gpt",
            "state": "idle"
        }"#;

    let parsed: Result<UserConfig, serde_json::Error> = serde_json::from_str(json);
    assert!(parsed.is_ok());
    let config = parsed.ok();
    assert!(config.is_some());
    assert_eq!(config.and_then(|cfg| cfg.current_chat_uuid), None);
}

#[test]
fn user_config_roundtrip_preserves_current_chat_uuid() {
    let config = UserConfig {
        system_prompt: Some("You are helpful".to_string()),
        model_name: Some("gpt".to_string()),
        state: Some("chat_mode".to_string()),
        current_chat_uuid: Some("123e4567-e89b-12d3-a456-426614174000".to_string()),
        contexts: HashMap::new(),
    };

    let json = serde_json::to_string(&config);
    assert!(json.is_ok());

    let parsed: Result<UserConfig, serde_json::Error> =
        serde_json::from_str(&json.unwrap_or_default());
    assert!(parsed.is_ok());

    let parsed = parsed.unwrap_or_default();
    assert_eq!(
        parsed.current_chat_uuid,
        Some("123e4567-e89b-12d3-a456-426614174000".to_string())
    );
}

#[test]
fn user_config_roundtrip_preserves_context_scoped_metadata() {
    let mut contexts = HashMap::new();
    contexts.insert(
        "-1001:42".to_string(),
        UserContextConfig {
            state: Some("agent_mode".to_string()),
            current_chat_uuid: Some("chat-42".to_string()),
            current_agent_flow_id: Some("flow-42".to_string()),
            chat_id: Some(-1001),
            thread_id: Some(42),
            forum_topic_name: Some("Topic 42".to_string()),
            forum_topic_icon_color: Some(7_322_096),
            forum_topic_icon_custom_emoji_id: Some("emoji-42".to_string()),
            forum_topic_closed: true,
        },
    );
    let config = UserConfig {
        contexts,
        ..UserConfig::default()
    };

    let json = serde_json::to_string(&config).expect("config must encode");
    let parsed: UserConfig = serde_json::from_str(&json).expect("config must decode");

    assert_eq!(parsed.contexts.len(), 1);
    assert_eq!(
        parsed
            .contexts
            .get("-1001:42")
            .and_then(|context| context.state.as_deref()),
        Some("agent_mode")
    );
    assert_eq!(
        parsed
            .contexts
            .get("-1001:42")
            .and_then(|context| context.current_agent_flow_id.as_deref()),
        Some("flow-42")
    );
    assert_eq!(
        parsed
            .contexts
            .get("-1001:42")
            .and_then(|context| context.forum_topic_name.as_deref()),
        Some("Topic 42")
    );
    assert!(parsed
        .contexts
        .get("-1001:42")
        .is_some_and(|context| context.forum_topic_closed));
}

#[test]
fn user_config_key_stays_stable() {
    let key = user_config_key(42);
    assert_eq!(key, "users/42/config.json");
}

#[test]
fn agent_profile_key_uses_control_plane_namespace() {
    let key = agent_profile_key(42, "agent-a");
    assert_eq!(key, "users/42/control_plane/agent_profiles/agent-a.json");
}

#[test]
fn topic_binding_key_uses_control_plane_namespace() {
    let key = topic_binding_key(42, "topic-a");
    assert_eq!(key, "users/42/control_plane/topic_bindings/topic-a.json");
}

#[test]
fn topic_context_key_uses_control_plane_namespace() {
    let key = topic_context_key(42, "topic-a");
    assert_eq!(key, "users/42/control_plane/topic_contexts/topic-a.json");
}

#[test]
fn topic_agents_md_key_uses_control_plane_namespace() {
    let key = topic_agents_md_key(42, "topic-a");
    assert_eq!(key, "users/42/control_plane/topic_agents_md/topic-a.json");
}

#[test]
fn topic_infra_config_key_uses_control_plane_namespace() {
    let key = topic_infra_config_key(42, "topic-a");
    assert_eq!(key, "users/42/control_plane/topic_infra/topic-a.json");
}

#[test]
fn private_secret_key_uses_private_namespace() {
    let key = private_secret_key(42, "ssh/prod-key");
    assert_eq!(key, "users/42/private/secrets/ssh/prod-key");
}

#[test]
fn audit_events_key_uses_control_plane_namespace() {
    let key = audit_events_key(42);
    assert_eq!(key, "users/42/control_plane/audit/events.json");
}
