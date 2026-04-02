//! Stack logs provider for compose-stack log discovery and retrieval.

use crate::agent::provider::ToolProvider;
use crate::llm::ToolDefinition;
use crate::sandbox::broker::{StackLogsFetchRequest, StackLogsListSourcesRequest};
use crate::sandbox::SandboxManager;
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

const TOOL_STACK_LOGS_LIST_SOURCES: &str = "stack_logs_list_sources";
const TOOL_STACK_LOGS_FETCH: &str = "stack_logs_fetch";

/// Provider that exposes compose-stack log discovery and fetch tools.
pub struct StackLogsProvider;

impl Default for StackLogsProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl StackLogsProvider {
    /// Create a new stack logs provider.
    #[must_use]
    pub const fn new() -> Self {
        Self
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

    async fn handle_list_sources(arguments: &str) -> Result<String> {
        let request: StackLogsListSourcesArgs = if arguments.trim().is_empty() {
            StackLogsListSourcesArgs::default()
        } else {
            serde_json::from_str(arguments)?
        };

        match SandboxManager::list_stack_log_sources(request.into()).await {
            Ok(response) => serde_json::to_string(&response).map_err(Into::into),
            Err(error) => serde_json::to_string(&json!({
                "error": error.to_string(),
            }))
            .map_err(Into::into),
        }
    }

    async fn handle_fetch(arguments: &str) -> Result<String> {
        let request: StackLogsFetchArgs = if arguments.trim().is_empty() {
            StackLogsFetchArgs::default()
        } else {
            serde_json::from_str(arguments)?
        };

        match SandboxManager::fetch_stack_logs(request.into()).await {
            Ok(response) => serde_json::to_string(&response).map_err(Into::into),
            Err(error) => serde_json::to_string(&json!({
                "error": error.to_string(),
            }))
            .map_err(Into::into),
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

#[async_trait]
impl ToolProvider for StackLogsProvider {
    fn name(&self) -> &'static str {
        "stack_logs"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![Self::list_sources_definition(), Self::fetch_definition()]
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        matches!(
            tool_name,
            TOOL_STACK_LOGS_LIST_SOURCES | TOOL_STACK_LOGS_FETCH
        )
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        _progress_tx: Option<&tokio::sync::mpsc::Sender<crate::agent::progress::AgentEvent>>,
        _cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        match tool_name {
            TOOL_STACK_LOGS_LIST_SOURCES => Self::handle_list_sources(arguments).await,
            TOOL_STACK_LOGS_FETCH => Self::handle_fetch(arguments).await,
            _ => anyhow::bail!("Unknown stack logs tool: {tool_name}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_registers_stack_log_tools() {
        let provider = StackLogsProvider::new();
        let tools = provider.tools();

        assert!(tools
            .iter()
            .any(|tool| tool.name == TOOL_STACK_LOGS_LIST_SOURCES));
        assert!(tools.iter().any(|tool| tool.name == TOOL_STACK_LOGS_FETCH));
        assert!(provider.can_handle(TOOL_STACK_LOGS_LIST_SOURCES));
        assert!(provider.can_handle(TOOL_STACK_LOGS_FETCH));
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
        let tool = StackLogsProvider::new()
            .tools()
            .into_iter()
            .find(|tool| tool.name == TOOL_STACK_LOGS_FETCH)
            .expect("stack_logs_fetch registered");

        assert!(tool.description.contains("cursor"));
        assert!(tool.description.contains("suppression"));
    }
}
