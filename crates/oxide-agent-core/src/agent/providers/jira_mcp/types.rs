//! MCP client types and DTOs for Jira MCP provider.

use serde::Deserialize;

/// Arguments for jira_read tool.
/// Used for schema validation via serde; fields intentionally not read directly.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct JiraReadArgs {
    #[serde(default)]
    pub keys: Vec<String>,
    #[serde(default)]
    pub jql: Option<String>,
    #[serde(default)]
    pub resource: Option<String>,
    #[serde(default)]
    pub board_id: Option<i64>,
    #[serde(default)]
    pub sprint_id: Option<i64>,
    #[serde(default)]
    pub project_key: Option<String>,
    #[serde(default)]
    pub board_name: Option<String>,
    #[serde(default)]
    pub board_type: Option<String>,
    #[serde(default)]
    pub sprint_state: Option<String>,
    #[serde(default)]
    pub fields: Option<String>,
    #[serde(default)]
    pub expand: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub start_at: i64,
}

fn default_limit() -> i64 {
    100
}

/// Arguments for jira_write tool.
/// Used for schema validation via serde; fields intentionally not read directly.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct JiraWriteArgs {
    pub action: String,
    pub items: Vec<JiraWriteItem>,
    #[serde(default)]
    pub dry_run: bool,
}

/// Single item for jira_write action.
/// Used for schema validation via serde; fields intentionally not read directly.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct JiraWriteItem {
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub issue_type: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub assignee: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub transition_id: Option<String>,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub comment_id: Option<String>,
    #[serde(default)]
    pub sprint_id: Option<i64>,
    #[serde(rename = "fields_json", default)]
    pub fields_json: Option<String>,
}

/// Arguments for jira_schema tool.
/// Used for schema validation via serde; fields intentionally not read directly.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct JiraSchemaArgs {
    pub resource: String,
    #[serde(default)]
    pub issue_key: Option<String>,
    #[serde(default)]
    pub field_id: Option<String>,
}
