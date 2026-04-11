use super::*;

#[tokio::test]
async fn agent_profile_upsert_rejects_non_object_profile() {
    let storage = Arc::new(crate::storage::MockStorageProvider::new());
    let provider = ManagerControlPlaneProvider::new(storage, 77);
    let err = provider
        .execute(
            TOOL_AGENT_PROFILE_UPSERT,
            r#"{"agent_id":"agent-a","profile":[1,2,3]}"#,
            None,
            None,
        )
        .await
        .expect_err("expected profile validation error");

    assert!(err.to_string().contains("profile must be a JSON object"));
}

#[tokio::test]
async fn agent_profile_upsert_rejects_legacy_tools_shorthand() {
    let storage = Arc::new(crate::storage::MockStorageProvider::new());
    let provider = ManagerControlPlaneProvider::new(storage, 77);
    let err = provider
        .execute(
            TOOL_AGENT_PROFILE_UPSERT,
            r#"{"agent_id":"agent-a","profile":{"tools":["ssh"]}}"#,
            None,
            None,
        )
        .await
        .expect_err("expected unsupported profile.tools validation error");

    assert!(err.to_string().contains("allowedTools/blockedTools"));
}

#[tokio::test]
async fn agent_profile_rollback_deletes_when_previous_snapshot_absent() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .returning(|_, agent_id| {
            Ok(Some(agent_profile_record(
                agent_id,
                2,
                json!({"mode":"current"}),
                10,
                20,
            )))
        });
    mock.expect_list_audit_events_page()
        .with(eq(77_i64), eq(None), eq(ROLLBACK_AUDIT_PAGE_SIZE))
        .returning(|_, _, _| {
            Ok(vec![crate::storage::AuditEventRecord {
                schema_version: 1,
                version: 3,
                event_id: "evt-3".to_string(),
                user_id: 77,
                topic_id: None,
                agent_id: Some("agent-a".to_string()),
                action: TOOL_AGENT_PROFILE_DELETE.to_string(),
                payload: json!({"agent_id":"agent-a","previous":null,"outcome":"applied"}),
                created_at: 30,
            }])
        });
    mock.expect_delete_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .returning(|_, _| Ok(()));
    mock.expect_append_audit_event()
        .withf(|options: &AppendAuditEventOptions| {
            options.user_id == 77
                && options.action == TOOL_AGENT_PROFILE_ROLLBACK
                && options.payload.get("operation") == Some(&json!("delete"))
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
                created_at: 40,
            })
        });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_AGENT_PROFILE_ROLLBACK,
            r#"{"agent_id":"agent-a"}"#,
            None,
            None,
        )
        .await
        .expect("agent rollback should succeed");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["operation"], "delete");
    assert!(parsed["profile"].is_null());
    assert_eq!(parsed["audit_status"], "written");
}
