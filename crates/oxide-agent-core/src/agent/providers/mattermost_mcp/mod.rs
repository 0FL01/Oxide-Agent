//! Mattermost MCP provider for Mattermost workspace integration.
//!
//! Disabled by default - must be enabled via `topic_agent_tools_enable`.

use crate::agent::tool_runtime::{
    OutputNormalizer, ToolExecutor, ToolInvocation, ToolName, ToolOutput, ToolRuntimeConfig,
    ToolRuntimeError,
};
use crate::llm::ToolDefinition;
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

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

    /// Build native typed runtime executors for Mattermost MCP tools.
    #[must_use]
    pub fn tool_runtime_executors(self: &Arc<Self>) -> Vec<Arc<dyn ToolExecutor>> {
        let execution_lock = Arc::new(Mutex::new(()));
        Self::tool_definitions()
            .into_iter()
            .map(|spec| {
                Arc::new(MattermostMcpToolExecutor {
                    provider: Arc::clone(self),
                    name: ToolName::from(spec.name.clone()),
                    spec,
                    execution_lock: Arc::clone(&execution_lock),
                }) as Arc<dyn ToolExecutor>
            })
            .collect()
    }

    fn tool_definitions() -> Vec<ToolDefinition> {
        let mut tools = Vec::new();
        tools.extend(team_tools());
        tools.extend(channel_tools());
        tools.extend(message_tools());
        tools.extend(user_tools());
        tools.extend(file_tools());
        tools
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

    async fn execute_tool(&self, tool_name: &str, arguments: &str) -> Result<String> {
        let upstream_tool_name = upstream_tool_name(tool_name)
            .ok_or_else(|| anyhow!("unknown mattermost tool: {tool_name}"))?;

        let args_value: serde_json::Value =
            serde_json::from_str(arguments).context("failed to parse tool arguments as JSON")?;
        let args = args_value
            .as_object()
            .cloned()
            .ok_or_else(|| anyhow!("tool arguments must be a JSON object"))?;

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

        client
            .call_tool(upstream_tool_name, args)
            .await
            .with_context(|| format!("mattermost-mcp tool '{}' execution failed", tool_name))
    }
}

struct MattermostMcpToolExecutor {
    provider: Arc<MattermostMcpProvider>,
    name: ToolName,
    spec: ToolDefinition,
    execution_lock: Arc<Mutex<()>>,
}

#[async_trait]
impl ToolExecutor for MattermostMcpToolExecutor {
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
            .map_err(mattermost_mcp_runtime_error)
    }
}

fn mattermost_mcp_runtime_error(error: anyhow::Error) -> ToolRuntimeError {
    let message = error.to_string();
    if error.downcast_ref::<serde_json::Error>().is_some()
        || message.contains("failed to parse tool arguments as JSON")
        || message.contains("tool arguments must be a JSON object")
    {
        ToolRuntimeError::InvalidArguments(message)
    } else {
        ToolRuntimeError::Failure(message)
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
    use crate::agent::identity::SessionId;
    use crate::agent::tool_runtime::{
        ModelMetadata, OutputNormalizer, ProviderMetadata, ToolBatchId, ToolCallId,
        ToolExecutionContext, ToolOutputStatus, ToolRuntimeConfig, ToolTimeoutConfig, TurnId,
    };
    use crate::llm::InvocationId;
    use chrono::Utc;
    use tokio_util::sync::CancellationToken;

    fn runtime_invocation(tool_name: &str, raw_arguments: &str) -> ToolInvocation {
        let now = Utc::now();
        ToolInvocation {
            session_id: SessionId::from(77),
            turn_id: TurnId::from("turn-mattermost-mcp"),
            batch_id: ToolBatchId::from("batch-mattermost-mcp"),
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
    fn typed_runtime_specs_include_mattermost_tool_definitions() {
        let provider = Arc::new(MattermostMcpProvider::new(test_config()));
        let tools = provider
            .tool_runtime_executors()
            .into_iter()
            .map(|executor| executor.spec())
            .collect::<Vec<_>>();

        assert_eq!(tools.len(), TOOL_MAPPINGS.len());
        assert!(
            tools
                .iter()
                .any(|tool| tool.name == TOOL_MATTERMOST_POST_MESSAGE)
        );
        assert!(
            tools
                .iter()
                .any(|tool| tool.name == TOOL_MATTERMOST_SEARCH_USERS)
        );
        assert!(
            tools
                .iter()
                .any(|tool| tool.name == TOOL_MATTERMOST_UPLOAD_FILE)
        );
    }

    #[test]
    fn typed_runtime_executors_register_only_oxide_mattermost_tools() {
        let provider = Arc::new(MattermostMcpProvider::new(test_config()));
        let names = provider
            .tool_runtime_executors()
            .into_iter()
            .map(|executor| executor.name().into_inner())
            .collect::<std::collections::BTreeSet<_>>();

        assert!(names.contains(TOOL_MATTERMOST_LIST_CHANNELS));
        assert!(names.contains(TOOL_MATTERMOST_GET_THREAD));
        assert!(!names.contains("post_message"));
        assert!(!names.contains("unknown_tool"));
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

    #[test]
    fn typed_runtime_executors_register_mattermost_tools() {
        let provider = Arc::new(MattermostMcpProvider::new(test_config()));

        let names = provider
            .tool_runtime_executors()
            .into_iter()
            .map(|executor| executor.name().into_inner())
            .collect::<std::collections::BTreeSet<_>>();

        assert!(names.contains(TOOL_MATTERMOST_LIST_TEAMS));
        assert!(names.contains(TOOL_MATTERMOST_POST_MESSAGE));
        assert!(names.contains(TOOL_MATTERMOST_UPLOAD_FILE));
        assert_eq!(names.len(), TOOL_MAPPINGS.len());
    }

    #[tokio::test]
    async fn typed_runtime_executor_rejects_malformed_arguments_before_mcp() {
        let provider = Arc::new(MattermostMcpProvider::new(test_config()));
        let executor = provider
            .tool_runtime_executors()
            .into_iter()
            .find(|executor| executor.name().as_str() == TOOL_MATTERMOST_LIST_TEAMS)
            .expect("mattermost_list_teams executor");

        let error = executor
            .execute(runtime_invocation(TOOL_MATTERMOST_LIST_TEAMS, "{"))
            .await
            .expect_err("malformed args must fail before MCP init");

        let output = OutputNormalizer::new(ToolRuntimeConfig::default())
            .executor_error(&runtime_invocation(TOOL_MATTERMOST_LIST_TEAMS, "{"), error);
        assert_eq!(output.status, ToolOutputStatus::InvalidArguments);
    }
}
