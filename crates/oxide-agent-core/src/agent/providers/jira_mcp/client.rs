//! MCP client wrapper for Jira MCP binary.

use anyhow::{Context, Result};
use rmcp::{
    model::CallToolRequestParams,
    service::{Peer, RoleClient},
    transport::{ConfigureCommandExt, TokioChildProcess},
    ServiceExt,
};
use std::process::Stdio;
use tokio::process::Command;

use super::config::JiraMcpConfig;

/// MCP client for communicating with jira-mcp binary.
pub struct JiraMcpClient {
    peer: Peer<RoleClient>,
    _stderr_task: tokio::task::JoinHandle<()>,
}

impl JiraMcpClient {
    /// Creates a new MCP client connected to the jira-mcp binary.
    pub async fn new(config: &JiraMcpConfig) -> Result<Self> {
        let cmd = Command::new(&config.binary_path);
        
        let (transport, stderr) = TokioChildProcess::builder(cmd.configure(|cmd| {
            cmd.env("JIRA_URL", &config.jira_url)
                .env("JIRA_EMAIL", &config.jira_email)
                .env("JIRA_API_TOKEN", &config.jira_token);
        }))
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn jira-mcp binary")?;

        let service = ()
            .serve(transport)
            .await
            .context("failed to initialize jira-mcp service")?;

        let peer = service.peer().clone();

        // Spawn task to drain stderr and prevent blocking
        let stderr_task = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            if let Some(mut stderr) = stderr {
                let mut buf = String::new();
                let _ = stderr.read_to_string(&mut buf).await;
            }
        });

        Ok(Self {
            peer,
            _stderr_task: stderr_task,
        })
    }

    /// Calls a tool on the MCP server.
    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: serde_json::Map<String, serde_json::Value>,
    ) -> Result<String> {
        let params = CallToolRequestParams::new(tool_name.to_string()).with_arguments(arguments);

        let result = self
            .peer
            .call_tool(params)
            .await
            .context("jira-mcp call_tool failed")?;

        // Extract text content from result
        let text: String = result
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.clone()))
            .collect::<Vec<_>>()
            .join("\n");

        if result.is_error.unwrap_or(false) {
            anyhow::bail!("jira tool '{}' failed: {}", tool_name, text);
        }

        Ok(text)
    }
}
