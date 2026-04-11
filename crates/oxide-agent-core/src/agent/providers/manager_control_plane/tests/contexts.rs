use super::*;

#[tokio::test]
async fn topic_context_upsert_persists_and_audits() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_context()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_upsert_topic_context()
        .withf(|options| {
            options.user_id == 77
                && options.topic_id == "topic-a"
                && options.context == "Use maintenance window rules"
        })
        .returning(|options| {
            Ok(topic_context_record(
                options.topic_id,
                1,
                options.context,
                10,
                10,
            ))
        });
    mock.expect_append_audit_event()
        .withf(|options: &AppendAuditEventOptions| {
            options.user_id == 77
                && options.topic_id.as_deref() == Some("topic-a")
                && options.action == TOOL_TOPIC_CONTEXT_UPSERT
        })
        .returning(|options| {
            Ok(crate::storage::AuditEventRecord {
                schema_version: 1,
                version: 1,
                event_id: "evt-1".to_string(),
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                action: options.action,
                payload: options.payload,
                created_at: 11,
            })
        });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_CONTEXT_UPSERT,
            r#"{"topic_id":"topic-a","context":"Use maintenance window rules"}"#,
            None,
            None,
        )
        .await
        .expect("topic context upsert should succeed");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["topic_context"]["topic_id"], "topic-a");
    assert_eq!(parsed["audit_status"], "written");
}

#[tokio::test]
async fn topic_context_get_reports_missing_record() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_context()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_CONTEXT_GET,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("topic context get should succeed");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["found"], false);
    assert!(parsed["topic_context"].is_null());
}

#[tokio::test]
async fn topic_context_upsert_rejects_agents_style_content() {
    let mock = crate::storage::MockStorageProvider::new();
    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);

    let error = provider
        .execute(
            TOOL_TOPIC_CONTEXT_UPSERT,
            r##"{"topic_id":"topic-a","context":"# AGENTS\nUse release checklist"}"##,
            None,
            None,
        )
        .await
        .expect_err("AGENTS-style topic context must be rejected");

    assert!(error
        .to_string()
        .contains("store AGENTS.md-style documents in topic_agents_md"));
}

#[tokio::test]
async fn topic_context_upsert_surfaces_duplicate_topic_prompt_error() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_context()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_upsert_topic_context().returning(|_| {
        Err(crate::storage::StorageError::DuplicateTopicPromptContent {
            topic_id: "topic-a".to_string(),
            existing_kind: "topic_agents_md".to_string(),
            attempted_kind: "topic_context".to_string(),
        })
    });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let error = provider
        .execute(
            TOOL_TOPIC_CONTEXT_UPSERT,
            r#"{"topic_id":"topic-a","context":"Use release checklist"}"#,
            None,
            None,
        )
        .await
        .expect_err("duplicate topic prompt must be rejected");

    assert!(error
        .to_string()
        .contains("duplicate topic prompt content for topic topic-a"));
}

#[tokio::test]
async fn topic_context_rollback_restores_previous_snapshot() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_context()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, topic_id| {
            Ok(Some(topic_context_record(
                topic_id,
                3,
                "current context",
                10,
                30,
            )))
        });
    mock.expect_list_audit_events_page()
        .with(eq(77_i64), eq(None), eq(ROLLBACK_AUDIT_PAGE_SIZE))
        .returning(|_, _, _| {
            Ok(vec![crate::storage::AuditEventRecord {
                schema_version: 1,
                version: 2,
                event_id: "evt-2".to_string(),
                user_id: 77,
                topic_id: Some("topic-a".to_string()),
                agent_id: None,
                action: TOOL_TOPIC_CONTEXT_UPSERT.to_string(),
                payload: json!({
                    "topic_id": "topic-a",
                    "previous": {
                        "schema_version": 1,
                        "version": 1,
                        "user_id": 77,
                        "topic_id": "topic-a",
                        "context": "previous context",
                        "created_at": 5,
                        "updated_at": 6
                    },
                    "outcome": "applied"
                }),
                created_at: 20,
            }])
        });
    mock.expect_upsert_topic_context()
        .withf(|options| {
            options.user_id == 77
                && options.topic_id == "topic-a"
                && options.context == "previous context"
        })
        .returning(|options| {
            Ok(topic_context_record(
                options.topic_id,
                4,
                options.context,
                5,
                40,
            ))
        });
    mock.expect_append_audit_event()
        .withf(|options: &AppendAuditEventOptions| {
            options.user_id == 77
                && options.action == TOOL_TOPIC_CONTEXT_ROLLBACK
                && options.payload.get("operation") == Some(&json!("restore"))
        })
        .returning(|options| {
            Ok(crate::storage::AuditEventRecord {
                schema_version: 1,
                version: 4,
                event_id: "evt-4".to_string(),
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                action: options.action,
                payload: options.payload,
                created_at: 50,
            })
        });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_CONTEXT_ROLLBACK,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("topic context rollback should succeed");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["operation"], "restore");
    assert_eq!(parsed["topic_context"]["context"], "previous context");
    assert_eq!(parsed["audit_status"], "written");
}

#[tokio::test]
async fn topic_context_rollback_rejects_duplicate_topic_prompt_restore() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_context()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, topic_id| {
            Ok(Some(topic_context_record(
                topic_id,
                3,
                "current context",
                10,
                30,
            )))
        });
    mock.expect_list_audit_events_page()
        .with(eq(77_i64), eq(None), eq(ROLLBACK_AUDIT_PAGE_SIZE))
        .returning(|_, _, _| {
            Ok(vec![crate::storage::AuditEventRecord {
                schema_version: 1,
                version: 2,
                event_id: "evt-2".to_string(),
                user_id: 77,
                topic_id: Some("topic-a".to_string()),
                agent_id: None,
                action: TOOL_TOPIC_CONTEXT_UPSERT.to_string(),
                payload: json!({
                    "topic_id": "topic-a",
                    "previous": {
                        "schema_version": 1,
                        "version": 1,
                        "user_id": 77,
                        "topic_id": "topic-a",
                        "context": "duplicate context",
                        "created_at": 5,
                        "updated_at": 6
                    },
                    "outcome": "applied"
                }),
                created_at: 20,
            }])
        });
    mock.expect_upsert_topic_context().returning(|_| {
        Err(crate::storage::StorageError::DuplicateTopicPromptContent {
            topic_id: "topic-a".to_string(),
            existing_kind: "topic_agents_md".to_string(),
            attempted_kind: "topic_context".to_string(),
        })
    });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let error = provider
        .execute(
            TOOL_TOPIC_CONTEXT_ROLLBACK,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect_err("duplicate rollback restore must be rejected");

    assert!(error
        .to_string()
        .contains("duplicate topic prompt content for topic topic-a"));
}
