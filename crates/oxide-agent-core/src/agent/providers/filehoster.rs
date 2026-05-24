//! File delivery provider - sends sandbox files to chat or external hosting.
//!
//! Currently supports chat delivery and GoFile for files that are too large
//! for chat delivery.

use crate::agent::progress::{AgentEvent, FileDeliveryKind};
use crate::agent::provider::ToolProvider;
use crate::agent::tool_runtime::{
    CleanupStatus, OutputNormalizer, ToolExecutor, ToolInvocation, ToolName, ToolOutput,
    ToolRuntimeConfig, ToolRuntimeError,
};
use crate::llm::ToolDefinition;
use crate::sandbox::{SandboxManager, SandboxScope};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use shell_escape::escape;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tracing::{debug, error, info, warn};

use super::file_delivery::{
    deliver_file_via_progress, format_generic_delivery_report, FileDeliveryReport,
    FileDeliveryRequest, FileDeliveryStatus, CHAT_DELIVERY_MAX_FILE_SIZE_BYTES,
};
use super::path::resolve_file_path;
use super::sandbox::SandboxRuntime;

const MAX_UPLOAD_SIZE_BYTES: u64 = 4 * 1024 * 1024 * 1024; // 4 GiB (safety limit)
const GOFILE_UPLOAD_URL: &str = "https://upload.gofile.io/uploadfile";
const GOFILE_DOWNLOAD_PAGE_PREFIX: &str = "https://gofile.io/d/";

const FILE_DELIVERY_TOOL_NAMES: &[&str] = &["send_file_to_user", "upload_file"];

/// Provider for file delivery tools backed by shared sandbox runtime state.
pub struct FileHosterProvider {
    runtime: Arc<SandboxRuntime>,
}

impl FileHosterProvider {
    /// Create a new `FileHosterProvider` (sandbox is lazily initialized).
    #[must_use]
    pub fn new(sandbox_scope: impl Into<SandboxScope>) -> Self {
        Self::from_runtime(Arc::new(SandboxRuntime::new(sandbox_scope)))
    }

    /// Create a file-delivery provider from shared sandbox runtime state.
    #[must_use]
    pub fn from_runtime(runtime: Arc<SandboxRuntime>) -> Self {
        Self { runtime }
    }

    /// Build typed runtime executors for file delivery tools.
    #[must_use]
    pub fn tool_runtime_executors(
        self: &Arc<Self>,
        progress_tx: Option<Sender<AgentEvent>>,
    ) -> Vec<Arc<dyn ToolExecutor>> {
        file_delivery_tool_definitions()
            .into_iter()
            .map(|spec| {
                Arc::new(FileDeliveryToolExecutor {
                    provider: Arc::clone(self),
                    name: ToolName::from(spec.name.clone()),
                    spec,
                    progress_tx: progress_tx.clone(),
                }) as Arc<dyn ToolExecutor>
            })
            .collect()
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

    async fn handle_upload_file(
        &self,
        sandbox: &mut SandboxManager,
        arguments: &str,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        let args: UploadFileArgs = serde_json::from_str(arguments)?;
        info!(path = %args.path, "upload_file called");

        let resolved_path = match resolve_file_path(sandbox, &args.path).await {
            Ok(p) => p,
            Err(e) => {
                warn!(path = %args.path, error = %e, "Failed to resolve file path");
                return Ok(format!("❌ {e}"));
            }
        };

        let file_size = match sandbox
            .file_size_bytes(&resolved_path, cancellation_token)
            .await
        {
            Ok(size) => size,
            Err(e) => {
                error!(resolved_path = %resolved_path, error = %e, "Failed to check file size");
                return Ok(format!("❌ File size check error: {e}"));
            }
        };

        if file_size > MAX_UPLOAD_SIZE_BYTES {
            return Ok("⛔ FATAL ERROR: File exceeds upload limit (4 GB). Upload impossible. Immediately inform the user that the task cannot be completed.".to_string());
        }

        let token_opt = std::env::var("GOFILE_TOKEN").ok().filter(|t| !t.is_empty());
        let token_part = token_opt.as_deref().map_or(String::new(), |token| {
            format!(" -F {}", escape(format!("token={token}").into()))
        });

        let cmd = format!(
            "curl -sS --fail-with-body --retry 3 --retry-all-errors --retry-delay 2 --retry-max-time 60 \
             -F {file}{token_part} {url}",
            file = escape(format!("file=@{resolved_path}").into()),
            token_part = token_part,
            url = escape(GOFILE_UPLOAD_URL.into()),
        );

        let result = match sandbox.exec_command(&cmd, cancellation_token).await {
            Ok(r) => r,
            Err(e) => return Ok(format!("❌ GoFile upload error: {e}")),
        };

        if !result.success() {
            return Ok(format!(
                "❌ GoFile upload error (code {}): {}",
                result.exit_code,
                result.combined_output()
            ));
        }

        let resp: GoFileUploadResponse = match serde_json::from_str(result.stdout.trim()) {
            Ok(r) => r,
            Err(e) => {
                return Ok(format!(
                    "❌ GoFile returned unexpected response (not JSON): {e}\n{}",
                    result.combined_output()
                ));
            }
        };

        let download_page = match resp.into_download_page() {
            Ok(url) => url,
            Err(msg) => {
                return Ok(format!(
                    "❌ GoFile returned error:\n{msg}\n{}",
                    result.combined_output()
                ));
            }
        };

        if !download_page.starts_with(GOFILE_DOWNLOAD_PAGE_PREFIX) {
            return Ok(format!(
                "❌ GoFile returned unexpected response instead of link:\n{}",
                result.combined_output()
            ));
        }

        let rm_cmd = format!("rm -f {}", escape(resolved_path.as_str().into()));
        match sandbox.exec_command(&rm_cmd, cancellation_token).await {
            Ok(rm_res) if rm_res.success() => {}
            Ok(rm_res) => warn!(
                resolved_path = %resolved_path,
                output = %rm_res.combined_output(),
                "Failed to remove uploaded file from sandbox"
            ),
            Err(e) => {
                warn!(resolved_path = %resolved_path, error = %e, "Failed to remove uploaded file from sandbox")
            }
        }

        Ok(download_page)
    }
}

/// Arguments for `upload_file` tool
#[derive(Debug, Deserialize)]
struct UploadFileArgs {
    path: String,
}

/// Arguments for `send_file_to_user` tool.
#[derive(Debug, Deserialize)]
struct SendFileArgs {
    path: String,
}

#[derive(Debug, Deserialize)]
struct GoFileUploadResponse {
    status: String,
    #[serde(default)]
    data: Option<GoFileUploadData>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GoFileUploadData {
    #[serde(rename = "downloadPage")]
    download_page: Option<String>,
}

impl GoFileUploadResponse {
    fn into_download_page(self) -> std::result::Result<String, String> {
        if self.status == "ok" {
            let url = self
                .data
                .and_then(|d| d.download_page)
                .filter(|u| !u.trim().is_empty());
            return url.ok_or_else(|| "GoFile: missing downloadPage in response".to_string());
        }

        let msg = self
            .error
            .or(self.message)
            .unwrap_or_else(|| "GoFile: unknown error".to_string());
        Err(msg)
    }
}

struct FileDeliveryToolExecutor {
    provider: Arc<FileHosterProvider>,
    name: ToolName,
    spec: ToolDefinition,
    progress_tx: Option<Sender<AgentEvent>>,
}

#[async_trait]
impl ToolExecutor for FileDeliveryToolExecutor {
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
        let output = self
            .provider
            .execute(
                self.name.as_str(),
                &invocation.raw_arguments,
                self.progress_tx.as_ref(),
                Some(&invocation.cancellation_token),
            )
            .await
            .map_err(|error| ToolRuntimeError::Failure(error.to_string()))?;

        if self.name.as_str() == "send_file_to_user" {
            return typed_json_string_result(&invocation, &output);
        }

        let normalizer = file_delivery_normalizer(&invocation);
        let mut tool_output = normalizer.success(&invocation, &output, "");
        tool_output.cleanup_status = CleanupStatus::NotNeeded;
        Ok(tool_output)
    }
}

fn file_delivery_tool_definitions() -> Vec<ToolDefinition> {
    vec![
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
            name: "upload_file".to_string(),
            description: "Upload a file from the sandbox to external hosting (GoFile). Use this for files too large for chat delivery (>50 MB). Returns a public link on success.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file in the sandbox (relative or absolute)"
                    }
                },
                "required": ["path"]
            }),
        },
    ]
}

fn file_delivery_normalizer(invocation: &ToolInvocation) -> OutputNormalizer {
    OutputNormalizer::new(ToolRuntimeConfig {
        timeout: invocation.timeout.clone(),
        artifact_dir: invocation.execution_context.artifact_dir.clone(),
        ..ToolRuntimeConfig::default()
    })
}

fn typed_json_string_result(
    invocation: &ToolInvocation,
    json_string: &str,
) -> std::result::Result<ToolOutput, ToolRuntimeError> {
    match serde_json::from_str::<Value>(json_string) {
        Ok(value) => {
            let normalizer = file_delivery_normalizer(invocation);
            let mut output = normalizer.success(invocation, "", "");
            output.structured_payload = Some(value);
            output.cleanup_status = CleanupStatus::NotNeeded;
            Ok(output)
        }
        Err(error) => Err(ToolRuntimeError::Failure(format!(
            "file delivery returned invalid JSON: {error}"
        ))),
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

#[async_trait]
impl ToolProvider for FileHosterProvider {
    fn name(&self) -> &'static str {
        "filehoster"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        file_delivery_tool_definitions()
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        FILE_DELIVERY_TOOL_NAMES.contains(&tool_name)
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        progress_tx: Option<&Sender<AgentEvent>>,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        debug!(tool = tool_name, "Executing filehoster tool");

        if !self.can_handle(tool_name) {
            anyhow::bail!("Unknown file delivery tool: {tool_name}");
        }

        let _shared = self.runtime.shared_execution_guard().await;
        let mut sandbox = self.runtime.get_or_create_sandbox().await?;

        match tool_name {
            "send_file_to_user" => {
                Self::handle_send_file(
                    self.runtime.progress_tx().or(progress_tx),
                    &mut sandbox,
                    arguments,
                )
                .await
            }
            "upload_file" => {
                self.handle_upload_file(&mut sandbox, arguments, cancellation_token)
                    .await
            }
            _ => unreachable!("validated file delivery tool name"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn provider_exposes_file_delivery_tools() {
        let provider = FileHosterProvider::new(1);
        let tool_names: Vec<_> = provider.tools().into_iter().map(|tool| tool.name).collect();

        assert_eq!(tool_names, ["send_file_to_user", "upload_file"]);
        assert!(provider.can_handle("send_file_to_user"));
        assert!(provider.can_handle("upload_file"));
        assert!(!provider.can_handle("read_file"));
    }

    #[test]
    fn runtime_executors_cover_file_delivery_tool_specs() {
        let provider = Arc::new(FileHosterProvider::new(1));
        let executor_names: Vec<String> = provider
            .tool_runtime_executors(None)
            .into_iter()
            .map(|executor| executor.name().into_inner())
            .collect();

        assert_eq!(executor_names, ["send_file_to_user", "upload_file"]);
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
        assert_eq!(
            payload["status"],
            Value::String("delivery_failed".to_string())
        );
        assert_eq!(
            payload["error"],
            Value::String("upload refused".to_string())
        );
        assert_eq!(
            payload["file_name"],
            Value::String("report.txt".to_string())
        );
    }
}
