use super::*;

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
