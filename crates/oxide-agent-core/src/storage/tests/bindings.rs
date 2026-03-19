use super::*;

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
