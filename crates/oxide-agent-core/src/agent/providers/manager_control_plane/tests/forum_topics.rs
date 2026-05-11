use super::*;

#[tokio::test]
async fn forum_topic_tools_unavailable_without_lifecycle_service() {
    let provider =
        ManagerControlPlaneProvider::new(Arc::new(crate::storage::MockStorageProvider::new()), 77);
    let tool_names: Vec<String> = provider.tools().into_iter().map(|tool| tool.name).collect();

    assert!(!tool_names
        .iter()
        .any(|name| name == TOOL_FORUM_TOPIC_CREATE));
    assert!(!tool_names.iter().any(|name| name == TOOL_FORUM_TOPIC_LIST));
    assert!(!provider.can_handle(TOOL_FORUM_TOPIC_CREATE));
    assert!(!provider.can_handle(TOOL_FORUM_TOPIC_LIST));
}

#[tokio::test]
async fn forum_topic_dry_run_mutations_do_not_call_lifecycle_service() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_append_audit_event()
        .times(3)
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
                created_at: 100,
            })
        });

    let lifecycle = Arc::new(FakeTopicLifecycle::new());
    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77)
        .with_topic_lifecycle(lifecycle.clone());

    provider
        .execute(
            TOOL_FORUM_TOPIC_CREATE,
            r#"{"name":"topic-a","dry_run":true}"#,
            None,
            None,
        )
        .await
        .expect("create dry-run should succeed");
    provider
        .execute(
            TOOL_FORUM_TOPIC_EDIT,
            r#"{"thread_id":42,"name":"topic-b","dry_run":true}"#,
            None,
            None,
        )
        .await
        .expect("edit dry-run should succeed");
    provider
        .execute(
            TOOL_FORUM_TOPIC_DELETE,
            r#"{"thread_id":42,"dry_run":true}"#,
            None,
            None,
        )
        .await
        .expect("delete dry-run should succeed");

    assert!(lifecycle.calls().is_empty());
}

#[tokio::test]
async fn forum_topic_create_invokes_lifecycle_and_audits_success() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_user_config()
        .times(1)
        .returning(|_| Ok(UserConfig::default()));
    mock.expect_update_user_config()
        .withf(|user_id, config| {
            *user_id == 77
                && config.contexts.get("-100999:313").is_some_and(|context| {
                    context.chat_id == Some(-100999)
                        && context.thread_id == Some(313)
                        && context.forum_topic_name.as_deref() == Some("topic-a")
                        && context.forum_topic_icon_color == Some(9_367_192)
                        && !context.forum_topic_closed
                })
        })
        .times(1)
        .returning(|_, _| Ok(()));
    mock.expect_append_audit_event()
        .withf(|options: &AppendAuditEventOptions| {
            options.user_id == 77
                && options.topic_id.as_deref() == Some("-100999:313")
                && options.action == TOOL_FORUM_TOPIC_CREATE
                && options.payload.get("outcome") == Some(&json!("applied"))
                && options
                    .payload
                    .get("result")
                    .and_then(|result| result.get("thread_id"))
                    == Some(&json!(313))
        })
        .returning(|options| {
            Ok(crate::storage::AuditEventRecord {
                schema_version: 1,
                version: 1,
                event_id: "evt-topic-create".to_string(),
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                action: options.action,
                payload: options.payload,
                created_at: 100,
            })
        });

    let lifecycle = Arc::new(FakeTopicLifecycle::new());
    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77)
        .with_topic_lifecycle(lifecycle.clone());

    let response = provider
        .execute(
            TOOL_FORUM_TOPIC_CREATE,
            r#"{"chat_id":-100999,"name":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("forum topic create should succeed");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["topic"]["thread_id"], 313);
    assert_eq!(parsed["audit_status"], "written");
    assert_eq!(lifecycle.calls().len(), 1);
}

#[tokio::test]
async fn forum_topic_list_returns_persisted_topics_for_current_chat() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_user_config().times(1).returning(|_| {
        Ok(user_config_with_contexts([
            (
                "-100777:12".to_string(),
                forum_topic_context(
                    -100777,
                    12,
                    Some("Alfa"),
                    Some(16_766_590),
                    Some("emoji-1"),
                    false,
                ),
            ),
            (
                "-100777:20".to_string(),
                forum_topic_context(-100777, 20, Some("Beta"), Some(7_322_096), None, true),
            ),
            (
                "-100888:7".to_string(),
                forum_topic_context(-100888, 7, Some("Gamma"), None, None, false),
            ),
        ]))
    });

    let lifecycle = Arc::new(FakeTopicLifecycle::new());
    let provider =
        ManagerControlPlaneProvider::new(Arc::new(mock), 77).with_topic_lifecycle(lifecycle);

    let response = provider
        .execute(TOOL_FORUM_TOPIC_LIST, r#"{}"#, None, None)
        .await
        .expect("forum topic list should succeed");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["chat_id"], -100777);
    assert_eq!(parsed["count"], 1);
    assert_eq!(parsed["topics"][0]["topic_id"], "-100777:12");
    assert_eq!(parsed["topics"][0]["name"], "Alfa");
    assert_eq!(parsed["topics"][0]["closed"], false);
}

#[tokio::test]
async fn forum_topic_delete_cleans_topic_storage_and_sandbox() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_clear_agent_memory_for_context()
        .with(eq(77_i64), eq("-100999:42".to_string()))
        .times(1)
        .returning(|_, _| Ok(()));
    mock.expect_clear_chat_history_for_context()
        .with(eq(77_i64), eq("-100999:42".to_string()))
        .times(1)
        .returning(|_, _| Ok(()));
    mock.expect_delete_topic_context()
        .with(eq(77_i64), eq("-100999:42".to_string()))
        .times(1)
        .returning(|_, _| Ok(()));
    mock.expect_delete_topic_agents_md()
        .with(eq(77_i64), eq("-100999:42".to_string()))
        .times(1)
        .returning(|_, _| Ok(()));
    mock.expect_delete_topic_infra_config()
        .with(eq(77_i64), eq("-100999:42".to_string()))
        .times(1)
        .returning(|_, _| Ok(()));
    mock.expect_delete_topic_binding()
        .with(eq(77_i64), eq("-100999:42".to_string()))
        .times(1)
        .returning(|_, _| Ok(()));
    mock.expect_delete_topic_binding()
        .with(eq(77_i64), eq("42".to_string()))
        .times(1)
        .returning(|_, _| Ok(()));
    mock.expect_get_user_config()
        .times(1)
        .returning(|_| Ok(topic_delete_user_config()));
    mock.expect_update_user_config()
        .withf(|user_id, config| *user_id == 77 && !config.contexts.contains_key("-100999:42"))
        .times(1)
        .returning(|_, _| Ok(()));
    mock.expect_append_audit_event()
        .withf(|options: &AppendAuditEventOptions| {
            options.user_id == 77
                && options.topic_id.as_deref() == Some("-100999:42")
                && options.action == TOOL_FORUM_TOPIC_DELETE
                && options
                    .payload
                    .get("cleanup")
                    .and_then(|cleanup| cleanup.get("deleted_container"))
                    == Some(&json!(true))
        })
        .times(1)
        .returning(|options| {
            Ok(audit_event(
                1,
                options.topic_id.as_deref(),
                None,
                &options.action,
                options.payload,
            ))
        });

    let lifecycle = Arc::new(FakeTopicLifecycle::new());
    let sandbox_cleanup = Arc::new(FakeTopicSandboxCleanup::new());
    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77)
        .with_topic_lifecycle(lifecycle.clone())
        .with_topic_sandbox_cleanup(sandbox_cleanup.clone());

    let response = provider
        .execute(
            TOOL_FORUM_TOPIC_DELETE,
            r#"{"chat_id":-100999,"thread_id":42}"#,
            None,
            None,
        )
        .await
        .expect("forum topic delete should clean topic artifacts");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["ok"], true);
    let cleanup = &parsed["cleanup"];
    assert_eq!(parsed["topic"]["thread_id"], 42);
    assert_eq!(cleanup["context_key"], "-100999:42");
    assert_eq!(cleanup["deleted_topic_agents_md"], true);
    assert_eq!(cleanup["deleted_container"], true);
    assert_eq!(parsed["audit_status"], "written");
    assert_eq!(
        lifecycle.calls(),
        vec![LifecycleCall::Delete(ForumTopicThreadRequest {
            chat_id: Some(-100999),
            thread_id: 42,
        })]
    );
    assert_eq!(sandbox_cleanup.calls(), vec![(77, -100999, 42)]);
}

#[tokio::test]
async fn forum_topic_provision_ssh_agent_creates_canonical_binding_and_infra() {
    let lifecycle = Arc::new(FakeTopicLifecycle::new());
    let provider =
        ManagerControlPlaneProvider::new(Arc::new(mock_storage_for_forum_topic_provision()), 77)
            .with_topic_lifecycle(lifecycle.clone());
    let response = provider
        .execute(
            TOOL_FORUM_TOPIC_PROVISION_SSH_AGENT,
            r#"{"name":"n-ru1","host":"213.171.27.211","port":31924,"remote_user":"user1","auth_mode":"none"}"#,
            None,
            None,
        )
        .await
        .expect("atomic ssh topic provisioning should succeed");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["topic"]["topic_id"], "-100777:313");
    assert_eq!(parsed["binding"]["topic_id"], "-100777:313");
    assert_eq!(parsed["topic_infra"]["topic_id"], "-100777:313");
    assert_eq!(
        parsed["profile"]["profile"]["blockedTools"],
        json!(topic_agent_default_blocked_tools())
    );
    let allowed_tools = parsed["profile"]["profile"]["allowedTools"]
        .as_array()
        .expect("allowedTools must be present");
    assert!(allowed_tools
        .iter()
        .any(|value| value.as_str() == Some("ssh_send_file_to_user")));
    assert!(TOPIC_AGENT_REMINDER_TOOLS.iter().all(|tool| {
        allowed_tools
            .iter()
            .any(|value| value.as_str() == Some(*tool))
    }));
    assert_eq!(
        parsed["topic_infra"]["allowed_tool_modes"],
        json!([
            "exec",
            "sudo_exec",
            "read_file",
            "apply_file_edit",
            "check_process",
            "transfer"
        ])
    );
    assert_eq!(parsed["preflight"]["provider_enabled"], true);

    let calls = lifecycle.calls();
    assert!(matches!(calls.first(), Some(LifecycleCall::Create(_))));
}
