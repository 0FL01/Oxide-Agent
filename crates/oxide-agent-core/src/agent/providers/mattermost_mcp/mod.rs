//! Mattermost MCP provider for Mattermost workspace integration.
//!
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

use client::MattermostMcpClient;
pub use config::MattermostMcpConfig;

// === Team tools ===
const TOOL_MATTERMOST_LIST_TEAMS: &str = "mattermost_list_teams";
const TOOL_MATTERMOST_GET_TEAM: &str = "mattermost_get_team";
const TOOL_MATTERMOST_GET_TEAM_MEMBERS: &str = "mattermost_get_team_members";

// === Channel tools ===
const TOOL_MATTERMOST_LIST_CHANNELS: &str = "mattermost_list_channels";
const TOOL_MATTERMOST_GET_CHANNEL: &str = "mattermost_get_channel";
const TOOL_MATTERMOST_GET_CHANNEL_BY_NAME: &str = "mattermost_get_channel_by_name";
const TOOL_MATTERMOST_CREATE_CHANNEL: &str = "mattermost_create_channel";
const TOOL_MATTERMOST_JOIN_CHANNEL: &str = "mattermost_join_channel";
const TOOL_MATTERMOST_CREATE_DIRECT_CHANNEL: &str = "mattermost_create_direct_channel";

// === Message tools ===
const TOOL_MATTERMOST_POST_MESSAGE: &str = "mattermost_post_message";
const TOOL_MATTERMOST_GET_CHANNEL_MESSAGES: &str = "mattermost_get_channel_messages";
const TOOL_MATTERMOST_SEARCH_MESSAGES: &str = "mattermost_search_messages";
const TOOL_MATTERMOST_UPDATE_MESSAGE: &str = "mattermost_update_message";
const TOOL_MATTERMOST_GET_THREAD: &str = "mattermost_get_thread";

// === User tools ===
const TOOL_MATTERMOST_GET_ME: &str = "mattermost_get_me";
const TOOL_MATTERMOST_GET_USER: &str = "mattermost_get_user";
const TOOL_MATTERMOST_GET_USER_BY_USERNAME: &str = "mattermost_get_user_by_username";
const TOOL_MATTERMOST_SEARCH_USERS: &str = "mattermost_search_users";

// === File tools ===
const TOOL_MATTERMOST_UPLOAD_FILE: &str = "mattermost_upload_file";

const TOOL_MAPPINGS: &[(&str, &str)] = &[
    // Team tools
    (TOOL_MATTERMOST_LIST_TEAMS, "list_teams"),
    (TOOL_MATTERMOST_GET_TEAM, "get_team"),
    (TOOL_MATTERMOST_GET_TEAM_MEMBERS, "get_team_members"),
    // Channel tools
    (TOOL_MATTERMOST_LIST_CHANNELS, "list_channels"),
    (TOOL_MATTERMOST_GET_CHANNEL, "get_channel"),
    (TOOL_MATTERMOST_GET_CHANNEL_BY_NAME, "get_channel_by_name"),
    (TOOL_MATTERMOST_CREATE_CHANNEL, "create_channel"),
    (TOOL_MATTERMOST_JOIN_CHANNEL, "join_channel"),
    (
        TOOL_MATTERMOST_CREATE_DIRECT_CHANNEL,
        "create_direct_channel",
    ),
    // Message tools
    (TOOL_MATTERMOST_POST_MESSAGE, "post_message"),
    (TOOL_MATTERMOST_GET_CHANNEL_MESSAGES, "get_channel_messages"),
    (TOOL_MATTERMOST_SEARCH_MESSAGES, "search_messages"),
    (TOOL_MATTERMOST_UPDATE_MESSAGE, "update_message"),
    (TOOL_MATTERMOST_GET_THREAD, "get_thread"),
    // User tools
    (TOOL_MATTERMOST_GET_ME, "get_me"),
    (TOOL_MATTERMOST_GET_USER, "get_user"),
    (TOOL_MATTERMOST_GET_USER_BY_USERNAME, "get_user_by_username"),
    (TOOL_MATTERMOST_SEARCH_USERS, "search_users"),
    // File tools
    (TOOL_MATTERMOST_UPLOAD_FILE, "upload_file"),
];

/// Mattermost MCP provider implementation.
pub struct MattermostMcpProvider {
    config: MattermostMcpConfig,
    client: Arc<Mutex<Option<Arc<MattermostMcpClient>>>>,
}

impl MattermostMcpProvider {
    /// Creates a new Mattermost MCP provider.
    pub fn new(config: MattermostMcpConfig) -> Self {
        Self {
            config,
            client: Arc::new(Mutex::new(None)),
        }
    }

    async fn ensure_client(&self) -> Result<Arc<MattermostMcpClient>> {
        let mut guard = self.client.lock().await;

        if let Some(ref client) = *guard {
            return Ok(Arc::clone(client));
        }

        let client = Arc::new(
            MattermostMcpClient::new(&self.config)
                .await
                .context("failed to initialize mattermost-mcp client")?,
        );

        *guard = Some(Arc::clone(&client));
        Ok(client)
    }
}

#[async_trait]
impl ToolProvider for MattermostMcpProvider {
    fn name(&self) -> &'static str {
        "mattermost_mcp"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        let mut tools = Vec::new();
        tools.extend(team_tools());
        tools.extend(channel_tools());
        tools.extend(message_tools());
        tools.extend(user_tools());
        tools.extend(file_tools());
        tools
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        upstream_tool_name(tool_name).is_some()
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        _progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        _cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        let upstream_tool_name = upstream_tool_name(tool_name)
            .ok_or_else(|| anyhow!("unknown mattermost tool: {tool_name}"))?;
        let client = self
            .ensure_client()
            .await
            .context("failed to initialize mattermost-mcp client")?;

        if !client.supports_tool(upstream_tool_name) {
            anyhow::bail!(
                "mattermost-mcp upstream does not expose tool '{}'",
                upstream_tool_name
            );
        }

        let args_value: serde_json::Value =
            serde_json::from_str(arguments).context("failed to parse tool arguments as JSON")?;
        let args = args_value
            .as_object()
            .cloned()
            .ok_or_else(|| anyhow!("tool arguments must be a JSON object"))?;

        client
            .call_tool(upstream_tool_name, args)
            .await
            .with_context(|| format!("mattermost-mcp tool '{}' execution failed", tool_name))
    }
}

fn upstream_tool_name(tool_name: &str) -> Option<&'static str> {
    TOOL_MAPPINGS
        .iter()
        .find_map(|(oxide_tool_name, upstream_tool_name)| {
            (*oxide_tool_name == tool_name).then_some(*upstream_tool_name)
        })
}

fn tool_definition(name: &str, description: &str, parameters: serde_json::Value) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: description.to_string(),
        parameters,
    }
}

fn team_tools() -> Vec<ToolDefinition> {
    vec![
        tool_definition(
            TOOL_MATTERMOST_LIST_TEAMS,
            "List all teams the configured Mattermost account belongs to. Use this first to discover available team IDs before listing channels or searching messages.",
            json!({
                "type": "object",
                "properties": {}
            }),
        ),
        tool_definition(
            TOOL_MATTERMOST_GET_TEAM,
            "Get detailed information about a Mattermost team by team ID.",
            json!({
                "type": "object",
                "properties": {
                    "team_id": {"type": "string", "description": "Mattermost team ID (26-character alphanumeric)"}
                },
                "required": ["team_id"]
            }),
        ),
        tool_definition(
            TOOL_MATTERMOST_GET_TEAM_MEMBERS,
            "Get list of users who belong to a Mattermost team.",
            json!({
                "type": "object",
                "properties": {
                    "team_id": {"type": "string", "description": "Mattermost team ID (26-character alphanumeric)"},
                    "page": {"type": "integer", "description": "Page number (0-indexed)", "default": 0},
                    "per_page": {"type": "integer", "description": "Results per page (max 200)", "default": 60}
                },
                "required": ["team_id"]
            }),
        ),
    ]
}

fn channel_tools() -> Vec<ToolDefinition> {
    vec![
        tool_definition(
            TOOL_MATTERMOST_LIST_CHANNELS,
            "List public and private channels in a team that the configured Mattermost account can access.",
            json!({
                "type": "object",
                "properties": {
                    "team_id": {"type": "string", "description": "Mattermost team ID"},
                    "page": {"type": "integer", "description": "Page number (0-indexed)", "default": 0},
                    "per_page": {"type": "integer", "description": "Results per page", "default": 60}
                },
                "required": ["team_id"]
            }),
        ),
        tool_definition(
            TOOL_MATTERMOST_GET_CHANNEL,
            "Get detailed information about a Mattermost channel by channel ID.",
            json!({
                "type": "object",
                "properties": {
                    "channel_id": {"type": "string", "description": "Mattermost channel ID"}
                },
                "required": ["channel_id"]
            }),
        ),
        tool_definition(
            TOOL_MATTERMOST_GET_CHANNEL_BY_NAME,
            "Look up a Mattermost channel by team ID and channel name.",
            json!({
                "type": "object",
                "properties": {
                    "team_id": {"type": "string", "description": "Mattermost team ID"},
                    "channel_name": {"type": "string", "description": "Mattermost channel name (without #)"}
                },
                "required": ["team_id", "channel_name"]
            }),
        ),
        tool_definition(
            TOOL_MATTERMOST_CREATE_CHANNEL,
            "Create a new public or private Mattermost channel in a team.",
            json!({
                "type": "object",
                "properties": {
                    "team_id": {"type": "string", "description": "Mattermost team ID"},
                    "name": {"type": "string", "description": "Internal channel name"},
                    "display_name": {"type": "string", "description": "Human-readable channel title"},
                    "channel_type": {"type": "string", "enum": ["O", "P"], "description": "O for public, P for private", "default": "O"},
                    "purpose": {"type": "string", "description": "Optional channel purpose"},
                    "header": {"type": "string", "description": "Optional channel header"}
                },
                "required": ["team_id", "name", "display_name"]
            }),
        ),
        tool_definition(
            TOOL_MATTERMOST_JOIN_CHANNEL,
            "Join a public Mattermost channel as the configured account.",
            json!({
                "type": "object",
                "properties": {
                    "channel_id": {"type": "string", "description": "Mattermost channel ID"}
                },
                "required": ["channel_id"]
            }),
        ),
        tool_definition(
            TOOL_MATTERMOST_CREATE_DIRECT_CHANNEL,
            "Create or fetch a Mattermost direct-message channel between two users.",
            json!({
                "type": "object",
                "properties": {
                    "user_id_1": {"type": "string", "description": "First Mattermost user ID"},
                    "user_id_2": {"type": "string", "description": "Second Mattermost user ID"}
                },
                "required": ["user_id_1", "user_id_2"]
            }),
        ),
    ]
}

fn message_tools() -> Vec<ToolDefinition> {
    vec![
        tool_definition(
            TOOL_MATTERMOST_POST_MESSAGE,
            "Post a Mattermost message to a channel, optionally in a thread or with file attachments.",
            json!({
                "type": "object",
                "properties": {
                    "channel_id": {"type": "string", "description": "Mattermost channel ID"},
                    "message": {"type": "string", "description": "Message content with Markdown support"},
                    "root_id": {"type": "string", "description": "Optional root post ID for thread replies"},
                    "file_ids": {"type": "array", "items": {"type": "string"}, "description": "Uploaded Mattermost file IDs"},
                    "attachments": {"type": "array", "items": {"type": "object"}, "description": "Rich attachment objects accepted by Mattermost"}
                },
                "required": ["channel_id", "message"]
            }),
        ),
        tool_definition(
            TOOL_MATTERMOST_GET_CHANNEL_MESSAGES,
            "Read recent Mattermost messages from a channel.",
            json!({
                "type": "object",
                "properties": {
                    "channel_id": {"type": "string", "description": "Mattermost channel ID"},
                    "page": {"type": "integer", "description": "Page number (0-indexed)", "default": 0},
                    "per_page": {"type": "integer", "description": "Results per page", "default": 60}
                },
                "required": ["channel_id"]
            }),
        ),
        tool_definition(
            TOOL_MATTERMOST_SEARCH_MESSAGES,
            "Search Mattermost messages within a team using Mattermost search syntax.",
            json!({
                "type": "object",
                "properties": {
                    "team_id": {"type": "string", "description": "Mattermost team ID"},
                    "terms": {"type": "string", "description": "Search terms, optionally with from:/in:/after: filters"},
                    "is_or_search": {"type": "boolean", "description": "Use OR instead of AND for multiple terms", "default": false}
                },
                "required": ["team_id", "terms"]
            }),
        ),
        tool_definition(
            TOOL_MATTERMOST_UPDATE_MESSAGE,
            "Edit an existing Mattermost message authored by the configured account or an admin.",
            json!({
                "type": "object",
                "properties": {
                    "post_id": {"type": "string", "description": "Mattermost post ID"},
                    "message": {"type": "string", "description": "Replacement message content"},
                    "attachments": {"type": "array", "items": {"type": "object"}, "description": "Optional replacement attachments"}
                },
                "required": ["post_id", "message"]
            }),
        ),
        tool_definition(
            TOOL_MATTERMOST_GET_THREAD,
            "Read the full Mattermost thread for a post, including the root message and replies.",
            json!({
                "type": "object",
                "properties": {
                    "post_id": {"type": "string", "description": "Root or reply Mattermost post ID"}
                },
                "required": ["post_id"]
            }),
        ),
    ]
}

fn user_tools() -> Vec<ToolDefinition> {
    vec![
        tool_definition(
            TOOL_MATTERMOST_GET_ME,
            "Get the configured Mattermost account profile.",
            json!({
                "type": "object",
                "properties": {}
            }),
        ),
        tool_definition(
            TOOL_MATTERMOST_GET_USER,
            "Get a Mattermost user profile by user ID.",
            json!({
                "type": "object",
                "properties": {
                    "user_id": {"type": "string", "description": "Mattermost user ID"}
                },
                "required": ["user_id"]
            }),
        ),
        tool_definition(
            TOOL_MATTERMOST_GET_USER_BY_USERNAME,
            "Get a Mattermost user profile by username.",
            json!({
                "type": "object",
                "properties": {
                    "username": {"type": "string", "description": "Mattermost username without @"}
                },
                "required": ["username"]
            }),
        ),
        tool_definition(
            TOOL_MATTERMOST_SEARCH_USERS,
            "Search Mattermost users by username, first name, last name, or nickname.",
            json!({
                "type": "object",
                "properties": {
                    "term": {"type": "string", "description": "Search term"},
                    "team_id": {"type": "string", "description": "Optional team ID to limit the search"}
                },
                "required": ["term"]
            }),
        ),
    ]
}

fn file_tools() -> Vec<ToolDefinition> {
    vec![tool_definition(
        TOOL_MATTERMOST_UPLOAD_FILE,
        "Upload a local file to a Mattermost channel and return the file ID for later message attachments.",
        json!({
            "type": "object",
            "properties": {
                "channel_id": {"type": "string", "description": "Mattermost channel ID"},
                "file_path": {"type": "string", "description": "Local path to the file on disk"},
                "filename": {"type": "string", "description": "Optional filename override shown in Mattermost"}
            },
            "required": ["channel_id", "file_path"]
        }),
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> MattermostMcpConfig {
        MattermostMcpConfig {
            binary_path: "/bin/mattermost-mcp".to_string(),
            mattermost_url: "https://mattermost.test".to_string(),
            mattermost_token: "token".to_string(),
            timeout_secs: 30,
            max_retries: 3,
            verify_ssl: true,
        }
    }

    #[test]
    fn test_provider_name() {
        let provider = MattermostMcpProvider::new(test_config());
        assert_eq!(provider.name(), "mattermost_mcp");
    }

    #[test]
    fn test_provider_tools() {
        let provider = MattermostMcpProvider::new(test_config());
        let tools = provider.tools();

        assert_eq!(tools.len(), TOOL_MAPPINGS.len());
        assert!(tools
            .iter()
            .any(|tool| tool.name == TOOL_MATTERMOST_POST_MESSAGE));
        assert!(tools
            .iter()
            .any(|tool| tool.name == TOOL_MATTERMOST_SEARCH_USERS));
        assert!(tools
            .iter()
            .any(|tool| tool.name == TOOL_MATTERMOST_UPLOAD_FILE));
    }

    #[test]
    fn test_can_handle() {
        let provider = MattermostMcpProvider::new(test_config());

        assert!(provider.can_handle(TOOL_MATTERMOST_LIST_CHANNELS));
        assert!(provider.can_handle(TOOL_MATTERMOST_GET_THREAD));
        assert!(!provider.can_handle("post_message"));
        assert!(!provider.can_handle("unknown_tool"));
    }

    #[test]
    fn test_upstream_tool_name_mapping() {
        assert_eq!(upstream_tool_name(TOOL_MATTERMOST_GET_ME), Some("get_me"));
        assert_eq!(
            upstream_tool_name(TOOL_MATTERMOST_GET_THREAD),
            Some("get_thread")
        );
        assert_eq!(upstream_tool_name("nope"), None);
    }
}
