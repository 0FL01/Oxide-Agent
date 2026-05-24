//! Sandbox Provider - executes tools in Docker sandbox
//!
//! Provides `execute_command`, `read_file`, `write_file`, `list_files`, and
//! `recreate_sandbox` tools.

use crate::agent::progress::AgentEvent;
use crate::agent::provider::ToolProvider;
use crate::agent::tool_runtime::{
    CleanupStatus, OutputNormalizer, OutputPreview, ToolExecutor, ToolInvocation, ToolName,
    ToolOutput, ToolRuntimeConfig, ToolRuntimeError,
};
use crate::llm::ToolDefinition;
use crate::sandbox::{ExecResult, SandboxManager, SandboxScope};
use anyhow::Result;
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use shell_escape::escape;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tracing::debug;

const SANDBOX_EXEC_TOOL_NAMES: &[&str] = &["execute_command"];
const SANDBOX_FILEOPS_TOOL_NAMES: &[&str] = &["write_file", "read_file", "list_files"];
const SANDBOX_LIFECYCLE_TOOL_NAMES: &[&str] = &["recreate_sandbox"];

/// Shared runtime state used by sandbox tool capability slices.
pub struct SandboxRuntime {
    sandbox: Arc<Mutex<Option<SandboxManager>>>,
    execution_gate: Arc<RwLock<()>>,
    sandbox_scope: SandboxScope,
    progress_tx: Option<Sender<AgentEvent>>,
}

impl SandboxRuntime {
    /// Create a new sandbox runtime (sandbox is lazily initialized).
    #[must_use]
    pub fn new(sandbox_scope: impl Into<SandboxScope>) -> Self {
        Self {
            sandbox: Arc::new(Mutex::new(None)),
            execution_gate: Arc::new(RwLock::new(())),
            sandbox_scope: sandbox_scope.into(),
            progress_tx: None,
        }
    }

    /// Set the progress channel for sending events (like file transfers).
    #[must_use]
    pub fn with_progress_tx(mut self, tx: Sender<AgentEvent>) -> Self {
        self.progress_tx = Some(tx);
        self
    }

    /// Set the sandbox manager (for when sandbox is created externally).
    pub async fn set_sandbox(&self, sandbox: SandboxManager) {
        let mut guard = self.sandbox.lock().await;
        *guard = Some(sandbox);
    }

    pub(crate) async fn get_or_create_sandbox(&self) -> Result<SandboxManager> {
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

    /// Acquires the shared sandbox execution guard used by non-lifecycle tools.
    pub(crate) async fn shared_execution_guard(&self) -> tokio::sync::RwLockReadGuard<'_, ()> {
        self.execution_gate.read().await
    }

    /// Returns the progress channel associated with this runtime, if any.
    #[must_use]
    pub(crate) fn progress_tx(&self) -> Option<&Sender<AgentEvent>> {
        self.progress_tx.as_ref()
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
}

struct SandboxToolHandlers;

impl SandboxToolHandlers {
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

    async fn execute_typed_sandbox_tool(
        runtime: &SandboxRuntime,
        invocation: &ToolInvocation,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        debug!(tool = %invocation.tool_name, "Executing typed sandbox tool");

        match invocation.tool_name.as_str() {
            "recreate_sandbox" => {
                let _exclusive = runtime.execution_gate.write().await;
                let mut sandbox = runtime
                    .get_or_init_sandbox_manager()
                    .await
                    .map_err(tool_failure)?;
                let _args: RecreateSandboxArgs = parse_invocation_args(invocation)?;
                let result = sandbox.recreate().await;
                runtime.set_sandbox(sandbox).await;
                Ok(typed_simple_result(
                    invocation,
                    result.map(|()| {
                        json!({
                            "ok": true,
                            "status": "recreated",
                            "message": "Sandbox recreated successfully. Previous workspace contents were removed.",
                        })
                    }),
                ))
            }
            "execute_command" => {
                let _shared = runtime.execution_gate.read().await;
                let args: ExecuteCommandArgs = parse_invocation_args(invocation)?;
                let mut sandbox = runtime
                    .get_or_create_sandbox()
                    .await
                    .map_err(tool_failure)?;
                let result = sandbox
                    .exec_command(&args.command, Some(&invocation.cancellation_token))
                    .await;
                Ok(match result {
                    Ok(result) => typed_exec_result(invocation, &args.command, result),
                    Err(error) => typed_simple_result::<Value>(
                        invocation,
                        Err(anyhow::anyhow!("sandbox command failed: {error}")),
                    ),
                })
            }
            "write_file" => {
                let _shared = runtime.execution_gate.read().await;
                let args: WriteFileArgs = parse_invocation_args(invocation)?;
                let mut sandbox = runtime
                    .get_or_create_sandbox()
                    .await
                    .map_err(tool_failure)?;
                let result = sandbox
                    .write_file(&args.path, args.content.as_bytes())
                    .await;
                Ok(typed_simple_result(
                    invocation,
                    result.map(|()| {
                        json!({
                            "ok": true,
                            "path": args.path,
                            "bytes_written": args.content.len(),
                        })
                    }),
                ))
            }
            "read_file" => {
                let _shared = runtime.execution_gate.read().await;
                let args: ReadFileArgs = parse_invocation_args(invocation)?;
                let mut sandbox = runtime
                    .get_or_create_sandbox()
                    .await
                    .map_err(tool_failure)?;
                let result = sandbox.read_file(&args.path).await;
                Ok(match result {
                    Ok(content) => typed_read_file_result(invocation, &args.path, &content),
                    Err(error) => typed_simple_result::<Value>(
                        invocation,
                        Err(anyhow::anyhow!("sandbox read_file failed: {error}")),
                    ),
                })
            }
            "list_files" => {
                let _shared = runtime.execution_gate.read().await;
                let args: ListFilesRuntimeArgs = parse_invocation_args(invocation)?;
                let mut sandbox = runtime
                    .get_or_create_sandbox()
                    .await
                    .map_err(tool_failure)?;
                let cmd = format!(
                    "tree -L 3 -h --du {} 2>/dev/null || find {} -type f -o -type d | head -100",
                    escape(args.path.as_str().into()),
                    escape(args.path.as_str().into())
                );
                let result = sandbox.exec_command(&cmd, None).await;
                Ok(match result {
                    Ok(result) => typed_list_files_result(invocation, &args.path, result),
                    Err(error) => typed_simple_result::<Value>(
                        invocation,
                        Err(anyhow::anyhow!("sandbox list_files failed: {error}")),
                    ),
                })
            }
            other => Err(ToolRuntimeError::Internal(format!(
                "typed sandbox executor received unknown tool: {other}"
            ))),
        }
    }

    async fn execute_legacy_sandbox_tool(
        runtime: &SandboxRuntime,
        tool_name: &str,
        arguments: &str,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        debug!(tool = tool_name, "Executing sandbox tool");

        match tool_name {
            "recreate_sandbox" => {
                let _exclusive = runtime.execution_gate.write().await;
                let mut sandbox = runtime.get_or_init_sandbox_manager().await?;
                let result = Self::handle_recreate_sandbox(&mut sandbox, arguments).await;
                runtime.set_sandbox(sandbox).await;
                result
            }
            "execute_command" => {
                let _shared = runtime.execution_gate.read().await;
                let mut sandbox = runtime.get_or_create_sandbox().await?;
                Self::handle_execute_command(&mut sandbox, arguments, cancellation_token).await
            }
            "write_file" => {
                let _shared = runtime.execution_gate.read().await;
                let mut sandbox = runtime.get_or_create_sandbox().await?;
                Self::handle_write_file(&mut sandbox, arguments).await
            }
            "read_file" => {
                let _shared = runtime.execution_gate.read().await;
                let mut sandbox = runtime.get_or_create_sandbox().await?;
                Self::handle_read_file(&mut sandbox, arguments).await
            }
            "list_files" => {
                let _shared = runtime.execution_gate.read().await;
                let mut sandbox = runtime.get_or_create_sandbox().await?;
                Self::handle_list_files(&mut sandbox, arguments).await
            }
            _ => anyhow::bail!("Unknown sandbox tool: {tool_name}"),
        }
    }
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct ListFilesRuntimeArgs {
    #[serde(default = "default_workspace_path")]
    path: String,
}

fn default_workspace_path() -> String {
    "/workspace".to_string()
}

fn parse_invocation_args<T>(invocation: &ToolInvocation) -> std::result::Result<T, ToolRuntimeError>
where
    T: DeserializeOwned,
{
    match serde_json::from_value(invocation.normalized_arguments.clone()) {
        Ok(args) => Ok(args),
        Err(value_error) => {
            serde_json::from_str(&invocation.raw_arguments).map_err(|string_error| {
                ToolRuntimeError::InvalidArguments(format!(
                "invalid sandbox arguments: {value_error}; raw JSON parse error: {string_error}"
            ))
            })
        }
    }
}

fn tool_failure(error: anyhow::Error) -> ToolRuntimeError {
    ToolRuntimeError::Failure(error.to_string())
}

fn typed_exec_result(invocation: &ToolInvocation, command: &str, result: ExecResult) -> ToolOutput {
    let normalizer = sandbox_normalizer(invocation);
    let mut output = if result.success() {
        normalizer.success(invocation, &result.stdout, &result.stderr)
    } else {
        normalizer
            .failure(
                invocation,
                format!("sandbox command exited with code {}", result.exit_code),
            )
            .with_streams(
                normalizer.stdout_preview(&result.stdout),
                normalizer.stderr_preview(&result.stderr),
            )
    };
    output.exit_code = Some(i32::try_from(result.exit_code).unwrap_or(-1));
    output.cleanup_status = CleanupStatus::NotNeeded;
    output.structured_payload = Some(json!({
        "ok": result.success(),
        "command": command,
        "exit_code": result.exit_code,
    }));
    output
}

fn typed_list_files_result(
    invocation: &ToolInvocation,
    path: &str,
    result: ExecResult,
) -> ToolOutput {
    let mut output = typed_exec_result(invocation, "list_files", result);
    if let Some(payload) = output.structured_payload.as_mut() {
        payload["path"] = json!(path);
    }
    output
}

fn typed_read_file_result(invocation: &ToolInvocation, path: &str, content: &[u8]) -> ToolOutput {
    let normalizer = sandbox_normalizer(invocation);
    let binary = bytes_look_binary(content);
    let text = String::from_utf8_lossy(content);
    let mut output = normalizer.success(invocation, "", "");
    output.cleanup_status = CleanupStatus::NotNeeded;
    output.structured_payload = Some(json!({
        "ok": true,
        "path": path,
        "bytes": content.len(),
        "binary": binary,
    }));
    output.stdout = if binary {
        OutputPreview {
            text: None,
            bytes_captured: 0,
            bytes_total_known: Some(content.len()),
            truncated: false,
            binary: true,
            ..OutputPreview::default()
        }
    } else {
        normalizer.stdout_preview(&text)
    };
    output.truncation.stdout_truncated = output.stdout.truncated;
    output
}

fn typed_simple_result<T>(invocation: &ToolInvocation, result: Result<T>) -> ToolOutput
where
    T: Into<Value>,
{
    let normalizer = sandbox_normalizer(invocation);
    match result {
        Ok(payload) => {
            let mut output = normalizer.success(invocation, "", "");
            output.structured_payload = Some(payload.into());
            output.cleanup_status = CleanupStatus::NotNeeded;
            output
        }
        Err(error) => normalizer
            .failure(invocation, error.to_string())
            .with_cleanup_status(CleanupStatus::NotNeeded),
    }
}

fn sandbox_normalizer(invocation: &ToolInvocation) -> OutputNormalizer {
    let config = ToolRuntimeConfig {
        timeout: invocation.timeout.clone(),
        artifact_dir: invocation.execution_context.artifact_dir.clone(),
        ..ToolRuntimeConfig::default()
    };
    OutputNormalizer::new(config)
}

fn bytes_look_binary(bytes: &[u8]) -> bool {
    let inspected = &bytes[..bytes.len().min(65_536)];
    inspected.contains(&0)
}

fn sandbox_tool_runtime_executors(
    runtime: Arc<SandboxRuntime>,
    specs: Vec<ToolDefinition>,
) -> Vec<Arc<dyn ToolExecutor>> {
    specs
        .into_iter()
        .map(|spec| {
            Arc::new(SandboxToolExecutor {
                runtime: Arc::clone(&runtime),
                name: ToolName::from(spec.name.clone()),
                spec,
            }) as Arc<dyn ToolExecutor>
        })
        .collect()
}

fn sandbox_tool_definitions() -> Vec<ToolDefinition> {
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
            name: "list_files".to_string(),
            description: "List files in the sandbox workspace. Returns JSON with ok, path, listing, and exit_code. Useful for finding file paths before file operations.".to_string(),
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

fn sandbox_tool_definitions_for(tool_names: &[&str]) -> Vec<ToolDefinition> {
    sandbox_tool_definitions()
        .into_iter()
        .filter(|definition| tool_names.contains(&definition.name.as_str()))
        .collect()
}

struct SandboxToolExecutor {
    runtime: Arc<SandboxRuntime>,
    name: ToolName,
    spec: ToolDefinition,
}

#[async_trait]
impl ToolExecutor for SandboxToolExecutor {
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
        SandboxToolHandlers::execute_typed_sandbox_tool(&self.runtime, &invocation).await
    }
}

fn serialize_json(value: serde_json::Value) -> Result<String> {
    serde_json::to_string(&value).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::identity::SessionId;
    use crate::agent::tool_runtime::{
        ModelMetadata, ProviderMetadata, ToolBatchId, ToolCallId, ToolExecutionContext,
        ToolOutputStatus, ToolTimeoutConfig, TurnId,
    };
    use crate::llm::InvocationId;
    use chrono::Utc;
    use serde_json::Value;
    use std::path::PathBuf;

    #[test]
    fn recreate_sandbox_is_registered() {
        let provider = SandboxLifecycleProvider::new(Arc::new(SandboxRuntime::new(1)));
        let tools = provider.tools();

        assert!(tools.iter().any(|tool| tool.name == "recreate_sandbox"));
        assert!(provider.can_handle("recreate_sandbox"));
    }

    #[test]
    fn execute_command_tool_description_mentions_json_response() {
        let provider = SandboxExecProvider::new(Arc::new(SandboxRuntime::new(1)));
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

    #[test]
    fn runtime_executors_cover_sandbox_tool_specs() {
        let runtime = Arc::new(SandboxRuntime::new(1));
        let exec_provider = Arc::new(SandboxExecProvider::new(Arc::clone(&runtime)));
        let fileops_provider = Arc::new(SandboxFileOpsProvider::new(Arc::clone(&runtime)));
        let lifecycle_provider = Arc::new(SandboxLifecycleProvider::new(runtime));

        let executor_names: Vec<String> = exec_provider
            .tool_runtime_executors()
            .into_iter()
            .chain(fileops_provider.tool_runtime_executors())
            .chain(lifecycle_provider.tool_runtime_executors())
            .map(|executor| executor.name().into_inner())
            .collect();

        assert_eq!(
            executor_names,
            vec![
                "execute_command",
                "write_file",
                "read_file",
                "list_files",
                "recreate_sandbox"
            ]
        );
    }

    #[test]
    fn narrow_sandbox_providers_expose_disjoint_tool_slices() {
        let runtime = Arc::new(SandboxRuntime::new(1));
        let exec_provider = Arc::new(SandboxExecProvider::new(Arc::clone(&runtime)));
        let fileops_provider = Arc::new(SandboxFileOpsProvider::new(Arc::clone(&runtime)));
        let fileops_without_delivery_provider = Arc::new(SandboxFileOpsProvider::without_delivery(
            Arc::clone(&runtime),
        ));
        let lifecycle_provider = Arc::new(SandboxLifecycleProvider::new(runtime));

        let exec_tools: Vec<_> = exec_provider
            .tools()
            .into_iter()
            .map(|tool| tool.name)
            .collect();
        let fileops_tools: Vec<_> = fileops_provider
            .tools()
            .into_iter()
            .map(|tool| tool.name)
            .collect();
        let lifecycle_tools: Vec<_> = lifecycle_provider
            .tools()
            .into_iter()
            .map(|tool| tool.name)
            .collect();
        let fileops_without_delivery_tools: Vec<_> = fileops_without_delivery_provider
            .tools()
            .into_iter()
            .map(|tool| tool.name)
            .collect();

        assert_eq!(exec_tools, ["execute_command"]);
        assert_eq!(fileops_tools, ["write_file", "read_file", "list_files"]);
        assert_eq!(
            fileops_without_delivery_tools,
            ["write_file", "read_file", "list_files"]
        );
        assert_eq!(lifecycle_tools, ["recreate_sandbox"]);
    }

    #[test]
    fn typed_exec_result_preserves_nonzero_exit_and_streams() {
        let invocation = test_invocation("execute_command");
        let output = typed_exec_result(
            &invocation,
            "false",
            ExecResult {
                stdout: "out".to_string(),
                stderr: "err".to_string(),
                exit_code: 7,
            },
        );

        assert_eq!(output.status, ToolOutputStatus::Failure);
        assert_eq!(output.exit_code, Some(7));
        assert_eq!(output.stdout.text.as_deref(), Some("out"));
        assert_eq!(output.stderr.text.as_deref(), Some("err"));
        assert_eq!(output.cleanup_status, CleanupStatus::NotNeeded);
    }

    #[test]
    fn typed_read_file_result_marks_binary_output_without_inlining() {
        let invocation = test_invocation("read_file");
        let output = typed_read_file_result(&invocation, "/workspace/blob.bin", b"abc\0def");

        assert_eq!(output.status, ToolOutputStatus::Success);
        assert!(output.stdout.binary);
        assert!(output.stdout.text.is_none());
        assert_eq!(
            output
                .structured_payload
                .as_ref()
                .and_then(|payload| payload.get("binary"))
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    fn test_invocation(tool_name: &str) -> ToolInvocation {
        let now = Utc::now();
        ToolInvocation {
            session_id: SessionId::from(42),
            turn_id: TurnId::from("turn_sandbox"),
            batch_id: ToolBatchId::from("batch_sandbox"),
            batch_index: 0,
            invocation_id: InvocationId::from(format!("invocation_{tool_name}")),
            tool_call_id: ToolCallId::from(format!("call_{tool_name}")),
            provider_tool_call_id: None,
            tool_name: ToolName::from(tool_name),
            raw_provider_payload: json!({}),
            raw_arguments: "{}".to_string(),
            normalized_arguments: json!({}),
            cancellation_token: tokio_util::sync::CancellationToken::new(),
            timeout: ToolTimeoutConfig::default(),
            execution_context: ToolExecutionContext::new(PathBuf::from(".oxide/tool-artifacts")),
            provider_metadata: ProviderMetadata {
                provider: "opencode-go".to_string(),
                protocol: "chat_like".to_string(),
            },
            model_metadata: ModelMetadata {
                model: "deepseek-v4-flash".to_string(),
            },
            working_directory: None,
            environment_metadata: None,
            created_at: now,
            started_at: Some(now),
        }
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

/// Arguments for `recreate_sandbox` tool
#[derive(Debug, Default, Deserialize)]
struct RecreateSandboxArgs {}

/// Provider for sandbox command execution tools.
pub struct SandboxExecProvider {
    runtime: Arc<SandboxRuntime>,
}

impl SandboxExecProvider {
    /// Create an exec-only provider backed by shared sandbox runtime state.
    #[must_use]
    pub fn new(runtime: Arc<SandboxRuntime>) -> Self {
        Self { runtime }
    }

    /// Build typed runtime executors for exec tools.
    #[must_use]
    pub fn tool_runtime_executors(self: &Arc<Self>) -> Vec<Arc<dyn ToolExecutor>> {
        sandbox_tool_runtime_executors(
            Arc::clone(&self.runtime),
            sandbox_tool_definitions_for(SANDBOX_EXEC_TOOL_NAMES),
        )
    }
}

#[async_trait]
impl ToolProvider for SandboxExecProvider {
    fn name(&self) -> &'static str {
        "sandbox_exec"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        sandbox_tool_definitions_for(SANDBOX_EXEC_TOOL_NAMES)
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        SANDBOX_EXEC_TOOL_NAMES.contains(&tool_name)
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        progress_tx: Option<&tokio::sync::mpsc::Sender<crate::agent::progress::AgentEvent>>,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        if !self.can_handle(tool_name) {
            anyhow::bail!("Unknown sandbox exec tool: {tool_name}");
        }
        let _ = progress_tx;
        SandboxToolHandlers::execute_legacy_sandbox_tool(
            &self.runtime,
            tool_name,
            arguments,
            cancellation_token,
        )
        .await
    }
}

/// Provider for sandbox file operation tools.
pub struct SandboxFileOpsProvider {
    runtime: Arc<SandboxRuntime>,
    tool_names: &'static [&'static str],
}

impl SandboxFileOpsProvider {
    /// Create a fileops-only provider backed by shared sandbox runtime state.
    #[must_use]
    pub fn new(runtime: Arc<SandboxRuntime>) -> Self {
        Self {
            runtime,
            tool_names: SANDBOX_FILEOPS_TOOL_NAMES,
        }
    }

    /// Create a fileops provider without chat/file-delivery tools.
    #[must_use]
    pub fn without_delivery(runtime: Arc<SandboxRuntime>) -> Self {
        Self::new(runtime)
    }

    /// Build typed runtime executors for file operation tools.
    #[must_use]
    pub fn tool_runtime_executors(self: &Arc<Self>) -> Vec<Arc<dyn ToolExecutor>> {
        sandbox_tool_runtime_executors(
            Arc::clone(&self.runtime),
            sandbox_tool_definitions_for(self.tool_names),
        )
    }
}

#[async_trait]
impl ToolProvider for SandboxFileOpsProvider {
    fn name(&self) -> &'static str {
        "sandbox_fileops"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        sandbox_tool_definitions_for(self.tool_names)
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        self.tool_names.contains(&tool_name)
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        progress_tx: Option<&tokio::sync::mpsc::Sender<crate::agent::progress::AgentEvent>>,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        if !self.can_handle(tool_name) {
            anyhow::bail!("Unknown sandbox fileops tool: {tool_name}");
        }
        let _ = progress_tx;
        SandboxToolHandlers::execute_legacy_sandbox_tool(
            &self.runtime,
            tool_name,
            arguments,
            cancellation_token,
        )
        .await
    }
}

/// Provider for sandbox lifecycle tools.
pub struct SandboxLifecycleProvider {
    runtime: Arc<SandboxRuntime>,
}

impl SandboxLifecycleProvider {
    /// Create a lifecycle-only provider backed by shared sandbox runtime state.
    #[must_use]
    pub fn new(runtime: Arc<SandboxRuntime>) -> Self {
        Self { runtime }
    }

    /// Build typed runtime executors for lifecycle tools.
    #[must_use]
    pub fn tool_runtime_executors(self: &Arc<Self>) -> Vec<Arc<dyn ToolExecutor>> {
        sandbox_tool_runtime_executors(
            Arc::clone(&self.runtime),
            sandbox_tool_definitions_for(SANDBOX_LIFECYCLE_TOOL_NAMES),
        )
    }
}

#[async_trait]
impl ToolProvider for SandboxLifecycleProvider {
    fn name(&self) -> &'static str {
        "sandbox_lifecycle"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        sandbox_tool_definitions_for(SANDBOX_LIFECYCLE_TOOL_NAMES)
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        SANDBOX_LIFECYCLE_TOOL_NAMES.contains(&tool_name)
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        progress_tx: Option<&tokio::sync::mpsc::Sender<crate::agent::progress::AgentEvent>>,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        if !self.can_handle(tool_name) {
            anyhow::bail!("Unknown sandbox lifecycle tool: {tool_name}");
        }
        let _ = progress_tx;
        SandboxToolHandlers::execute_legacy_sandbox_tool(
            &self.runtime,
            tool_name,
            arguments,
            cancellation_token,
        )
        .await
    }
}
