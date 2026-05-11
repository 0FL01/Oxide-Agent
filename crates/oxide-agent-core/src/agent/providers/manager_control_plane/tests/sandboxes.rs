use super::*;

#[tokio::test]
async fn topic_sandbox_list_marks_disabled_container_as_orphaned() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_user_config().times(1).returning(|_| {
        Ok(user_config_with_contexts([(
            "-100777:240".to_string(),
            forum_topic_context(-100777, 240, None, None, None, false),
        )]))
    });
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("-100777:240".to_string()))
        .times(1)
        .returning(|_, _| Ok(Some(binding(77, "-100777:240", "agent-a", 1))));
    mock.expect_get_topic_infra_config()
        .with(eq(77_i64), eq("-100777:240".to_string()))
        .times(1)
        .returning(|_, _| Ok(None));
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .times(1)
        .returning(|_, _| {
            Ok(Some(agent_profile_record(
                "agent-a",
                3,
                json!({
                    "blockedTools": TOPIC_AGENT_SANDBOX_TOOLS,
                }),
                10,
                20,
            )))
        });

    let sandbox_control = Arc::new(FakeTopicSandboxControl::new(vec![
        FakeTopicSandboxControl::sandbox_record(77, "-100777:240"),
    ]));
    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77)
        .with_topic_sandbox_control(sandbox_control);
    let response = provider
        .execute(
            TOOL_TOPIC_SANDBOX_LIST,
            r#"{"orphaned_only":true}"#,
            None,
            None,
        )
        .await
        .expect("topic sandbox list should succeed");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["count"], 1);
    assert_eq!(parsed["sandboxes"][0]["orphan_reason"], "sandbox_disabled");
    assert_eq!(parsed["sandboxes"][0]["sandbox_tools_enabled"], false);
}

#[tokio::test]
async fn topic_sandbox_create_ensures_container_for_tracked_topic() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_user_config().times(2).returning(|_| {
        Ok(user_config_with_contexts([(
            "-100777:240".to_string(),
            forum_topic_context(-100777, 240, None, None, None, false),
        )]))
    });
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("-100777:240".to_string()))
        .times(1)
        .returning(|_, _| Ok(None));
    mock.expect_append_audit_event()
        .times(1)
        .returning(|options| {
            Ok(audit_event(
                1,
                options.topic_id.as_deref(),
                options.agent_id.as_deref(),
                &options.action,
                options.payload,
            ))
        });

    let sandbox_control = Arc::new(FakeTopicSandboxControl::new(Vec::new()));
    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77)
        .with_topic_sandbox_control(sandbox_control.clone());
    let response = provider
        .execute(
            TOOL_TOPIC_SANDBOX_CREATE,
            r#"{"topic_id":"-100777:240"}"#,
            None,
            None,
        )
        .await
        .expect("topic sandbox create should succeed");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["sandbox"]["topic_id"], "-100777:240");
    assert_eq!(sandbox_control.ensured(), vec!["-100777:240".to_string()]);
}

#[tokio::test]
async fn topic_sandbox_delete_supports_container_name_lookup() {
    let sandbox_record = FakeTopicSandboxControl::sandbox_record(77, "-100777:240");
    let container_name = sandbox_record.container_name.clone();

    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_user_config().times(1).returning(|_| {
        Ok(user_config_with_contexts([(
            "-100777:240".to_string(),
            forum_topic_context(-100777, 240, None, None, None, false),
        )]))
    });
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("-100777:240".to_string()))
        .times(1)
        .returning(|_, _| Ok(None));
    mock.expect_append_audit_event()
        .times(1)
        .returning(|options| {
            Ok(audit_event(
                1,
                options.topic_id.as_deref(),
                options.agent_id.as_deref(),
                &options.action,
                options.payload,
            ))
        });

    let sandbox_control = Arc::new(FakeTopicSandboxControl::new(vec![sandbox_record]));
    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77)
        .with_topic_sandbox_control(sandbox_control.clone());
    let response = provider
        .execute(
            TOOL_TOPIC_SANDBOX_DELETE,
            &format!(r#"{{"container_name":"{container_name}"}}"#),
            None,
            None,
        )
        .await
        .expect("topic sandbox delete should succeed");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["deleted"], true);
    assert_eq!(sandbox_control.deleted(), vec![container_name]);
}

#[tokio::test]
async fn topic_sandbox_recreate_calls_control_plane() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_user_config().times(1).returning(|_| {
        Ok(user_config_with_contexts([(
            "-100777:240".to_string(),
            forum_topic_context(-100777, 240, None, None, None, false),
        )]))
    });
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("-100777:240".to_string()))
        .times(1)
        .returning(|_, _| Ok(None));
    mock.expect_append_audit_event()
        .times(1)
        .returning(|options| {
            Ok(audit_event(
                1,
                options.topic_id.as_deref(),
                options.agent_id.as_deref(),
                &options.action,
                options.payload,
            ))
        });

    let sandbox_control = Arc::new(FakeTopicSandboxControl::new(Vec::new()));
    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77)
        .with_topic_sandbox_control(sandbox_control.clone());
    let response = provider
        .execute(
            TOOL_TOPIC_SANDBOX_RECREATE,
            r#"{"topic_id":"-100777:240"}"#,
            None,
            None,
        )
        .await
        .expect("topic sandbox recreate should succeed");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["sandbox"]["topic_id"], "-100777:240");
    assert_eq!(sandbox_control.recreated(), vec!["-100777:240".to_string()]);
}

#[tokio::test]
async fn topic_sandbox_prune_dry_run_reports_binding_missing_candidates() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_user_config().times(1).returning(|_| {
        Ok(user_config_with_contexts([(
            "-100777:240".to_string(),
            forum_topic_context(-100777, 240, None, None, None, false),
        )]))
    });
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("-100777:240".to_string()))
        .times(1)
        .returning(|_, _| Ok(None));
    mock.expect_append_audit_event()
        .times(1)
        .returning(|options| {
            Ok(audit_event(
                1,
                options.topic_id.as_deref(),
                options.agent_id.as_deref(),
                &options.action,
                options.payload,
            ))
        });

    let sandbox_control = Arc::new(FakeTopicSandboxControl::new(vec![
        FakeTopicSandboxControl::sandbox_record(77, "-100777:240"),
    ]));
    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77)
        .with_topic_sandbox_control(sandbox_control.clone());
    let response = provider
        .execute(
            TOOL_TOPIC_SANDBOX_PRUNE,
            r#"{"reason":"binding_missing","dry_run":true}"#,
            None,
            None,
        )
        .await
        .expect("topic sandbox prune dry-run should succeed");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["count"], 1);
    assert_eq!(parsed["candidates"][0]["orphan_reason"], "binding_missing");
    assert!(sandbox_control.deleted().is_empty());
}
