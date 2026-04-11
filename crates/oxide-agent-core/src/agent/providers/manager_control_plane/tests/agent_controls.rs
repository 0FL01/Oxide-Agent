use super::*;

#[tokio::test]
async fn topic_agent_tools_get_hides_blocked_tools_from_effective_snapshot() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(Some(binding(77, "topic-a", "agent-a", 1))));
    mock.expect_get_topic_infra_config()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .returning(|_, _| {
            Ok(Some(agent_profile_record(
                "agent-a",
                1,
                json!({
                    "blockedTools": TOPIC_AGENT_YTDLP_TOOLS,
                }),
                10,
                10,
            )))
        });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_AGENT_TOOLS_GET,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("topic agent tools get should succeed");

    let parsed = parse_json_response(&response);
    let active_tools = parsed["tools"]["active_tools"]
        .as_array()
        .expect("active_tools must be an array");
    assert!(!active_tools
        .iter()
        .any(|tool| { TOPIC_AGENT_YTDLP_TOOLS.contains(&tool.as_str().unwrap_or_default()) }));

    let ytdlp_status = provider_status(&parsed, "ytdlp");
    assert_eq!(ytdlp_status["enabled"], false);

    let reminder_status = provider_status(&parsed, "reminder");
    assert_eq!(reminder_status["enabled"], true);
    assert!(reminder_status["available_tools"]
        .as_array()
        .is_some_and(|tools| tools.iter().any(|tool| tool == "reminder_schedule")));
}

#[tokio::test]
async fn topic_agent_tools_get_keeps_reminders_enabled_for_allowlisted_profiles() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(Some(binding(77, "topic-a", "agent-a", 1))));
    mock.expect_get_topic_infra_config()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .returning(|_, _| {
            Ok(Some(agent_profile_record(
                "agent-a",
                1,
                json!({
                    "allowedTools": ["execute_command"],
                }),
                10,
                10,
            )))
        });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_AGENT_TOOLS_GET,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("topic agent tools get should succeed");

    let parsed = parse_json_response(&response);
    let active_tools = parsed["tools"]["active_tools"]
        .as_array()
        .expect("active_tools must be an array");
    assert!(active_tools
        .iter()
        .any(|tool| tool.as_str() == Some("reminder_schedule")));
    let reminder_status = provider_status(&parsed, "reminder");
    assert_eq!(reminder_status["enabled"], true);
    assert!(reminder_status["active_tools"]
        .as_array()
        .is_some_and(|tools| tools.iter().any(|tool| tool == "reminder_schedule")));
}

#[tokio::test]
async fn topic_agent_tools_disable_expands_provider_alias_and_persists_profile() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_user_config().returning(|_| {
        Ok(user_config_with_contexts([(
            "-100777:240".to_string(),
            forum_topic_context(-100777, 240, Some("n-ru1"), Some(9_367_192), None, false),
        )]))
    });
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("-100777:240".to_string()))
        .returning(|_, _| Ok(Some(binding(77, "-100777:240", "agent-a", 1))));
    mock.expect_get_topic_infra_config()
        .with(eq(77_i64), eq("-100777:240".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .returning(|_, _| {
            Ok(Some(agent_profile_record(
                "agent-a",
                3,
                json!({
                    "systemPrompt": "infra agent",
                }),
                10,
                20,
            )))
        });
    mock.expect_upsert_agent_profile()
        .withf(|options| {
            options.agent_id == "agent-a"
                && options.profile["systemPrompt"] == "infra agent"
                && options
                    .profile
                    .get("blockedTools")
                    .and_then(|value| value.as_array())
                    .is_some_and(|tools| {
                        TOPIC_AGENT_YTDLP_TOOLS
                            .iter()
                            .all(|tool| tools.iter().any(|value| value.as_str() == Some(*tool)))
                    })
        })
        .returning(|options| {
            Ok(agent_profile_record(
                options.agent_id,
                4,
                options.profile,
                10,
                30,
            ))
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

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77)
        .with_topic_lifecycle(Arc::new(FakeTopicLifecycle::new()));
    let response = provider
        .execute(
            TOOL_TOPIC_AGENT_TOOLS_DISABLE,
            r#"{"topic_id":"n-ru1","tools":["ytdlp"]}"#,
            None,
            None,
        )
        .await
        .expect("topic agent tools disable should succeed");

    let parsed = parse_json_response(&response);
    let blocked_tools = parsed["profile"]["profile"]["blockedTools"]
        .as_array()
        .expect("blockedTools must be present");
    assert_eq!(blocked_tools.len(), TOPIC_AGENT_YTDLP_TOOLS.len());
    let ytdlp_status = provider_status(&parsed, "ytdlp");
    assert_eq!(ytdlp_status["enabled"], false);
}

#[tokio::test]
async fn topic_agent_tools_enable_accepts_reminder_provider_alias() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_user_config()
        .returning(|_| Ok(UserConfig::default()));
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(Some(binding(77, "topic-a", "agent-a", 1))));
    mock.expect_get_topic_infra_config()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .returning(|_, _| {
            Ok(Some(agent_profile_record(
                "agent-a",
                1,
                json!({
                    "blockedTools": TOPIC_AGENT_REMINDER_TOOLS,
                }),
                10,
                10,
            )))
        });
    mock.expect_upsert_agent_profile()
        .withf(|options| {
            options.agent_id == "agent-a" && options.profile.get("blockedTools").is_none()
        })
        .returning(|options| {
            Ok(agent_profile_record(
                options.agent_id,
                2,
                options.profile,
                10,
                20,
            ))
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

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_AGENT_TOOLS_ENABLE,
            r#"{"topic_id":"topic-a","tools":["reminder"]}"#,
            None,
            None,
        )
        .await
        .expect("topic agent tools enable should succeed");

    let parsed = parse_json_response(&response);
    let reminder_status = provider_status(&parsed, "reminder");
    assert_eq!(reminder_status["enabled"], true);
}

#[tokio::test]
async fn topic_agent_tools_disable_accepts_stack_logs_provider_alias() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_user_config()
        .returning(|_| Ok(UserConfig::default()));
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(Some(binding(77, "topic-a", "agent-a", 1))));
    mock.expect_get_topic_infra_config()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .returning(|_, _| {
            Ok(Some(agent_profile_record(
                "agent-a",
                1,
                json!({
                    "systemPrompt": "ops agent",
                }),
                10,
                10,
            )))
        });
    mock.expect_upsert_agent_profile()
        .withf(|options| {
            options.agent_id == "agent-a"
                && options.profile["systemPrompt"] == "ops agent"
                && options
                    .profile
                    .get("blockedTools")
                    .and_then(|value| value.as_array())
                    .is_some_and(|tools| {
                        TOPIC_AGENT_STACK_LOGS_TOOLS
                            .iter()
                            .all(|tool| tools.iter().any(|value| value.as_str() == Some(*tool)))
                    })
        })
        .returning(|options| {
            Ok(agent_profile_record(
                options.agent_id,
                2,
                options.profile,
                10,
                20,
            ))
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

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_AGENT_TOOLS_DISABLE,
            r#"{"topic_id":"topic-a","tools":["stack_logs"]}"#,
            None,
            None,
        )
        .await
        .expect("topic agent tools disable should accept stack_logs alias");

    let parsed = parse_json_response(&response);
    let stack_logs_status = provider_status(&parsed, "stack_logs");
    assert_eq!(stack_logs_status["enabled"], false);
    assert!(stack_logs_status["blocked_tools"]
        .as_array()
        .is_some_and(|tools| {
            TOPIC_AGENT_STACK_LOGS_TOOLS
                .iter()
                .all(|tool| tools.iter().any(|value| value.as_str() == Some(*tool)))
        }));
}

#[cfg(feature = "browser_use")]
#[tokio::test]
async fn topic_agent_tools_get_reports_browser_use_provider_status_when_enabled() {
    let _guard = crate::config::test_env_async_mutex().lock().await;
    std::env::set_var("BROWSER_USE_URL", "http://browser-use:8000");
    std::env::set_var("BROWSER_USE_ENABLED", "true");

    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(Some(binding(77, "topic-a", "agent-a", 1))));
    mock.expect_get_topic_infra_config()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .returning(|_, _| Ok(None));

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_AGENT_TOOLS_GET,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("topic agent tools get should succeed");

    let parsed = parse_json_response(&response);
    let browser_status = provider_status(&parsed, "browser_use");

    assert_eq!(browser_status["enabled"], true);
    assert!(browser_status["available_tools"]
        .as_array()
        .is_some_and(|tools| {
            tools.iter().any(|tool| tool == "browser_use_run_task")
                && tools.iter().any(|tool| tool == "browser_use_get_session")
                && tools.iter().any(|tool| tool == "browser_use_close_session")
                && tools
                    .iter()
                    .any(|tool| tool == "browser_use_extract_content")
                && tools.iter().any(|tool| tool == "browser_use_screenshot")
        }));

    std::env::remove_var("BROWSER_USE_ENABLED");
    std::env::remove_var("BROWSER_USE_URL");
}

#[cfg(feature = "browser_use")]
#[tokio::test]
async fn topic_agent_tools_disable_accepts_browser_provider_alias() {
    let _guard = crate::config::test_env_async_mutex().lock().await;
    std::env::set_var("BROWSER_USE_URL", "http://browser-use:8000");
    std::env::set_var("BROWSER_USE_ENABLED", "true");
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_user_config()
        .returning(|_| Ok(UserConfig::default()));
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(Some(binding(77, "topic-a", "agent-a", 1))));
    mock.expect_get_topic_infra_config()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .returning(|_, _| {
            Ok(Some(agent_profile_record(
                "agent-a",
                3,
                json!({
                    "systemPrompt": "browser agent",
                }),
                10,
                20,
            )))
        });
    mock.expect_upsert_agent_profile()
        .withf(|options| {
            options.agent_id == "agent-a"
                && options.profile["systemPrompt"] == "browser agent"
                && options
                    .profile
                    .get("blockedTools")
                    .and_then(|value| value.as_array())
                    .is_some_and(|tools| {
                        [
                            "browser_use_run_task",
                            "browser_use_get_session",
                            "browser_use_close_session",
                            "browser_use_extract_content",
                            "browser_use_screenshot",
                        ]
                        .iter()
                        .all(|tool| tools.iter().any(|value| value.as_str() == Some(*tool)))
                    })
        })
        .returning(|options| {
            Ok(agent_profile_record(
                options.agent_id,
                4,
                options.profile,
                10,
                30,
            ))
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

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_AGENT_TOOLS_DISABLE,
            r#"{"topic_id":"topic-a","tools":["browser"]}"#,
            None,
            None,
        )
        .await
        .expect("topic agent tools disable should accept browser alias");
    let parsed = parse_json_response(&response);
    let blocked_tools = parsed["profile"]["profile"]["blockedTools"]
        .as_array()
        .expect("blockedTools must be present");
    assert_eq!(blocked_tools.len(), 5);
    let browser_status = provider_status(&parsed, "browser_use");
    assert_eq!(browser_status["enabled"], false);
    std::env::remove_var("BROWSER_USE_ENABLED");
    std::env::remove_var("BROWSER_USE_URL");
}

#[tokio::test]
async fn topic_agent_tools_enable_accepts_ssh_send_file_to_user_when_topic_has_infra() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_user_config()
        .returning(|_| Ok(UserConfig::default()));
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(Some(binding(77, "topic-a", "agent-a", 1))));
    mock.expect_get_topic_infra_config()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(Some(topic_infra(77, "topic-a", 1))));
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .returning(|_, _| {
            Ok(Some(agent_profile_record(
                "agent-a",
                1,
                json!({
                    "blockedTools": ["ssh_send_file_to_user"],
                }),
                10,
                10,
            )))
        });
    mock.expect_upsert_agent_profile()
        .withf(|options| {
            options.agent_id == "agent-a" && options.profile.get("blockedTools").is_none()
        })
        .returning(|options| {
            Ok(agent_profile_record(
                options.agent_id,
                2,
                options.profile,
                10,
                20,
            ))
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

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_AGENT_TOOLS_ENABLE,
            r#"{"topic_id":"topic-a","tools":["ssh_send_file_to_user"]}"#,
            None,
            None,
        )
        .await
        .expect("topic agent tools enable should accept ssh_send_file_to_user");

    let parsed = parse_json_response(&response);
    let ssh_status = provider_status(&parsed, "ssh");
    assert!(ssh_status["available_tools"]
        .as_array()
        .expect("available_tools must be present")
        .iter()
        .any(|value| value.as_str() == Some("ssh_send_file_to_user")));
    assert_eq!(ssh_status["enabled"], true);
}

#[tokio::test]
async fn topic_agent_tools_disable_sandbox_triggers_container_cleanup() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("-100777:240".to_string()))
        .returning(|_, _| Ok(Some(binding(77, "-100777:240", "agent-a", 1))));
    mock.expect_get_topic_infra_config()
        .with(eq(77_i64), eq("-100777:240".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .returning(|_, _| {
            Ok(Some(agent_profile_record(
                "agent-a",
                3,
                json!({
                    "systemPrompt": "infra agent",
                }),
                10,
                20,
            )))
        });
    mock.expect_upsert_agent_profile()
        .withf(|options| {
            options.agent_id == "agent-a"
                && TOPIC_AGENT_SANDBOX_TOOLS.iter().all(|tool| {
                    options.profile["blockedTools"]
                        .as_array()
                        .is_some_and(|tools| {
                            tools.iter().any(|value| value.as_str() == Some(*tool))
                        })
                })
        })
        .returning(|options| {
            Ok(agent_profile_record(
                options.agent_id,
                4,
                options.profile,
                10,
                30,
            ))
        });
    mock.expect_append_audit_event()
        .withf(|options| {
            options.action == TOOL_TOPIC_AGENT_TOOLS_DISABLE
                && options
                    .payload
                    .get("sandbox_cleanup")
                    .and_then(|value| value.get("deleted_container"))
                    == Some(&json!(true))
        })
        .returning(|options| {
            Ok(audit_event(
                1,
                options.topic_id.as_deref(),
                options.agent_id.as_deref(),
                &options.action,
                options.payload,
            ))
        });

    let sandbox_cleanup = Arc::new(FakeTopicSandboxCleanup::new());
    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77)
        .with_topic_sandbox_cleanup(sandbox_cleanup.clone());
    let response = provider
        .execute(
            TOOL_TOPIC_AGENT_TOOLS_DISABLE,
            r#"{"topic_id":"-100777:240","tools":["sandbox"]}"#,
            None,
            None,
        )
        .await
        .expect("topic agent sandbox disable should succeed");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["sandbox_cleanup"]["deleted_container"], true);
    assert_eq!(sandbox_cleanup.calls(), vec![(77, -100777, 240)]);
}

#[tokio::test]
async fn topic_agent_tools_enable_is_noop_without_existing_profile() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(Some(binding(77, "topic-a", "agent-a", 1))));
    mock.expect_get_topic_infra_config()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_append_audit_event().returning(|options| {
        Ok(audit_event(
            1,
            options.topic_id.as_deref(),
            options.agent_id.as_deref(),
            &options.action,
            options.payload,
        ))
    });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_AGENT_TOOLS_ENABLE,
            r#"{"topic_id":"topic-a","tools":["ytdlp_get_video_metadata"]}"#,
            None,
            None,
        )
        .await
        .expect("topic agent tools enable should succeed");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["updated"], false);
    assert_eq!(parsed["tools"]["blocked_tools"], json!([]));
}

#[tokio::test]
async fn topic_agent_hooks_get_reports_manageable_and_protected_hooks() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(Some(binding(77, "topic-a", "agent-a", 1))));
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .returning(|_, _| {
            Ok(Some(agent_profile_record(
                "agent-a",
                1,
                json!({
                    "disabledHooks": ["search_budget"],
                }),
                10,
                10,
            )))
        });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_AGENT_HOOKS_GET,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("topic agent hooks get should succeed");

    let parsed = parse_json_response(&response);
    assert_eq!(
        parsed["hooks"]["active_hooks"].as_array().map(Vec::len),
        Some(7)
    );
    assert_eq!(parsed["hooks"]["disabled_hooks"], json!(["search_budget"]));
    assert!(parsed["hooks"]["hook_statuses"]
        .as_array()
        .expect("hook_statuses must be an array")
        .iter()
        .any(|entry| entry["hook"] == "completion_check" && entry["protected"] == true));
}

#[tokio::test]
async fn topic_agent_hooks_disable_rejects_protected_hook() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(Some(binding(77, "topic-a", "agent-a", 1))));

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let err = provider
        .execute(
            TOOL_TOPIC_AGENT_HOOKS_DISABLE,
            r#"{"topic_id":"topic-a","hooks":["completion_check"]}"#,
            None,
            None,
        )
        .await
        .expect_err("protected hook must not be disableable");

    assert!(err.to_string().contains("system-protected"));
}

#[tokio::test]
async fn topic_agent_hooks_disable_persists_manageable_hook_change() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(Some(binding(77, "topic-a", "agent-a", 1))));
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .returning(|_, _| Ok(Some(agent_profile_record("agent-a", 1, json!({}), 10, 10))));
    mock.expect_upsert_agent_profile()
        .withf(|options| {
            options.agent_id == "agent-a"
                && options.profile["disabledHooks"] == json!(["timeout_report"])
        })
        .returning(|options| {
            Ok(agent_profile_record(
                options.agent_id,
                2,
                options.profile,
                10,
                20,
            ))
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

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_AGENT_HOOKS_DISABLE,
            r#"{"topic_id":"topic-a","hooks":["timeout"]}"#,
            None,
            None,
        )
        .await
        .expect("manageable hook disable should succeed");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["hooks"]["disabled_hooks"], json!(["timeout_report"]));
    assert_eq!(
        parsed["profile"]["profile"]["disabledHooks"],
        json!(["timeout_report"])
    );
}
