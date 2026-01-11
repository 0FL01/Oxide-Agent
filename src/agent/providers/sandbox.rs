//! Sandbox Provider - executes tools in Docker sandbox
//!
//! Provides `execute_command`, `read_file`, `write_file`, `send_file_to_user` tools.

use crate::agent::progress::AgentEvent;
use crate::agent::provider::ToolProvider;
use crate::llm::ToolDefinition;
use crate::sandbox::SandboxManager;
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use shell_escape::escape;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use super::path::resolve_file_path;

const TELEGRAM_MAX_FILE_SIZE_BYTES: u64 = 50 * 1024 * 1024;
const TELEGRAM_DELIVERY_CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(120);

/// Provider for Docker sandbox tools
pub struct SandboxProvider {
    sandbox: Arc<Mutex<Option<SandboxManager>>>,
    user_id: i64,
    progress_tx: Option<Sender<AgentEvent>>,
}

struct FileDeliveryRequest {
    file_name: String,
    content: Vec<u8>,
    sandbox_path: String,
}

impl SandboxProvider {
    /// Create a new sandbox provider (sandbox is lazily initialized)
    #[must_use]
    pub fn new(user_id: i64) -> Self {
        Self {
            sandbox: Arc::new(Mutex::new(None)),
            user_id,
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

    /// Get or create the sandbox
    async fn ensure_sandbox(&self) -> Result<()> {
        if self
            .sandbox
            .lock()
            .await
            .as_ref()
            .is_some_and(SandboxManager::is_running)
        {
            return Ok(());
        }

        debug!(user_id = self.user_id, "Creating new sandbox for provider");
        let mut sandbox = SandboxManager::new(self.user_id).await?;
        sandbox.create_sandbox().await?;

        *self.sandbox.lock().await = Some(sandbox);
        Ok(())
    }

    async fn deliver_file_via_telegram(&self, request: FileDeliveryRequest) -> String {
        let FileDeliveryRequest {
            file_name,
            content,
            sandbox_path,
        } = request;

        if content.is_empty() {
            return format!(
                "❌ ERROR: File '{file_name}' is empty (0 bytes) and cannot be sent to Telegram.\n\
                 Path in sandbox: {sandbox_path}"
            );
        }

        let size_mb = content.len() as f64 / 1024.0 / 1024.0;

        let Some(ref tx) = self.progress_tx else {
            warn!(file_name = %file_name, "Progress channel not available");
            return format!(
                "⚠️ File '{file_name}' read ({size_mb:.2} MB), but send channel is not available.\n\
                 Path in sandbox: {sandbox_path}"
            );
        };

        let (confirm_tx, confirm_rx) = tokio::sync::oneshot::channel();
        if let Err(e) = tx
            .send(AgentEvent::FileToSendWithConfirmation {
                file_name: file_name.clone(),
                content,
                sandbox_path: sandbox_path.clone(),
                confirmation_tx: confirm_tx,
            })
            .await
        {
            warn!(file_name = %file_name, error = %e, "Failed to send FileToSendWithConfirmation event");
            return format!(
                "⚠️ File '{file_name}' read ({size_mb:.2} MB), but failed to send to Telegram: {e}\n\
                 Path in sandbox: {sandbox_path}"
            );
        }

        match tokio::time::timeout(TELEGRAM_DELIVERY_CONFIRMATION_TIMEOUT, confirm_rx).await {
            Ok(Ok(Ok(()))) => {
                info!(file_name = %file_name, sandbox_path = %sandbox_path, "File delivered successfully");
                format!("✅ File '{file_name}' delivered to user")
            }
            Ok(Ok(Err(e))) => {
                warn!(file_name = %file_name, error = %e, "File delivery failed");
                format!(
                    "❌ Failed to send file '{file_name}' to user through Telegram: {e}\n\
                     Path in sandbox: {sandbox_path}"
                )
            }
            Ok(Err(_)) => {
                warn!(file_name = %file_name, "Confirmation channel closed unexpectedly");
                format!(
                    "⚠️ Status of file '{file_name}' delivery unknown (confirmation channel closed).\n\
                     Path in sandbox: {sandbox_path}"
                )
            }
            Err(_) => {
                warn!(file_name = %file_name, "File delivery confirmation timeout");
                format!(
                    "⚠️ File '{file_name}' delivery confirmation timeout (2 minutes).\n\
                     Path in sandbox: {sandbox_path}"
                )
            }
        }
    }

    async fn handle_execute_command(
        sandbox: &SandboxManager,
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

    async fn handle_write_file(sandbox: &SandboxManager, arguments: &str) -> Result<String> {
        let args: WriteFileArgs = serde_json::from_str(arguments)?;
        match sandbox
            .write_file(&args.path, args.content.as_bytes())
            .await
        {
            Ok(()) => Ok(format!("File {} successfully written", args.path)),
            Err(e) => Ok(format!("Error writing file: {e}")),
        }
    }

    async fn handle_read_file(sandbox: &SandboxManager, arguments: &str) -> Result<String> {
        let args: ReadFileArgs = serde_json::from_str(arguments)?;
        match sandbox.read_file(&args.path).await {
            Ok(content) => Ok(String::from_utf8_lossy(&content).to_string()),
            Err(e) => Ok(format!("Error reading file: {e}")),
        }
    }

    async fn handle_send_file(&self, sandbox: &SandboxManager, arguments: &str) -> Result<String> {
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
                "❌ ERROR: File '{file_name}' is empty (0 bytes) and cannot be sent to Telegram.\n\
                 Path in sandbox: {resolved_path}"
            ));
        }

        if file_size > TELEGRAM_MAX_FILE_SIZE_BYTES {
            return Ok(
                "⚠️ ERROR: File too large for Telegram (>50 MB). Please use the upload_file tool to upload it to the cloud."
                    .to_string(),
            );
        }

        match sandbox.download_file(&resolved_path).await {
            Ok(content) => {
                let message = self
                    .deliver_file_via_telegram(FileDeliveryRequest {
                        file_name: file_name.clone(),
                        content,
                        sandbox_path: resolved_path.clone(),
                    })
                    .await;
                Ok(message)
            }
            Err(e) => {
                error!(path = %args.path, resolved_path = %resolved_path, error = %e, "Failed to download file");
                Ok(format!("❌ Error downloading file: {e}"))
            }
        }
    }

    async fn handle_list_files(sandbox: &SandboxManager, arguments: &str) -> Result<String> {
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
                    Ok(format!(
                        "❌ Error reading directory: {}",
                        result.stderr
                    ))
                }
            }
            Err(e) => Ok(format!("❌ Error executing command: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn deliver_file_returns_success_only_after_confirmation() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(1);
        let provider = SandboxProvider::new(1).with_progress_tx(tx);

        tokio::spawn(async move {
            if let Some(AgentEvent::FileToSendWithConfirmation {
                confirmation_tx, ..
            }) = rx.recv().await
            {
                let _ = confirmation_tx.send(Ok(()));
            }
        });

        let result = provider
            .deliver_file_via_telegram(FileDeliveryRequest {
                file_name: "ok.txt".to_string(),
                content: b"hello".to_vec(),
                sandbox_path: "/workspace/ok.txt".to_string(),
            })
            .await;

        assert!(result.starts_with("✅"), "unexpected result: {result}");
    }

    #[tokio::test]
    async fn deliver_file_propagates_telegram_error_to_agent() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(1);
        let provider = SandboxProvider::new(1).with_progress_tx(tx);

        tokio::spawn(async move {
            if let Some(AgentEvent::FileToSendWithConfirmation {
                confirmation_tx, ..
            }) = rx.recv().await
            {
                let _ =
                    confirmation_tx.send(Err("Bad Request: file must be non-empty".to_string()));
            }
        });

        let result = provider
            .deliver_file_via_telegram(FileDeliveryRequest {
                file_name: "empty.txt".to_string(),
                content: b"x".to_vec(),
                sandbox_path: "/workspace/empty.txt".to_string(),
            })
            .await;

        assert!(result.starts_with("❌"), "unexpected result: {result}");
        assert!(
            result.contains("Bad Request: file must be non-empty"),
            "missing telegram error in: {result}"
        );
    }

    #[tokio::test]
    async fn deliver_file_fails_when_queue_is_unavailable() {
        let (tx, rx) = tokio::sync::mpsc::channel::<AgentEvent>(1);
        drop(rx);

        let provider = SandboxProvider::new(1).with_progress_tx(tx);
        let result = provider
            .deliver_file_via_telegram(FileDeliveryRequest {
                file_name: "file.txt".to_string(),
                content: b"hello".to_vec(),
                sandbox_path: "/workspace/file.txt".to_string(),
            })
            .await;

        assert!(!result.starts_with("✅"), "unexpected result: {result}");
        assert!(result.starts_with("⚠️"), "unexpected result: {result}");
    }

    #[tokio::test]
    async fn deliver_file_rejects_empty_content() {
        let provider = SandboxProvider::new(1);
        let result = provider
            .deliver_file_via_telegram(FileDeliveryRequest {
                file_name: "empty.bin".to_string(),
                content: Vec::new(),
                sandbox_path: "/workspace/empty.bin".to_string(),
            })
            .await;

        assert!(result.starts_with("❌"), "unexpected result: {result}");
        assert!(result.contains("0 bytes"), "unexpected result: {result}");
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
                description: "Send a file from the sandbox to the user via Telegram. Use this when you need to deliver generated files, images, documents, or any output to the user. Supports both absolute paths (/workspace/file.txt) and relative paths (file.txt) - will automatically search in /workspace if not found.".to_string(),
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
        ]
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        matches!(
            tool_name,
            "execute_command" | "read_file" | "write_file" | "send_file_to_user" | "list_files"
        )
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        debug!(tool = tool_name, "Executing sandbox tool");

        // Ensure sandbox is running
        self.ensure_sandbox().await?;

        let sandbox = {
            let guard = self.sandbox.lock().await;
            guard
                .as_ref()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Sandbox not initialized"))?
        };

        match tool_name {
            "execute_command" => {
                Self::handle_execute_command(&sandbox, arguments, cancellation_token).await
            }
            "write_file" => Self::handle_write_file(&sandbox, arguments).await,
            "read_file" => Self::handle_read_file(&sandbox, arguments).await,
            "send_file_to_user" => self.handle_send_file(&sandbox, arguments).await,
            "list_files" => Self::handle_list_files(&sandbox, arguments).await,
            _ => anyhow::bail!("Unknown sandbox tool: {tool_name}"),
        }
    }
}
