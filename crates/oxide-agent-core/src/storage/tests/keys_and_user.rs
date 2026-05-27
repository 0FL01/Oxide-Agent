use super::*;

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
fn generate_flow_id_returns_v4_uuid() {
    let flow_id = generate_flow_id();
    let parsed = Uuid::parse_str(&flow_id);
    assert!(parsed.is_ok());
    let version = parsed.map(|uuid| uuid.get_version_num());
    assert_eq!(version, Ok(4));
}

#[test]
fn user_config_deserializes_without_removed_chat_fields() {
    let json = r#"{
            "system_prompt": "You are helpful",
            "model_name": "gpt",
            "state": "idle"
        }"#;

    let parsed: Result<UserConfig, serde_json::Error> = serde_json::from_str(json);
    assert!(parsed.is_ok());
    let config = parsed.ok();
    assert!(config.is_some());
    assert_eq!(config.and_then(|cfg| cfg.state), Some("idle".to_string()));
}

#[test]
fn user_config_roundtrip_preserves_context_scoped_metadata() {
    let mut contexts = HashMap::new();
    contexts.insert(
        "-1001:42".to_string(),
        UserContextConfig {
            state: Some("agent_mode".to_string()),
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

#[test]
fn wiki_global_key_uses_versioned_prefix() {
    let key = wiki_global_key("prod", "index.md");
    assert_eq!(key, "prod/wiki/v1/global/index.md");
}

#[test]
fn wiki_keys_trim_storage_prefix_slashes() {
    let key = wiki_context_key("/prod/", "telegram-topic-a13f9c2b", "overview.md");
    assert_eq!(
        key,
        "prod/wiki/v1/contexts/telegram-topic-a13f9c2b/overview.md"
    );
}

#[test]
fn wiki_keys_work_without_storage_prefix() {
    let key = wiki_context_key("", "telegram-topic-a13f9c2b", "index.md");
    assert_eq!(key, "wiki/v1/contexts/telegram-topic-a13f9c2b/index.md");
}

#[test]
fn wiki_context_page_key_uses_pages_namespace() {
    let key = wiki_context_page_key("prod", "ctx-12345678", "deploy-runbook");
    assert_eq!(
        key,
        "prod/wiki/v1/contexts/ctx-12345678/pages/deploy-runbook.md"
    );
}

#[test]
fn wiki_context_inbox_key_uses_inbox_namespace() {
    let key = wiki_context_inbox_key("prod", "ctx-12345678", "2026-05-19-task-low-confidence");
    assert_eq!(
        key,
        "prod/wiki/v1/contexts/ctx-12345678/inbox/2026-05-19-task-low-confidence.md"
    );
}

#[test]
fn wiki_context_raw_key_uses_month_partition() {
    let key = wiki_context_raw_key("prod", "ctx-12345678", "2026-05", "run-abc");
    assert_eq!(
        key,
        "prod/wiki/v1/contexts/ctx-12345678/raw/2026-05/run-abc.md"
    );
}
