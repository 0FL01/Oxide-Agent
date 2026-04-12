use super::*;

#[tokio::test]
async fn manager_dry_run_mutation_does_not_persist_via_executor_registry() {
    let mut mock = MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_upsert_topic_binding().times(0);
    mock.expect_append_audit_event()
        .withf(|options: &AppendAuditEventOptions| {
            options.user_id == 77
                && options.action == "topic_binding_set"
                && options.payload.get("outcome") == Some(&json!("dry_run"))
        })
        .returning(|options| Ok(build_audit_record(options)));

    let executor = build_executor().with_manager_control_plane(Arc::new(mock), 77);
    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    let response = registry
        .execute(
            "topic_binding_set",
            r#"{"topic_id":"topic-a","agent_id":"agent-a","dry_run":true}"#,
            None,
            None,
        )
        .await
        .expect("dry-run manager mutation must succeed");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("dry-run response must be valid json");
    assert_eq!(parsed["dry_run"], true);
    assert_eq!(parsed["preview"]["topic_id"], "topic-a");
}

#[tokio::test]
async fn manager_dry_run_mutation_reports_audit_write_failure_non_fatally() {
    let mut mock = MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_upsert_topic_binding().times(0);
    mock.expect_append_audit_event().returning(|_| {
        Err(crate::storage::StorageError::Config(
            "audit unavailable".to_string(),
        ))
    });

    let executor = build_executor().with_manager_control_plane(Arc::new(mock), 77);
    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    let response = registry
        .execute(
            "topic_binding_set",
            r#"{"topic_id":"topic-a","agent_id":"agent-a","dry_run":true}"#,
            None,
            None,
        )
        .await
        .expect("dry-run manager mutation must remain non-fatal when audit write fails");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("dry-run response must be valid json");
    assert_eq!(parsed["dry_run"], true);
    assert_eq!(parsed["audit_status"], "write_failed");
    assert_eq!(parsed["preview"]["topic_id"], "topic-a");
}

#[tokio::test]
async fn manager_executor_forum_topic_create_uses_lifecycle_with_non_fatal_audit() {
    let mut mock = MockStorageProvider::new();
    mock.expect_get_user_config()
        .returning(|_| Ok(crate::storage::UserConfig::default()));
    mock.expect_update_user_config().returning(|_, _| Ok(()));
    mock.expect_append_audit_event().returning(|_| {
        Err(crate::storage::StorageError::Config(
            "audit unavailable".to_string(),
        ))
    });

    let lifecycle = Arc::new(RecordingTopicLifecycle::new());
    let executor = build_executor()
        .with_manager_control_plane(Arc::new(mock), 77)
        .with_manager_topic_lifecycle(lifecycle.clone());
    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    let response = registry
        .execute(
            "forum_topic_create",
            r#"{"chat_id":-100777,"name":"runtime-topic"}"#,
            None,
            None,
        )
        .await
        .expect("forum_topic_create must succeed when lifecycle succeeds");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("forum topic create response must be valid json");
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["topic"]["thread_id"], 313);
    assert_eq!(parsed["topic"]["name"], "runtime-topic");
    assert_eq!(parsed["audit_status"], "write_failed");

    let calls = lifecycle.create_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].chat_id, Some(-100_777));
    assert_eq!(calls[0].name, "runtime-topic");
}

#[tokio::test]
async fn manager_executor_forum_topic_create_dry_run_skips_lifecycle() {
    let mut mock = MockStorageProvider::new();
    mock.expect_append_audit_event()
        .returning(|options| Ok(build_audit_record(options)));

    let lifecycle = Arc::new(RecordingTopicLifecycle::new());
    let executor = build_executor()
        .with_manager_control_plane(Arc::new(mock), 77)
        .with_manager_topic_lifecycle(lifecycle.clone());
    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    let response = registry
        .execute(
            "forum_topic_create",
            r#"{"chat_id":-100777,"name":"dry-run","dry_run":true}"#,
            None,
            None,
        )
        .await
        .expect("dry-run forum_topic_create must succeed");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("dry-run response must be valid json");
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["dry_run"], true);
    assert_eq!(parsed["audit_status"], "written");
    assert!(lifecycle.create_calls().is_empty());
}

#[tokio::test]
async fn manager_rollback_restores_snapshot_via_executor_registry() {
    let mut mock = MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|user_id, topic_id| {
            Ok(Some(TopicBindingRecord {
                schema_version: 1,
                version: 5,
                user_id,
                topic_id,
                agent_id: "agent-current".to_string(),
                binding_kind: TopicBindingKind::Manual,
                chat_id: None,
                thread_id: None,
                expires_at: None,
                last_activity_at: Some(20),
                created_at: 10,
                updated_at: 20,
            }))
        });
    mock.expect_list_audit_events_page()
        .with(eq(77_i64), eq(None), eq(200_usize))
        .returning(|_, _, _| {
            Ok(vec![AuditEventRecord {
                schema_version: 1,
                version: 4,
                event_id: "evt-4".to_string(),
                user_id: 77,
                topic_id: Some("topic-a".to_string()),
                agent_id: Some("agent-previous".to_string()),
                action: "topic_binding_set".to_string(),
                payload: json!({
                    "topic_id": "topic-a",
                    "previous": {
                        "schema_version": 1,
                        "version": 2,
                        "user_id": 77,
                        "topic_id": "topic-a",
                        "agent_id": "agent-previous",
                        "created_at": 1,
                        "updated_at": 2
                    },
                    "outcome": "applied"
                }),
                created_at: 30,
            }])
        });
    mock.expect_upsert_topic_binding()
        .withf(|options| {
            options.user_id == 77
                && options.topic_id == "topic-a"
                && options.agent_id == "agent-previous"
        })
        .returning(|options| {
            Ok(TopicBindingRecord {
                schema_version: 1,
                version: 6,
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                binding_kind: options.binding_kind.unwrap_or(TopicBindingKind::Manual),
                chat_id: options.chat_id.for_new_record(),
                thread_id: options.thread_id.for_new_record(),
                expires_at: options.expires_at.for_new_record(),
                last_activity_at: options.last_activity_at,
                created_at: 40,
                updated_at: 50,
            })
        });
    mock.expect_delete_topic_binding().times(0);
    mock.expect_append_audit_event()
        .withf(|options: &AppendAuditEventOptions| {
            options.action == "topic_binding_rollback"
                && options.payload.get("operation") == Some(&json!("restore"))
        })
        .returning(|options| Ok(build_audit_record(options)));

    let executor = build_executor().with_manager_control_plane(Arc::new(mock), 77);
    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    let response = registry
        .execute(
            "topic_binding_rollback",
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("rollback restore path must succeed");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("rollback response must be valid json");
    assert_eq!(parsed["operation"], "restore");
    assert_eq!(parsed["binding"]["agent_id"], "agent-previous");
}

#[tokio::test]
async fn manager_rollback_deletes_when_snapshot_is_empty_via_executor_registry() {
    let mut mock = MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|user_id, topic_id| {
            Ok(Some(TopicBindingRecord {
                schema_version: 1,
                version: 5,
                user_id,
                topic_id,
                agent_id: "agent-current".to_string(),
                binding_kind: TopicBindingKind::Manual,
                chat_id: None,
                thread_id: None,
                expires_at: None,
                last_activity_at: Some(20),
                created_at: 10,
                updated_at: 20,
            }))
        });
    mock.expect_list_audit_events_page()
        .with(eq(77_i64), eq(None), eq(200_usize))
        .returning(|_, _, _| {
            Ok(vec![AuditEventRecord {
                schema_version: 1,
                version: 4,
                event_id: "evt-4".to_string(),
                user_id: 77,
                topic_id: Some("topic-a".to_string()),
                agent_id: Some("agent-current".to_string()),
                action: "topic_binding_delete".to_string(),
                payload: json!({
                    "topic_id": "topic-a",
                    "previous": null,
                    "outcome": "applied"
                }),
                created_at: 30,
            }])
        });
    mock.expect_upsert_topic_binding().times(0);
    mock.expect_delete_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(()));
    mock.expect_append_audit_event()
        .withf(|options: &AppendAuditEventOptions| {
            options.action == "topic_binding_rollback"
                && options.payload.get("operation") == Some(&json!("delete"))
        })
        .returning(|options| Ok(build_audit_record(options)));

    let executor = build_executor().with_manager_control_plane(Arc::new(mock), 77);
    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    let response = registry
        .execute(
            "topic_binding_rollback",
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("rollback delete path must succeed");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("rollback response must be valid json");
    assert_eq!(parsed["operation"], "delete");
    assert!(parsed["binding"].is_null());
}
