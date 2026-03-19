
use super::{
    agent_profile_key, audit_events_key, binding_is_active, build_agent_flow_record,
    build_agent_profile_record, build_audit_event_record, build_topic_agents_md_record,
    build_topic_binding_record, build_topic_context_record, build_topic_infra_config_record,
    compute_cron_next_run_at, compute_next_reminder_run_at, generate_chat_uuid,
    next_record_version, normalize_topic_prompt_payload, parse_reminder_timezone,
    private_secret_key, resolve_active_topic_binding, select_audit_events_page,
    should_retry_control_plane_rmw, topic_agents_md_key, topic_binding_key, topic_context_key,
    topic_infra_config_key, user_chat_history_key, user_config_key, user_context_agent_flow_key,
    user_context_agent_flow_memory_key, user_context_agent_flows_prefix,
    user_context_agent_memory_key, user_context_chat_history_prefix, user_history_key,
    validate_topic_agents_md_content, validate_topic_context_content, AgentFlowRecord,
    AgentProfileRecord, AppendAuditEventOptions, AuditEventRecord, ControlPlaneLocks,
    OptionalMetadataPatch, ReminderJobRecord, ReminderJobStatus, ReminderScheduleKind,
    ReminderThreadKind, TopicAgentsMdRecord, TopicBindingKind, TopicBindingRecord,
    TopicContextRecord, TopicInfraAuthMode, TopicInfraConfigRecord, TopicInfraToolMode,
    UpsertAgentProfileOptions, UpsertTopicAgentsMdOptions, UpsertTopicBindingOptions,
    UpsertTopicContextOptions, UpsertTopicInfraConfigOptions, UserConfig, UserContextConfig,
    TOPIC_AGENTS_MD_MAX_LINES, TOPIC_CONTEXT_MAX_CHARS, TOPIC_CONTEXT_MAX_LINES,
};
use chrono::TimeZone;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::timeout;
use uuid::Uuid;

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
fn build_agent_flow_record_preserves_created_at() {
    let existing = AgentFlowRecord {
        schema_version: 1,
        user_id: 7,
        context_key: "topic-a".to_string(),
        flow_id: "flow-a".to_string(),
        created_at: 123,
        updated_at: 124,
    };

    let updated = build_agent_flow_record(
        7,
        "topic-a".to_string(),
        "flow-a".to_string(),
        Some(existing),
        999,
    );

    assert_eq!(updated.schema_version, 1);
    assert_eq!(updated.created_at, 123);
    assert_eq!(updated.updated_at, 999);
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
fn parse_reminder_timezone_defaults_to_utc() {
    let timezone = parse_reminder_timezone(None).expect("timezone should parse");
    assert_eq!(timezone.name(), "UTC");
}

#[test]
fn compute_cron_next_run_at_uses_timezone() {
    let after = chrono::Utc
        .with_ymd_and_hms(2026, 6, 1, 6, 0, 0)
        .single()
        .expect("valid datetime")
        .timestamp();
    let next = compute_cron_next_run_at("0 0 9 * * * *", Some("Europe/Berlin"), after)
        .expect("cron should resolve");
    let expected = chrono::Utc
        .with_ymd_and_hms(2026, 6, 1, 7, 0, 0)
        .single()
        .expect("valid datetime")
        .timestamp();
    assert_eq!(next, expected);
}

#[test]
fn compute_next_reminder_run_at_supports_cron_records() {
    let record = ReminderJobRecord {
        schema_version: 2,
        version: 1,
        reminder_id: "rem-1".to_string(),
        user_id: 1,
        context_key: "ctx".to_string(),
        flow_id: "flow".to_string(),
        chat_id: 1,
        thread_id: None,
        thread_kind: ReminderThreadKind::Dm,
        task_prompt: "Ping".to_string(),
        schedule_kind: ReminderScheduleKind::Cron,
        status: ReminderJobStatus::Scheduled,
        next_run_at: 0,
        interval_secs: None,
        cron_expression: Some("0 0 9 * * * *".to_string()),
        timezone: Some("UTC".to_string()),
        lease_until: None,
        last_run_at: None,
        last_error: None,
        run_count: 0,
        created_at: 0,
        updated_at: 0,
    };
    let after = chrono::Utc
        .with_ymd_and_hms(2026, 6, 1, 8, 0, 0)
        .single()
        .expect("valid datetime")
        .timestamp();

    let next = compute_next_reminder_run_at(&record, after).expect("next run should compute");
    let expected = chrono::Utc
        .with_ymd_and_hms(2026, 6, 1, 9, 0, 0)
        .single()
        .expect("valid datetime")
        .timestamp();
    assert_eq!(next, Some(expected));
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
fn normalize_topic_prompt_payload_normalizes_line_endings_and_trailing_spaces() {
    let normalized = normalize_topic_prompt_payload("  line 1  \r\nline 2\t\r\n\r\n");
    assert_eq!(normalized, "line 1\nline 2");
}

#[test]
fn validate_topic_context_rejects_markdown_documents() {
    let error = validate_topic_context_content("# AGENTS\nDo the thing")
        .expect_err("markdown document must be rejected");
    assert!(error
        .to_string()
        .contains("store AGENTS.md-style documents in topic_agents_md"));
}

#[test]
fn validate_topic_context_rejects_oversized_payload() {
    let oversized = vec!["line"; TOPIC_CONTEXT_MAX_LINES + 1].join("\n");
    let error = validate_topic_context_content(&oversized)
        .expect_err("oversized topic context must be rejected");
    assert!(error.to_string().contains(&format!(
        "context must not exceed {TOPIC_CONTEXT_MAX_LINES} lines"
    )));
}

#[test]
fn validate_topic_context_rejects_too_many_characters() {
    let oversized = "x".repeat(TOPIC_CONTEXT_MAX_CHARS + 1);
    let error = validate_topic_context_content(&oversized)
        .expect_err("oversized topic context must be rejected");
    assert!(error.to_string().contains(&format!(
        "context must not exceed {TOPIC_CONTEXT_MAX_CHARS} characters"
    )));
}

#[test]
fn validate_topic_agents_md_normalizes_payload() {
    let normalized = validate_topic_agents_md_content("\r\n# Topic AGENTS  \r\nUse checklist\r\n")
        .expect("agents md must normalize");
    assert_eq!(normalized, "# Topic AGENTS\nUse checklist");
}

#[test]
fn validate_topic_agents_md_rejects_oversized_payload() {
    let oversized = vec!["line"; TOPIC_AGENTS_MD_MAX_LINES + 1].join("\n");
    let error = validate_topic_agents_md_content(&oversized)
        .expect_err("oversized agents md must be rejected");
    assert!(error.to_string().contains(&format!(
        "agents_md must not exceed {TOPIC_AGENTS_MD_MAX_LINES} lines"
    )));
}

#[test]
fn next_record_version_starts_at_one() {
    assert_eq!(next_record_version(None), 1);
}

#[test]
fn next_record_version_increments_existing_value() {
    assert_eq!(next_record_version(Some(7)), 8);
}

#[test]
fn next_record_version_saturates_on_overflow_boundary() {
    assert_eq!(next_record_version(Some(u64::MAX)), u64::MAX);
}

#[test]
fn upsert_agent_profile_increments_version_and_preserves_created_at() {
    let existing = AgentProfileRecord {
        schema_version: 1,
        version: 3,
        user_id: 7,
        agent_id: "agent-a".to_string(),
        profile: json!({"name": "before"}),
        created_at: 123,
        updated_at: 124,
    };

    let updated = build_agent_profile_record(
        UpsertAgentProfileOptions {
            user_id: 7,
            agent_id: "agent-a".to_string(),
            profile: json!({"name": "after"}),
        },
        Some(existing),
        999,
    );

    assert_eq!(updated.version, 4);
    assert_eq!(updated.created_at, 123);
    assert_eq!(updated.updated_at, 999);
}

#[test]
fn upsert_agent_profile_initial_insert_starts_version_and_sets_timestamps() {
    let created = build_agent_profile_record(
        UpsertAgentProfileOptions {
            user_id: 7,
            agent_id: "agent-a".to_string(),
            profile: json!({"name": "new"}),
        },
        None,
        777,
    );

    assert_eq!(created.version, 1);
    assert_eq!(created.created_at, 777);
    assert_eq!(created.updated_at, 777);
}

#[test]
fn upsert_topic_context_increments_version_and_preserves_created_at() {
    let existing = TopicContextRecord {
        schema_version: 1,
        version: 3,
        user_id: 7,
        topic_id: "topic-a".to_string(),
        context: "before".to_string(),
        created_at: 123,
        updated_at: 124,
    };

    let updated = build_topic_context_record(
        UpsertTopicContextOptions {
            user_id: 7,
            topic_id: "topic-a".to_string(),
            context: "after".to_string(),
        },
        Some(existing),
        999,
    );

    assert_eq!(updated.version, 4);
    assert_eq!(updated.created_at, 123);
    assert_eq!(updated.updated_at, 999);
    assert_eq!(updated.context, "after");
}

#[test]
fn upsert_topic_context_initial_insert_starts_version_and_sets_timestamps() {
    let created = build_topic_context_record(
        UpsertTopicContextOptions {
            user_id: 7,
            topic_id: "topic-a".to_string(),
            context: "topic instructions".to_string(),
        },
        None,
        777,
    );

    assert_eq!(created.version, 1);
    assert_eq!(created.created_at, 777);
    assert_eq!(created.updated_at, 777);
    assert_eq!(created.schema_version, 1);
}

#[test]
fn upsert_topic_agents_md_increments_version_and_preserves_created_at() {
    let existing = TopicAgentsMdRecord {
        schema_version: 1,
        version: 3,
        user_id: 7,
        topic_id: "topic-a".to_string(),
        agents_md: "before".to_string(),
        created_at: 123,
        updated_at: 124,
    };

    let updated = build_topic_agents_md_record(
        UpsertTopicAgentsMdOptions {
            user_id: 7,
            topic_id: "topic-a".to_string(),
            agents_md: "after".to_string(),
        },
        Some(existing),
        999,
    );

    assert_eq!(updated.version, 4);
    assert_eq!(updated.created_at, 123);
    assert_eq!(updated.updated_at, 999);
    assert_eq!(updated.agents_md, "after");
}

#[test]
fn upsert_topic_agents_md_initial_insert_starts_version_and_sets_timestamps() {
    let created = build_topic_agents_md_record(
        UpsertTopicAgentsMdOptions {
            user_id: 7,
            topic_id: "topic-a".to_string(),
            agents_md: "# Topic agent instructions".to_string(),
        },
        None,
        777,
    );

    assert_eq!(created.version, 1);
    assert_eq!(created.created_at, 777);
    assert_eq!(created.updated_at, 777);
    assert_eq!(created.schema_version, 1);
}

#[test]
fn upsert_topic_infra_config_increments_version_and_preserves_created_at() {
    let existing = TopicInfraConfigRecord {
        schema_version: 1,
        version: 2,
        user_id: 7,
        topic_id: "topic-a".to_string(),
        target_name: "prod-app".to_string(),
        host: "prod.example.com".to_string(),
        port: 22,
        remote_user: "deploy".to_string(),
        auth_mode: TopicInfraAuthMode::PrivateKey,
        secret_ref: Some("storage:ssh/prod-key".to_string()),
        sudo_secret_ref: Some("storage:ssh/prod-sudo".to_string()),
        environment: Some("prod".to_string()),
        tags: vec!["prod".to_string()],
        allowed_tool_modes: vec![TopicInfraToolMode::Exec],
        approval_required_modes: vec![TopicInfraToolMode::SudoExec],
        created_at: 123,
        updated_at: 124,
    };

    let updated = build_topic_infra_config_record(
        UpsertTopicInfraConfigOptions {
            user_id: 7,
            topic_id: "topic-a".to_string(),
            target_name: "prod-app-new".to_string(),
            host: "prod2.example.com".to_string(),
            port: 2222,
            remote_user: "ops".to_string(),
            auth_mode: TopicInfraAuthMode::Password,
            secret_ref: Some("env:SSH_PASSWORD".to_string()),
            sudo_secret_ref: None,
            environment: Some("prod".to_string()),
            tags: vec!["prod".to_string(), "critical".to_string()],
            allowed_tool_modes: vec![TopicInfraToolMode::Exec, TopicInfraToolMode::ReadFile],
            approval_required_modes: vec![TopicInfraToolMode::Exec],
        },
        Some(existing),
        999,
    );

    assert_eq!(updated.version, 3);
    assert_eq!(updated.created_at, 123);
    assert_eq!(updated.updated_at, 999);
    assert_eq!(updated.target_name, "prod-app-new");
    assert_eq!(updated.port, 2222);
}

#[test]
fn upsert_topic_infra_config_initial_insert_starts_version_and_sets_timestamps() {
    let created = build_topic_infra_config_record(
        UpsertTopicInfraConfigOptions {
            user_id: 7,
            topic_id: "topic-a".to_string(),
            target_name: "stage-app".to_string(),
            host: "stage.example.com".to_string(),
            port: 22,
            remote_user: "deploy".to_string(),
            auth_mode: TopicInfraAuthMode::PrivateKey,
            secret_ref: Some("storage:ssh/stage-key".to_string()),
            sudo_secret_ref: None,
            environment: Some("stage".to_string()),
            tags: vec!["stage".to_string()],
            allowed_tool_modes: vec![TopicInfraToolMode::Exec],
            approval_required_modes: vec![TopicInfraToolMode::SudoExec],
        },
        None,
        777,
    );

    assert_eq!(created.version, 1);
    assert_eq!(created.created_at, 777);
    assert_eq!(created.updated_at, 777);
    assert_eq!(created.schema_version, 1);
}

#[test]
fn upsert_topic_binding_increments_version_and_preserves_created_at() {
    let existing = TopicBindingRecord {
        schema_version: 1,
        version: 8,
        user_id: 7,
        topic_id: "topic-a".to_string(),
        agent_id: "agent-a".to_string(),
        binding_kind: TopicBindingKind::Manual,
        chat_id: Some(100),
        thread_id: Some(7),
        expires_at: Some(10_000),
        last_activity_at: Some(501),
        created_at: 500,
        updated_at: 501,
    };

    let updated = build_topic_binding_record(
        UpsertTopicBindingOptions {
            user_id: 7,
            topic_id: "topic-a".to_string(),
            agent_id: "agent-b".to_string(),
            binding_kind: None,
            chat_id: OptionalMetadataPatch::Keep,
            thread_id: OptionalMetadataPatch::Keep,
            expires_at: OptionalMetadataPatch::Keep,
            last_activity_at: None,
        },
        Some(existing),
        1_000,
    );

    assert_eq!(updated.version, 9);
    assert_eq!(updated.created_at, 500);
    assert_eq!(updated.updated_at, 1_000);
    assert_eq!(updated.agent_id, "agent-b");
    assert_eq!(updated.binding_kind, TopicBindingKind::Manual);
    assert_eq!(updated.chat_id, Some(100));
    assert_eq!(updated.thread_id, Some(7));
    assert_eq!(updated.expires_at, Some(10_000));
    assert_eq!(updated.last_activity_at, Some(1_000));
}

#[test]
fn upsert_topic_binding_explicit_clear_resets_optional_metadata_fields() {
    let existing = TopicBindingRecord {
        schema_version: 1,
        version: 8,
        user_id: 7,
        topic_id: "topic-a".to_string(),
        agent_id: "agent-a".to_string(),
        binding_kind: TopicBindingKind::Manual,
        chat_id: Some(100),
        thread_id: Some(7),
        expires_at: Some(10_000),
        last_activity_at: Some(501),
        created_at: 500,
        updated_at: 501,
    };

    let updated = build_topic_binding_record(
        UpsertTopicBindingOptions {
            user_id: 7,
            topic_id: "topic-a".to_string(),
            agent_id: "agent-a".to_string(),
            binding_kind: None,
            chat_id: OptionalMetadataPatch::Clear,
            thread_id: OptionalMetadataPatch::Clear,
            expires_at: OptionalMetadataPatch::Clear,
            last_activity_at: None,
        },
        Some(existing),
        1_000,
    );

    assert_eq!(updated.chat_id, None);
    assert_eq!(updated.thread_id, None);
    assert_eq!(updated.expires_at, None);
}

#[test]
fn upsert_topic_binding_initial_insert_starts_version_and_sets_timestamps() {
    let created = build_topic_binding_record(
        UpsertTopicBindingOptions {
            user_id: 7,
            topic_id: "topic-a".to_string(),
            agent_id: "agent-a".to_string(),
            binding_kind: Some(TopicBindingKind::Runtime),
            chat_id: OptionalMetadataPatch::Set(42),
            thread_id: OptionalMetadataPatch::Set(99),
            expires_at: OptionalMetadataPatch::Set(2_100),
            last_activity_at: None,
        },
        None,
        2_000,
    );

    assert_eq!(created.version, 1);
    assert_eq!(created.created_at, 2_000);
    assert_eq!(created.updated_at, 2_000);
    assert_eq!(created.schema_version, 2);
    assert_eq!(created.binding_kind, TopicBindingKind::Runtime);
    assert_eq!(created.chat_id, Some(42));
    assert_eq!(created.thread_id, Some(99));
    assert_eq!(created.expires_at, Some(2_100));
    assert_eq!(created.last_activity_at, Some(2_000));
}

#[test]
fn topic_binding_record_backward_compatible_deserialization_defaults_new_fields() {
    let raw = r#"{
            "schema_version": 1,
            "version": 3,
            "user_id": 7,
            "topic_id": "topic-a",
            "agent_id": "agent-a",
            "created_at": 100,
            "updated_at": 200
        }"#;

    let record: TopicBindingRecord = serde_json::from_str(raw).expect("record must deserialize");
    assert_eq!(record.binding_kind, TopicBindingKind::Manual);
    assert_eq!(record.chat_id, None);
    assert_eq!(record.thread_id, None);
    assert_eq!(record.expires_at, None);
    assert_eq!(record.last_activity_at, None);
}

#[test]
fn topic_binding_record_roundtrip_preserves_runtime_metadata() {
    let record = TopicBindingRecord {
        schema_version: 2,
        version: 1,
        user_id: 7,
        topic_id: "topic-a".to_string(),
        agent_id: "agent-a".to_string(),
        binding_kind: TopicBindingKind::Runtime,
        chat_id: Some(10),
        thread_id: Some(20),
        expires_at: Some(500),
        last_activity_at: Some(400),
        created_at: 100,
        updated_at: 200,
    };

    let encoded = serde_json::to_string(&record).expect("record must encode");
    let decoded_record: TopicBindingRecord =
        serde_json::from_str(&encoded).expect("roundtrip should decode");
    assert_eq!(decoded_record.binding_kind, TopicBindingKind::Runtime);
    assert_eq!(decoded_record.chat_id, Some(10));
    assert_eq!(decoded_record.thread_id, Some(20));
    assert_eq!(decoded_record.expires_at, Some(500));
    assert_eq!(decoded_record.last_activity_at, Some(400));
    assert_eq!(decoded_record.schema_version, 2);
}

#[test]
fn binding_activity_helper_distinguishes_active_and_expired_records() {
    let active_record = TopicBindingRecord {
        schema_version: 2,
        version: 1,
        user_id: 7,
        topic_id: "topic-a".to_string(),
        agent_id: "agent-a".to_string(),
        binding_kind: TopicBindingKind::Runtime,
        chat_id: Some(10),
        thread_id: Some(20),
        expires_at: Some(500),
        last_activity_at: Some(450),
        created_at: 100,
        updated_at: 200,
    };
    let expired_record = TopicBindingRecord {
        expires_at: Some(300),
        ..active_record.clone()
    };

    assert!(binding_is_active(&active_record, 499));
    assert!(!binding_is_active(&expired_record, 300));
    assert!(resolve_active_topic_binding(Some(active_record), 499).is_some());
    assert!(resolve_active_topic_binding(Some(expired_record), 300).is_none());
}

#[test]
fn append_audit_event_versions_are_monotonic() {
    let first = build_audit_event_record(
        AppendAuditEventOptions {
            user_id: 9,
            topic_id: Some("topic-a".to_string()),
            agent_id: Some("agent-a".to_string()),
            action: "created".to_string(),
            payload: json!({"k": 1}),
        },
        None,
        10,
        "event-1".to_string(),
    );

    let second = build_audit_event_record(
        AppendAuditEventOptions {
            user_id: 9,
            topic_id: Some("topic-a".to_string()),
            agent_id: Some("agent-a".to_string()),
            action: "updated".to_string(),
            payload: json!({"k": 2}),
        },
        Some(first.version),
        11,
        "event-2".to_string(),
    );

    assert_eq!(first.version, 1);
    assert_eq!(second.version, 2);
}

#[test]
fn append_audit_event_version_saturates_at_upper_bound() {
    let event = build_audit_event_record(
        AppendAuditEventOptions {
            user_id: 9,
            topic_id: None,
            agent_id: None,
            action: "updated".to_string(),
            payload: json!({"k": 2}),
        },
        Some(u64::MAX),
        11,
        "event-2".to_string(),
    );

    assert_eq!(event.version, u64::MAX);
}

#[test]
fn audit_page_cursor_returns_descending_window() {
    let events = vec![
        AuditEventRecord {
            schema_version: 1,
            version: 1,
            event_id: "evt-1".to_string(),
            user_id: 9,
            topic_id: None,
            agent_id: None,
            action: "a".to_string(),
            payload: json!({}),
            created_at: 1,
        },
        AuditEventRecord {
            schema_version: 1,
            version: 2,
            event_id: "evt-2".to_string(),
            user_id: 9,
            topic_id: None,
            agent_id: None,
            action: "b".to_string(),
            payload: json!({}),
            created_at: 2,
        },
        AuditEventRecord {
            schema_version: 1,
            version: 3,
            event_id: "evt-3".to_string(),
            user_id: 9,
            topic_id: None,
            agent_id: None,
            action: "c".to_string(),
            payload: json!({}),
            created_at: 3,
        },
    ];

    let first_page: Vec<u64> = select_audit_events_page(events.clone(), None, 2)
        .iter()
        .map(|event| event.version)
        .collect();
    let second_page: Vec<u64> = select_audit_events_page(events, Some(2), 2)
        .iter()
        .map(|event| event.version)
        .collect();

    assert_eq!(first_page, vec![3, 2]);
    assert_eq!(second_page, vec![1]);
}

#[test]
fn control_plane_retry_policy_stops_at_max_attempt() {
    assert!(should_retry_control_plane_rmw(1));
    assert!(should_retry_control_plane_rmw(4));
    assert!(!should_retry_control_plane_rmw(5));
    assert!(!should_retry_control_plane_rmw(6));
}

#[tokio::test]
async fn control_plane_lock_serializes_same_key_updates() {
    let locks = Arc::new(ControlPlaneLocks::new());
    let first_guard = locks
        .acquire("users/7/control_plane/topic_bindings/topic-a.json".to_string())
        .await;

    let locks_for_task = Arc::clone(&locks);
    let (tx, rx) = oneshot::channel();
    let join = tokio::spawn(async move {
        let _second_guard = locks_for_task
            .acquire("users/7/control_plane/topic_bindings/topic-a.json".to_string())
            .await;
        let _ = tx.send(());
    });

    let blocked_result = timeout(Duration::from_millis(50), rx).await;
    assert!(blocked_result.is_err());

    drop(first_guard);

    let join_result = timeout(Duration::from_secs(1), join).await;
    assert!(join_result.is_ok());
}
