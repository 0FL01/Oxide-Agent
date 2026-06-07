//! Lightweight topic-scoped `AGENTS.md` tools for the active agent context.

use crate::agent::tool_runtime::{
    OutputNormalizer, ToolExecutor, ToolInvocation, ToolName, ToolOutput, ToolRuntimeConfig,
    ToolRuntimeError,
};
use crate::llm::ToolDefinition;
use crate::storage::{
    StorageProvider, UpsertTopicAgentsMdOptions, validate_topic_agents_md_content,
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

const TOOL_AGENTS_MD_GET: &str = "agents_md_get";
const TOOL_AGENTS_MD_UPDATE: &str = "agents_md_update";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentsMdUpdateArgs {
    agents_md: String,
    #[serde(default)]
    dry_run: bool,
}

/// Tool names that should stay available to top-level agents by default.
pub fn agents_md_tool_names() -> Vec<String> {
    vec![
        TOOL_AGENTS_MD_GET.to_string(),
        TOOL_AGENTS_MD_UPDATE.to_string(),
    ]
}

/// Tool provider that lets the current agent read and update its own topic `AGENTS.md`.
pub struct AgentsMdProvider {
    storage: Arc<dyn StorageProvider>,
    user_id: i64,
    topic_id: String,
}

impl AgentsMdProvider {
    /// Create a provider bound to the current user and topic scope.
    #[must_use]
    pub fn new(storage: Arc<dyn StorageProvider>, user_id: i64, topic_id: String) -> Self {
        Self {
            storage,
            user_id,
            topic_id,
        }
    }

    /// Build native typed runtime executors for topic AGENTS.md tools.
    #[must_use]
    pub fn tool_runtime_executors(self: &Arc<Self>) -> Vec<Arc<dyn ToolExecutor>> {
        let execution_lock = Arc::new(Mutex::new(()));
        Self::tools_definitions()
            .into_iter()
            .map(|spec| {
                Arc::new(AgentsMdToolExecutor {
                    provider: Arc::clone(self),
                    name: ToolName::from(spec.name.clone()),
                    spec,
                    execution_lock: Arc::clone(&execution_lock),
                }) as Arc<dyn ToolExecutor>
            })
            .collect()
    }

    fn tools_definitions() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: TOOL_AGENTS_MD_GET.to_string(),
                description: "Get the current topic AGENTS.md".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false,
                }),
            },
            ToolDefinition {
                name: TOOL_AGENTS_MD_UPDATE.to_string(),
                description: "Update the current topic AGENTS.md for future and current flow steps (max 300 lines)".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "agents_md": {
                            "type": "string",
                            "description": "Full AGENTS.md content for the current topic"
                        },
                        "dry_run": {
                            "type": "boolean",
                            "description": "Validate and preview without persisting"
                        }
                    },
                    "required": ["agents_md"],
                    "additionalProperties": false,
                }),
            },
        ]
    }

    fn parse_args<T: for<'de> Deserialize<'de>>(arguments: &str, tool_name: &str) -> Result<T> {
        serde_json::from_str(arguments)
            .map_err(|error| anyhow!("invalid arguments for {tool_name}: {error}"))
    }

    async fn execute_get(&self, arguments: &str) -> Result<String> {
        let _: serde_json::Map<String, serde_json::Value> =
            Self::parse_args(arguments, TOOL_AGENTS_MD_GET)?;

        let record = self
            .storage
            .get_topic_agents_md(self.user_id, self.topic_id.clone())
            .await
            .map_err(|error| anyhow!("failed to get topic AGENTS.md: {error}"))?;

        Ok(json!({
            "ok": true,
            "found": record.is_some(),
            "topic_id": self.topic_id.clone(),
            "topic_agents_md": record,
        })
        .to_string())
    }

    async fn execute_update(&self, arguments: &str) -> Result<String> {
        let args: AgentsMdUpdateArgs = Self::parse_args(arguments, TOOL_AGENTS_MD_UPDATE)?;
        let agents_md = validate_topic_agents_md_content(&args.agents_md)?;
        let previous = self
            .storage
            .get_topic_agents_md(self.user_id, self.topic_id.clone())
            .await
            .map_err(|error| anyhow!("failed to get current topic AGENTS.md: {error}"))?;

        if args.dry_run {
            return Ok(json!({
                "ok": true,
                "dry_run": true,
                "topic_id": self.topic_id.clone(),
                "preview": {
                    "operation": "update",
                    "agents_md": agents_md,
                },
                "previous": previous,
            })
            .to_string());
        }

        let record = self
            .storage
            .upsert_topic_agents_md(UpsertTopicAgentsMdOptions {
                user_id: self.user_id,
                topic_id: self.topic_id.clone(),
                agents_md,
            })
            .await
            .map_err(|error| anyhow!("failed to update topic AGENTS.md: {error}"))?;

        Ok(json!({
            "ok": true,
            "topic_id": self.topic_id.clone(),
            "topic_agents_md": record,
            "previous": previous,
        })
        .to_string())
    }

    async fn execute_tool(&self, tool_name: &str, arguments: &str) -> Result<String> {
        match tool_name {
            TOOL_AGENTS_MD_GET => self.execute_get(arguments).await,
            TOOL_AGENTS_MD_UPDATE => self.execute_update(arguments).await,
            _ => Err(anyhow!("Unknown AGENTS.md tool: {tool_name}")),
        }
    }
}

struct AgentsMdToolExecutor {
    provider: Arc<AgentsMdProvider>,
    name: ToolName,
    spec: ToolDefinition,
    execution_lock: Arc<Mutex<()>>,
}

#[async_trait]
impl ToolExecutor for AgentsMdToolExecutor {
    fn name(&self) -> ToolName {
        self.name.clone()
    }

    fn spec(&self) -> ToolDefinition {
        self.spec.clone()
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        let _guard = self.execution_lock.lock().await;
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig {
            timeout: invocation.timeout.clone(),
            artifact_dir: invocation.execution_context.artifact_dir.clone(),
            ..ToolRuntimeConfig::default()
        });
        self.provider
            .execute_tool(self.name.as_str(), &invocation.raw_arguments)
            .await
            .map(|output| normalizer.success(&invocation, &output, ""))
            .map_err(|error| ToolRuntimeError::Failure(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::identity::SessionId;
    use crate::agent::tool_runtime::{
        ModelMetadata, ProviderMetadata, ToolBatchId, ToolCallId, ToolExecutionContext,
        ToolOutputStatus, ToolTimeoutConfig, TurnId,
    };
    use crate::llm::InvocationId;
    use crate::storage::{MockStorageProvider, TopicAgentsMdRecord};
    use chrono::Utc;
    use mockall::predicate::eq;
    use tokio_util::sync::CancellationToken;

    fn runtime_invocation(tool_name: &str, raw_arguments: &str) -> ToolInvocation {
        let now = Utc::now();
        ToolInvocation {
            session_id: SessionId::from(77),
            turn_id: TurnId::from("turn-agents-md"),
            batch_id: ToolBatchId::from("batch-agents-md"),
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

    fn typed_executor(provider: &Arc<AgentsMdProvider>, tool_name: &str) -> Arc<dyn ToolExecutor> {
        provider
            .tool_runtime_executors()
            .into_iter()
            .find(|executor| executor.name().as_str() == tool_name)
            .expect("typed AGENTS.md executor registered")
    }

    #[tokio::test]
    async fn get_returns_current_topic_record() {
        let mut storage = MockStorageProvider::new();
        storage
            .expect_get_topic_agents_md()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|user_id, topic_id| {
                Ok(Some(TopicAgentsMdRecord {
                    schema_version: 1,
                    version: 2,
                    user_id,
                    topic_id,
                    agents_md: "# Topic AGENTS\nStay focused.".to_string(),
                    created_at: 10,
                    updated_at: 20,
                }))
            });

        let provider = Arc::new(AgentsMdProvider::new(
            Arc::new(storage),
            77,
            "topic-a".to_string(),
        ));
        let output = typed_executor(&provider, TOOL_AGENTS_MD_GET)
            .execute(runtime_invocation(TOOL_AGENTS_MD_GET, "{}"))
            .await
            .expect("get must succeed");
        assert_eq!(output.status, ToolOutputStatus::Success);

        let parsed: serde_json::Value =
            serde_json::from_str(output.stdout.text.as_deref().expect("stdout text"))
                .expect("valid json");
        assert_eq!(parsed["found"], true);
        assert_eq!(
            parsed["topic_agents_md"]["agents_md"],
            "# Topic AGENTS\nStay focused."
        );
    }

    #[tokio::test]
    async fn typed_runtime_executor_gets_current_topic_record() {
        let mut storage = MockStorageProvider::new();
        storage
            .expect_get_topic_agents_md()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|user_id, topic_id| {
                Ok(Some(TopicAgentsMdRecord {
                    schema_version: 1,
                    version: 2,
                    user_id,
                    topic_id,
                    agents_md: "# Topic AGENTS\nStay focused.".to_string(),
                    created_at: 10,
                    updated_at: 20,
                }))
            });
        let provider = Arc::new(AgentsMdProvider::new(
            Arc::new(storage),
            77,
            "topic-a".to_string(),
        ));
        let output = typed_executor(&provider, TOOL_AGENTS_MD_GET)
            .execute(runtime_invocation(TOOL_AGENTS_MD_GET, "{}"))
            .await
            .expect("typed AGENTS.md get succeeds");

        assert_eq!(output.status, ToolOutputStatus::Success);
        assert!(
            output
                .stdout
                .text
                .as_deref()
                .expect("stdout text")
                .contains("Stay focused")
        );
    }

    #[tokio::test]
    async fn update_dry_run_validates_without_persisting() {
        let mut storage = MockStorageProvider::new();
        storage
            .expect_get_topic_agents_md()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, _| Ok(None));
        storage.expect_upsert_topic_agents_md().times(0);

        let provider = Arc::new(AgentsMdProvider::new(
            Arc::new(storage),
            77,
            "topic-a".to_string(),
        ));
        let arguments = json!({
            "agents_md": "# Topic AGENTS\nUse checklists",
            "dry_run": true,
        })
        .to_string();
        let output = typed_executor(&provider, TOOL_AGENTS_MD_UPDATE)
            .execute(runtime_invocation(TOOL_AGENTS_MD_UPDATE, &arguments))
            .await
            .expect("dry run must succeed");
        assert_eq!(output.status, ToolOutputStatus::Success);

        let parsed: serde_json::Value =
            serde_json::from_str(output.stdout.text.as_deref().expect("stdout text"))
                .expect("valid json");
        assert_eq!(parsed["dry_run"], true);
        assert_eq!(
            parsed["preview"]["agents_md"],
            "# Topic AGENTS\nUse checklists"
        );
    }

    #[tokio::test]
    async fn update_persists_current_topic_record() {
        let mut storage = MockStorageProvider::new();
        storage
            .expect_get_topic_agents_md()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, _| Ok(None));
        storage
            .expect_upsert_topic_agents_md()
            .withf(|options| {
                options.user_id == 77
                    && options.topic_id == "topic-a"
                    && options.agents_md == "# Topic AGENTS\nUse checklists"
            })
            .returning(|options| {
                Ok(TopicAgentsMdRecord {
                    schema_version: 1,
                    version: 3,
                    user_id: options.user_id,
                    topic_id: options.topic_id,
                    agents_md: options.agents_md,
                    created_at: 10,
                    updated_at: 30,
                })
            });

        let provider = Arc::new(AgentsMdProvider::new(
            Arc::new(storage),
            77,
            "topic-a".to_string(),
        ));
        let arguments = json!({
            "agents_md": "# Topic AGENTS\nUse checklists",
        })
        .to_string();
        let output = typed_executor(&provider, TOOL_AGENTS_MD_UPDATE)
            .execute(runtime_invocation(TOOL_AGENTS_MD_UPDATE, &arguments))
            .await
            .expect("update must succeed");
        assert_eq!(output.status, ToolOutputStatus::Success);

        let parsed: serde_json::Value =
            serde_json::from_str(output.stdout.text.as_deref().expect("stdout text"))
                .expect("valid json");
        assert_eq!(parsed["topic_agents_md"]["topic_id"], "topic-a");
        assert_eq!(parsed["topic_agents_md"]["version"], 3);
    }

    #[tokio::test]
    async fn update_rejects_more_than_300_lines() {
        let storage = MockStorageProvider::new();
        let provider = Arc::new(AgentsMdProvider::new(
            Arc::new(storage),
            77,
            "topic-a".to_string(),
        ));
        let oversized = vec!["line"; 301].join("\n");
        let arguments = json!({ "agents_md": oversized }).to_string();

        let error = typed_executor(&provider, TOOL_AGENTS_MD_UPDATE)
            .execute(runtime_invocation(TOOL_AGENTS_MD_UPDATE, &arguments))
            .await
            .expect_err("oversized AGENTS.md must be rejected");

        assert!(
            error
                .to_string()
                .contains("agents_md must not exceed 300 lines")
        );
    }
}
