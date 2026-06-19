// Allow clone_on_ref_ptr in integration tests due to trait object coercion requirements
#![allow(clippy::clone_on_ref_ptr)]
#![cfg_attr(not(oxide_module_tool_todos), allow(dead_code, unused_imports))]

use oxide_agent_core::agent::identity::SessionId;
use oxide_agent_core::agent::providers::DelegationProvider;
use oxide_agent_core::agent::tool_runtime::{
    ModelMetadata, ProviderMetadata, ToolBatchId, ToolCallId, ToolExecutionContext, ToolInvocation,
    ToolName, ToolOutputStatus, ToolTimeoutConfig, TurnId,
};
use oxide_agent_core::config::AgentSettings;
use oxide_agent_core::llm::InvocationId;
use oxide_agent_core::llm::{
    ChatResponse, ChatWithToolsRequest, LlmClient, LlmError, LlmProvider, Message,
};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio::sync::{Mutex as AsyncMutex, watch};
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;

const TOOL_SPAWN_SUB_AGENTS: &str = "spawn_sub_agents";
const TOOL_WAIT_SUB_AGENTS: &str = "wait_sub_agents";

fn runtime_invocation(tool_name: &str, raw_arguments: &str) -> ToolInvocation {
    let now = chrono::Utc::now();
    ToolInvocation {
        session_id: SessionId::from(77),
        turn_id: TurnId::from("turn-sub-agent-delegation"),
        batch_id: ToolBatchId::from("batch-sub-agent-delegation"),
        batch_index: 0,
        invocation_id: InvocationId::from(format!("invoke-{tool_name}")),
        tool_call_id: ToolCallId::from(format!("call-{tool_name}")),
        provider_tool_call_id: None,
        tool_name: ToolName::from(tool_name),
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

async fn execute_delegation_tool(
    provider: &Arc<DelegationProvider>,
    tool_name: &str,
    arguments: &str,
) -> anyhow::Result<String> {
    let executor = provider
        .tool_runtime_executors(None)
        .into_iter()
        .find(|executor| executor.name().as_str() == tool_name)
        .expect("delegation typed executor registered");
    let output = executor
        .execute(runtime_invocation(tool_name, arguments))
        .await
        .map_err(|error| anyhow::anyhow!(error))?;

    anyhow::ensure!(
        output.status == ToolOutputStatus::Success,
        "delegation tool {tool_name} failed: {:?}",
        output.error_message
    );

    Ok(output.stdout.text.unwrap_or_default())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordedSubAgentRequest {
    model_id: String,
    max_tokens: u32,
    message_count: usize,
}

struct BudgetProbeProvider {
    requests: Arc<Mutex<Vec<RecordedSubAgentRequest>>>,
}

struct GatedProbeProvider {
    release_rx: AsyncMutex<watch::Receiver<bool>>,
}

#[async_trait::async_trait]
impl LlmProvider for BudgetProbeProvider {
    async fn complete_internal_text(
        &self,
        _system_prompt: &str,
        _history: &[Message],
        _user_message: &str,
        _model_id: &str,
        _max_tokens: u32,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown(
            "delegation smoke test uses chat_with_tools".to_string(),
        ))
    }

    async fn transcribe_audio(
        &self,
        _audio_bytes: Vec<u8>,
        _mime_type: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        unreachable!("delegation smoke test does not transcribe audio")
    }

    async fn analyze_image(
        &self,
        _image_bytes: Vec<u8>,
        _text_prompt: &str,
        _system_prompt: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        unreachable!("delegation smoke test does not analyze images")
    }

    async fn chat_with_tools<'a>(
        &self,
        request: ChatWithToolsRequest<'a>,
    ) -> Result<ChatResponse, LlmError> {
        self.requests
            .lock()
            .expect("probe lock")
            .push(RecordedSubAgentRequest {
                model_id: request.model_id.to_string(),
                max_tokens: request.max_tokens,
                message_count: request.messages.len(),
            });

        Ok(ChatResponse {
            content: Some(
                r#"{"thought":"done","tool_call":null,"final_answer":"budget-path-ok"}"#
                    .to_string(),
            ),
            tool_calls: Vec::new(),
            finish_reason: "stop".to_string(),
            reasoning_content: None,
            usage: None,
        })
    }
}

#[async_trait::async_trait]
impl LlmProvider for GatedProbeProvider {
    async fn complete_internal_text(
        &self,
        _system_prompt: &str,
        _history: &[Message],
        _user_message: &str,
        _model_id: &str,
        _max_tokens: u32,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown(
            "delegation async test uses chat_with_tools".to_string(),
        ))
    }

    async fn transcribe_audio(
        &self,
        _audio_bytes: Vec<u8>,
        _mime_type: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        unreachable!("delegation async test does not transcribe audio")
    }

    async fn analyze_image(
        &self,
        _image_bytes: Vec<u8>,
        _text_prompt: &str,
        _system_prompt: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        unreachable!("delegation async test does not analyze images")
    }

    async fn chat_with_tools<'a>(
        &self,
        _request: ChatWithToolsRequest<'a>,
    ) -> Result<ChatResponse, LlmError> {
        let mut release_rx = self.release_rx.lock().await;
        while !*release_rx.borrow() {
            release_rx
                .changed()
                .await
                .map_err(|_| LlmError::Unknown("release channel closed".to_string()))?;
        }

        Ok(ChatResponse {
            content: Some(
                r#"{"thought":"done","tool_call":null,"final_answer":"async-path-ok"}"#.to_string(),
            ),
            tool_calls: Vec::new(),
            finish_reason: "stop".to_string(),
            reasoning_content: None,
            usage: None,
        })
    }
}

#[cfg(oxide_module_tool_todos)]
#[tokio::test]
async fn sub_agent_delegation_budget_path_smoke_test() -> anyhow::Result<()> {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let settings = Arc::new(AgentSettings {
        sub_agent_model_id: Some("sub-model".to_string()),
        sub_agent_model_provider: Some("opencode-go".to_string()),
        sub_agent_max_output_tokens: Some(1_234),
        sub_agent_context_window_tokens: Some(48_000),
        ..AgentSettings::default()
    });
    let mut llm = LlmClient::new(&settings);
    llm.register_provider(
        "opencode-go".to_string(),
        Arc::new(BudgetProbeProvider {
            requests: Arc::clone(&requests),
        }),
    );
    let provider = Arc::new(DelegationProvider::new(Arc::new(llm), 1, settings.clone()));

    let args = json!({
        "tasks": [{
            "task": "Return a short confirmation.",
            "tools": ["write_todos"],
            "context": "Keep the answer short."
        }]
    });

    let spawn_result =
        execute_delegation_tool(&provider, TOOL_SPAWN_SUB_AGENTS, &args.to_string()).await?;
    let spawn_json: serde_json::Value = serde_json::from_str(&spawn_result)?;
    let sub_agent_id = spawn_json["started"][0]["id"]
        .as_str()
        .expect("spawn returns sub-agent id");

    let wait_result = execute_delegation_tool(
        &provider,
        TOOL_WAIT_SUB_AGENTS,
        &json!({
            "ids": [sub_agent_id],
            "timeout_ms": 30_000
        })
        .to_string(),
    )
    .await?;
    let wait_json: serde_json::Value = serde_json::from_str(&wait_result)?;
    assert_eq!(wait_json["statuses"][0]["status"], "completed");
    assert_eq!(wait_json["statuses"][0]["output"], "budget-path-ok");

    let captured = requests.lock().expect("probe lock").clone();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].model_id, "sub-model");
    assert_eq!(captured[0].max_tokens, 1_234);
    assert_eq!(captured[0].message_count, 1);

    Ok(())
}

#[cfg(oxide_module_tool_todos)]
#[tokio::test]
async fn sub_agent_spawn_returns_before_background_result() -> anyhow::Result<()> {
    let settings = Arc::new(AgentSettings {
        sub_agent_model_id: Some("sub-model".to_string()),
        sub_agent_model_provider: Some("opencode-go".to_string()),
        ..AgentSettings::default()
    });
    let (release_tx, release_rx) = watch::channel(false);
    let mut llm = LlmClient::new(&settings);
    llm.register_provider(
        "opencode-go".to_string(),
        Arc::new(GatedProbeProvider {
            release_rx: AsyncMutex::new(release_rx),
        }),
    );
    let provider = Arc::new(DelegationProvider::new(Arc::new(llm), 1, settings));

    let args = json!({
        "tasks": [{
            "task": "Return a short confirmation after release.",
            "tools": ["write_todos"]
        }]
    });

    let spawn_result = tokio::time::timeout(
        Duration::from_millis(250),
        execute_delegation_tool(&provider, TOOL_SPAWN_SUB_AGENTS, &args.to_string()),
    )
    .await
    .expect("spawn_sub_agents must return without waiting for the sub-agent")?;
    let spawn_json: serde_json::Value = serde_json::from_str(&spawn_result)?;
    let sub_agent_id = spawn_json["started"][0]["id"]
        .as_str()
        .expect("spawn returns sub-agent id");

    let poll_result = execute_delegation_tool(
        &provider,
        TOOL_WAIT_SUB_AGENTS,
        &json!({
            "ids": [sub_agent_id],
            "timeout_ms": 0
        })
        .to_string(),
    )
    .await?;
    let poll_json: serde_json::Value = serde_json::from_str(&poll_result)?;
    assert_eq!(poll_json["statuses"][0]["status"], "running");

    release_tx.send(true)?;
    let wait_result = execute_delegation_tool(
        &provider,
        TOOL_WAIT_SUB_AGENTS,
        &json!({
            "ids": [sub_agent_id],
            "timeout_ms": 30_000
        })
        .to_string(),
    )
    .await?;
    let wait_json: serde_json::Value = serde_json::from_str(&wait_result)?;
    assert_eq!(wait_json["statuses"][0]["status"], "completed");
    assert_eq!(wait_json["statuses"][0]["output"], "async-path-ok");

    Ok(())
}

#[tokio::test]
#[ignore = "Requires real LLM provider credentials and network access"]
async fn sub_agent_delegation_smoke_test() -> anyhow::Result<()> {
    let settings = Arc::new(AgentSettings::new()?);
    let llm = Arc::new(LlmClient::new(&settings));
    let provider = Arc::new(DelegationProvider::new(llm, 1, settings.clone()));

    let args = json!({
        "tasks": [{
            "task": "Make a short summary of what Rust is and where it is used.",
            "tools": ["write_todos"],
            "context": "The answer should be brief, 3-5 sentences."
        }]
    });

    let spawn_result =
        execute_delegation_tool(&provider, TOOL_SPAWN_SUB_AGENTS, &args.to_string()).await?;
    let spawn_json: serde_json::Value = serde_json::from_str(&spawn_result)?;
    let sub_agent_id = spawn_json["started"][0]["id"]
        .as_str()
        .expect("spawn returns sub-agent id");
    let result = execute_delegation_tool(
        &provider,
        TOOL_WAIT_SUB_AGENTS,
        &json!({
            "ids": [sub_agent_id],
            "timeout_ms": 60_000
        })
        .to_string(),
    )
    .await?;

    assert!(!result.trim().is_empty());
    Ok(())
}
