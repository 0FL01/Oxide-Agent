use super::*;

#[tokio::test]
async fn topic_infra_upsert_resolves_unique_forum_topic_name_alias() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_user_config().returning(|_| {
        Ok(user_config_with_contexts([(
            "-100777:240".to_string(),
            forum_topic_context(-100777, 240, Some("n-ru1"), Some(9_367_192), None, false),
        )]))
    });
    mock.expect_get_topic_infra_config()
        .with(eq(77_i64), eq("-100777:240".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_upsert_topic_infra_config()
        .withf(|options| options.topic_id == "-100777:240")
        .returning(|options| {
            Ok(TopicInfraConfigRecord {
                schema_version: 1,
                version: 1,
                user_id: options.user_id,
                topic_id: options.topic_id,
                target_name: options.target_name,
                host: options.host,
                port: options.port,
                remote_user: options.remote_user,
                auth_mode: options.auth_mode,
                secret_ref: options.secret_ref,
                sudo_secret_ref: options.sudo_secret_ref,
                environment: options.environment,
                tags: options.tags,
                allowed_tool_modes: options.allowed_tool_modes,
                approval_required_modes: options.approval_required_modes,
                created_at: 10,
                updated_at: 10,
            })
        });
    mock.expect_append_audit_event().returning(|options| {
        Ok(audit_event(
            1,
            options.topic_id.as_deref(),
            options.agent_id.as_deref(),
            &options.action,
            options.payload,
        ))
    });

    let lifecycle = Arc::new(FakeTopicLifecycle::new());
    let provider =
        ManagerControlPlaneProvider::new(Arc::new(mock), 77).with_topic_lifecycle(lifecycle);
    let response = provider
        .execute(
            TOOL_TOPIC_INFRA_UPSERT,
            r#"{"topic_id":"n-ru1","target_name":"n-ru1","host":"213.171.27.211","port":31924,"remote_user":"user1","auth_mode":"none","allowed_tool_modes":["exec"]}"#,
            None,
            None,
        )
        .await
        .expect("alias resolution should canonicalize forum topic name");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["topic_infra"]["topic_id"], "-100777:240");
}

#[tokio::test]
async fn topic_infra_upsert_persists_and_audits() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_infra_config()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_get_secret_value().returning(|_, _| Ok(None));
    mock.expect_upsert_topic_infra_config()
        .withf(|options| {
            options.user_id == 77
                && options.topic_id == "topic-a"
                && options.target_name == "prod-app"
                && options.host == "prod.example.com"
                && options.remote_user == "deploy"
        })
        .returning(|options| {
            Ok(TopicInfraConfigRecord {
                schema_version: 1,
                version: 1,
                user_id: options.user_id,
                topic_id: options.topic_id,
                target_name: options.target_name,
                host: options.host,
                port: options.port,
                remote_user: options.remote_user,
                auth_mode: options.auth_mode,
                secret_ref: options.secret_ref,
                sudo_secret_ref: options.sudo_secret_ref,
                environment: options.environment,
                tags: options.tags,
                allowed_tool_modes: options.allowed_tool_modes,
                approval_required_modes: options.approval_required_modes,
                created_at: 10,
                updated_at: 10,
            })
        });
    mock.expect_append_audit_event()
        .withf(|options: &AppendAuditEventOptions| {
            options.user_id == 77
                && options.topic_id.as_deref() == Some("topic-a")
                && options.action == TOOL_TOPIC_INFRA_UPSERT
        })
        .returning(|options| {
            Ok(audit_event(
                1,
                options.topic_id.as_deref(),
                None,
                &options.action,
                options.payload,
            ))
        });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_INFRA_UPSERT,
            r#"{"topic_id":"topic-a","target_name":"prod-app","host":"prod.example.com","remote_user":"deploy","auth_mode":"private_key","secret_ref":"storage:ssh/prod-key","sudo_secret_ref":"storage:ssh/prod-sudo","environment":"prod","tags":["prod"],"allowed_tool_modes":["exec","read_file"],"approval_required_modes":["sudo_exec"]}"#,
            None,
            None,
        )
        .await
        .expect("topic infra upsert should succeed");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["topic_infra"]["topic_id"], "topic-a");
    assert_eq!(parsed["topic_infra"]["target_name"], "prod-app");
    assert_eq!(parsed["preflight"]["provider_enabled"], false);
    assert_eq!(parsed["audit_status"], "written");
}

#[tokio::test]
async fn topic_infra_rollback_restores_previous_snapshot() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_infra_config()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, topic_id| Ok(Some(topic_infra(77, &topic_id, 3))));
    mock.expect_get_secret_value().returning(|_, _| Ok(None));
    mock.expect_list_audit_events_page()
        .with(eq(77_i64), eq(None), eq(ROLLBACK_AUDIT_PAGE_SIZE))
        .returning(|_, _, _| {
            Ok(vec![audit_event(
                2,
                Some("topic-a"),
                None,
                TOOL_TOPIC_INFRA_UPSERT,
                json!({
                    "topic_id": "topic-a",
                    "previous": topic_infra(77, "topic-a", 1),
                    "outcome": "applied"
                }),
            )])
        });
    mock.expect_upsert_topic_infra_config()
        .withf(|options| {
            options.user_id == 77
                && options.topic_id == "topic-a"
                && options.target_name == "prod-app"
                && options.host == "prod.example.com"
        })
        .returning(|options| {
            Ok(TopicInfraConfigRecord {
                schema_version: 1,
                version: 4,
                user_id: options.user_id,
                topic_id: options.topic_id,
                target_name: options.target_name,
                host: options.host,
                port: options.port,
                remote_user: options.remote_user,
                auth_mode: options.auth_mode,
                secret_ref: options.secret_ref,
                sudo_secret_ref: options.sudo_secret_ref,
                environment: options.environment,
                tags: options.tags,
                allowed_tool_modes: options.allowed_tool_modes,
                approval_required_modes: options.approval_required_modes,
                created_at: 10,
                updated_at: 40,
            })
        });
    mock.expect_append_audit_event()
        .withf(|options: &AppendAuditEventOptions| {
            options.user_id == 77
                && options.action == TOOL_TOPIC_INFRA_ROLLBACK
                && options.payload.get("operation") == Some(&json!("restore"))
        })
        .returning(|options| {
            Ok(audit_event(
                4,
                options.topic_id.as_deref(),
                None,
                &options.action,
                options.payload,
            ))
        });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_INFRA_ROLLBACK,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("topic infra rollback should succeed");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["operation"], "restore");
    assert_eq!(parsed["topic_infra"]["host"], "prod.example.com");
    assert_eq!(parsed["preflight"]["provider_enabled"], false);
    assert_eq!(parsed["audit_status"], "written");
}

#[tokio::test]
async fn private_secret_probe_reports_presence_without_exposing_content() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_secret_value()
        .with(eq(77_i64), eq("vds".to_string()))
        .returning(|_, _| {
            Ok(Some(
                "-----BEGIN OPENSSH PRIVATE KEY-----\ninvalid\n-----END OPENSSH PRIVATE KEY-----\n"
                    .to_string(),
            ))
        });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_PRIVATE_SECRET_PROBE,
            r#"{"secret_ref":"storage:vds","kind":"ssh_private_key"}"#,
            None,
            None,
        )
        .await
        .expect("private secret probe should succeed");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["secret_probe"]["secret_ref"], "storage:vds");
    assert_eq!(parsed["secret_probe"]["present"], true);
    assert!(parsed["secret_probe"].get("content").is_none());
}
