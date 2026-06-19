//! Stack logs provider for compose-stack log discovery and retrieval.
//!
//! Documentation: `docs/stack-logs-stage0.md`

use crate::agent::tool_runtime::{
    OutputNormalizer, ToolExecutor, ToolInvocation, ToolName, ToolOutput, ToolRuntimeConfig,
    ToolRuntimeError,
};
use crate::llm::ToolDefinition;
use crate::sandbox::broker::{StackLogsFetchRequest, StackLogsListSourcesRequest};
use crate::sandbox::{SandboxDiagnostics, SandboxDiagnosticsRuntime};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

const TOOL_STACK_LOGS_LIST_SOURCES: &str = "stack_logs_list_sources";
const TOOL_STACK_LOGS_FETCH: &str = "stack_logs_fetch";

/// Provider that exposes compose-stack log discovery and fetch tools.
pub struct StackLogsProvider {
    diagnostics: Arc<dyn SandboxDiagnostics>,
}

impl Default for StackLogsProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl StackLogsProvider {
    /// Create a new stack logs provider.
    #[must_use]
    pub fn new() -> Self {
        Self::with_diagnostics(Arc::new(SandboxDiagnosticsRuntime::new()))
    }

    /// Create a provider from a narrow sandbox diagnostics backend.
    #[must_use]
    pub fn with_diagnostics(diagnostics: Arc<dyn SandboxDiagnostics>) -> Self {
        Self { diagnostics }
    }

    /// Build native typed runtime executors for stack log tools.
    #[must_use]
    pub fn tool_runtime_executors(self: &Arc<Self>) -> Vec<Arc<dyn ToolExecutor>> {
        let execution_lock = Arc::new(Mutex::new(()));
        Self::tool_definitions()
            .into_iter()
            .map(|spec| {
                Arc::new(StackLogsToolExecutor {
                    provider: Arc::clone(self),
                    name: ToolName::from(spec.name.clone()),
                    spec,
                    execution_lock: Arc::clone(&execution_lock),
                }) as Arc<dyn ToolExecutor>
            })
            .collect()
    }

    fn tool_definitions() -> Vec<ToolDefinition> {
        vec![Self::list_sources_definition(), Self::fetch_definition()]
    }

    fn list_sources_definition() -> ToolDefinition {
        ToolDefinition {
            name: TOOL_STACK_LOGS_LIST_SOURCES.to_string(),
            description: "List compose-stack log sources available for inspection. Returns JSON with stack_selector and containers so the agent can choose services before fetching logs.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "selector": {
                        "type": "object",
                        "description": "Optional compose stack selector override",
                        "properties": {
                            "compose_project": {
                                "type": "string",
                                "description": "Optional Docker Compose project name override"
                            }
                        },
                        "additionalProperties": false
                    },
                    "services": {
                        "type": "array",
                        "description": "Optional service names to include",
                        "items": { "type": "string" }
                    },
                    "include_stopped": {
                        "type": "boolean",
                        "description": "Whether to include stopped containers"
                    }
                },
                "additionalProperties": false
            }),
        }
    }

    fn fetch_definition() -> ToolDefinition {
        ToolDefinition {
            name: TOOL_STACK_LOGS_FETCH.to_string(),
            description: "Fetch compose-stack logs as bounded JSON entries. Supports time windows, per-service filtering, stderr selection, and cursor-based line-by-line pagination with suppression metadata.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "selector": {
                        "type": "object",
                        "description": "Optional compose stack selector override",
                        "properties": {
                            "compose_project": {
                                "type": "string",
                                "description": "Optional Docker Compose project name override"
                            }
                        },
                        "additionalProperties": false
                    },
                    "services": {
                        "type": "array",
                        "description": "Optional service names to include",
                        "items": { "type": "string" }
                    },
                    "since": {
                        "type": "string",
                        "description": "Optional RFC3339 start timestamp"
                    },
                    "until": {
                        "type": "string",
                        "description": "Optional RFC3339 end timestamp"
                    },
                    "cursor": {
                        "type": "object",
                        "description": "Optional cursor returned by a previous stack_logs_fetch call",
                        "properties": {
                            "ts": { "type": "string" },
                            "service": { "type": "string" },
                            "stream": { "type": "string" },
                            "ordinal": { "type": "integer", "minimum": 0 }
                        },
                        "required": ["ts", "service", "stream", "ordinal"],
                        "additionalProperties": false
                    },
                    "max_entries": {
                        "type": "integer",
                        "description": "Maximum number of log entries to return"
                    },
                    "include_noise": {
                        "type": "boolean",
                        "description": "Whether to disable conservative noise filtering"
                    },
                    "include_stderr": {
                        "type": "boolean",
                        "description": "Whether to include stderr log lines"
                    }
                },
                "additionalProperties": false
            }),
        }
    }

    async fn handle_list_sources(&self, arguments: &str) -> Result<String> {
        let request: StackLogsListSourcesArgs = if arguments.trim().is_empty() {
            StackLogsListSourcesArgs::default()
        } else {
            serde_json::from_str(arguments)?
        };

        match self
            .diagnostics
            .list_stack_log_sources(request.into())
            .await
        {
            Ok(response) => serde_json::to_string(&response).map_err(Into::into),
            Err(error) => serde_json::to_string(&json!({
                "error": error.to_string(),
            }))
            .map_err(Into::into),
        }
    }

    async fn handle_fetch(&self, arguments: &str) -> Result<String> {
        let request: StackLogsFetchArgs = if arguments.trim().is_empty() {
            StackLogsFetchArgs::default()
        } else {
            serde_json::from_str(arguments)?
        };

        match self.diagnostics.fetch_stack_logs(request.into()).await {
            Ok(response) => serde_json::to_string(&response).map_err(Into::into),
            Err(error) => serde_json::to_string(&json!({
                "error": error.to_string(),
            }))
            .map_err(Into::into),
        }
    }

    async fn execute_tool(&self, tool_name: &str, arguments: &str) -> Result<String> {
        match tool_name {
            TOOL_STACK_LOGS_LIST_SOURCES => self.handle_list_sources(arguments).await,
            TOOL_STACK_LOGS_FETCH => self.handle_fetch(arguments).await,
            _ => anyhow::bail!("Unknown stack logs tool: {tool_name}"),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct StackLogsListSourcesArgs {
    #[serde(default)]
    selector: crate::sandbox::broker::StackLogsSelector,
    #[serde(default)]
    services: Vec<String>,
    #[serde(default)]
    include_stopped: bool,
}

impl From<StackLogsListSourcesArgs> for StackLogsListSourcesRequest {
    fn from(value: StackLogsListSourcesArgs) -> Self {
        Self {
            selector: value.selector,
            services: value.services,
            include_stopped: value.include_stopped,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StackLogsFetchArgs {
    #[serde(default)]
    selector: crate::sandbox::broker::StackLogsSelector,
    #[serde(default)]
    services: Vec<String>,
    #[serde(default)]
    since: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    until: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    cursor: Option<crate::sandbox::broker::StackLogCursor>,
    #[serde(default = "default_stack_logs_max_entries")]
    max_entries: u32,
    #[serde(default)]
    include_noise: bool,
    #[serde(default = "default_include_stderr")]
    include_stderr: bool,
}

impl Default for StackLogsFetchArgs {
    fn default() -> Self {
        Self {
            selector: crate::sandbox::broker::StackLogsSelector::default(),
            services: Vec::new(),
            since: None,
            until: None,
            cursor: None,
            max_entries: default_stack_logs_max_entries(),
            include_noise: false,
            include_stderr: default_include_stderr(),
        }
    }
}

const fn default_stack_logs_max_entries() -> u32 {
    200
}

const fn default_include_stderr() -> bool {
    true
}

impl From<StackLogsFetchArgs> for StackLogsFetchRequest {
    fn from(value: StackLogsFetchArgs) -> Self {
        Self {
            selector: value.selector,
            services: value.services,
            since: value.since,
            until: value.until,
            cursor: value.cursor,
            max_entries: value.max_entries,
            include_noise: value.include_noise,
            include_stderr: value.include_stderr,
        }
    }
}

struct StackLogsToolExecutor {
    provider: Arc<StackLogsProvider>,
    name: ToolName,
    spec: ToolDefinition,
    execution_lock: Arc<Mutex<()>>,
}

#[async_trait]
impl ToolExecutor for StackLogsToolExecutor {
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
    use crate::sandbox::broker::{
        ResolvedStackLogsSelector, StackLogSource, StackLogsFetchResponse,
        StackLogsListSourcesResponse, StackLogsWindow,
    };
    use crate::sandbox::{SandboxBackend, SandboxBackendId, SandboxCapability};
    use chrono::Utc;
    use tokio_util::sync::CancellationToken;

    struct FakeDiagnostics;

    impl SandboxBackend for FakeDiagnostics {
        fn id(&self) -> SandboxBackendId {
            SandboxBackendId::new("sandbox/fake-diagnostics")
        }

        fn capabilities(&self) -> &'static [SandboxCapability] {
            &[SandboxCapability::Diagnostics]
        }
    }

    #[async_trait]
    impl SandboxDiagnostics for FakeDiagnostics {
        async fn list_stack_log_sources(
            &self,
            request: StackLogsListSourcesRequest,
        ) -> Result<StackLogsListSourcesResponse> {
            Ok(StackLogsListSourcesResponse {
                stack_selector: ResolvedStackLogsSelector {
                    compose_project: request
                        .selector
                        .compose_project
                        .unwrap_or_else(|| "oxide".to_string()),
                },
                containers: vec![StackLogSource {
                    service: "api".to_string(),
                    container_name: "oxide-api-1".to_string(),
                    container_id: "container-api".to_string(),
                    state: "running".to_string(),
                    started_at: None,
                }],
            })
        }

        async fn fetch_stack_logs(
            &self,
            _request: StackLogsFetchRequest,
        ) -> Result<StackLogsFetchResponse> {
            Ok(StackLogsFetchResponse {
                window: StackLogsWindow {
                    since: None,
                    until: None,
                },
                entries: Vec::new(),
                suppressed: Vec::new(),
                truncated: false,
                next_cursor: None,
                warnings: Vec::new(),
            })
        }
    }

    fn runtime_invocation(tool_name: &str, raw_arguments: &str) -> ToolInvocation {
        let now = Utc::now();
        ToolInvocation {
            session_id: SessionId::from(77),
            turn_id: TurnId::from("turn-stack-logs"),
            batch_id: ToolBatchId::from("batch-stack-logs"),
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

    #[test]
    fn typed_runtime_registers_stack_log_tools() {
        let provider = Arc::new(StackLogsProvider::new());
        let tools = provider.tool_runtime_executors();

        assert!(
            tools
                .iter()
                .any(|tool| tool.name().as_str() == TOOL_STACK_LOGS_LIST_SOURCES)
        );
        assert!(
            tools
                .iter()
                .any(|tool| tool.name().as_str() == TOOL_STACK_LOGS_FETCH)
        );
    }

    #[tokio::test]
    async fn typed_runtime_executor_lists_stack_log_sources() {
        let provider = Arc::new(StackLogsProvider::with_diagnostics(Arc::new(
            FakeDiagnostics,
        )));
        let executor = provider
            .tool_runtime_executors()
            .into_iter()
            .find(|executor| executor.name().as_str() == TOOL_STACK_LOGS_LIST_SOURCES)
            .expect("typed stack logs list executor registered");

        let output = executor
            .execute(runtime_invocation(
                TOOL_STACK_LOGS_LIST_SOURCES,
                r#"{"selector":{"compose_project":"oxide-test"}}"#,
            ))
            .await
            .expect("typed stack logs list succeeds");

        assert_eq!(output.status, ToolOutputStatus::Success);
        let stdout = output.stdout.text.as_deref().expect("stdout text");
        assert!(stdout.contains("oxide-test"));
        assert!(stdout.contains("oxide-api-1"));
    }

    #[test]
    fn fetch_args_default_to_stage_contract_values() {
        let args = StackLogsFetchArgs::default();

        assert_eq!(args.max_entries, 200);
        assert!(args.include_stderr);
        assert!(!args.include_noise);
        assert!(args.cursor.is_none());
        assert!(args.since.is_none());
        assert!(args.until.is_none());
    }

    #[test]
    fn fetch_schema_mentions_cursor_and_suppression_oriented_usage() {
        let tool = Arc::new(StackLogsProvider::new())
            .tool_runtime_executors()
            .into_iter()
            .find(|tool| tool.name().as_str() == TOOL_STACK_LOGS_FETCH)
            .expect("stack_logs_fetch registered");
        let spec = tool.spec();

        assert!(spec.description.contains("cursor"));
        assert!(spec.description.contains("suppression"));
    }
}
