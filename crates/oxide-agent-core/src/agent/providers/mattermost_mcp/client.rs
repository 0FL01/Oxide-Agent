//! MCP client wrapper for Mattermost MCP binary.

use anyhow::{Context, Result};
use rmcp::{
    model::CallToolRequestParams,
    service::{Peer, RoleClient, RunningService},
    transport::{ConfigureCommandExt, TokioChildProcess},
    ServiceExt,
};
use std::collections::HashSet;
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::Mutex;

use super::config::MattermostMcpConfig;

const STDERR_TAIL_MAX_BYTES: usize = 16 * 1024;

/// MCP client for communicating with the Mattermost MCP binary.
pub struct MattermostMcpClient {
    _service: RunningService<RoleClient, ()>,
    peer: Peer<RoleClient>,
    binary_path: String,
    child_pid: Option<u32>,
    stderr_tail: Arc<Mutex<String>>,
    supported_tools: HashSet<String>,
    _stderr_task: tokio::task::JoinHandle<()>,
}

impl MattermostMcpClient {
    /// Creates a new MCP client connected to the mattermost MCP binary.
    pub async fn new(config: &MattermostMcpConfig) -> Result<Self> {
        let cmd = Self::configured_command(config);

        let (transport, stderr) = TokioChildProcess::builder(cmd)
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "failed to spawn mattermost-mcp binary at {}",
                    config.binary_path
                )
            })?;

        let child_pid = transport.id();
        let stderr_tail = Arc::new(Mutex::new(String::new()));

        let service = ().serve(transport).await.with_context(|| {
            format!(
                "failed to initialize mattermost-mcp service (binary={}, pid={:?})",
                config.binary_path, child_pid
            )
        })?;

        let peer = service.peer().clone();

        tracing::info!(
            binary_path = %config.binary_path,
            child_pid = child_pid,
            "mattermost-mcp child process started"
        );

        let stderr_task =
            Self::spawn_stderr_task(stderr, Arc::clone(&stderr_tail), child_pid, config);

        let supported_tools =
            Self::preflight(&peer, &config.binary_path, child_pid, &stderr_tail).await?;

        Ok(Self {
            _service: service,
            peer,
            binary_path: config.binary_path.clone(),
            child_pid,
            stderr_tail,
            supported_tools,
            _stderr_task: stderr_task,
        })
    }

    fn configured_command(config: &MattermostMcpConfig) -> Command {
        let cmd = Command::new(&config.binary_path);
        cmd.configure(|cmd| {
            cmd.env("MATTERMOST_URL", &config.mattermost_url)
                .env("MATTERMOST_TOKEN", &config.mattermost_token)
                .env("MATTERMOST_TIMEOUT", config.timeout_secs.to_string())
                .env("MATTERMOST_MAX_RETRIES", config.max_retries.to_string())
                .env(
                    "MATTERMOST_VERIFY_SSL",
                    if config.verify_ssl { "true" } else { "false" },
                );
        })
    }

    fn spawn_stderr_task(
        stderr: Option<tokio::process::ChildStderr>,
        stderr_tail: Arc<Mutex<String>>,
        child_pid: Option<u32>,
        config: &MattermostMcpConfig,
    ) -> tokio::task::JoinHandle<()> {
        let binary_path = config.binary_path.clone();
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            if let Some(mut stderr) = stderr {
                let mut buf = vec![0_u8; 4096];
                loop {
                    match stderr.read(&mut buf).await {
                        Ok(0) => {
                            log_stderr_close(&stderr_tail, &binary_path, child_pid).await;
                            break;
                        }
                        Ok(n) => {
                            append_stderr_chunk(&stderr_tail, &binary_path, child_pid, &buf[..n])
                                .await;
                        }
                        Err(error) => {
                            tracing::warn!(
                                binary_path = %binary_path,
                                child_pid = child_pid,
                                %error,
                                "failed to read mattermost-mcp stderr"
                            );
                            break;
                        }
                    }
                }
            }
        })
    }

    pub fn supports_tool(&self, tool_name: &str) -> bool {
        self.supported_tools.contains(tool_name)
    }

    async fn preflight(
        peer: &Peer<RoleClient>,
        binary_path: &str,
        child_pid: Option<u32>,
        stderr_tail: &Arc<Mutex<String>>,
    ) -> Result<HashSet<String>> {
        let tools = peer.list_all_tools().await.map_err(|error| {
            let stderr_tail = stderr_tail_snapshot_blocking(stderr_tail, error.to_string());
            tracing::error!(
                binary_path,
                child_pid = child_pid,
                stderr_tail = %stderr_tail,
                error = %error,
                "mattermost-mcp preflight tools/list failed"
            );
            anyhow::anyhow!(
                "mattermost-mcp preflight tools/list failed (binary={}, pid={:?}, stderr_tail={})",
                binary_path,
                child_pid,
                stderr_tail
            )
        })?;

        let supported_tools = tools
            .iter()
            .map(|tool| tool.name.to_string())
            .collect::<HashSet<_>>();

        tracing::info!(
            binary_path,
            child_pid = child_pid,
            tool_count = supported_tools.len(),
            tool_names = ?supported_tools,
            "mattermost-mcp preflight succeeded"
        );

        Ok(supported_tools)
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
            let stderr_tail = stderr_tail_snapshot_blocking(&self.stderr_tail, error.to_string());
            tracing::error!(
                binary_path = %self.binary_path,
                child_pid = self.child_pid,
                tool_name,
                stderr_tail = %stderr_tail,
                error = %error,
                "mattermost-mcp call_tool failed"
            );
            anyhow::anyhow!(
                "mattermost-mcp call_tool failed (binary={}, pid={:?}, tool={}, stderr_tail={})",
                self.binary_path,
                self.child_pid,
                tool_name,
                stderr_tail
            )
        })?;

        let text: String = result
            .content
            .iter()
            .filter_map(|content| content.raw.as_text().map(|text| text.text.clone()))
            .collect::<Vec<_>>()
            .join("\n");

        if result.is_error.unwrap_or(false) {
            let stderr_tail = self.stderr_tail_snapshot().await;
            anyhow::bail!(
                "mattermost tool '{}' failed: {} (binary={}, pid={:?}, stderr_tail={})",
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

fn stderr_tail_snapshot_blocking(stderr_tail: &Arc<Mutex<String>>, fallback: String) -> String {
    match stderr_tail.try_lock() {
        Ok(guard) if !guard.trim().is_empty() => guard.clone(),
        _ => fallback,
    }
}

async fn log_stderr_close(
    stderr_tail: &Arc<Mutex<String>>,
    binary_path: &str,
    child_pid: Option<u32>,
) {
    let stderr_tail = {
        let guard = stderr_tail.lock().await;
        guard.clone()
    };
    if stderr_tail.trim().is_empty() {
        tracing::debug!(
            binary_path = %binary_path,
            child_pid = child_pid,
            "mattermost-mcp stderr stream closed"
        );
    } else {
        tracing::warn!(
            binary_path = %binary_path,
            child_pid = child_pid,
            stderr_tail = %stderr_tail,
            "mattermost-mcp stderr stream closed after emitting diagnostics"
        );
    }
}

async fn append_stderr_chunk(
    stderr_tail: &Arc<Mutex<String>>,
    binary_path: &str,
    child_pid: Option<u32>,
    chunk: &[u8],
) {
    let chunk = String::from_utf8_lossy(chunk);
    tracing::warn!(
        binary_path = %binary_path,
        child_pid = child_pid,
        stderr_chunk = %chunk.trim_end(),
        "mattermost-mcp emitted stderr"
    );

    let mut guard = stderr_tail.lock().await;
    guard.push_str(&chunk);
    if guard.len() > STDERR_TAIL_MAX_BYTES {
        let excess = guard.len() - STDERR_TAIL_MAX_BYTES;
        guard.drain(..excess);
    }
}
