use super::*;

#[tokio::test]
async fn tool_registry_routes_to_manager_provider() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-x".to_string()))
        .returning(|user_id, agent_id| {
            Ok(Some(AgentProfileRecord {
                schema_version: 1,
                version: 5,
                user_id,
                agent_id,
                profile: json!({"role":"support"}),
                created_at: 10,
                updated_at: 20,
            }))
        });

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ManagerControlPlaneProvider::new(
        Arc::new(mock),
        77,
    )));

    let response = registry
        .execute(
            TOOL_AGENT_PROFILE_GET,
            r#"{"agent_id":"agent-x"}"#,
            None,
            None,
        )
        .await
        .expect("registry execution should succeed");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["found"], true);
    assert_eq!(parsed["profile"]["agent_id"], "agent-x");
}

#[tokio::test]
async fn tool_registry_routes_topic_agents_md_to_manager_provider() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_agents_md()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, topic_id| {
            Ok(Some(topic_agents_md_record(
                topic_id,
                1,
                "# Topic AGENTS",
                10,
                10,
            )))
        });

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ManagerControlPlaneProvider::new(
        Arc::new(mock),
        77,
    )));

    let response = registry
        .execute(
            TOOL_TOPIC_AGENTS_MD_GET,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("registry execution should succeed");

    let parsed = parse_json_response(&response);
    assert_eq!(parsed["found"], true);
    assert_eq!(parsed["topic_agents_md"]["topic_id"], "topic-a");
}

#[tokio::test]
async fn tool_registry_without_manager_provider_rejects_manager_tools() {
    let registry = ToolRegistry::new();
    let err = registry
        .execute(
            TOOL_TOPIC_BINDING_GET,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect_err("manager tools must be unavailable without provider");

    assert!(err.to_string().contains("Unknown tool"));
}
