//! Sandbox Provider - executes tools in Docker sandbox
//!
//! Provides `execute_command`, `read_file`, `write_file`, `send_file_to_user`,
//! `list_files`, and `recreate_sandbox` tools.

use crate::agent::progress::AgentEvent;
use crate::agent::progress::FileDeliveryKind;
use crate::agent::provider::ToolProvider;
use crate::llm::ToolDefinition;
use crate::sandbox::{SandboxManager, SandboxScope};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use shell_escape::escape;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use super::file_delivery::{
    deliver_file_via_progress, format_generic_delivery_report, FileDeliveryRequest,
    CHAT_DELIVERY_MAX_FILE_SIZE_BYTES,
};
use super::path::resolve_file_path;

/// Provider for Docker sandbox tools
pub struct SandboxProvider {
    sandbox: Arc<Mutex<Option<SandboxManager>>>,
    execution_gate: Arc<RwLock<()>>,
    sandbox_scope: SandboxScope,
    progress_tx: Option<Sender<AgentEvent>>,
}

impl SandboxProvider {
    /// Create a new sandbox provider (sandbox is lazily initialized)
    #[must_use]
    pub fn new(sandbox_scope: impl Into<SandboxScope>) -> Self {
        Self {
            sandbox: Arc::new(Mutex::new(None)),
            execution_gate: Arc::new(RwLock::new(())),
            sandbox_scope: sandbox_scope.into(),
            progress_tx: None,
        }
    }

    /// Set the progress channel for sending events (like file transfers)
    #[must_use]
    pub fn with_progress_tx(mut self, tx: Sender<AgentEvent>) -> Self {
        self.progress_tx = Some(tx);
        self
    }

    /// Set the sandbox manager (for when sandbox is created externally)
    pub async fn set_sandbox(&self, sandbox: SandboxManager) {
        let mut guard = self.sandbox.lock().await;
        *guard = Some(sandbox);
    }

    async fn get_or_create_sandbox(&self) -> Result<SandboxManager> {
        let mut guard = self.sandbox.lock().await;

        if guard.as_ref().is_none_or(|sandbox| !sandbox.is_running()) {
            debug!(scope = %self.sandbox_scope.namespace(), "Creating new sandbox for provider");
            let mut sandbox = SandboxManager::new(self.sandbox_scope.clone()).await?;
            sandbox.create_sandbox().await?;
            *guard = Some(sandbox);
        }

        guard
            .as_ref()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Sandbox not initialized"))
    }

    async fn get_or_init_sandbox_manager(&self) -> Result<SandboxManager> {
        let mut guard = self.sandbox.lock().await;

        if guard.is_none() {
            debug!(
                scope = %self.sandbox_scope.namespace(),
                "Initializing sandbox manager for provider"
            );
            *guard = Some(SandboxManager::new(self.sandbox_scope.clone()).await?);
        }

        guard
            .as_ref()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Sandbox not initialized"))
    }

    async fn handle_execute_command(
        sandbox: &mut SandboxManager,
        arguments: &str,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        let args: ExecuteCommandArgs = serde_json::from_str(arguments)?;

        // Pass cancellation_token to exec_command
        match sandbox
            .exec_command(&args.command, cancellation_token)
            .await
        {
            Ok(result) => {
                if result.success() {
                    if result.stdout.is_empty() {
                        Ok("(command executed successfully, output is empty)".to_string())
                    } else {
                        Ok(result.stdout)
                    }
                } else {
                    Ok(format!(
                        "Command failed (exit code {}): {}",
                        result.exit_code,
                        result.combined_output()
                    ))
                }
            }
            Err(e) => Ok(format!("Command execution failed: {e}")),
        }
    }

    async fn handle_write_file(sandbox: &mut SandboxManager, arguments: &str) -> Result<String> {
        let args: WriteFileArgs = serde_json::from_str(arguments)?;
        match sandbox
            .write_file(&args.path, args.content.as_bytes())
            .await
        {
            Ok(()) => Ok(format!("File {} successfully written", args.path)),
            Err(e) => Ok(format!("Error writing file: {e}")),
        }
    }

    async fn handle_read_file(sandbox: &mut SandboxManager, arguments: &str) -> Result<String> {
        let args: ReadFileArgs = serde_json::from_str(arguments)?;
        match sandbox.read_file(&args.path).await {
            Ok(content) => Ok(String::from_utf8_lossy(&content).to_string()),
            Err(e) => Ok(format!("Error reading file: {e}")),
        }
    }

    async fn handle_send_file(
        progress_tx: Option<&Sender<AgentEvent>>,
        sandbox: &mut SandboxManager,
        arguments: &str,
    ) -> Result<String> {
        let args: SendFileArgs = serde_json::from_str(arguments)?;
        info!(path = %args.path, "send_file_to_user called");

        let resolved_path = match resolve_file_path(sandbox, &args.path).await {
            Ok(p) => p,
            Err(e) => {
                warn!(path = %args.path, error = %e, "Failed to resolve file path");
                return Ok(format!("❌ {e}"));
            }
        };

        let file_name = std::path::Path::new(&resolved_path)
            .file_name()
            .map_or_else(|| "file".to_string(), |n| n.to_string_lossy().to_string());

        let file_size = match sandbox.file_size_bytes(&resolved_path, None).await {
            Ok(size) => size,
            Err(e) => {
                error!(resolved_path = %resolved_path, error = %e, "Failed to check file size");
                return Ok(format!("❌ Error checking file size: {e}"));
            }
        };

        if file_size == 0 {
            return Ok(format!(
                "❌ ERROR: File '{file_name}' is empty (0 bytes) and cannot be sent.\n\
                 Source path: {resolved_path}"
            ));
        }

        if file_size > CHAT_DELIVERY_MAX_FILE_SIZE_BYTES {
            return Ok(
                "⚠️ ERROR: File too large for chat delivery (>50 MB). Please use the upload_file tool to upload it to the cloud."
                    .to_string(),
            );
        }

        match sandbox.download_file(&resolved_path).await {
            Ok(content) => {
                let report = deliver_file_via_progress(
                    progress_tx,
                    FileDeliveryRequest {
                        kind: FileDeliveryKind::Auto,
                        file_name: file_name.clone(),
                        content,
                        source_path: resolved_path.clone(),
                    },
                )
                .await;
                Ok(format_generic_delivery_report(&report))
            }
            Err(e) => {
                error!(path = %args.path, resolved_path = %resolved_path, error = %e, "Failed to download file");
                Ok(format!("❌ Error downloading file: {e}"))
            }
        }
    }

    async fn handle_list_files(sandbox: &mut SandboxManager, arguments: &str) -> Result<String> {
        #[derive(Debug, Deserialize)]
        struct ListFilesArgs {
            #[serde(default = "default_workspace_path")]
            path: String,
        }

        fn default_workspace_path() -> String {
            "/workspace".to_string()
        }

        let args: ListFilesArgs = serde_json::from_str(arguments)?;
        let cmd = format!(
            "tree -L 3 -h --du {} 2>/dev/null || find {} -type f -o -type d | head -100",
            escape(args.path.as_str().into()),
            escape(args.path.as_str().into())
        );

        match sandbox.exec_command(&cmd, None).await {
            Ok(result) => {
                if result.success() {
                    if result.stdout.is_empty() {
                        Ok(format!(
                            "Directory '{}' is empty or does not exist",
                            args.path
                        ))
                    } else {
                        Ok(format!(
                            "📁 Directory '{}':\n\n```\n{}\n```",
                            args.path, result.stdout
                        ))
                    }
                } else {
                    Ok(format!("❌ Error reading directory: {}", result.stderr))
                }
            }
            Err(e) => Ok(format!("❌ Error executing command: {e}")),
        }
    }

    async fn handle_recreate_sandbox(
        sandbox: &mut SandboxManager,
        arguments: &str,
    ) -> Result<String> {
        let _: RecreateSandboxArgs = if arguments.trim().is_empty() {
            RecreateSandboxArgs::default()
        } else {
            serde_json::from_str(arguments)?
        };

        match sandbox.recreate().await {
            Ok(()) => Ok(
                "Sandbox recreated successfully. Previous workspace contents were removed."
                    .to_string(),
            ),
            Err(e) => Ok(format!("Error recreating sandbox: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recreate_sandbox_is_registered() {
        let provider = SandboxProvider::new(1);
        let tools = provider.tools();

        assert!(tools.iter().any(|tool| tool.name == "recreate_sandbox"));
        assert!(provider.can_handle("recreate_sandbox"));
    }
}

/// Arguments for `execute_command` tool
#[derive(Debug, Deserialize)]
struct ExecuteCommandArgs {
    command: String,
}

/// Arguments for `write_file` tool
#[derive(Debug, Deserialize)]
struct WriteFileArgs {
    path: String,
    content: String,
}

/// Arguments for `read_file` tool
#[derive(Debug, Deserialize)]
struct ReadFileArgs {
    path: String,
}

/// Arguments for `send_file_to_user` tool
#[derive(Debug, Deserialize)]
struct SendFileArgs {
    path: String,
}

/// Arguments for `recreate_sandbox` tool
#[derive(Debug, Default, Deserialize)]
struct RecreateSandboxArgs {}

#[async_trait]
impl ToolProvider for SandboxProvider {
    fn name(&self) -> &'static str {
        "sandbox"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "execute_command".to_string(),
                description: "Execute a bash command in the isolated sandbox environment. Available commands include: python3, pip, ffmpeg, yt-dlp, curl, wget, date, cat, ls, grep, and other standard Unix tools.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "The bash command to execute"
                        }
                    },
                    "required": ["command"]
                }),
            },
            ToolDefinition {
                name: "write_file".to_string(),
                description: "Write content to a file in the sandbox. Creates parent directories if needed.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the file (relative to /workspace or absolute)"
                        },
                        "content": {
                            "type": "string",
                            "description": "Content to write to the file"
                        }
                    },
                    "required": ["path", "content"]
                }),
            },
            ToolDefinition {
                name: "read_file".to_string(),
                description: "Read content from a file in the sandbox.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the file to read"
                        }
                    },
                    "required": ["path"]
                }),
            },
            ToolDefinition {
                name: "send_file_to_user".to_string(),
                description: "Send a file from the sandbox to the user via the chat transport. Use this when you need to deliver generated files, images, documents, or any output to the user. Supports both absolute paths (/workspace/file.txt) and relative paths (file.txt) - will automatically search in /workspace if not found.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the file in the sandbox to send to the user (relative or absolute)"
                        }
                    },
                    "required": ["path"]
                }),
            },
            ToolDefinition {
                name: "list_files".to_string(),
                description: "List files in the sandbox workspace. Returns a tree-like structure of files and directories. Useful for finding file paths before using send_file_to_user.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Optional path to list (defaults to /workspace)"
                        }
                    }
                }),
            },
            ToolDefinition {
                name: "recreate_sandbox".to_string(),
                description: "Recreate the sandbox container from scratch, wiping all previous workspace contents. Use this when the sandbox state is corrupted or you need a completely clean environment.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {}
                }),
            },
        ]
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        matches!(
            tool_name,
            "execute_command"
                | "read_file"
                | "write_file"
                | "send_file_to_user"
                | "list_files"
                | "recreate_sandbox"
        )
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        _progress_tx: Option<&tokio::sync::mpsc::Sender<crate::agent::progress::AgentEvent>>,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        debug!(tool = tool_name, "Executing sandbox tool");

        let progress_tx = self.progress_tx.clone();
        match tool_name {
            "recreate_sandbox" => {
                let _exclusive = self.execution_gate.write().await;
                let mut sandbox = self.get_or_init_sandbox_manager().await?;
                let result = Self::handle_recreate_sandbox(&mut sandbox, arguments).await;
                self.set_sandbox(sandbox).await;
                result
            }
            "execute_command" => {
                let _shared = self.execution_gate.read().await;
                let mut sandbox = self.get_or_create_sandbox().await?;
                Self::handle_execute_command(&mut sandbox, arguments, cancellation_token).await
            }
            "write_file" => {
                let _shared = self.execution_gate.read().await;
                let mut sandbox = self.get_or_create_sandbox().await?;
                Self::handle_write_file(&mut sandbox, arguments).await
            }
            "read_file" => {
                let _shared = self.execution_gate.read().await;
                let mut sandbox = self.get_or_create_sandbox().await?;
                Self::handle_read_file(&mut sandbox, arguments).await
            }
            "send_file_to_user" => {
                let _shared = self.execution_gate.read().await;
                let mut sandbox = self.get_or_create_sandbox().await?;
                Self::handle_send_file(progress_tx.as_ref(), &mut sandbox, arguments).await
            }
            "list_files" => {
                let _shared = self.execution_gate.read().await;
                let mut sandbox = self.get_or_create_sandbox().await?;
                Self::handle_list_files(&mut sandbox, arguments).await
            }
            _ => anyhow::bail!("Unknown sandbox tool: {tool_name}"),
        }
    }
}
