//! Sandbox Provider - executes tools in Docker sandbox
//!
//! Provides `execute_command`, `read_file`, `write_file`, `apply_file_edit`,
//! `list_files`, and `recreate_sandbox` tools.

use crate::agent::progress::AgentEvent;
use crate::agent::tool_runtime::{
    CleanupStatus, OutputNormalizer, OutputPreview, ToolExecutor, ToolInvocation, ToolName,
    ToolOutput, ToolRuntimeConfig, ToolRuntimeError,
};
use crate::llm::ToolDefinition;
use crate::sandbox::{
    ExecResult, SandboxApplyFileEditResult, SandboxBackend, SandboxBackendId, SandboxCapability,
    SandboxEditReadGuard, SandboxExec, SandboxFileEdit, SandboxFileListing, SandboxFileOps,
    SandboxLifecycle, SandboxManager, SandboxScope,
};
use anyhow::Result;
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tracing::debug;

const SANDBOX_EXEC_TOOL_NAMES: &[&str] = &["execute_command"];
const SANDBOX_FILEOPS_TOOL_NAMES: &[&str] =
    &["write_file", "read_file", "apply_file_edit", "list_files"];
const SANDBOX_LIFECYCLE_TOOL_NAMES: &[&str] = &["recreate_sandbox"];
const SANDBOX_RUNTIME_BACKEND_ID: SandboxBackendId = SandboxBackendId::new("sandbox/runtime");
const SANDBOX_RUNTIME_CAPABILITIES: &[SandboxCapability] = &[
    SandboxCapability::FileOps,
    SandboxCapability::Exec,
    SandboxCapability::Lifecycle,
];

/// Shared runtime state used by sandbox tool capability slices.
pub struct SandboxRuntime {
    sandbox: Arc<Mutex<Option<SandboxManager>>>,
    execution_gate: Arc<RwLock<()>>,
    read_snapshots: Arc<Mutex<HashMap<String, ReadSnapshot>>>,
    sandbox_scope: SandboxScope,
    progress_tx: Option<Sender<AgentEvent>>,
}

#[derive(Debug, Clone)]
struct ReadSnapshot {
    sha256: String,
    bytes: usize,
}

impl SandboxRuntime {
    /// Create a new sandbox runtime (sandbox is lazily initialized).
    #[must_use]
    pub fn new(sandbox_scope: impl Into<SandboxScope>) -> Self {
        Self {
            sandbox: Arc::new(Mutex::new(None)),
            execution_gate: Arc::new(RwLock::new(())),
            read_snapshots: Arc::new(Mutex::new(HashMap::new())),
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
        self.read_snapshots.lock().await.clear();
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

impl SandboxBackend for SandboxRuntime {
    fn id(&self) -> SandboxBackendId {
        SANDBOX_RUNTIME_BACKEND_ID
    }

    fn capabilities(&self) -> &'static [SandboxCapability] {
        SANDBOX_RUNTIME_CAPABILITIES
    }
}

#[async_trait]
impl SandboxExec for SandboxRuntime {
    async fn exec(
        &self,
        command: &str,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<ExecResult> {
        let _shared = self.execution_gate.read().await;
        let mut sandbox = self.get_or_create_sandbox().await?;
        sandbox.exec_command(command, cancellation_token).await
    }
}

#[async_trait]
impl SandboxFileOps for SandboxRuntime {
    async fn write_file(&self, path: &str, bytes: &[u8]) -> Result<()> {
        let _shared = self.execution_gate.read().await;
        let mut sandbox = self.get_or_create_sandbox().await?;
        let result = sandbox.write_file(path, bytes).await;
        if result.is_ok() {
            self.read_snapshots.lock().await.remove(path);
        }
        result
    }

    async fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        let _shared = self.execution_gate.read().await;
        let mut sandbox = self.get_or_create_sandbox().await?;
        let bytes = sandbox.read_file(path).await?;
        self.read_snapshots.lock().await.insert(
            path.to_string(),
            ReadSnapshot {
                sha256: sha256_hex(&bytes),
                bytes: bytes.len(),
            },
        );
        Ok(bytes)
    }

    async fn file_size_bytes(
        &self,
        path: &str,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<u64> {
        let _shared = self.execution_gate.read().await;
        let mut sandbox = self.get_or_create_sandbox().await?;
        sandbox.file_size_bytes(path, cancellation_token).await
    }

    async fn list_files(&self, path: &str) -> Result<SandboxFileListing> {
        let _shared = self.execution_gate.read().await;
        let mut sandbox = self.get_or_create_sandbox().await?;
        sandbox.list_files(path).await
    }

    async fn apply_file_edit(
        &self,
        path: &str,
        edit: SandboxFileEdit,
    ) -> Result<SandboxApplyFileEditResult> {
        let _exclusive = self.execution_gate.write().await;
        let mut sandbox = self.get_or_create_sandbox().await?;
        let read_guard =
            self.read_snapshots
                .lock()
                .await
                .get(path)
                .map(|snapshot| SandboxEditReadGuard {
                    sha256: snapshot.sha256.clone(),
                    bytes: snapshot.bytes,
                });
        let result = sandbox.apply_file_edit(path, edit, read_guard).await?;

        self.read_snapshots.lock().await.insert(
            path.to_string(),
            ReadSnapshot {
                sha256: result.new_sha256.clone(),
                bytes: result.bytes_written,
            },
        );

        Ok(result)
    }
}

#[async_trait]
impl SandboxLifecycle for SandboxRuntime {
    async fn recreate(&self) -> Result<()> {
        let _exclusive = self.execution_gate.write().await;
        let mut sandbox = self.get_or_init_sandbox_manager().await?;
        let result = sandbox.recreate().await;
        self.set_sandbox(sandbox).await;
        self.read_snapshots.lock().await.clear();
        result
    }
}

struct SandboxToolHandlers;

impl SandboxToolHandlers {
    async fn execute_typed_exec_tool(
        exec: &dyn SandboxExec,
        invocation: &ToolInvocation,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        debug!(tool = %invocation.tool_name, "Executing typed sandbox tool");

        match invocation.tool_name.as_str() {
            "execute_command" => {
                let args: ExecuteCommandArgs = parse_invocation_args(invocation)?;
                let result = exec
                    .exec(&args.command, Some(&invocation.cancellation_token))
                    .await;
                Ok(match result {
                    Ok(result) => typed_exec_result(invocation, &args.command, result),
                    Err(error) => typed_simple_result::<Value>(
                        invocation,
                        Err(anyhow::anyhow!("sandbox command failed: {error}")),
                    ),
                })
            }
            other => Err(ToolRuntimeError::Internal(format!(
                "typed sandbox exec executor received unknown tool: {other}"
            ))),
        }
    }

    async fn execute_typed_fileops_tool(
        fileops: &dyn SandboxFileOps,
        invocation: &ToolInvocation,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        debug!(tool = %invocation.tool_name, "Executing typed sandbox tool");

        match invocation.tool_name.as_str() {
            "write_file" => {
                let args: WriteFileArgs = parse_invocation_args(invocation)?;
                let result = fileops
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
                let args: ReadFileArgs = parse_invocation_args(invocation)?;
                let result = fileops.read_file(&args.path).await;
                Ok(match result {
                    Ok(content) => typed_read_file_result(invocation, &args.path, &content),
                    Err(error) => typed_simple_result::<Value>(
                        invocation,
                        Err(anyhow::anyhow!("sandbox read_file failed: {error}")),
                    ),
                })
            }
            "apply_file_edit" => {
                let args: ApplyFileEditArgs = parse_invocation_args(invocation)?;
                let edit = args.to_edit();
                let path = args.path;
                let result = fileops.apply_file_edit(&path, edit).await;
                Ok(match result {
                    Ok(result) => typed_apply_file_edit_result(invocation, result),
                    Err(error) => typed_simple_result::<Value>(
                        invocation,
                        Err(anyhow::anyhow!("sandbox apply_file_edit failed: {error}")),
                    ),
                })
            }
            "list_files" => {
                let args: ListFilesArgs = parse_invocation_args(invocation)?;
                let result = fileops.list_files(&args.path).await;
                Ok(match result {
                    Ok(listing) => typed_list_files_result(invocation, listing),
                    Err(error) => typed_simple_result::<Value>(
                        invocation,
                        Err(anyhow::anyhow!("sandbox list_files failed: {error}")),
                    ),
                })
            }
            other => Err(ToolRuntimeError::Internal(format!(
                "typed sandbox fileops executor received unknown tool: {other}"
            ))),
        }
    }

    async fn execute_typed_lifecycle_tool(
        lifecycle: &dyn SandboxLifecycle,
        invocation: &ToolInvocation,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        debug!(tool = %invocation.tool_name, "Executing typed sandbox tool");

        match invocation.tool_name.as_str() {
            "recreate_sandbox" => {
                let _args: RecreateSandboxArgs = parse_invocation_args(invocation)?;
                let result = lifecycle.recreate().await;
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
            other => Err(ToolRuntimeError::Internal(format!(
                "typed sandbox lifecycle executor received unknown tool: {other}"
            ))),
        }
    }
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct ListFilesArgs {
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

fn typed_list_files_result(invocation: &ToolInvocation, listing: SandboxFileListing) -> ToolOutput {
    let normalizer = sandbox_normalizer(invocation);
    let mut output = if listing.success() {
        normalizer.success(invocation, &listing.listing, &listing.stderr)
    } else {
        normalizer
            .failure(
                invocation,
                format!("sandbox list_files exited with code {}", listing.exit_code),
            )
            .with_streams(
                normalizer.stdout_preview(&listing.listing),
                normalizer.stderr_preview(&listing.stderr),
            )
    };
    output.exit_code = Some(i32::try_from(listing.exit_code).unwrap_or(-1));
    output.cleanup_status = CleanupStatus::NotNeeded;
    output.structured_payload = Some(json!({
        "ok": listing.success(),
        "path": listing.path,
        "exit_code": listing.exit_code,
        "is_empty": listing.is_empty(),
    }));
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
        "sha256": sha256_hex(content),
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

fn typed_apply_file_edit_result(
    invocation: &ToolInvocation,
    result: SandboxApplyFileEditResult,
) -> ToolOutput {
    typed_simple_result(
        invocation,
        Ok(json!({
            "ok": true,
            "path": result.path,
            "status": result.status,
            "replacements": result.replacements,
            "previous_sha256": result.previous_sha256,
            "new_sha256": result.new_sha256,
            "bytes_before": result.bytes_before,
            "bytes_written": result.bytes_written,
            "changed": result.changed,
        })),
    )
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

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
fn validate_edit_read_guard(
    path: &str,
    current_sha256: &str,
    current_bytes: usize,
    snapshot: Option<&ReadSnapshot>,
) -> Result<()> {
    if current_bytes == 0 {
        return Ok(());
    }

    let snapshot = snapshot.ok_or_else(|| {
        anyhow::anyhow!(
            "file must be read with read_file before apply_file_edit; empty files are exempt: {path}"
        )
    })?;

    if snapshot.sha256 != current_sha256 {
        anyhow::bail!(
            "file changed after last read; call read_file again before editing: {path} (last_read_sha256={}, current_sha256={}, last_read_bytes={}, current_bytes={})",
            snapshot.sha256,
            current_sha256,
            snapshot.bytes,
            current_bytes
        );
    }

    Ok(())
}

#[cfg(test)]
fn apply_exact_text_edit(current: &[u8], edit: &SandboxFileEdit) -> Result<(Vec<u8>, usize)> {
    if bytes_look_binary(current) {
        anyhow::bail!("apply_file_edit only supports text files; binary content was detected");
    }

    let current_text = std::str::from_utf8(current)
        .map_err(|error| anyhow::anyhow!("apply_file_edit only supports UTF-8 text: {error}"))?;

    if edit.expected_replacements == 0 {
        anyhow::bail!("expected_replacements must be greater than zero");
    }

    if edit.search.is_empty() {
        if current.is_empty() {
            if edit.expected_replacements != 1 {
                anyhow::bail!(
                    "expected {} replacements, found 1 for empty-file insert; file was not modified",
                    edit.expected_replacements
                );
            }
            return Ok((edit.replace.as_bytes().to_vec(), 1));
        }
        anyhow::bail!("search must not be empty for non-empty files");
    }

    let replacements = current_text.matches(&edit.search).count();
    if replacements != edit.expected_replacements {
        anyhow::bail!(
            "expected {} replacements, found {}; file was not modified",
            edit.expected_replacements,
            replacements
        );
    }

    Ok((
        current_text
            .replace(&edit.search, &edit.replace)
            .into_bytes(),
        replacements,
    ))
}

#[derive(Clone)]
enum SandboxToolExecutorBackend {
    Exec(Arc<dyn SandboxExec>),
    FileOps(Arc<dyn SandboxFileOps>),
    Lifecycle(Arc<dyn SandboxLifecycle>),
}

fn sandbox_tool_runtime_executors(
    backend: SandboxToolExecutorBackend,
    specs: Vec<ToolDefinition>,
) -> Vec<Arc<dyn ToolExecutor>> {
    specs
        .into_iter()
        .map(|spec| {
            Arc::new(SandboxToolExecutor {
                backend: backend.clone(),
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
            description: "Execute a shell command in the isolated sandbox environment with /workspace as the working directory. Do not assume Bash-specific syntax unless the sandbox image provides bash. Returns JSON with ok, stdout, stderr, and exit_code. Common commands may include python3, pip, ffmpeg, yt-dlp, curl, wget, date, cat, ls, grep, and other standard Unix tools.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute inside the sandbox"
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
                        "description": "Workspace file path. Relative paths resolve under /workspace; absolute paths must start with /workspace/."
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
                        "description": "Workspace file path. Relative paths resolve under /workspace; absolute paths must start with /workspace/."
                    }
                },
                "required": ["path"]
            }),
        },
        ToolDefinition {
            name: "apply_file_edit".to_string(),
            description: "Apply a targeted exact text replacement to a sandbox file. Non-empty files must be read with read_file first; empty files are exempt. Returns JSON with ok, path, status, replacements, hashes, and bytes_written or error.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Workspace file path. Relative paths resolve under /workspace; absolute paths must start with /workspace/."
                    },
                    "search": {
                        "type": "string",
                        "description": "Exact text fragment to replace. May be empty only when the current file is empty."
                    },
                    "replace": {
                        "type": "string",
                        "description": "Replacement text"
                    },
                    "expected_replacements": {
                        "type": "integer",
                        "description": "Exact number of replacements expected; defaults to 1"
                    }
                },
                "required": ["path", "search", "replace"]
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
                        "description": "Optional workspace path to list. Defaults to /workspace; absolute paths must start with /workspace/."
                    }
                }
            }),
        },
        ToolDefinition {
            name: "recreate_sandbox".to_string(),
            description: "Recreate the sandbox instance from scratch, wiping all previous workspace contents. Returns JSON with ok, status, and message or error.".to_string(),
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
    backend: SandboxToolExecutorBackend,
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
        match &self.backend {
            SandboxToolExecutorBackend::Exec(exec) => {
                SandboxToolHandlers::execute_typed_exec_tool(exec.as_ref(), &invocation).await
            }
            SandboxToolExecutorBackend::FileOps(fileops) => {
                SandboxToolHandlers::execute_typed_fileops_tool(fileops.as_ref(), &invocation).await
            }
            SandboxToolExecutorBackend::Lifecycle(lifecycle) => {
                SandboxToolHandlers::execute_typed_lifecycle_tool(lifecycle.as_ref(), &invocation)
                    .await
            }
        }
    }
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
        let provider = Arc::new(SandboxLifecycleProvider::new(Arc::new(
            SandboxRuntime::new(1),
        )));
        let executors = provider.tool_runtime_executors();

        assert!(executors
            .iter()
            .any(|executor| executor.name().as_str() == "recreate_sandbox"));
        assert!(executors
            .iter()
            .any(|executor| executor.spec().name == "recreate_sandbox"));
    }

    #[test]
    fn execute_command_tool_description_mentions_json_response() {
        let provider = Arc::new(SandboxExecProvider::new(Arc::new(SandboxRuntime::new(1))));
        let execute_command = provider
            .tool_runtime_executors()
            .into_iter()
            .map(|executor| executor.spec())
            .find(|tool| tool.name == "execute_command")
            .expect("execute_command registered");

        assert!(execute_command.description.contains("JSON"));
        assert!(execute_command.description.contains("stdout"));
        assert!(execute_command.description.contains("exit_code"));
        assert!(execute_command.description.contains("shell command"));
        assert!(execute_command.description.contains("/workspace"));
        assert!(!execute_command.description.contains("bash command"));
    }

    #[test]
    fn sandbox_file_tool_descriptions_are_workspace_scoped() {
        let provider = Arc::new(SandboxFileOpsProvider::new(Arc::new(SandboxRuntime::new(
            1,
        ))));
        let specs: Vec<_> = provider
            .tool_runtime_executors()
            .into_iter()
            .map(|executor| executor.spec())
            .collect();

        for tool_name in ["write_file", "read_file", "apply_file_edit", "list_files"] {
            let spec = specs
                .iter()
                .find(|tool| tool.name == tool_name)
                .unwrap_or_else(|| panic!("{tool_name} registered"));
            let path_description = spec
                .parameters
                .get("properties")
                .and_then(|properties| properties.get("path"))
                .and_then(|path| path.get("description"))
                .and_then(Value::as_str)
                .unwrap_or("");

            assert!(
                path_description.contains("/workspace"),
                "{tool_name} path description should mention /workspace"
            );
            assert!(
                path_description.contains("absolute paths must start with /workspace/"),
                "{tool_name} path description should reject non-workspace absolutes"
            );
        }
    }

    #[test]
    fn sandbox_runtime_exposes_narrow_backend_capabilities() {
        let runtime = SandboxRuntime::new(1);
        let capabilities = SandboxBackend::capabilities(&runtime);

        assert_eq!(SandboxBackend::id(&runtime).as_str(), "sandbox/runtime");
        assert!(capabilities.contains(&SandboxCapability::Exec));
        assert!(capabilities.contains(&SandboxCapability::FileOps));
        assert!(capabilities.contains(&SandboxCapability::Lifecycle));
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
                "apply_file_edit",
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
            .tool_runtime_executors()
            .into_iter()
            .map(|executor| executor.spec().name)
            .collect();
        let fileops_tools: Vec<_> = fileops_provider
            .tool_runtime_executors()
            .into_iter()
            .map(|executor| executor.spec().name)
            .collect();
        let lifecycle_tools: Vec<_> = lifecycle_provider
            .tool_runtime_executors()
            .into_iter()
            .map(|executor| executor.spec().name)
            .collect();
        let fileops_without_delivery_tools: Vec<_> = fileops_without_delivery_provider
            .tool_runtime_executors()
            .into_iter()
            .map(|executor| executor.spec().name)
            .collect();

        assert_eq!(exec_tools, ["execute_command"]);
        assert_eq!(
            fileops_tools,
            ["write_file", "read_file", "apply_file_edit", "list_files"]
        );
        assert_eq!(
            fileops_without_delivery_tools,
            ["write_file", "read_file", "apply_file_edit", "list_files"]
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

    #[test]
    fn exact_text_edit_replaces_single_match_by_default() {
        let edit = SandboxFileEdit {
            search: "hello".to_string(),
            replace: "hi".to_string(),
            expected_replacements: 1,
        };
        let (updated, replacements) =
            apply_exact_text_edit(b"hello world\n", &edit).expect("edit should succeed");

        assert_eq!(updated, b"hi world\n");
        assert_eq!(replacements, 1);
    }

    #[test]
    fn exact_text_edit_rejects_ambiguous_matches() {
        let edit = SandboxFileEdit {
            search: "same".to_string(),
            replace: "new".to_string(),
            expected_replacements: 1,
        };
        let error =
            apply_exact_text_edit(b"same\nsame\n", &edit).expect_err("ambiguous edit should fail");

        assert!(error.to_string().contains("expected 1 replacements"));
    }

    #[test]
    fn exact_text_edit_allows_empty_file_insert() {
        let edit = SandboxFileEdit {
            search: String::new(),
            replace: "created\n".to_string(),
            expected_replacements: 1,
        };
        let (updated, replacements) =
            apply_exact_text_edit(b"", &edit).expect("empty file insert should succeed");

        assert_eq!(updated, b"created\n");
        assert_eq!(replacements, 1);
    }

    #[test]
    fn exact_text_edit_rejects_empty_search_for_non_empty_file() {
        let edit = SandboxFileEdit {
            search: String::new(),
            replace: "prefix".to_string(),
            expected_replacements: 1,
        };
        let error = apply_exact_text_edit(b"existing", &edit)
            .expect_err("empty search should fail for non-empty files");

        assert!(error.to_string().contains("search must not be empty"));
    }

    #[test]
    fn edit_read_guard_rejects_missing_snapshot_for_non_empty_file() {
        let error = validate_edit_read_guard("/workspace/app.py", "current", 7, None)
            .expect_err("missing read snapshot must fail");

        assert!(error.to_string().contains("must be read with read_file"));
    }

    #[test]
    fn edit_read_guard_allows_missing_snapshot_for_empty_file() {
        validate_edit_read_guard("/workspace/empty.txt", "empty-hash", 0, None)
            .expect("empty files are exempt from read guard");
    }

    #[test]
    fn edit_read_guard_rejects_stale_snapshot() {
        let snapshot = ReadSnapshot {
            sha256: "old".to_string(),
            bytes: 4,
        };
        let error = validate_edit_read_guard("/workspace/app.py", "new", 5, Some(&snapshot))
            .expect_err("stale read snapshot must fail");

        assert!(error.to_string().contains("file changed after last read"));
    }

    #[test]
    fn typed_apply_file_edit_result_exposes_memory_hook_fields() {
        let invocation = test_invocation("apply_file_edit");
        let output = typed_apply_file_edit_result(
            &invocation,
            SandboxApplyFileEditResult {
                path: "/workspace/app.py".to_string(),
                status: "updated".to_string(),
                replacements: 1,
                previous_sha256: "old".to_string(),
                new_sha256: "new".to_string(),
                bytes_before: 10,
                bytes_written: 12,
                changed: true,
            },
        );
        let payload = output
            .structured_payload
            .expect("apply_file_edit payload should be structured");

        assert_eq!(output.status, ToolOutputStatus::Success);
        assert_eq!(payload.get("ok").and_then(Value::as_bool), Some(true));
        assert_eq!(
            payload.get("path").and_then(Value::as_str),
            Some("/workspace/app.py")
        );
        assert_eq!(
            payload.get("status").and_then(Value::as_str),
            Some("updated")
        );
        assert_eq!(
            payload.get("bytes_written").and_then(Value::as_u64),
            Some(12)
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

/// Arguments for `apply_file_edit` tool
#[derive(Debug, Deserialize)]
struct ApplyFileEditArgs {
    path: String,
    search: String,
    replace: String,
    #[serde(default)]
    expected_replacements: Option<usize>,
}

impl ApplyFileEditArgs {
    fn to_edit(&self) -> SandboxFileEdit {
        SandboxFileEdit {
            search: self.search.clone(),
            replace: self.replace.clone(),
            expected_replacements: self.expected_replacements.unwrap_or(1),
        }
    }
}

/// Arguments for `recreate_sandbox` tool
#[derive(Debug, Default, Deserialize)]
struct RecreateSandboxArgs {}

/// Provider for sandbox command execution tools.
pub struct SandboxExecProvider {
    exec: Arc<dyn SandboxExec>,
}

impl SandboxExecProvider {
    /// Create an exec-only provider backed by a narrow sandbox exec capability.
    #[must_use]
    pub fn new<T>(exec: Arc<T>) -> Self
    where
        T: SandboxExec + 'static,
    {
        Self { exec }
    }

    /// Build typed runtime executors for exec tools.
    #[must_use]
    pub fn tool_runtime_executors(self: &Arc<Self>) -> Vec<Arc<dyn ToolExecutor>> {
        sandbox_tool_runtime_executors(
            SandboxToolExecutorBackend::Exec(Arc::clone(&self.exec)),
            sandbox_tool_definitions_for(SANDBOX_EXEC_TOOL_NAMES),
        )
    }
}

/// Provider for sandbox file operation tools.
pub struct SandboxFileOpsProvider {
    fileops: Arc<dyn SandboxFileOps>,
    tool_names: &'static [&'static str],
}

impl SandboxFileOpsProvider {
    /// Create a fileops-only provider backed by a narrow sandbox fileops capability.
    #[must_use]
    pub fn new<T>(fileops: Arc<T>) -> Self
    where
        T: SandboxFileOps + 'static,
    {
        Self {
            fileops,
            tool_names: SANDBOX_FILEOPS_TOOL_NAMES,
        }
    }

    /// Create a fileops provider without chat/file-delivery tools.
    #[must_use]
    pub fn without_delivery<T>(fileops: Arc<T>) -> Self
    where
        T: SandboxFileOps + 'static,
    {
        Self::new(fileops)
    }

    /// Build typed runtime executors for file operation tools.
    #[must_use]
    pub fn tool_runtime_executors(self: &Arc<Self>) -> Vec<Arc<dyn ToolExecutor>> {
        sandbox_tool_runtime_executors(
            SandboxToolExecutorBackend::FileOps(Arc::clone(&self.fileops)),
            sandbox_tool_definitions_for(self.tool_names),
        )
    }
}

/// Provider for sandbox lifecycle tools.
pub struct SandboxLifecycleProvider {
    lifecycle: Arc<dyn SandboxLifecycle>,
}

impl SandboxLifecycleProvider {
    /// Create a lifecycle-only provider backed by a narrow sandbox lifecycle capability.
    #[must_use]
    pub fn new<T>(lifecycle: Arc<T>) -> Self
    where
        T: SandboxLifecycle + 'static,
    {
        Self { lifecycle }
    }

    /// Build typed runtime executors for lifecycle tools.
    #[must_use]
    pub fn tool_runtime_executors(self: &Arc<Self>) -> Vec<Arc<dyn ToolExecutor>> {
        sandbox_tool_runtime_executors(
            SandboxToolExecutorBackend::Lifecycle(Arc::clone(&self.lifecycle)),
            sandbox_tool_definitions_for(SANDBOX_LIFECYCLE_TOOL_NAMES),
        )
    }
}
