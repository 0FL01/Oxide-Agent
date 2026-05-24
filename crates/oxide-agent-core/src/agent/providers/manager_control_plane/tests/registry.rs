use super::*;
use crate::agent::identity::SessionId;
use crate::agent::tool_runtime::{
    ModelMetadata, ProviderMetadata, ToolBatchId, ToolCallId, ToolExecutionContext,
    ToolOutputStatus, ToolTimeoutConfig, TurnId,
};
use crate::llm::InvocationId;
use chrono::Utc;
use tokio_util::sync::CancellationToken;

fn runtime_invocation(
    tool_name: &str,
    raw_arguments: &str,
) -> crate::agent::tool_runtime::ToolInvocation {
    let now = Utc::now();
    crate::agent::tool_runtime::ToolInvocation {
        session_id: SessionId::from(77),
        turn_id: TurnId::from("turn-manager"),
        batch_id: ToolBatchId::from("batch-manager"),
        batch_index: 0,
        invocation_id: InvocationId::from(format!("invoke-{tool_name}")),
        tool_call_id: ToolCallId::from(format!("call-{tool_name}")),
        provider_tool_call_id: None,
        tool_name: crate::agent::tool_runtime::ToolName::from(tool_name),
        raw_provider_payload: json!({}),
        raw_arguments: raw_arguments.to_string(),
        normalized_arguments: serde_json::Value::Null,
        cancellation_token: CancellationToken::new(),
        timeout: ToolTimeoutConfig::default(),
        execution_context: ToolExecutionContext::new(std::env::temp_dir()),
        provider_metadata: ProviderMetadata {
            provider: "test".to_string(),
            protocol: "chat_like".to_string(),
        },
        model_metadata: ModelMetadata {
            model: "test-model".to_string(),
        },
        working_directory: None,
        environment_metadata: None,
        created_at: now,
        started_at: Some(now),
    }
}

#[tokio::test]
async fn typed_runtime_executor_routes_to_manager_provider() {
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
    let provider = Arc::new(ManagerControlPlaneProvider::new(Arc::new(mock), 77));
    let executor = provider
        .tool_runtime_executors()
        .into_iter()
        .find(|executor| executor.name().as_str() == TOOL_AGENT_PROFILE_GET)
        .expect("typed manager get executor registered");

    let output = executor
        .execute(runtime_invocation(
            TOOL_AGENT_PROFILE_GET,
            r#"{"agent_id":"agent-x"}"#,
        ))
        .await
        .expect("typed manager execution should succeed");

    assert_eq!(output.status, ToolOutputStatus::Success);
    let parsed = parse_json_response(output.stdout.text.as_deref().expect("stdout text"));
    assert_eq!(parsed["found"], true);
    assert_eq!(parsed["profile"]["agent_id"], "agent-x");
}
