//! Jira MCP provider for Jira Server 7.5.0 integration.
//!
//! Provides tools for reading, writing, and schema discovery via MCP protocol.
//! Disabled by default - must be enabled via `topic_agent_tools_enable`.

use crate::agent::progress::AgentEvent;
use crate::agent::provider::ToolProvider;
use crate::llm::ToolDefinition;
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

mod client;
mod config;
mod types;

use client::JiraMcpClient;
pub use config::JiraMcpConfig;
use types::{JiraReadArgs, JiraSchemaArgs, JiraWriteArgs};

const TOOL_JIRA_READ: &str = "jira_read";
const TOOL_JIRA_WRITE: &str = "jira_write";
const TOOL_JIRA_SCHEMA: &str = "jira_schema";

/// Jira MCP provider implementation.
///
/// Communicates with jira-mcp binary via Model Context Protocol (MCP).
/// Requires environment variables to be configured:
/// - JIRA_MCP_BINARY_PATH
/// - JIRA_URL
/// - JIRA_EMAIL
/// - JIRA_API_TOKEN
pub struct JiraMcpProvider {
    config: JiraMcpConfig,
    client: Arc<Mutex<Option<Arc<JiraMcpClient>>>>,
}

impl JiraMcpProvider {
    /// Creates a new Jira MCP provider with the given configuration.
    pub fn new(config: JiraMcpConfig) -> Self {
        Self {
            config,
            client: Arc::new(Mutex::new(None)),
        }
    }

    /// Lazily initializes the MCP client.
    async fn ensure_client(&self) -> Result<Arc<JiraMcpClient>> {
        let mut guard = self.client.lock().await;
        
        if let Some(ref client) = *guard {
            return Ok(Arc::clone(client));
        }

        let client = Arc::new(
            JiraMcpClient::new(&self.config)
                .await
                .context("failed to initialize jira-mcp client")?,
        );
        
        *guard = Some(Arc::clone(&client));
        Ok(client)
    }
}

#[async_trait]
impl ToolProvider for JiraMcpProvider {
    fn name(&self) -> &'static str {
        "jira_mcp"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: TOOL_JIRA_READ.to_string(),
                description: concat!(
                    "Read Jira issues, search via JQL, or list resources (projects, boards, sprints). ",
                    "Three modes (mutually exclusive): keys=[KEYS] to fetch specific issues, ",
                    "jql=QUERY to search, resource=TYPE to list projects/boards/sprints. ",
                    "For Jira Server 7.5.0 compatibility - uses REST API v2 and plain text descriptions."
                )
                .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "keys": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Issue keys (e.g., ['PROJ-1', 'PROJ-2']). Mutually exclusive with jql/resource."
                        },
                        "jql": {
                            "type": "string",
                            "description": "JQL search query. Mutually exclusive with keys/resource."
                        },
                        "resource": {
                            "type": "string",
                            "enum": ["projects", "boards", "sprints", "sprint_issues"],
                            "description": "Resource type to list. Mutually exclusive with keys/jql."
                        },
                        "board_id": {
                            "type": "integer",
                            "description": "Board ID (required for resource=sprints)"
                        },
                        "sprint_id": {
                            "type": "integer",
                            "description": "Sprint ID (required for resource=sprint_issues)"
                        },
                        "project_key": {
                            "type": "string",
                            "description": "Filter boards by project key"
                        },
                        "board_name": {
                            "type": "string",
                            "description": "Filter boards by name substring"
                        },
                        "board_type": {
                            "type": "string",
                            "enum": ["scrum", "kanban"],
                            "description": "Filter boards by type"
                        },
                        "sprint_state": {
                            "type": "string",
                            "enum": ["active", "closed", "future"],
                            "description": "Filter sprints by state"
                        },
                        "fields": {
                            "type": "string",
                            "description": "Comma-separated field names to return"
                        },
                        "expand": {
                            "type": "string",
                            "description": "Comma-separated expansions (renderedFields, transitions, changelog)"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Max results (default 100)",
                            "default": 100
                        },
                        "start_at": {
                            "type": "integer",
                            "description": "Pagination offset",
                            "default": 0
                        }
                    }
                }),
            },
            ToolDefinition {
                name: TOOL_JIRA_WRITE.to_string(),
                description: concat!(
                    "Create, update, delete, transition issues; add/edit comments; move issues to sprints. ",
                    "Supports dry_run for preview. Actions: create, update, delete, transition, ",
                    "comment, edit_comment, move_to_sprint. ",
                    "For Jira Server 7.5.0: uses username (not accountId), plain text descriptions (not ADF)."
                )
                .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["create", "update", "delete", "transition", "comment", "edit_comment", "move_to_sprint"],
                            "description": "Action to perform"
                        },
                        "items": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "key": {"type": "string", "description": "Issue key (for update/delete/transition/comment)"},
                                    "project": {"type": "string", "description": "Project key (for create)"},
                                    "summary": {"type": "string"},
                                    "issue_type": {"type": "string", "description": "Bug, Task, Story, Epic, etc."},
                                    "priority": {"type": "string"},
                                    "assignee": {"type": "string", "description": "Username (NOT accountId for Jira Server 7.x)"},
                                    "description": {"type": "string", "description": "Plain text/wiki markup (NOT ADF)"},
                                    "labels": {
                                        "type": "array",
                                        "items": {"type": "string"}
                                    },
                                    "transition_id": {"type": "string"},
                                    "comment": {"type": "string"},
                                    "comment_id": {"type": "string", "description": "For edit_comment"},
                                    "sprint_id": {"type": "integer"},
                                    "fields_json": {"type": "string", "description": "Raw JSON for custom fields"}
                                }
                            }
                        },
                        "dry_run": {
                            "type": "boolean",
                            "description": "Preview changes without applying",
                            "default": false
                        }
                    },
                    "required": ["action", "items"]
                }),
            },
            ToolDefinition {
                name: TOOL_JIRA_SCHEMA.to_string(),
                description: concat!(
                    "Discover Jira metadata: fields, transitions, allowed values. ",
                    "Resources: fields (all fields), transitions (for issue). ",
                    "Note: field_options NOT supported on Jira Server 7.x"
                )
                .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "resource": {
                            "type": "string",
                            "enum": ["fields", "transitions"],
                            "description": "Schema resource to fetch"
                        },
                        "issue_key": {
                            "type": "string",
                            "description": "Required for resource=transitions"
                        }
                    },
                    "required": ["resource"]
                }),
            },
        ]
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        matches!(tool_name, TOOL_JIRA_READ | TOOL_JIRA_WRITE | TOOL_JIRA_SCHEMA)
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        _progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        _cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        let client = self
            .ensure_client()
            .await
            .context("failed to initialize jira-mcp client")?;

        // Parse and validate arguments based on tool type
        let args = match tool_name {
            TOOL_JIRA_READ => {
                let _: JiraReadArgs = serde_json::from_str(arguments)
                    .context("failed to parse jira_read arguments")?;
                serde_json::from_str::<serde_json::Value>(arguments)
                    .context("failed to parse arguments to JSON")?
                    .as_object()
                    .cloned()
                    .ok_or_else(|| anyhow!("arguments must be a JSON object"))?
            }
            TOOL_JIRA_WRITE => {
                let _: JiraWriteArgs = serde_json::from_str(arguments)
                    .context("failed to parse jira_write arguments")?;
                serde_json::from_str::<serde_json::Value>(arguments)
                    .context("failed to parse arguments to JSON")?
                    .as_object()
                    .cloned()
                    .ok_or_else(|| anyhow!("arguments must be a JSON object"))?
            }
            TOOL_JIRA_SCHEMA => {
                let _: JiraSchemaArgs = serde_json::from_str(arguments)
                    .context("failed to parse jira_schema arguments")?;
                serde_json::from_str::<serde_json::Value>(arguments)
                    .context("failed to parse arguments to JSON")?
                    .as_object()
                    .cloned()
                    .ok_or_else(|| anyhow!("arguments must be a JSON object"))?
            }
            _ => return Err(anyhow!("unknown tool: {}", tool_name)),
        };

        // Call the MCP tool
        client
            .call_tool(tool_name, args)
            .await
            .with_context(|| format!("jira-mcp tool '{}' execution failed", tool_name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_name() {
        let config = JiraMcpConfig {
            binary_path: "/bin/jira-mcp".to_string(),
            jira_url: "https://jira.test".to_string(),
            jira_email: "test@test.com".to_string(),
            jira_token: "token".to_string(),
        };
        let provider = JiraMcpProvider::new(config);
        assert_eq!(provider.name(), "jira_mcp");
    }

    #[test]
    fn test_provider_tools() {
        let config = JiraMcpConfig {
            binary_path: "/bin/jira-mcp".to_string(),
            jira_url: "https://jira.test".to_string(),
            jira_email: "test@test.com".to_string(),
            jira_token: "token".to_string(),
        };
        let provider = JiraMcpProvider::new(config);
        let tools = provider.tools();
        
        assert_eq!(tools.len(), 3);
        assert!(tools.iter().any(|t| t.name == "jira_read"));
        assert!(tools.iter().any(|t| t.name == "jira_write"));
        assert!(tools.iter().any(|t| t.name == "jira_schema"));
    }

    #[test]
    fn test_can_handle() {
        let config = JiraMcpConfig {
            binary_path: "/bin/jira-mcp".to_string(),
            jira_url: "https://jira.test".to_string(),
            jira_email: "test@test.com".to_string(),
            jira_token: "token".to_string(),
        };
        let provider = JiraMcpProvider::new(config);
        
        assert!(provider.can_handle("jira_read"));
        assert!(provider.can_handle("jira_write"));
        assert!(provider.can_handle("jira_schema"));
        assert!(!provider.can_handle("unknown_tool"));
        assert!(!provider.can_handle("ssh_exec"));
    }
}
