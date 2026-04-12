use super::*;

#[tokio::test]
async fn topic_binding_set_rejects_empty_topic_id() {
    let storage = Arc::new(crate::storage::MockStorageProvider::new());
    let provider = ManagerControlPlaneProvider::new(storage, 77);
    let err = provider
        .execute(
            TOOL_TOPIC_BINDING_SET,
            r#"{"topic_id":"   ","agent_id":"agent-1"}"#,
            None,
            None,
        )
        .await
        .expect_err("expected validation error");

    assert!(err.to_string().contains("topic_id must not be empty"));
}

#[tokio::test]
async fn topic_binding_get_rejects_unknown_fields() {
    let storage = Arc::new(crate::storage::MockStorageProvider::new());
    let provider = ManagerControlPlaneProvider::new(storage, 77);
    let err = provider
        .execute(
            TOOL_TOPIC_BINDING_GET,
            r#"{"topic_id":"topic-a","extra":true}"#,
            None,
            None,
        )
        .await
        .expect_err("expected strict serde validation error");

    assert!(err.to_string().contains("unknown field"));
}

#[tokio::test]
async fn topic_binding_set_persists_and_audits() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));

    mock.expect_upsert_topic_binding()
        .withf(|options| {
            options.user_id == 77
                && options.topic_id == "topic-a"
                && options.agent_id == "agent-a"
                && options.binding_kind.is_none()
                && options.chat_id == OptionalMetadataPatch::Keep
                && options.thread_id == OptionalMetadataPatch::Keep
                && options.expires_at == OptionalMetadataPatch::Keep
                && options.last_activity_at.is_none()
        })
        .returning(|options| {
            Ok(TopicBindingRecord {
                schema_version: 1,
                version: 2,
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                binding_kind: options.binding_kind.unwrap_or(TopicBindingKind::Manual),
                chat_id: options.chat_id.for_new_record(),
                thread_id: options.thread_id.for_new_record(),
                expires_at: options.expires_at.for_new_record(),
                last_activity_at: options.last_activity_at,
                created_at: 100,
                updated_at: 200,
            })
        });

    mock.expect_append_audit_event()
        .withf(|options: &AppendAuditEventOptions| {
            options.user_id == 77
                && options.action == TOOL_TOPIC_BINDING_SET
                && options.topic_id.as_deref() == Some("topic-a")
                && options.agent_id.as_deref() == Some("agent-a")
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
                created_at: 300,
            })
        });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_BINDING_SET,
            r#"{"topic_id":"topic-a","agent_id":"agent-a"}"#,
            None,
            None,
        )
        .await
        .expect("topic binding set should succeed");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed.get("ok"), Some(&serde_json::Value::Bool(true)));
    assert_eq!(parsed["binding"]["topic_id"], "topic-a");
    assert_eq!(parsed["audit_status"], "written");
}

#[tokio::test]
async fn topic_binding_set_supports_explicit_null_to_clear_metadata() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| {
            Ok(Some(TopicBindingRecord {
                schema_version: 1,
                version: 1,
                user_id: 77,
                topic_id: "topic-a".to_string(),
                agent_id: "agent-a".to_string(),
                binding_kind: TopicBindingKind::Runtime,
                chat_id: Some(100),
                thread_id: Some(7),
                expires_at: Some(10_000),
                last_activity_at: Some(20),
                created_at: 10,
                updated_at: 20,
            }))
        });

    mock.expect_upsert_topic_binding()
        .withf(|options| {
            options.user_id == 77
                && options.topic_id == "topic-a"
                && options.agent_id == "agent-a"
                && options.chat_id == OptionalMetadataPatch::Clear
                && options.thread_id == OptionalMetadataPatch::Clear
                && options.expires_at == OptionalMetadataPatch::Clear
        })
        .returning(|options| {
            Ok(TopicBindingRecord {
                schema_version: 1,
                version: 2,
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                binding_kind: options.binding_kind.unwrap_or(TopicBindingKind::Manual),
                chat_id: options.chat_id.for_new_record(),
                thread_id: options.thread_id.for_new_record(),
                expires_at: options.expires_at.for_new_record(),
                last_activity_at: options.last_activity_at,
                created_at: 10,
                updated_at: 30,
            })
        });

    mock.expect_append_audit_event().returning(|options| {
        Ok(crate::storage::AuditEventRecord {
            schema_version: 1,
            version: 1,
            event_id: "evt-1".to_string(),
            user_id: options.user_id,
            topic_id: options.topic_id,
            agent_id: options.agent_id,
            action: options.action,
            payload: options.payload,
            created_at: 300,
        })
    });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_BINDING_SET,
            r#"{"topic_id":"topic-a","agent_id":"agent-a","chat_id":null,"thread_id":null,"expires_at":null}"#,
            None,
            None,
        )
        .await
        .expect("topic binding set should support null clears");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed.get("ok"), Some(&serde_json::Value::Bool(true)));
    assert_eq!(parsed["binding"]["chat_id"], serde_json::Value::Null);
    assert_eq!(parsed["binding"]["thread_id"], serde_json::Value::Null);
    assert_eq!(parsed["binding"]["expires_at"], serde_json::Value::Null);
}

#[tokio::test]
async fn topic_binding_set_succeeds_when_audit_write_fails() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));

    mock.expect_upsert_topic_binding()
        .withf(|options| {
            options.user_id == 77 && options.topic_id == "topic-a" && options.agent_id == "agent-a"
        })
        .returning(|options| {
            Ok(TopicBindingRecord {
                schema_version: 1,
                version: 1,
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                binding_kind: options.binding_kind.unwrap_or(TopicBindingKind::Manual),
                chat_id: options.chat_id.for_new_record(),
                thread_id: options.thread_id.for_new_record(),
                expires_at: options.expires_at.for_new_record(),
                last_activity_at: options.last_activity_at,
                created_at: 100,
                updated_at: 100,
            })
        });

    mock.expect_append_audit_event().returning(|_| {
        Err(crate::storage::StorageError::Config(
            "audit unavailable".to_string(),
        ))
    });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_BINDING_SET,
            r#"{"topic_id":"topic-a","agent_id":"agent-a"}"#,
            None,
            None,
        )
        .await
        .expect("mutation should succeed even when audit write fails");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["binding"]["topic_id"], "topic-a");
    assert_eq!(parsed["audit_status"], "write_failed");
    assert!(parsed["audit_error"].as_str().is_some());
}

#[tokio::test]
async fn topic_binding_set_dry_run_does_not_persist() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_append_audit_event()
        .withf(|options: &AppendAuditEventOptions| {
            options.user_id == 77
                && options.action == TOOL_TOPIC_BINDING_SET
                && options.payload.get("outcome") == Some(&json!("dry_run"))
        })
        .returning(|options| {
            Ok(crate::storage::AuditEventRecord {
                schema_version: 1,
                version: 1,
                event_id: "evt-dry-run".to_string(),
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                action: options.action,
                payload: options.payload,
                created_at: 300,
            })
        });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_BINDING_SET,
            r#"{"topic_id":"topic-a","agent_id":"agent-a","dry_run":true}"#,
            None,
            None,
        )
        .await
        .expect("dry-run set should succeed");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["dry_run"], true);
    assert_eq!(parsed["preview"]["operation"], "upsert");
    assert_eq!(parsed["audit_status"], "written");
}

#[tokio::test]
async fn topic_binding_set_dry_run_reports_audit_write_failure() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_upsert_topic_binding().times(0);
    mock.expect_append_audit_event().returning(|_| {
        Err(crate::storage::StorageError::Config(
            "audit unavailable".to_string(),
        ))
    });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_BINDING_SET,
            r#"{"topic_id":"topic-a","agent_id":"agent-a","dry_run":true}"#,
            None,
            None,
        )
        .await
        .expect("dry-run should succeed even when audit write fails");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["dry_run"], true);
    assert_eq!(parsed["audit_status"], "write_failed");
}

#[tokio::test]
async fn topic_binding_rollback_restores_previous_snapshot() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, topic_id| Ok(Some(binding(77, &topic_id, "agent-new", 4))));
    mock.expect_list_audit_events_page()
        .with(eq(77_i64), eq(None), eq(ROLLBACK_AUDIT_PAGE_SIZE))
        .returning(|_, _, _| {
            Ok(vec![crate::storage::AuditEventRecord {
                schema_version: 1,
                version: 9,
                event_id: "evt-9".to_string(),
                user_id: 77,
                topic_id: Some("topic-a".to_string()),
                agent_id: Some("agent-new".to_string()),
                action: TOOL_TOPIC_BINDING_SET.to_string(),
                payload: json!({
                    "topic_id": "topic-a",
                    "agent_id": "agent-new",
                    "previous": {
                        "schema_version": 1,
                        "version": 3,
                        "user_id": 77,
                        "topic_id": "topic-a",
                        "agent_id": "agent-old",
                        "created_at": 1,
                        "updated_at": 2
                    },
                    "outcome": "applied"
                }),
                created_at: 100,
            }])
        });
    mock.expect_upsert_topic_binding()
        .withf(|options| {
            options.user_id == 77
                && options.topic_id == "topic-a"
                && options.agent_id == "agent-old"
        })
        .returning(|options| {
            Ok(binding(
                options.user_id,
                &options.topic_id,
                &options.agent_id,
                5,
            ))
        });
    mock.expect_append_audit_event()
        .withf(|options: &AppendAuditEventOptions| {
            options.user_id == 77
                && options.action == TOOL_TOPIC_BINDING_ROLLBACK
                && options.payload.get("operation") == Some(&json!("restore"))
        })
        .returning(|options| {
            Ok(crate::storage::AuditEventRecord {
                schema_version: 1,
                version: 10,
                event_id: "evt-10".to_string(),
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                action: options.action,
                payload: options.payload,
                created_at: 110,
            })
        });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_BINDING_ROLLBACK,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("topic rollback should succeed");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["operation"], "restore");
    assert_eq!(parsed["binding"]["agent_id"], "agent-old");
    assert_eq!(parsed["audit_status"], "written");
}

#[tokio::test]
async fn topic_binding_rollback_succeeds_when_audit_write_fails() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, topic_id| Ok(Some(binding(77, &topic_id, "agent-new", 4))));
    mock.expect_list_audit_events_page()
        .with(eq(77_i64), eq(None), eq(ROLLBACK_AUDIT_PAGE_SIZE))
        .returning(|_, _, _| {
            Ok(vec![crate::storage::AuditEventRecord {
                schema_version: 1,
                version: 9,
                event_id: "evt-9".to_string(),
                user_id: 77,
                topic_id: Some("topic-a".to_string()),
                agent_id: Some("agent-new".to_string()),
                action: TOOL_TOPIC_BINDING_SET.to_string(),
                payload: json!({
                    "topic_id": "topic-a",
                    "previous": {
                        "schema_version": 1,
                        "version": 3,
                        "user_id": 77,
                        "topic_id": "topic-a",
                        "agent_id": "agent-old",
                        "created_at": 1,
                        "updated_at": 2
                    },
                    "outcome": "applied"
                }),
                created_at: 100,
            }])
        });
    mock.expect_upsert_topic_binding().returning(|options| {
        Ok(TopicBindingRecord {
            schema_version: 1,
            version: 5,
            user_id: options.user_id,
            topic_id: options.topic_id,
            agent_id: options.agent_id,
            binding_kind: options.binding_kind.unwrap_or(TopicBindingKind::Manual),
            chat_id: options.chat_id.for_new_record(),
            thread_id: options.thread_id.for_new_record(),
            expires_at: options.expires_at.for_new_record(),
            last_activity_at: options.last_activity_at,
            created_at: 10,
            updated_at: 30,
        })
    });
    mock.expect_append_audit_event().returning(|_| {
        Err(crate::storage::StorageError::Config(
            "audit unavailable".to_string(),
        ))
    });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_BINDING_ROLLBACK,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("rollback should succeed even when audit write fails");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["operation"], "restore");
    assert_eq!(parsed["audit_status"], "write_failed");
}

#[tokio::test]
async fn topic_binding_rollback_scans_multiple_audit_pages() {
    let mut mock = crate::storage::MockStorageProvider::new();
    let mut sequence = Sequence::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, topic_id| Ok(Some(binding(77, &topic_id, "agent-new", 8))));
    mock.expect_list_audit_events_page()
        .with(eq(77_i64), eq(None), eq(ROLLBACK_AUDIT_PAGE_SIZE))
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _, _| {
            Ok(vec![audit_event(
                500,
                Some("other-topic"),
                Some("agent-z"),
                TOOL_TOPIC_BINDING_SET,
                json!({"outcome":"applied"}),
            )])
        });
    mock.expect_list_audit_events_page()
        .with(eq(77_i64), eq(Some(500_u64)), eq(ROLLBACK_AUDIT_PAGE_SIZE))
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _, _| {
            Ok(vec![audit_event(
                499,
                Some("topic-a"),
                Some("agent-new"),
                TOOL_TOPIC_BINDING_SET,
                json!({
                    "topic_id": "topic-a",
                    "previous": {
                        "schema_version": 1,
                        "version": 7,
                        "user_id": 77,
                        "topic_id": "topic-a",
                        "agent_id": "agent-old",
                        "created_at": 1,
                        "updated_at": 2
                    },
                    "outcome": "applied"
                }),
            )])
        });
    mock.expect_upsert_topic_binding()
        .withf(|options| {
            options.user_id == 77
                && options.topic_id == "topic-a"
                && options.agent_id == "agent-old"
        })
        .returning(|options| {
            Ok(binding(
                options.user_id,
                &options.topic_id,
                &options.agent_id,
                9,
            ))
        });
    mock.expect_append_audit_event().returning(|options| {
        Ok(crate::storage::AuditEventRecord {
            user_id: options.user_id,
            topic_id: options.topic_id,
            agent_id: options.agent_id,
            action: options.action,
            payload: options.payload,
            ..audit_event(501, None, None, TOOL_TOPIC_BINDING_ROLLBACK, json!({}))
        })
    });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_BINDING_ROLLBACK,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("rollback should search across audit pages");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["operation"], "restore");
    assert_eq!(parsed["binding"]["agent_id"], "agent-old");
    assert_eq!(parsed["audit_status"], "written");
}
