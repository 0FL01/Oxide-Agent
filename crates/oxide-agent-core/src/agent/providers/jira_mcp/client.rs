//! MCP client wrapper for Jira MCP binary.

use anyhow::{Context, Result};
use rmcp::{
    model::CallToolRequestParams,
    service::{Peer, RoleClient},
    transport::{ConfigureCommandExt, TokioChildProcess},
    ServiceExt,
};
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::Mutex;

use super::config::JiraMcpConfig;

const STDERR_TAIL_MAX_BYTES: usize = 16 * 1024;

/// MCP client for communicating with jira-mcp binary.
pub struct JiraMcpClient {
    peer: Peer<RoleClient>,
    binary_path: String,
    child_pid: Option<u32>,
    stderr_tail: Arc<Mutex<String>>,
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
        .with_context(|| format!("failed to spawn jira-mcp binary at {}", config.binary_path))?;

        let child_pid = transport.id();
        let stderr_tail = Arc::new(Mutex::new(String::new()));

        let service = ().serve(transport).await.with_context(|| {
            format!(
                "failed to initialize jira-mcp service (binary={}, pid={:?})",
                config.binary_path, child_pid
            )
        })?;

        let peer = service.peer().clone();

        tracing::info!(
            binary_path = %config.binary_path,
            child_pid = child_pid,
            "jira-mcp child process started"
        );

        // Spawn task to drain stderr, preserve a tail, and surface diagnostics.
        let stderr_tail_for_task = Arc::clone(&stderr_tail);
        let binary_path = config.binary_path.clone();
        let stderr_task = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            if let Some(mut stderr) = stderr {
                let mut buf = vec![0_u8; 4096];
                loop {
                    match stderr.read(&mut buf).await {
                        Ok(0) => {
                            let stderr_tail = {
                                let guard = stderr_tail_for_task.lock().await;
                                guard.clone()
                            };
                            if stderr_tail.trim().is_empty() {
                                tracing::debug!(
                                    binary_path = %binary_path,
                                    child_pid = child_pid,
                                    "jira-mcp stderr stream closed"
                                );
                            } else {
                                tracing::warn!(
                                    binary_path = %binary_path,
                                    child_pid = child_pid,
                                    stderr_tail = %stderr_tail,
                                    "jira-mcp stderr stream closed after emitting diagnostics"
                                );
                            }
                            break;
                        }
                        Ok(n) => {
                            let chunk = String::from_utf8_lossy(&buf[..n]);
                            tracing::warn!(
                                binary_path = %binary_path,
                                child_pid = child_pid,
                                stderr_chunk = %chunk.trim_end(),
                                "jira-mcp emitted stderr"
                            );

                            let mut guard = stderr_tail_for_task.lock().await;
                            guard.push_str(&chunk);
                            if guard.len() > STDERR_TAIL_MAX_BYTES {
                                let excess = guard.len() - STDERR_TAIL_MAX_BYTES;
                                guard.drain(..excess);
                            }
                        }
                        Err(error) => {
                            tracing::warn!(
                                binary_path = %binary_path,
                                child_pid = child_pid,
                                %error,
                                "failed to read jira-mcp stderr"
                            );
                            break;
                        }
                    }
                }
            }
        });

        let client = Self {
            peer,
            binary_path: config.binary_path.clone(),
            child_pid,
            stderr_tail,
            _stderr_task: stderr_task,
        };

        client.preflight().await?;

        Ok(client)
    }

    async fn preflight(&self) -> Result<()> {
        let tools = self.peer.list_all_tools().await.map_err(|error| {
            let stderr_tail = self.stderr_tail_snapshot_blocking(error.to_string());
            tracing::error!(
                binary_path = %self.binary_path,
                child_pid = self.child_pid,
                stderr_tail = %stderr_tail,
                error = %error,
                "jira-mcp preflight tools/list failed"
            );
            anyhow::anyhow!(
                "jira-mcp preflight tools/list failed (binary={}, pid={:?}, stderr_tail={})",
                self.binary_path,
                self.child_pid,
                stderr_tail
            )
        })?;

        tracing::info!(
            binary_path = %self.binary_path,
            child_pid = self.child_pid,
            tool_count = tools.len(),
            tool_names = ?tools.iter().map(|tool| tool.name.clone()).collect::<Vec<_>>(),
            "jira-mcp preflight succeeded"
        );

        Ok(())
    }

    fn stderr_tail_snapshot_blocking(&self, fallback: String) -> String {
        match self.stderr_tail.try_lock() {
            Ok(guard) if !guard.trim().is_empty() => guard.clone(),
            _ => fallback,
        }
    }

    async fn stderr_tail_snapshot(&self) -> String {
        let guard = self.stderr_tail.lock().await;
        if guard.trim().is_empty() {
            "<empty>".to_string()
        } else {
            guard.clone()
        }
    }

    /// Calls a tool on the MCP server.
    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: serde_json::Map<String, serde_json::Value>,
    ) -> Result<String> {
        let params = CallToolRequestParams::new(tool_name.to_string()).with_arguments(arguments);

        let result = self.peer.call_tool(params).await.map_err(|error| {
            let stderr_tail = self.stderr_tail_snapshot_blocking(error.to_string());
            tracing::error!(
                binary_path = %self.binary_path,
                child_pid = self.child_pid,
                tool_name,
                stderr_tail = %stderr_tail,
                error = %error,
                "jira-mcp call_tool failed"
            );
            anyhow::anyhow!(
                "jira-mcp call_tool failed (binary={}, pid={:?}, tool={}, stderr_tail={})",
                self.binary_path,
                self.child_pid,
                tool_name,
                stderr_tail
            )
        })?;

        // Extract text content from result
        let text: String = result
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.clone()))
            .collect::<Vec<_>>()
            .join("\n");

        if result.is_error.unwrap_or(false) {
            let stderr_tail = self.stderr_tail_snapshot().await;
            tracing::warn!(
                binary_path = %self.binary_path,
                child_pid = self.child_pid,
                tool_name,
                stderr_tail = %stderr_tail,
                tool_output = %text,
                "jira-mcp tool returned an error result"
            );
            anyhow::bail!(
                "jira tool '{}' failed: {} (binary={}, pid={:?}, stderr_tail={})",
                tool_name,
                text,
                self.binary_path,
                self.child_pid,
                stderr_tail
            );
        }

        Ok(text)
    }
}
