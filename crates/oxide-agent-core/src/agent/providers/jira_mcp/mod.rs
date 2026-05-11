//! Jira MCP provider for Jira Server 7.5.0 integration.
//!
//! Provides tools for reading, writing, and schema discovery via MCP protocol.
//! Disabled by default - must be enabled via `topic_agent_tools_enable`.

use crate::agent::progress::AgentEvent;
use crate::agent::provider::ToolProvider;
use crate::llm::ToolDefinition;
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Map, Value};
use std::sync::{Arc, LazyLock};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

mod client;
mod config;

use client::JiraMcpClient;
pub use config::JiraMcpConfig;

const TOOL_JIRA_READ: &str = "jira_read";
const TOOL_JIRA_WRITE: &str = "jira_write";
const TOOL_JIRA_SCHEMA: &str = "jira_schema";

static JIRA_READ_TOOL: LazyLock<ToolDefinition> = LazyLock::new(|| ToolDefinition {
    name: TOOL_JIRA_READ.to_string(),
    description: concat!(
        "Read Jira issues, search via JQL, or list resources (projects, boards, sprints). ",
        "Pass exactly one selector in each call: keys=[KEYS] to fetch specific issues, ",
        "jql=QUERY to search, or resource=TYPE to list projects/boards/sprints. ",
        "Never combine keys, jql, and resource in the same call. ",
        "For Jira Server 7.5.0 compatibility - uses REST API v2 and plain text descriptions."
    )
    .to_string(),
    parameters: json!({
        "type": "object",
        "additionalProperties": false,
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
});

fn json_value_is_integer(value: &Value) -> bool {
    matches!(value, Value::Number(number) if number.is_i64() || number.is_u64())
}

fn jira_read_selector_names(args: &Map<String, Value>) -> Vec<&'static str> {
    ["keys", "jql", "resource"]
        .into_iter()
        .filter(|selector| args.contains_key(*selector))
        .collect()
}

fn validate_jira_read_arguments(args: &Map<String, Value>) -> Result<()> {
    const ALLOWED_ARGUMENTS: &[&str] = &[
        "keys",
        "jql",
        "resource",
        "board_id",
        "sprint_id",
        "project_key",
        "board_name",
        "board_type",
        "sprint_state",
        "fields",
        "expand",
        "limit",
        "start_at",
    ];

    let unknown_keys: Vec<&str> = args
        .keys()
        .map(String::as_str)
        .filter(|key| !ALLOWED_ARGUMENTS.contains(key))
        .collect();
    if !unknown_keys.is_empty() {
        anyhow::bail!(
            "jira_read received unknown argument(s): {}",
            unknown_keys.join(", ")
        );
    }

    let selectors = jira_read_selector_names(args);
    if selectors.len() != 1 {
        let found = if selectors.is_empty() {
            "none".to_string()
        } else {
            selectors.join(", ")
        };
        anyhow::bail!(
            "Provide exactly one of: keys, jql, or resource — found {}.",
            found
        );
    }

    match args.get("keys") {
        Some(Value::Array(keys)) => {
            if keys.is_empty() {
                anyhow::bail!("jira_read 'keys' must contain at least one issue key");
            }

            if keys.iter().any(|key| !matches!(key, Value::String(_))) {
                anyhow::bail!("jira_read 'keys' must be an array of strings");
            }
        }
        Some(_) => anyhow::bail!("jira_read 'keys' must be an array of strings"),
        None => {}
    }

    match args.get("jql") {
        Some(Value::String(jql)) => {
            if jql.trim().is_empty() {
                anyhow::bail!("jira_read 'jql' must not be empty");
            }
        }
        Some(_) => anyhow::bail!("jira_read 'jql' must be a string"),
        None => {}
    }

    match args.get("resource") {
        Some(Value::String(resource)) => match resource.as_str() {
            "sprints" => match args.get("board_id") {
                Some(value) if json_value_is_integer(value) => {}
                Some(_) => {
                    anyhow::bail!("jira_read 'board_id' must be an integer when resource='sprints'")
                }
                None => anyhow::bail!("jira_read requires 'board_id' when resource='sprints'"),
            },
            "sprint_issues" => match args.get("sprint_id") {
                Some(value) if json_value_is_integer(value) => {}
                Some(_) => anyhow::bail!(
                    "jira_read 'sprint_id' must be an integer when resource='sprint_issues'"
                ),
                None => {
                    anyhow::bail!("jira_read requires 'sprint_id' when resource='sprint_issues'")
                }
            },
            _ => {}
        },
        Some(_) => anyhow::bail!("jira_read 'resource' must be a string"),
        None => {}
    }

    Ok(())
}

static JIRA_WRITE_TOOL: LazyLock<ToolDefinition> = LazyLock::new(|| ToolDefinition {
    name: TOOL_JIRA_WRITE.to_string(),
    description: concat!(
        "Create, update, delete, transition issues; add/edit comments; move issues to sprints; ",
        "add/update/delete worklogs (time tracking). ",
        "Supports dry_run for preview. Actions: create, update, delete, transition, ",
        "comment, edit_comment, move_to_sprint, add_worklog, update_worklog, delete_worklog. ",
        "For Jira Server 7.5.0: uses username (not accountId), plain text descriptions (not ADF). ",
        "Time tracking: use add_worklog action for time tracking."
    )
    .to_string(),
    parameters: json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": ["create", "update", "delete", "transition", "comment", "edit_comment", "move_to_sprint", "add_worklog", "update_worklog", "delete_worklog"],
                "description": "Action to perform"
            },
            "items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "key": {"type": "string", "description": "Issue key (for update/delete/transition/comment/worklog)"},
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
                        "fields_json": {"type": "string", "description": "Raw JSON for custom fields"},
                        "time_spent": {"type": "string", "description": "Time spent (e.g., '3h 20m', '1d'). For add_worklog/update_worklog."},
                        "time_spent_seconds": {"type": "integer", "description": "Time spent in seconds. Alternative to time_spent."},
                        "started": {"type": "string", "description": "ISO 8601 timestamp when work started. For add_worklog."},
                        "worklog_id": {"type": "string", "description": "Worklog ID. Required for update_worklog/delete_worklog."},
                        "visibility_type": {"type": "string", "enum": ["group", "role"], "description": "Who can see the worklog."},
                        "visibility_value": {"type": "string", "description": "Group or role name for visibility."},
                        "adjust_estimate": {"type": "string", "enum": ["auto", "new", "leave", "manual"], "description": "How to adjust remaining estimate."},
                        "new_estimate": {"type": "string", "description": "New estimate value (e.g., '2d')."},
                        "reduce_by": {"type": "string", "description": "Amount to reduce estimate by (for add_worklog)."},
                        "increase_by": {"type": "string", "description": "Amount to increase estimate by (for delete_worklog)."}
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
});

static JIRA_SCHEMA_TOOL: LazyLock<ToolDefinition> = LazyLock::new(|| ToolDefinition {
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
});

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
            JIRA_READ_TOOL.clone(),
            JIRA_WRITE_TOOL.clone(),
            JIRA_SCHEMA_TOOL.clone(),
        ]
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        matches!(
            tool_name,
            TOOL_JIRA_READ | TOOL_JIRA_WRITE | TOOL_JIRA_SCHEMA
        )
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        _progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        _cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        // Parse arguments into JSON object
        let args_value: serde_json::Value =
            serde_json::from_str(arguments).context("failed to parse tool arguments as JSON")?;

        let args = args_value
            .as_object()
            .cloned()
            .ok_or_else(|| anyhow!("tool arguments must be a JSON object"))?;

        if tool_name == TOOL_JIRA_READ {
            validate_jira_read_arguments(&args).with_context(|| {
                let selectors = jira_read_selector_names(&args);
                let found = if selectors.is_empty() {
                    "none".to_string()
                } else {
                    selectors.join(", ")
                };
                format!("invalid jira_read arguments (selectors: {found})")
            })?;
        }

        let client = self
            .ensure_client()
            .await
            .context("failed to initialize jira-mcp client")?;

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

    fn json_args(value: Value) -> Map<String, Value> {
        serde_json::from_value(value).expect("test JSON must deserialize into args map")
    }

    fn test_config() -> JiraMcpConfig {
        JiraMcpConfig {
            binary_path: "/bin/jira-mcp".to_string(),
            jira_url: "https://jira.test".to_string(),
            jira_email: "test@test.com".to_string(),
            jira_token: "token".to_string(),
        }
    }

    #[test]
    fn test_provider_name() {
        let provider = JiraMcpProvider::new(test_config());
        assert_eq!(provider.name(), "jira_mcp");
    }

    #[test]
    fn test_provider_tools() {
        let provider = JiraMcpProvider::new(test_config());
        let tools = provider.tools();

        assert_eq!(tools.len(), 3);
        assert!(tools.iter().any(|t| t.name == "jira_read"));
        assert!(tools.iter().any(|t| t.name == "jira_write"));
        assert!(tools.iter().any(|t| t.name == "jira_schema"));

        let jira_read_tool = tools
            .into_iter()
            .find(|tool| tool.name == "jira_read")
            .expect("jira_read tool definition");
        assert_eq!(
            jira_read_tool.parameters["additionalProperties"],
            json!(false)
        );
        assert!(jira_read_tool.parameters.get("oneOf").is_none());
    }

    #[test]
    fn test_can_handle() {
        let provider = JiraMcpProvider::new(test_config());

        assert!(provider.can_handle("jira_read"));
        assert!(provider.can_handle("jira_write"));
        assert!(provider.can_handle("jira_schema"));
        assert!(!provider.can_handle("unknown_tool"));
        assert!(!provider.can_handle("ssh_exec"));
    }

    #[test]
    fn test_validate_jira_read_arguments_accepts_single_selector_modes() {
        validate_jira_read_arguments(&json_args(json!({"keys": ["SYS-1260"]})))
            .expect("keys mode should be valid");
        validate_jira_read_arguments(&json_args(json!({"jql": "key = SYS-1260"})))
            .expect("jql mode should be valid");
        validate_jira_read_arguments(&json_args(json!({"resource": "sprints", "board_id": 42})))
            .expect("resource mode should be valid");
    }

    #[test]
    fn test_validate_jira_read_arguments_rejects_multiple_selectors() {
        let error = validate_jira_read_arguments(&json_args(json!({
            "keys": ["SYS-1260"],
            "resource": "projects"
        })))
        .expect_err("multiple selector modes must be rejected");

        assert!(error
            .to_string()
            .contains("Provide exactly one of: keys, jql, or resource"));
    }

    #[test]
    fn test_validate_jira_read_arguments_rejects_missing_selector() {
        let error = validate_jira_read_arguments(&json_args(json!({"limit": 10})))
            .expect_err("missing selector must be rejected");

        assert!(error
            .to_string()
            .contains("Provide exactly one of: keys, jql, or resource"));
    }

    #[test]
    fn test_validate_jira_read_arguments_rejects_missing_resource_dependencies() {
        let error = validate_jira_read_arguments(&json_args(json!({"resource": "sprints"})))
            .expect_err("sprints without board_id must be rejected");
        assert!(error.to_string().contains("requires 'board_id'"));

        let error = validate_jira_read_arguments(&json_args(json!({"resource": "sprint_issues"})))
            .expect_err("sprint_issues without sprint_id must be rejected");
        assert!(error.to_string().contains("requires 'sprint_id'"));
    }

    #[test]
    fn test_validate_jira_read_arguments_rejects_unknown_keys() {
        let error = validate_jira_read_arguments(&json_args(json!({
            "resource": "projects",
            "unexpected": true
        })))
        .expect_err("unknown jira_read keys must be rejected");

        assert!(error
            .to_string()
            .contains("unknown argument(s): unexpected"));
    }
}
