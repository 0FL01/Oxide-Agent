use super::*;

#[tokio::test]
async fn topic_agents_md_upsert_persists_and_audits() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_agents_md()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_upsert_topic_agents_md()
        .withf(|options| {
            options.user_id == 77
                && options.topic_id == "topic-a"
                && options.agents_md == "# Topic AGENTS\nFollow release process"
        })
        .returning(|options| {
            Ok(topic_agents_md_record(
                options.topic_id,
                1,
                options.agents_md,
                10,
                10,
            ))
        });
    mock.expect_append_audit_event()
        .withf(|options: &AppendAuditEventOptions| {
            options.user_id == 77
                && options.topic_id.as_deref() == Some("topic-a")
                && options.action == TOOL_TOPIC_AGENTS_MD_UPSERT
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
            TOOL_TOPIC_AGENTS_MD_UPSERT,
            r##"{"topic_id":"topic-a","agents_md":"# Topic AGENTS\nFollow release process"}"##,
            None,
            None,
        )
        .await
        .expect("topic AGENTS.md upsert should succeed");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["topic_agents_md"]["topic_id"], "topic-a");
    assert_eq!(parsed["audit_status"], "written");
}

#[tokio::test]
async fn topic_agents_md_get_reports_missing_record() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_agents_md()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_AGENTS_MD_GET,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("topic AGENTS.md get should succeed");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["found"], false);
    assert!(parsed["topic_agents_md"].is_null());
}

#[tokio::test]
async fn topic_agents_md_rollback_restores_previous_snapshot() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_agents_md()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, topic_id| {
            Ok(Some(topic_agents_md_record(
                topic_id,
                3,
                "# Current AGENTS",
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
                action: TOOL_TOPIC_AGENTS_MD_UPSERT.to_string(),
                payload: json!({
                    "topic_id": "topic-a",
                    "previous": {
                        "schema_version": 1,
                        "version": 1,
                        "user_id": 77,
                        "topic_id": "topic-a",
                        "agents_md": "# Previous AGENTS",
                        "created_at": 5,
                        "updated_at": 6
                    },
                    "outcome": "applied"
                }),
                created_at: 20,
            }])
        });
    mock.expect_upsert_topic_agents_md()
        .withf(|options| {
            options.user_id == 77
                && options.topic_id == "topic-a"
                && options.agents_md == "# Previous AGENTS"
        })
        .returning(|options| {
            Ok(topic_agents_md_record(
                options.topic_id,
                4,
                options.agents_md,
                5,
                40,
            ))
        });
    mock.expect_append_audit_event()
        .withf(|options: &AppendAuditEventOptions| {
            options.user_id == 77
                && options.action == TOOL_TOPIC_AGENTS_MD_ROLLBACK
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
            TOOL_TOPIC_AGENTS_MD_ROLLBACK,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("topic AGENTS.md rollback should succeed");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["operation"], "restore");
    assert_eq!(parsed["topic_agents_md"]["agents_md"], "# Previous AGENTS");
    assert_eq!(parsed["audit_status"], "written");
}

#[tokio::test]
async fn topic_agents_md_upsert_rejects_more_than_300_lines() {
    let mock = crate::storage::MockStorageProvider::new();
    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let oversized = vec!["line"; crate::storage::TOPIC_AGENTS_MD_MAX_LINES + 1].join("\n");
    let arguments = serde_json::json!({
        "topic_id": "topic-a",
        "agents_md": oversized,
    })
    .to_string();

    let error = provider
        .execute(TOOL_TOPIC_AGENTS_MD_UPSERT, &arguments, None, None)
        .await
        .expect_err("oversized AGENTS.md must be rejected");

    assert!(error
        .to_string()
        .contains("agents_md must not exceed 300 lines"));
}
