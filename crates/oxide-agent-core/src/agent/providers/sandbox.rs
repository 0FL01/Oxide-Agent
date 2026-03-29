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
    FileDeliveryReport, FileDeliveryStatus, CHAT_DELIVERY_MAX_FILE_SIZE_BYTES,
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
            Ok(result) => serialize_json(json!({
                "ok": result.success(),
                "command": args.command,
                "stdout": result.stdout,
                "stderr": result.stderr,
                "exit_code": result.exit_code,
            })),
            Err(error) => serialize_json(json!({
                "ok": false,
                "command": args.command,
                "error": error.to_string(),
            })),
        }
    }

    async fn handle_write_file(sandbox: &mut SandboxManager, arguments: &str) -> Result<String> {
        let args: WriteFileArgs = serde_json::from_str(arguments)?;
        match sandbox
            .write_file(&args.path, args.content.as_bytes())
            .await
        {
            Ok(()) => serialize_json(json!({
                "ok": true,
                "path": args.path,
                "bytes_written": args.content.len(),
            })),
            Err(error) => serialize_json(json!({
                "ok": false,
                "path": args.path,
                "error": error.to_string(),
            })),
        }
    }

    async fn handle_read_file(sandbox: &mut SandboxManager, arguments: &str) -> Result<String> {
        let args: ReadFileArgs = serde_json::from_str(arguments)?;
        match sandbox.read_file(&args.path).await {
            Ok(content) => serialize_json(json!({
                "ok": true,
                "path": args.path,
                "content": String::from_utf8_lossy(&content).to_string(),
            })),
            Err(error) => serialize_json(json!({
                "ok": false,
                "path": args.path,
                "error": error.to_string(),
            })),
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
                return serialize_json(json!({
                    "ok": false,
                    "path": args.path,
                    "status": "resolve_failed",
                    "error": e.to_string(),
                }));
            }
        };

        let file_name = std::path::Path::new(&resolved_path)
            .file_name()
            .map_or_else(|| "file".to_string(), |n| n.to_string_lossy().to_string());

        let file_size = match sandbox.file_size_bytes(&resolved_path, None).await {
            Ok(size) => size,
            Err(e) => {
                error!(resolved_path = %resolved_path, error = %e, "Failed to check file size");
                return serialize_json(json!({
                    "ok": false,
                    "path": resolved_path,
                    "status": "size_check_failed",
                    "error": e.to_string(),
                }));
            }
        };

        if file_size == 0 {
            return serialize_json(json!({
                "ok": false,
                "path": resolved_path,
                "file_name": file_name,
                "size_bytes": file_size,
                "status": "empty_content",
                "message": format!(
                    "❌ ERROR: File '{file_name}' is empty (0 bytes) and cannot be sent.\nSource path: {resolved_path}"
                ),
            }));
        }

        if file_size > CHAT_DELIVERY_MAX_FILE_SIZE_BYTES {
            return serialize_json(json!({
                "ok": false,
                "path": resolved_path,
                "file_name": file_name,
                "size_bytes": file_size,
                "status": "too_large",
                "message": "⚠️ ERROR: File too large for chat delivery (>50 MB). Please use the upload_file tool to upload it to the cloud.",
            }));
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
                serialize_json(build_send_file_response(&resolved_path, &report))
            }
            Err(e) => {
                error!(path = %args.path, resolved_path = %resolved_path, error = %e, "Failed to download file");
                serialize_json(json!({
                    "ok": false,
                    "path": resolved_path,
                    "file_name": file_name,
                    "status": "download_failed",
                    "error": e.to_string(),
                }))
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
                    serialize_json(json!({
                        "ok": true,
                        "path": args.path,
                        "listing": result.stdout,
                        "stderr": result.stderr,
                        "exit_code": result.exit_code,
                        "is_empty": result.stdout.is_empty(),
                    }))
                } else {
                    serialize_json(json!({
                        "ok": false,
                        "path": args.path,
                        "listing": result.stdout,
                        "stderr": result.stderr,
                        "exit_code": result.exit_code,
                    }))
                }
            }
            Err(error) => serialize_json(json!({
                "ok": false,
                "path": args.path,
                "error": error.to_string(),
            })),
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
            Ok(()) => serialize_json(json!({
                "ok": true,
                "status": "recreated",
                "message": "Sandbox recreated successfully. Previous workspace contents were removed.",
            })),
            Err(error) => serialize_json(json!({
                "ok": false,
                "status": "recreate_failed",
                "error": error.to_string(),
            })),
        }
    }
}

fn serialize_json(value: serde_json::Value) -> Result<String> {
    serde_json::to_string(&value).map_err(Into::into)
}

fn file_delivery_status_code(status: &FileDeliveryStatus) -> &'static str {
    match status {
        FileDeliveryStatus::Delivered => "delivered",
        FileDeliveryStatus::DeliveryFailed(_) => "delivery_failed",
        FileDeliveryStatus::ConfirmationChannelClosed => "confirmation_channel_closed",
        FileDeliveryStatus::TimedOut => "timed_out",
        FileDeliveryStatus::QueueUnavailable(_) => "queue_unavailable",
        FileDeliveryStatus::EmptyContent => "empty_content",
    }
}

fn build_send_file_response(path: &str, report: &FileDeliveryReport) -> serde_json::Value {
    let mut payload = json!({
        "ok": matches!(report.status, FileDeliveryStatus::Delivered),
        "status": file_delivery_status_code(&report.status),
        "path": path,
        "file_name": report.file_name,
        "size_bytes": report.size_bytes,
        "message": format_generic_delivery_report(report),
    });

    if let Some(object) = payload.as_object_mut() {
        match &report.status {
            FileDeliveryStatus::DeliveryFailed(error)
            | FileDeliveryStatus::QueueUnavailable(error) => {
                object.insert("error".to_string(), json!(error));
            }
            FileDeliveryStatus::ConfirmationChannelClosed
            | FileDeliveryStatus::TimedOut
            | FileDeliveryStatus::Delivered
            | FileDeliveryStatus::EmptyContent => {}
        }
    }

    payload
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn recreate_sandbox_is_registered() {
        let provider = SandboxProvider::new(1);
        let tools = provider.tools();

        assert!(tools.iter().any(|tool| tool.name == "recreate_sandbox"));
        assert!(provider.can_handle("recreate_sandbox"));
    }

    #[test]
    fn execute_command_tool_description_mentions_json_response() {
        let provider = SandboxProvider::new(1);
        let tools = provider.tools();
        let execute_command = tools
            .iter()
            .find(|tool| tool.name == "execute_command")
            .expect("execute_command registered");

        assert!(execute_command.description.contains("JSON"));
        assert!(execute_command.description.contains("stdout"));
        assert!(execute_command.description.contains("exit_code"));
    }

    #[test]
    fn build_send_file_response_serializes_delivery_status() {
        let payload = build_send_file_response(
            "/workspace/report.txt",
            &FileDeliveryReport {
                file_name: "report.txt".to_string(),
                source_path: "/workspace/report.txt".to_string(),
                size_bytes: 12,
                status: FileDeliveryStatus::DeliveryFailed("upload refused".to_string()),
            },
        );

        assert_eq!(payload["ok"], Value::Bool(false));
        assert_eq!(payload["status"], Value::String("delivery_failed".to_string()));
        assert_eq!(payload["error"], Value::String("upload refused".to_string()));
        assert_eq!(payload["file_name"], Value::String("report.txt".to_string()));
    }

    #[test]
    fn serialize_json_preserves_command_fields() {
        let payload = serialize_json(json!({
            "ok": true,
            "command": "pwd",
            "stdout": "/workspace\n",
            "stderr": "",
            "exit_code": 0,
        }))
        .expect("json serialization succeeds");
        let parsed: Value = serde_json::from_str(&payload).expect("valid json payload");

        assert_eq!(parsed["ok"], Value::Bool(true));
        assert_eq!(parsed["command"], Value::String("pwd".to_string()));
        assert_eq!(parsed["exit_code"], Value::Number(0.into()));
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
                description: "Execute a bash command in the isolated sandbox environment. Returns JSON with ok, stdout, stderr, and exit_code. Available commands include: python3, pip, ffmpeg, yt-dlp, curl, wget, date, cat, ls, grep, and other standard Unix tools.".to_string(),
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
                description: "Write content to a file in the sandbox. Creates parent directories if needed. Returns JSON with ok, path, and bytes_written or error.".to_string(),
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
                description: "Read content from a file in the sandbox. Returns JSON with ok, path, and content or error.".to_string(),
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
                description: "Send a file from the sandbox to the user via the chat transport. Returns JSON with ok, status, file_name, size_bytes, and message. Supports both absolute paths (/workspace/file.txt) and relative paths (file.txt) - will automatically search in /workspace if not found.".to_string(),
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
                description: "List files in the sandbox workspace. Returns JSON with ok, path, listing, and exit_code. Useful for finding file paths before using send_file_to_user.".to_string(),
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
                description: "Recreate the sandbox container from scratch, wiping all previous workspace contents. Returns JSON with ok, status, and message or error.".to_string(),
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
