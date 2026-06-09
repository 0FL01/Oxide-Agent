//! Topic-scoped SSH infrastructure provider.

use crate::agent::memory::AgentMessage;
use crate::agent::progress::{AgentEvent, FileDeliveryKind};
use crate::agent::tool_runtime::{
    CleanupStatus, OutputNormalizer, ToolExecutor, ToolInvocation, ToolName, ToolOutput,
    ToolRuntimeConfig, ToolRuntimeError,
};
use crate::llm::ToolDefinition;
use crate::storage::{
    StorageProvider, TopicInfraAuthMode, TopicInfraConfigRecord, TopicInfraToolMode,
};
use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use rmcp::{
    ServiceExt,
    model::CallToolRequestParams,
    service::{Peer, RoleClient, RunningService, ServiceError},
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use shell_escape::unix::escape;
use std::borrow::Cow;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tempfile::TempDir;
use tempfile::{Builder, NamedTempFile};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio::time::{Duration, timeout};
use uuid::Uuid;

use super::file_delivery::{
    FileDeliveryRequest, FileDeliveryStatus, chat_delivery_max_file_size_bytes,
    deliver_file_via_progress, format_generic_delivery_report,
};

const TOOL_SSH_EXEC: &str = "ssh_exec";
const TOOL_SSH_SUDO_EXEC: &str = "ssh_sudo_exec";
const TOOL_SSH_READ_FILE: &str = "ssh_read_file";
const TOOL_SSH_APPLY_FILE_EDIT: &str = "ssh_apply_file_edit";
const TOOL_SSH_CHECK_PROCESS: &str = "ssh_check_process";
const TOOL_SSH_SEND_FILE_TO_USER: &str = "ssh_send_file_to_user";

const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_REMOTE_OUTPUT_CHARS: usize = 16_000;
const KEY_PROBE_TIMEOUT_SECS: u64 = 10;
const DEFAULT_UPSTREAM_SSH_MCP_BINARY_PATH: &str = "/usr/local/bin/ssh-mcp";
const UPSTREAM_SSH_MCP_BINARY_ENV: &str = "OXIDE_SSH_MCP_BINARY";
const UPSTREAM_TOOL_EXEC: &str = "exec";
const UPSTREAM_TOOL_SUDO_EXEC: &str = "sudo-exec";
const UPSTREAM_TOOL_TRANSFER: &str = "transfer";
const UPSTREAM_TOOL_READ_FILE: &str = "read-file";
const UPSTREAM_TOOL_APPLY_FILE_EDIT: &str = "apply-file-edit";
const UPSTREAM_TOOL_CHECK_PROCESS: &str = "check-process";
const UPSTREAM_TIMEOUT_GRACE_MS: u64 = 30_000;
const UPSTREAM_MAX_OUTPUT_TOKENS: usize = 12_000;
const PRIVATE_KEY_TEMPFILE_PREFIX: &str = "oxide-agent-ssh-key-";
const TRANSFER_SESSION_DIR_PREFIX: &str = "oxide-agent-ssh-transfer-";

/// Supported secret probe kinds for manager-facing diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretProbeKind {
    /// Probe an opaque secret for presence only.
    Opaque,
    /// Probe and validate an SSH private key using `ssh-keygen`.
    SshPrivateKey,
}

/// Safe secret probe result exposed to manager tools and runtime context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SecretProbeReport {
    /// Original opaque secret reference.
    pub secret_ref: String,
    /// Secret source kind (`env` or `storage`).
    pub source: String,
    /// Requested probe kind.
    pub kind: SecretProbeKind,
    /// Whether the secret payload exists.
    pub present: bool,
    /// Whether the probed secret is safe to use.
    pub usable: bool,
    /// Probe status (`valid`, `missing`, `invalid`).
    pub status: String,
    /// Optional SSH key fingerprint from `ssh-keygen -l`.
    pub fingerprint: Option<String>,
    /// Optional SSH key algorithm label.
    pub key_type: Option<String>,
    /// Optional SSH key comment when available.
    pub comment: Option<String>,
    /// Safe error summary without secret material.
    pub error: Option<String>,
}

impl SecretProbeReport {
    fn valid(secret_ref: &str, source: &str, kind: SecretProbeKind) -> Self {
        Self {
            secret_ref: secret_ref.to_string(),
            source: source.to_string(),
            kind,
            present: true,
            usable: true,
            status: "valid".to_string(),
            fingerprint: None,
            key_type: None,
            comment: None,
            error: None,
        }
    }

    fn missing(
        secret_ref: &str,
        source: &str,
        kind: SecretProbeKind,
        error: Option<String>,
    ) -> Self {
        Self {
            secret_ref: secret_ref.to_string(),
            source: source.to_string(),
            kind,
            present: false,
            usable: false,
            status: "missing".to_string(),
            fingerprint: None,
            key_type: None,
            comment: None,
            error,
        }
    }

    fn invalid(secret_ref: &str, source: &str, kind: SecretProbeKind, error: String) -> Self {
        Self {
            secret_ref: secret_ref.to_string(),
            source: source.to_string(),
            kind,
            present: true,
            usable: false,
            status: "invalid".to_string(),
            fingerprint: None,
            key_type: None,
            comment: None,
            error: Some(error),
        }
    }

    fn summary(&self) -> String {
        match self.kind {
            SecretProbeKind::Opaque => match self.status.as_str() {
                "valid" => format!(
                    "secret_ref '{}' from {} is present and usable",
                    self.secret_ref, self.source
                ),
                "missing" => format!(
                    "secret_ref '{}' from {} is missing",
                    self.secret_ref, self.source
                ),
                _ => format!(
                    "secret_ref '{}' from {} is invalid{}",
                    self.secret_ref,
                    self.source,
                    self.error
                        .as_deref()
                        .map(|err| format!(": {err}"))
                        .unwrap_or_default()
                ),
            },
            SecretProbeKind::SshPrivateKey => match self.status.as_str() {
                "valid" => {
                    let mut parts = vec![format!(
                        "secret_ref '{}' from {} is a valid SSH private key",
                        self.secret_ref, self.source
                    )];
                    if let Some(fingerprint) = self.fingerprint.as_deref() {
                        parts.push(format!("fingerprint {fingerprint}"));
                    }
                    if let Some(key_type) = self.key_type.as_deref() {
                        parts.push(format!("type {key_type}"));
                    }
                    if let Some(comment) = self.comment.as_deref() {
                        parts.push(format!("comment {comment}"));
                    }
                    parts.join(", ")
                }
                "missing" => format!(
                    "secret_ref '{}' from {} is missing",
                    self.secret_ref, self.source
                ),
                _ => format!(
                    "secret_ref '{}' from {} is not a valid SSH private key{}",
                    self.secret_ref,
                    self.source,
                    self.error
                        .as_deref()
                        .map(|err| format!(": {err}"))
                        .unwrap_or_default()
                ),
            },
        }
    }
}

/// Safe preflight status for a topic-scoped SSH target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TopicInfraPreflightReport {
    /// Stable topic id where the target is attached.
    pub topic_id: String,
    /// Human-readable target name.
    pub target_name: String,
    /// Target SSH host.
    pub host: String,
    /// Target SSH port.
    pub port: u16,
    /// Remote SSH username.
    pub remote_user: String,
    /// Effective SSH auth mode.
    pub auth_mode: TopicInfraAuthMode,
    /// Whether `ssh_mcp` is safe to register for this topic.
    pub provider_enabled: bool,
    /// Auth secret probe details when a secret-backed mode is used.
    pub auth_secret: Option<SecretProbeReport>,
    /// Optional sudo secret probe details.
    pub sudo_secret: Option<SecretProbeReport>,
    /// Safe human-readable summary suitable for prompt injection.
    pub summary: String,
}

#[derive(Clone)]
enum SshExecutionBackend {
    Upstream(UpstreamSshMcpBackend),
}

impl SshExecutionBackend {
    async fn execute(
        &self,
        command: &str,
        timeout_secs: u64,
        sudo: bool,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<RemoteOutput> {
        match self {
            Self::Upstream(backend) => {
                backend
                    .execute(command, timeout_secs, sudo, cancellation_token)
                    .await
            }
        }
    }

    async fn transfer_get_file(
        &self,
        remote_path: &str,
        local_path: &str,
        timeout_secs: u64,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<()> {
        match self {
            Self::Upstream(backend) => {
                backend
                    .transfer_get_file(remote_path, local_path, timeout_secs, cancellation_token)
                    .await
            }
        }
    }

    async fn transfer_root_path(&self) -> Result<PathBuf> {
        match self {
            Self::Upstream(backend) => backend.transfer_root_path().await,
        }
    }

    async fn read_file(
        &self,
        remote_path: &str,
        timeout_secs: u64,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<UpstreamReadFileResponse> {
        match self {
            Self::Upstream(backend) => {
                backend
                    .read_file(remote_path, timeout_secs, cancellation_token)
                    .await
            }
        }
    }

    async fn apply_file_edit_partial(
        &self,
        remote_path: &str,
        old_text: &str,
        new_text: &str,
        replace_all: bool,
        timeout_secs: u64,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<UpstreamApplyFileEditResponse> {
        match self {
            Self::Upstream(backend) => {
                backend
                    .apply_file_edit_partial(
                        remote_path,
                        old_text,
                        new_text,
                        replace_all,
                        timeout_secs,
                        cancellation_token,
                    )
                    .await
            }
        }
    }

    async fn apply_file_edit_full(
        &self,
        remote_path: &str,
        new_content: &str,
        timeout_secs: u64,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<UpstreamApplyFileEditResponse> {
        match self {
            Self::Upstream(backend) => {
                backend
                    .apply_file_edit_full(
                        remote_path,
                        new_content,
                        timeout_secs,
                        cancellation_token,
                    )
                    .await
            }
        }
    }

    async fn check_process(
        &self,
        job_id: &str,
        tail_lines: usize,
        timeout_secs: u64,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<UpstreamCheckProcessResponse> {
        match self {
            Self::Upstream(backend) => {
                backend
                    .check_process(job_id, tail_lines, timeout_secs, cancellation_token)
                    .await
            }
        }
    }
}

#[derive(Clone)]
struct UpstreamSshMcpBackend {
    storage: Arc<dyn StorageProvider>,
    user_id: i64,
    config: TopicInfraConfigRecord,
    binary_path: PathBuf,
    session: Arc<Mutex<Option<Arc<UpstreamSshMcpSession>>>>,
    call_lock: Arc<Mutex<()>>,
}

struct UpstreamSshMcpSession {
    client: Mutex<Option<RunningService<RoleClient, ()>>>,
    peer: Peer<RoleClient>,
    stderr_task: Mutex<Option<JoinHandle<()>>>,
    _key_file: Option<NamedTempFile>,
    _transfer_root: TempDir,
}

#[derive(Debug, Deserialize)]
struct UpstreamTransferResponse {
    ok: bool,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpstreamReadFileResponse {
    path: String,
    content: String,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    returned_lines: Option<usize>,
    #[serde(default)]
    truncated: Option<bool>,
    #[serde(default)]
    approx_tokens_returned: Option<usize>,
    #[serde(default)]
    approx_tokens_total_estimate: Option<usize>,
    #[serde(default)]
    hint: Option<String>,
    #[serde(default)]
    sha256: Option<String>,
    #[serde(default)]
    read_ticket: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpstreamApplyFileEditResponse {
    path: String,
    previous_sha256: String,
    new_sha256: String,
    bytes_written: usize,
    changed: bool,
}

#[derive(Debug, Deserialize)]
struct UpstreamCheckProcessResponse {
    running: bool,
    exit_code: Option<u32>,
    elapsed_time: String,
    command: String,
    log_tail: String,
}

impl UpstreamSshMcpSession {
    async fn shutdown(&self) {
        if let Some(mut client) = self.client.lock().await.take() {
            let _ = client.close_with_timeout(Duration::from_secs(3)).await;
        }
        if let Some(stderr_task) = self.stderr_task.lock().await.take() {
            stderr_task.abort();
        }
    }
}

impl Drop for UpstreamSshMcpSession {
    fn drop(&mut self) {
        if let Ok(mut client) = self.client.try_lock() {
            client.take();
        }
        if let Ok(mut stderr_task) = self.stderr_task.try_lock()
            && let Some(task) = stderr_task.take()
        {
            task.abort();
        }
    }
}

impl UpstreamSshMcpBackend {
    fn new(
        storage: Arc<dyn StorageProvider>,
        user_id: i64,
        config: TopicInfraConfigRecord,
    ) -> Self {
        let binary_path = std::env::var(UPSTREAM_SSH_MCP_BINARY_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_UPSTREAM_SSH_MCP_BINARY_PATH));
        Self {
            storage,
            user_id,
            config,
            binary_path,
            session: Arc::new(Mutex::new(None)),
            call_lock: Arc::new(Mutex::new(())),
        }
    }

    async fn execute(
        &self,
        command: &str,
        timeout_secs: u64,
        sudo: bool,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<RemoteOutput> {
        let _call_guard = self.call_lock.lock().await;
        let wrapped = build_wrapped_remote_command(command);
        let response = self
            .call_tool(
                if sudo {
                    UPSTREAM_TOOL_SUDO_EXEC
                } else {
                    UPSTREAM_TOOL_EXEC
                },
                json!({
                    "command": wrapped.command,
                    "timeout_ms": upstream_timeout_ms(timeout_secs),
                }),
                timeout_secs,
                cancellation_token,
            )
            .await?;
        parse_wrapped_remote_output(&wrapped.markers, &response)
    }

    async fn transfer_get_file(
        &self,
        remote_path: &str,
        local_path: &str,
        timeout_secs: u64,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<()> {
        let _call_guard = self.call_lock.lock().await;
        let response = self
            .call_tool(
                UPSTREAM_TOOL_TRANSFER,
                json!({
                    "operation": "get",
                    "kind": "file",
                    "remote_path": remote_path,
                    "local_path": local_path,
                    "transport": "auto",
                    "overwrite": true,
                    "timeout_ms": upstream_timeout_ms(timeout_secs),
                }),
                timeout_secs,
                cancellation_token,
            )
            .await?;

        let parsed: UpstreamTransferResponse = serde_json::from_str(&response)
            .with_context(|| format!("failed to parse upstream transfer response: {response}"))?;
        if parsed.ok {
            return Ok(());
        }

        bail!(
            "upstream ssh-mcp transfer failed: {}",
            parsed
                .error
                .unwrap_or_else(|| "unknown transfer error".to_string())
        )
    }

    async fn transfer_root_path(&self) -> Result<PathBuf> {
        let session = self.ensure_session().await?;
        Ok(session._transfer_root.path().to_path_buf())
    }

    async fn read_file(
        &self,
        remote_path: &str,
        timeout_secs: u64,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<UpstreamReadFileResponse> {
        let _call_guard = self.call_lock.lock().await;
        self.call_tool_json(
            UPSTREAM_TOOL_READ_FILE,
            json!({
                "remote_path": remote_path,
                "mode": "full",
                "timeout_ms": upstream_timeout_ms(timeout_secs),
            }),
            timeout_secs,
            cancellation_token,
        )
        .await
    }

    async fn apply_file_edit_partial(
        &self,
        remote_path: &str,
        old_text: &str,
        new_text: &str,
        replace_all: bool,
        timeout_secs: u64,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<UpstreamApplyFileEditResponse> {
        let _call_guard = self.call_lock.lock().await;
        self.call_tool_json(
            UPSTREAM_TOOL_APPLY_FILE_EDIT,
            json!({
                "remote_path": remote_path,
                "old_text": old_text,
                "new_text": new_text,
                "replace_all": replace_all,
                "timeout_ms": upstream_timeout_ms(timeout_secs),
            }),
            timeout_secs,
            cancellation_token,
        )
        .await
    }

    async fn apply_file_edit_full(
        &self,
        remote_path: &str,
        new_content: &str,
        timeout_secs: u64,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<UpstreamApplyFileEditResponse> {
        let _call_guard = self.call_lock.lock().await;
        self.call_tool_json(
            UPSTREAM_TOOL_APPLY_FILE_EDIT,
            json!({
                "remote_path": remote_path,
                "new_content": new_content,
                "timeout_ms": upstream_timeout_ms(timeout_secs),
            }),
            timeout_secs,
            cancellation_token,
        )
        .await
    }

    async fn check_process(
        &self,
        job_id: &str,
        tail_lines: usize,
        timeout_secs: u64,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<UpstreamCheckProcessResponse> {
        let _call_guard = self.call_lock.lock().await;
        self.call_tool_json(
            UPSTREAM_TOOL_CHECK_PROCESS,
            json!({
                "job_id": job_id,
                "tail_lines": tail_lines,
            }),
            timeout_secs,
            cancellation_token,
        )
        .await
    }

    async fn call_tool_json<T: DeserializeOwned>(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
        timeout_secs: u64,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<T> {
        let response = self
            .call_tool(tool_name, arguments, timeout_secs, cancellation_token)
            .await?;
        serde_json::from_str(&response).with_context(|| {
            format!("failed to parse upstream ssh-mcp response for tool '{tool_name}': {response}")
        })
    }

    async fn call_tool(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
        timeout_secs: u64,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        let session = self.ensure_session().await?;
        let args = arguments
            .as_object()
            .cloned()
            .ok_or_else(|| anyhow!("invalid upstream ssh-mcp arguments"))?;
        let response = session
            .peer
            .call_tool(CallToolRequestParams::new(tool_name.to_string()).with_arguments(args));
        tokio::pin!(response);

        let result = tokio::select! {
            response = &mut response => response,
            _ = tokio::time::sleep(Duration::from_secs(timeout_secs)) => {
                self.reset_session().await;
                bail!("remote SSH command timed out after {timeout_secs} seconds")
            }
            _ = async {
                if let Some(token) = cancellation_token {
                    token.cancelled().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                self.reset_session().await;
                bail!("SSH command cancelled by user")
            }
        };

        let result = match result {
            Ok(result) => result,
            Err(error) => {
                self.reset_session().await;
                return Err(format_upstream_service_error(error));
            }
        };

        let text = extract_call_tool_text(&result);
        if result.is_error.unwrap_or(false) {
            bail!(
                "upstream ssh-mcp tool '{tool_name}' failed: {}",
                text.trim()
            );
        }
        Ok(text)
    }

    async fn ensure_session(&self) -> Result<Arc<UpstreamSshMcpSession>> {
        if let Some(existing) = self.session.lock().await.as_ref().cloned() {
            return Ok(existing);
        }

        let created = Arc::new(self.spawn_session().await?);
        let existing = {
            let mut guard = self.session.lock().await;
            if let Some(existing) = guard.as_ref().cloned() {
                Some(existing)
            } else {
                *guard = Some(Arc::clone(&created));
                None
            }
        };
        if let Some(existing) = existing {
            created.shutdown().await;
            return Ok(existing);
        }
        Ok(created)
    }

    async fn spawn_session(&self) -> Result<UpstreamSshMcpSession> {
        let resolved_auth = resolve_backend_auth(&self.storage, self.user_id, &self.config).await?;
        let binary_path = self.binary_path.clone();
        let transfer_root = Builder::new()
            .prefix(TRANSFER_SESSION_DIR_PREFIX)
            .tempdir()
            .context("failed to create local transfer root for upstream ssh-mcp session")?;

        let command = tokio::process::Command::new(&binary_path);
        let (transport, stderr) = TokioChildProcess::builder(command.configure(|cmd| {
            cmd.arg(format!("--host={}", self.config.host))
                .arg(format!("--port={}", self.config.port))
                .arg(format!("--user={}", self.config.remote_user))
                .arg(format!(
                    "--timeout={}",
                    upstream_timeout_ms(DEFAULT_TIMEOUT_SECS)
                ))
                .arg("--maxChars=none")
                .arg(format!("--max-output-tokens={UPSTREAM_MAX_OUTPUT_TOKENS}"))
                .arg("--log-level=warn")
                .current_dir(transfer_root.path());

            if let Some(password) = resolved_auth.password.as_deref() {
                cmd.env("SSH_MCP_PASSWORD", password);
            }
            if let Some(key_file) = resolved_auth.key_file.as_ref() {
                cmd.arg(format!("--key={}", key_file.path().display()));
            }
            if let Some(sudo_password) = resolved_auth.sudo_password.as_deref() {
                cmd.env("SSH_MCP_SUDO_PASSWORD", sudo_password);
            }
        }))
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "failed to spawn upstream ssh-mcp binary at {}",
                binary_path.display()
            )
        })?;

        let stderr_task = stderr.map(|stderr| {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                loop {
                    match lines.next_line().await {
                        Ok(Some(line)) => tracing::warn!(target: "ssh_mcp_upstream", "{line}"),
                        Ok(None) => break,
                        Err(error) => {
                            tracing::warn!(target: "ssh_mcp_upstream", "failed to read stderr: {error}");
                            break;
                        }
                    }
                }
            })
        });

        let client = match ().serve(transport).await {
            Ok(client) => client,
            Err(error) => {
                return Err(anyhow!(
                    "failed to initialize upstream ssh-mcp session: {error}"
                ));
            }
        };
        let peer = client.peer().clone();

        Ok(UpstreamSshMcpSession {
            client: Mutex::new(Some(client)),
            peer,
            stderr_task: Mutex::new(stderr_task),
            _key_file: resolved_auth.key_file,
            _transfer_root: transfer_root,
        })
    }

    async fn reset_session(&self) {
        let session = self.session.lock().await.take();
        if let Some(session) = session {
            session.shutdown().await;
        }
    }
}

/// Topic-scoped SSH tool provider backed by the upstream `ssh-mcp` binary.
#[derive(Clone)]
pub struct SshMcpProvider {
    topic_id: String,
    storage: Arc<dyn StorageProvider>,
    user_id: i64,
    config: TopicInfraConfigRecord,
}

impl SshMcpProvider {
    /// Create a new topic-scoped SSH provider instance.
    #[must_use]
    pub fn new(
        storage: Arc<dyn StorageProvider>,
        user_id: i64,
        topic_id: String,
        config: TopicInfraConfigRecord,
    ) -> Self {
        Self {
            topic_id,
            storage,
            user_id,
            config,
        }
    }

    /// Build typed v1 runtime executors for SSH process-like tools.
    #[must_use]
    pub fn tool_runtime_executors(
        self: &Arc<Self>,
        progress_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> Vec<Arc<dyn ToolExecutor>> {
        Self::runtime_tool_definitions()
            .into_iter()
            .map(|spec| {
                Arc::new(SshRuntimeToolExecutor {
                    provider: Arc::clone(self),
                    name: ToolName::from(spec.name.clone()),
                    spec,
                    progress_tx: progress_tx.clone(),
                }) as Arc<dyn ToolExecutor>
            })
            .collect()
    }

    fn runtime_tool_definitions() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: TOOL_SSH_EXEC.to_string(),
                description: "Run a remote SSH command on the topic infra target".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Remote shell command" },
                        "timeout_secs": { "type": "integer", "description": "Optional timeout in seconds" }
                    },
                    "required": ["command"]
                }),
            },
            ToolDefinition {
                name: TOOL_SSH_SUDO_EXEC.to_string(),
                description: "Run a sudo remote SSH command on the topic infra target".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Remote shell command executed under sudo" },
                        "timeout_secs": { "type": "integer", "description": "Optional timeout in seconds" }
                    },
                    "required": ["command"]
                }),
            },
            ToolDefinition {
                name: TOOL_SSH_READ_FILE.to_string(),
                description: "Read a remote text file from the topic infra target".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Remote absolute or relative file path" },
                        "max_bytes": { "type": "integer", "description": "Optional maximum bytes to return" }
                    },
                    "required": ["path"]
                }),
            },
            ToolDefinition {
                name: TOOL_SSH_APPLY_FILE_EDIT.to_string(),
                description:
                    "Apply a targeted text edit to a remote file on the topic infra target"
                        .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Remote file path" },
                        "search": { "type": "string", "description": "Exact text fragment to replace" },
                        "replace": { "type": "string", "description": "Replacement text" },
                        "create_if_missing": { "type": "boolean", "description": "Create the file with replacement content when it does not exist" },
                        "timeout_secs": { "type": "integer", "description": "Optional timeout in seconds" }
                    },
                    "required": ["path", "search", "replace"]
                }),
            },
            ToolDefinition {
                name: TOOL_SSH_CHECK_PROCESS.to_string(),
                description: "Check a remote process by job_id or process pattern on the topic infra target."
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Substring or process pattern to inspect via pgrep. Provide exactly one of pattern or job_id." },
                        "job_id": { "type": "string", "description": "Preferred background job id returned by ssh_exec/ssh_sudo_exec" },
                        "tail_lines": { "type": "integer", "description": "Optional tail line count when using job_id" }
                    }
                }),
            },
            ToolDefinition {
                name: TOOL_SSH_SEND_FILE_TO_USER.to_string(),
                description:
                    "Transfer a remote file from the topic infra target and send it to the user via the chat transport. Returns structured JSON status; some transports also include file_id and download_url. When download_url is present, reuse that exact link in the final answer so the user can download the delivered file directly."
                        .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Remote file path to transfer and send" },
                        "file_name": { "type": "string", "description": "Optional override for the delivered file name" },
                        "timeout_secs": { "type": "integer", "description": "Optional timeout in seconds" }
                    },
                    "required": ["path"]
                }),
            },
        ]
    }

    fn fresh_runtime_backend(&self) -> SshExecutionBackend {
        SshExecutionBackend::Upstream(UpstreamSshMcpBackend::new(
            Arc::clone(&self.storage),
            self.user_id,
            self.config.clone(),
        ))
    }

    fn ensure_mode_allowed(&self, mode: TopicInfraToolMode) -> Result<()> {
        if self.config.allowed_tool_modes.contains(&mode) {
            return Ok(());
        }
        bail!(
            "SSH tool mode '{:?}' is not allowed for topic '{}'",
            mode,
            self.topic_id
        )
    }

    async fn execute_typed_ssh_tool(
        &self,
        invocation: &ToolInvocation,
        progress_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        match invocation.tool_name.as_str() {
            TOOL_SSH_EXEC => self.execute_exec_typed(invocation, false).await,
            TOOL_SSH_SUDO_EXEC => self.execute_exec_typed(invocation, true).await,
            TOOL_SSH_READ_FILE => self.execute_read_file_typed(invocation).await,
            TOOL_SSH_APPLY_FILE_EDIT => self.execute_apply_file_edit_typed(invocation).await,
            TOOL_SSH_CHECK_PROCESS => self.execute_check_process_typed(invocation).await,
            TOOL_SSH_SEND_FILE_TO_USER => {
                self.execute_send_file_to_user_typed(invocation, progress_tx)
                    .await
            }
            other => Err(ToolRuntimeError::Internal(format!(
                "typed ssh executor received unsupported tool: {other}"
            ))),
        }
    }

    async fn execute_exec_typed(
        &self,
        invocation: &ToolInvocation,
        sudo: bool,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        let args: CommandArgs = parse_runtime_args(invocation)?;
        let command = validate_non_empty(args.command, "command")
            .map_err(|error| ToolRuntimeError::InvalidArguments(error.to_string()))?;
        let mode = if sudo {
            TopicInfraToolMode::SudoExec
        } else {
            TopicInfraToolMode::Exec
        };
        self.ensure_mode_allowed(mode)
            .map_err(ssh_runtime_failure)?;

        let backend = self.fresh_runtime_backend();
        let output = backend
            .execute(
                &command,
                args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS),
                sudo,
                Some(&invocation.cancellation_token),
            )
            .await
            .map_err(ssh_runtime_failure)?;

        Ok(typed_ssh_exec_output(
            invocation,
            &self.config,
            &command,
            sudo,
            output,
        ))
    }

    async fn execute_read_file_typed(
        &self,
        invocation: &ToolInvocation,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        let args: ReadFileArgs = parse_runtime_args(invocation)?;
        let path = validate_non_empty(args.path, "path")
            .map_err(|error| ToolRuntimeError::InvalidArguments(error.to_string()))?;
        self.ensure_mode_allowed(TopicInfraToolMode::ReadFile)
            .map_err(ssh_runtime_failure)?;

        let max_bytes = args.max_bytes.unwrap_or(16_384).max(1);
        let backend = self.fresh_runtime_backend();
        let response = backend
            .read_file(
                &path,
                DEFAULT_TIMEOUT_SECS,
                Some(&invocation.cancellation_token),
            )
            .await
            .map_err(ssh_runtime_failure)?;
        let (payload, content) = typed_read_file_payload(response, max_bytes);
        Ok(typed_ssh_payload_output(
            invocation,
            payload,
            &content,
            "",
            Some(0),
        ))
    }

    async fn execute_apply_file_edit_typed(
        &self,
        invocation: &ToolInvocation,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        let args: ApplyFileEditArgs = parse_runtime_args(invocation)?;
        let path = validate_non_empty(args.path.clone(), "path")
            .map_err(|error| ToolRuntimeError::InvalidArguments(error.to_string()))?;
        self.ensure_mode_allowed(TopicInfraToolMode::ApplyFileEdit)
            .map_err(ssh_runtime_failure)?;

        let timeout_secs = args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
        let backend = self.fresh_runtime_backend();
        let (response, created) = match backend
            .apply_file_edit_partial(
                &path,
                &args.search,
                &args.replace,
                false,
                timeout_secs,
                Some(&invocation.cancellation_token),
            )
            .await
        {
            Ok(response) => (response, false),
            Err(error)
                if args.create_if_missing && is_upstream_missing_remote_path_error(&error) =>
            {
                let response = backend
                    .apply_file_edit_full(
                        &path,
                        &args.replace,
                        timeout_secs,
                        Some(&invocation.cancellation_token),
                    )
                    .await
                    .map_err(ssh_runtime_failure)?;
                (response, true)
            }
            Err(error) => return Err(ssh_runtime_failure(error)),
        };
        let payload = typed_apply_file_edit_payload(response, created);
        Ok(typed_ssh_payload_output(
            invocation,
            payload,
            "",
            "",
            Some(0),
        ))
    }

    async fn execute_check_process_typed(
        &self,
        invocation: &ToolInvocation,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        let args: CheckProcessArgs = parse_runtime_args(invocation)?;
        let request = normalize_check_process_args(args)
            .map_err(|error| ToolRuntimeError::InvalidArguments(error.to_string()))?;
        self.ensure_mode_allowed(TopicInfraToolMode::CheckProcess)
            .map_err(ssh_runtime_failure)?;

        let backend = self.fresh_runtime_backend();
        match request {
            CheckProcessRequest::JobId { job_id, tail_lines } => {
                let response = backend
                    .check_process(
                        &job_id,
                        tail_lines,
                        DEFAULT_TIMEOUT_SECS,
                        Some(&invocation.cancellation_token),
                    )
                    .await
                    .map_err(ssh_runtime_failure)?;
                let exit_code = response.exit_code.and_then(|code| i32::try_from(code).ok());
                let log_tail = response.log_tail;
                Ok(typed_ssh_payload_output(
                    invocation,
                    json!({
                        "ok": true,
                        "job_id": job_id,
                        "running": response.running,
                        "exit_code": response.exit_code,
                        "elapsed_time": response.elapsed_time,
                        "command": response.command,
                        "log_tail": log_tail,
                        "cleanup_status": "best_effort_remote_cleanup",
                    }),
                    "",
                    &log_tail,
                    exit_code,
                ))
            }
            CheckProcessRequest::Pattern(pattern) => {
                let remote_script =
                    format!("pgrep -af -- {} || true", escape_shell_argument(&pattern));
                let output = backend
                    .execute(
                        &remote_script,
                        DEFAULT_TIMEOUT_SECS,
                        false,
                        Some(&invocation.cancellation_token),
                    )
                    .await
                    .map_err(ssh_runtime_failure)?;
                let stdout = output.stdout;
                let stderr = output.stderr;
                let exit_code = output.exit_code;
                Ok(typed_ssh_payload_output(
                    invocation,
                    json!({
                        "ok": true,
                        "pattern": pattern,
                        "matches": stdout,
                        "cleanup_status": "best_effort_remote_cleanup",
                    }),
                    &stdout,
                    &stderr,
                    Some(exit_code),
                ))
            }
        }
    }

    async fn execute_send_file_to_user_typed(
        &self,
        invocation: &ToolInvocation,
        progress_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        let args: SendRemoteFileArgs = parse_runtime_args(invocation)?;
        let path = validate_non_empty(args.path, "path")
            .map_err(|error| ToolRuntimeError::InvalidArguments(error.to_string()))?;
        self.ensure_mode_allowed(TopicInfraToolMode::Transfer)
            .map_err(ssh_runtime_failure)?;

        let timeout_secs = args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
        let delivered_file_name = match args.file_name {
            Some(file_name) => validate_chat_file_name(file_name)
                .map_err(|error| ToolRuntimeError::InvalidArguments(error.to_string()))?,
            None => default_remote_file_name(&path),
        };
        let local_path = unique_transfer_local_path(&delivered_file_name);
        let backend = self.fresh_runtime_backend();
        backend
            .transfer_get_file(
                &path,
                &local_path,
                timeout_secs,
                Some(&invocation.cancellation_token),
            )
            .await
            .map_err(ssh_runtime_failure)?;

        let download_path = backend
            .transfer_root_path()
            .await
            .map_err(ssh_runtime_failure)?
            .join(&local_path);
        let delivery_result =
            ssh_delivery_result(&download_path, &path, &delivered_file_name, progress_tx).await;

        let cleanup_result = tokio::fs::remove_file(&download_path).await;
        if let Err(error) = cleanup_result
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %download_path.display(), error = %error, "Failed to cleanup transferred SSH file");
        }

        let (payload, report, ok) = delivery_result.map_err(ssh_runtime_failure)?;
        Ok(typed_ssh_payload_output(
            invocation,
            payload,
            &report,
            "",
            Some(if ok { 0 } else { 1 }),
        ))
    }
}

struct SshRuntimeToolExecutor {
    provider: Arc<SshMcpProvider>,
    name: ToolName,
    spec: ToolDefinition,
    progress_tx: Option<mpsc::Sender<AgentEvent>>,
}

#[async_trait]
impl ToolExecutor for SshRuntimeToolExecutor {
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
        self.provider
            .execute_typed_ssh_tool(&invocation, self.progress_tx.as_ref())
            .await
    }
}

fn parse_runtime_args<T>(invocation: &ToolInvocation) -> std::result::Result<T, ToolRuntimeError>
where
    T: DeserializeOwned,
{
    match serde_json::from_value(invocation.normalized_arguments.clone()) {
        Ok(args) => Ok(args),
        Err(value_error) => {
            serde_json::from_str(&invocation.raw_arguments).map_err(|string_error| {
                ToolRuntimeError::InvalidArguments(format!(
                    "invalid ssh arguments: {value_error}; raw JSON parse error: {string_error}"
                ))
            })
        }
    }
}

fn ssh_runtime_failure(error: anyhow::Error) -> ToolRuntimeError {
    ToolRuntimeError::Failure(error.to_string())
}

fn typed_ssh_exec_output(
    invocation: &ToolInvocation,
    config: &TopicInfraConfigRecord,
    command: &str,
    sudo: bool,
    output: RemoteOutput,
) -> ToolOutput {
    typed_ssh_payload_output(
        invocation,
        json!({
            "ok": output.exit_code == 0,
            "target_name": config.target_name.clone(),
            "host": config.host.clone(),
            "command": command,
            "sudo": sudo,
            "exit_code": output.exit_code,
            "cleanup_status": "best_effort_remote_cleanup",
        }),
        &output.stdout,
        &output.stderr,
        Some(output.exit_code),
    )
}

fn typed_ssh_payload_output(
    invocation: &ToolInvocation,
    payload: Value,
    stdout: &str,
    stderr: &str,
    exit_code: Option<i32>,
) -> ToolOutput {
    let normalizer = ssh_normalizer(invocation);
    let ok = payload
        .get("ok")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| exit_code == Some(0));
    let mut output = if ok {
        normalizer.success(invocation, stdout, stderr)
    } else {
        normalizer
            .failure(invocation, "remote SSH tool reported failure")
            .with_streams(
                normalizer.stdout_preview(stdout),
                normalizer.stderr_preview(stderr),
            )
    };
    output.exit_code = exit_code;
    output.structured_payload = Some(payload);
    output.cleanup_status = CleanupStatus::BestEffortRemoteCleanup;
    output
}

fn ssh_normalizer(invocation: &ToolInvocation) -> OutputNormalizer {
    let config = ToolRuntimeConfig {
        timeout: invocation.timeout.clone(),
        artifact_dir: invocation.execution_context.artifact_dir.clone(),
        ..ToolRuntimeConfig::default()
    };
    OutputNormalizer::new(config)
}

struct ResolvedSecretMaterial {
    source: &'static str,
    value: Option<String>,
}

/// Probe a secret reference without exposing secret material.
pub async fn probe_secret_ref(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    secret_ref: &str,
    kind: SecretProbeKind,
) -> SecretProbeReport {
    match resolve_secret_material(storage, user_id, secret_ref).await {
        Ok(ResolvedSecretMaterial {
            source,
            value: None,
        }) => SecretProbeReport::missing(secret_ref, source, kind, None),
        Ok(ResolvedSecretMaterial {
            source,
            value: Some(value),
        }) => match kind {
            SecretProbeKind::Opaque => validate_opaque_secret(secret_ref, source, &value),
            SecretProbeKind::SshPrivateKey => {
                validate_ssh_private_key(secret_ref, source, &value).await
            }
        },
        Err(error) => SecretProbeReport::invalid(
            secret_ref,
            secret_source(secret_ref),
            kind,
            error.to_string(),
        ),
    }
}

/// Inspect a topic infra config and decide whether `ssh_mcp` should be enabled.
pub async fn inspect_topic_infra_config(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    topic_id: &str,
    config: &TopicInfraConfigRecord,
) -> TopicInfraPreflightReport {
    let auth_secret = match config.auth_mode {
        TopicInfraAuthMode::None => None,
        TopicInfraAuthMode::Password => {
            let report = match config.secret_ref.as_deref() {
                Some(secret_ref) => {
                    probe_secret_ref(storage, user_id, secret_ref, SecretProbeKind::Opaque).await
                }
                None => SecretProbeReport::invalid(
                    "<unset>",
                    "storage",
                    SecretProbeKind::Opaque,
                    "password auth requires secret_ref".to_string(),
                ),
            };
            Some(report)
        }
        TopicInfraAuthMode::PrivateKey => {
            let report = match config.secret_ref.as_deref() {
                Some(secret_ref) => {
                    probe_secret_ref(storage, user_id, secret_ref, SecretProbeKind::SshPrivateKey)
                        .await
                }
                None => SecretProbeReport::invalid(
                    "<unset>",
                    "storage",
                    SecretProbeKind::SshPrivateKey,
                    "private_key auth requires secret_ref".to_string(),
                ),
            };
            Some(report)
        }
    };

    let sudo_secret = match config.sudo_secret_ref.as_deref() {
        Some(secret_ref) => {
            Some(probe_secret_ref(storage, user_id, secret_ref, SecretProbeKind::Opaque).await)
        }
        None => None,
    };

    let provider_enabled = match config.auth_mode {
        TopicInfraAuthMode::None => true,
        TopicInfraAuthMode::Password | TopicInfraAuthMode::PrivateKey => {
            auth_secret.as_ref().is_some_and(|report| report.usable)
        }
    };

    let auth_summary = match config.auth_mode {
        TopicInfraAuthMode::None => {
            "host authentication is delegated to the runtime environment".to_string()
        }
        TopicInfraAuthMode::Password | TopicInfraAuthMode::PrivateKey => auth_secret
            .as_ref()
            .map(SecretProbeReport::summary)
            .unwrap_or_else(|| "auth secret is unavailable".to_string()),
    };
    let sudo_summary = sudo_secret
        .as_ref()
        .map(|report| format!(" Sudo secret check: {}.", report.summary()))
        .unwrap_or_else(|| {
            " Sudo secret check: no sudo secret configured; sudo will rely on passwordless sudo or fail.".to_string()
        });
    let availability = if provider_enabled {
        "ssh_mcp tools are enabled"
    } else {
        "ssh_mcp tools are disabled until auth issues are fixed"
    };
    let summary = format!(
        "SSH target '{}' for topic '{}' uses {}@{}:{} with auth mode {:?}. Auth check: {}. {}.{}",
        config.target_name,
        topic_id,
        config.remote_user,
        config.host,
        config.port,
        config.auth_mode,
        auth_summary,
        availability,
        sudo_summary,
    );

    TopicInfraPreflightReport {
        topic_id: topic_id.to_string(),
        target_name: config.target_name.clone(),
        host: config.host.clone(),
        port: config.port,
        remote_user: config.remote_user.clone(),
        auth_mode: config.auth_mode,
        provider_enabled,
        auth_secret,
        sudo_secret,
        summary,
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandArgs {
    command: String,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadFileArgs {
    path: String,
    #[serde(default)]
    max_bytes: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyFileEditArgs {
    path: String,
    search: String,
    replace: String,
    #[serde(default)]
    create_if_missing: bool,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckProcessArgs {
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    job_id: Option<String>,
    #[serde(default)]
    tail_lines: Option<usize>,
}

#[derive(Debug)]
enum CheckProcessRequest {
    JobId { job_id: String, tail_lines: usize },
    Pattern(String),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SendRemoteFileArgs {
    path: String,
    #[serde(default)]
    file_name: Option<String>,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

struct RemoteOutput {
    stdout: String,
    stderr: String,
    exit_code: i32,
}

struct WrappedRemoteCommand {
    command: String,
    markers: WrappedCommandMarkers,
}

struct WrappedCommandMarkers {
    stdout_begin: String,
    stdout_end: String,
    stderr_begin: String,
    stderr_end: String,
    exit_prefix: String,
}

struct ResolvedBackendAuth {
    password: Option<String>,
    key_file: Option<NamedTempFile>,
    sudo_password: Option<String>,
}

fn extract_call_tool_text(result: &rmcp::model::CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|content| content.raw.as_text().map(|text| text.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_upstream_service_error(error: ServiceError) -> anyhow::Error {
    anyhow!("upstream ssh-mcp request failed: {error}")
}

fn upstream_timeout_ms(timeout_secs: u64) -> u64 {
    timeout_secs
        .saturating_mul(1000)
        .saturating_add(UPSTREAM_TIMEOUT_GRACE_MS)
}

fn normalize_check_process_args(args: CheckProcessArgs) -> Result<CheckProcessRequest> {
    let pattern = args
        .pattern
        .map(|value| validate_non_empty(value, "pattern"));
    let job_id = args.job_id.map(|value| validate_non_empty(value, "job_id"));

    match (pattern.transpose()?, job_id.transpose()?) {
        (Some(pattern), None) => Ok(CheckProcessRequest::Pattern(pattern)),
        (None, Some(job_id)) => Ok(CheckProcessRequest::JobId {
            job_id,
            tail_lines: args.tail_lines.unwrap_or(50).max(1),
        }),
        (None, None) => bail!("either pattern or job_id must be provided"),
        (Some(_), Some(_)) => bail!("provide either pattern or job_id, not both"),
    }
}

fn is_upstream_missing_remote_path_error(error: &anyhow::Error) -> bool {
    error
        .to_string()
        .to_ascii_lowercase()
        .contains("remote_path does not exist")
}

fn truncate_utf8_to_bytes(value: &str, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value.to_string(), false);
    }

    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    (value[..end].to_string(), true)
}

fn typed_read_file_payload(
    response: UpstreamReadFileResponse,
    max_bytes: usize,
) -> (Value, String) {
    let UpstreamReadFileResponse {
        path,
        content,
        mode,
        returned_lines,
        truncated,
        approx_tokens_returned,
        approx_tokens_total_estimate,
        hint,
        sha256,
        read_ticket,
    } = response;
    let (content, locally_truncated) = truncate_utf8_to_bytes(&content, max_bytes);

    let mut payload = json!({
        "ok": true,
        "path": path,
        "content": content,
    });
    if let Some(mode) = mode {
        payload["mode"] = serde_json::Value::String(mode);
    }
    if let Some(returned_lines) = returned_lines {
        payload["returned_lines"] = json!(returned_lines);
    }
    if let Some(approx_tokens_returned) = approx_tokens_returned {
        payload["approx_tokens_returned"] = json!(approx_tokens_returned);
    }
    if let Some(approx_tokens_total_estimate) = approx_tokens_total_estimate {
        payload["approx_tokens_total_estimate"] = json!(approx_tokens_total_estimate);
    }
    if let Some(hint) = hint {
        payload["hint"] = serde_json::Value::String(hint);
    }
    if let Some(sha256) = sha256 {
        payload["sha256"] = serde_json::Value::String(sha256);
    }
    if let Some(read_ticket) = read_ticket {
        payload["read_ticket"] = serde_json::Value::String(read_ticket);
    }
    if truncated.unwrap_or(false) || locally_truncated {
        payload["truncated"] = serde_json::Value::Bool(true);
    }

    (payload, content)
}

fn typed_apply_file_edit_payload(response: UpstreamApplyFileEditResponse, created: bool) -> Value {
    let status = if created {
        "created"
    } else if response.changed {
        "updated"
    } else {
        "unchanged"
    };
    json!({
        "ok": true,
        "path": response.path,
        "status": status,
        "previous_sha256": response.previous_sha256,
        "new_sha256": response.new_sha256,
        "bytes_written": response.bytes_written,
        "changed": response.changed,
    })
}

async fn ssh_delivery_result(
    download_path: &Path,
    source_path: &str,
    delivered_file_name: &str,
    progress_tx: Option<&mpsc::Sender<AgentEvent>>,
) -> Result<(Value, String, bool)> {
    let metadata = tokio::fs::metadata(download_path).await.with_context(|| {
        format!(
            "failed to stat transferred file {}",
            download_path.display()
        )
    })?;
    if metadata.len() == 0 {
        let report = format!(
            "ERROR: File '{}' is empty (0 bytes) and cannot be sent.\nSource path: {}",
            delivered_file_name, source_path
        );
        return Ok((
            json!({
                "ok": false,
                "file_name": delivered_file_name,
                "source_path": source_path,
                "size_bytes": 0,
                "delivery_status": "empty_content",
            }),
            report,
            false,
        ));
    }
    let max_delivery_size_bytes = chat_delivery_max_file_size_bytes();
    if metadata.len() > max_delivery_size_bytes {
        let report = format!(
            "ERROR: File too large for chat delivery (>{:.2} MB). Please use another transfer/upload path.",
            max_delivery_size_bytes as f64 / 1024.0 / 1024.0
        );
        return Ok((
            json!({
                "ok": false,
                "file_name": delivered_file_name,
                "source_path": source_path,
                "size_bytes": metadata.len(),
                "delivery_status": "too_large",
                "max_size_bytes": max_delivery_size_bytes,
            }),
            report,
            false,
        ));
    }

    let content = tokio::fs::read(download_path).await.with_context(|| {
        format!(
            "failed to read transferred file {}",
            download_path.display()
        )
    })?;
    let report = deliver_file_via_progress(
        progress_tx,
        FileDeliveryRequest {
            kind: FileDeliveryKind::Auto,
            file_name: delivered_file_name.to_string(),
            content,
            source_path: source_path.to_string(),
        },
    )
    .await;
    let ok = matches!(report.status, FileDeliveryStatus::Delivered);
    let status = match &report.status {
        FileDeliveryStatus::Delivered => "delivered",
        FileDeliveryStatus::TooLarge { .. } => "too_large",
        FileDeliveryStatus::DeliveryFailed(_) => "delivery_failed",
        FileDeliveryStatus::ConfirmationChannelClosed => "confirmation_channel_closed",
        FileDeliveryStatus::TimedOut => "timed_out",
        FileDeliveryStatus::QueueUnavailable(_) => "queue_unavailable",
        FileDeliveryStatus::EmptyContent => "empty_content",
    };
    let report_text = format_generic_delivery_report(&report);
    Ok((
        json!({
            "ok": ok,
            "file_name": report.file_name,
            "source_path": report.source_path,
            "size_bytes": report.size_bytes,
            "delivery_status": status,
            "file_id": report.receipt.as_ref().and_then(|receipt| receipt.file_id.clone()),
            "download_url": report.receipt.as_ref().and_then(|receipt| receipt.download_url.clone()),
        }),
        report_text,
        ok,
    ))
}

async fn resolve_secret_ref(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    secret_ref: &str,
) -> Result<String> {
    if let Some(env_name) = secret_ref.strip_prefix("env:") {
        return std::env::var(env_name)
            .with_context(|| format!("missing environment secret '{env_name}'"));
    }

    let storage_key = secret_ref.strip_prefix("storage:").unwrap_or(secret_ref);
    storage
        .get_secret_value(user_id, storage_key.to_string())
        .await
        .map_err(|err| anyhow!("failed to load secret ref '{storage_key}': {err}"))?
        .ok_or_else(|| anyhow!("secret ref '{storage_key}' is not provisioned"))
}

async fn resolve_backend_auth(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    config: &TopicInfraConfigRecord,
) -> Result<ResolvedBackendAuth> {
    let password = match config.auth_mode {
        TopicInfraAuthMode::Password => {
            let secret_ref = config
                .secret_ref
                .as_deref()
                .ok_or_else(|| anyhow!("password auth requires secret_ref"))?;
            Some(resolve_secret_ref(storage, user_id, secret_ref).await?)
        }
        TopicInfraAuthMode::None | TopicInfraAuthMode::PrivateKey => None,
    };

    let key_file = match config.auth_mode {
        TopicInfraAuthMode::PrivateKey => {
            let secret_ref = config
                .secret_ref
                .as_deref()
                .ok_or_else(|| anyhow!("private_key auth requires secret_ref"))?;
            let private_key = resolve_secret_ref(storage, user_id, secret_ref).await?;
            Some(write_private_key_tempfile(&private_key)?)
        }
        TopicInfraAuthMode::None | TopicInfraAuthMode::Password => None,
    };

    let sudo_password = match config.sudo_secret_ref.as_deref() {
        Some(secret_ref) => Some(resolve_secret_ref(storage, user_id, secret_ref).await?),
        None => None,
    };

    Ok(ResolvedBackendAuth {
        password,
        key_file,
        sudo_password,
    })
}

fn build_wrapped_remote_command(command: &str) -> WrappedRemoteCommand {
    let token = Uuid::new_v4().simple().to_string();
    let markers = WrappedCommandMarkers {
        stdout_begin: format!("__OXIDE_SSH_STDOUT_BEGIN_{token}__"),
        stdout_end: format!("__OXIDE_SSH_STDOUT_END_{token}__"),
        stderr_begin: format!("__OXIDE_SSH_STDERR_BEGIN_{token}__"),
        stderr_end: format!("__OXIDE_SSH_STDERR_END_{token}__"),
        exit_prefix: format!("__OXIDE_SSH_EXIT_{token}__="),
    };
    let wrapped = format!(
        "tmp_dir=$(mktemp -d 2>/dev/null || mktemp -d -t oxide-agent-ssh) || exit 125; stdout_file=\"$tmp_dir/stdout\"; stderr_file=\"$tmp_dir/stderr\"; cleanup() {{ rm -rf \"$tmp_dir\"; }}; trap cleanup EXIT; rc=0; sh -lc {} >\"$stdout_file\" 2>\"$stderr_file\" || rc=$?; printf '%s\\n' {}; od -An -tx1 -v \"$stdout_file\" | tr -d ' \\n'; printf '\\n%s\\n' {}; printf '%s\\n' {}; od -An -tx1 -v \"$stderr_file\" | tr -d ' \\n'; printf '\\n%s\\n' {}; printf '%s%s\\n' {} \"$rc\"; exit 0",
        escape_shell_argument(command),
        escape_shell_argument(&markers.stdout_begin),
        escape_shell_argument(&markers.stdout_end),
        escape_shell_argument(&markers.stderr_begin),
        escape_shell_argument(&markers.stderr_end),
        escape_shell_argument(&markers.exit_prefix),
    );
    WrappedRemoteCommand {
        command: wrapped,
        markers,
    }
}

fn parse_wrapped_remote_output(
    markers: &WrappedCommandMarkers,
    raw_output: &str,
) -> Result<RemoteOutput> {
    let stdout_hex = extract_marker_body(raw_output, &markers.stdout_begin, &markers.stdout_end)?;
    let stderr_hex = extract_marker_body(raw_output, &markers.stderr_begin, &markers.stderr_end)?;
    let exit_code = extract_exit_code(raw_output, &markers.exit_prefix)?;

    let stdout = String::from_utf8_lossy(&decode_hex(stdout_hex)?).to_string();
    let stderr = String::from_utf8_lossy(&decode_hex(stderr_hex)?).to_string();

    Ok(RemoteOutput {
        stdout: truncate(&stdout, MAX_REMOTE_OUTPUT_CHARS),
        stderr: truncate(&stderr, MAX_REMOTE_OUTPUT_CHARS),
        exit_code,
    })
}

fn extract_marker_body<'a>(raw: &'a str, begin: &str, end: &str) -> Result<&'a str> {
    let begin_marker = format!("{begin}\n");
    let begin_index = raw
        .find(&begin_marker)
        .ok_or_else(|| anyhow!("upstream ssh-mcp response is missing marker '{begin}'"))?
        + begin_marker.len();
    let end_marker = format!("\n{end}\n");
    let end_index = raw[begin_index..]
        .find(&end_marker)
        .map(|index| begin_index + index)
        .ok_or_else(|| anyhow!("upstream ssh-mcp response is missing marker '{end}'"))?;
    Ok(&raw[begin_index..end_index])
}

fn extract_exit_code(raw: &str, exit_prefix: &str) -> Result<i32> {
    let start = raw
        .rfind(exit_prefix)
        .ok_or_else(|| anyhow!("upstream ssh-mcp response is missing exit code marker"))?
        + exit_prefix.len();
    let value = raw[start..]
        .lines()
        .next()
        .ok_or_else(|| anyhow!("upstream ssh-mcp response exit code is empty"))?;
    value
        .trim()
        .parse::<i32>()
        .map_err(|err| anyhow!("invalid upstream ssh-mcp exit code '{value}': {err}"))
}

fn decode_hex(value: &str) -> Result<Vec<u8>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if !trimmed.len().is_multiple_of(2) {
        bail!("invalid hex payload length {}", trimmed.len());
    }

    let mut bytes = Vec::with_capacity(trimmed.len() / 2);
    let raw = trimmed.as_bytes();
    let mut index = 0;
    while index < raw.len() {
        let high = hex_value(raw[index])?;
        let low = hex_value(raw[index + 1])?;
        bytes.push((high << 4) | low);
        index += 2;
    }
    Ok(bytes)
}

fn hex_value(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => bail!("invalid hex digit '{}'", byte as char),
    }
}

async fn run_command_with_timeout(mut command: Command, timeout_secs: u64) -> Result<RemoteOutput> {
    let wait_result = timeout(Duration::from_secs(timeout_secs), command.output()).await;
    match wait_result {
        Ok(output) => {
            let output = output.context("failed to wait for SSH command")?;
            Ok(RemoteOutput {
                stdout: truncate(
                    &String::from_utf8_lossy(&output.stdout),
                    MAX_REMOTE_OUTPUT_CHARS,
                ),
                stderr: truncate(
                    &String::from_utf8_lossy(&output.stderr),
                    MAX_REMOTE_OUTPUT_CHARS,
                ),
                exit_code: output.status.code().unwrap_or(-1),
            })
        }
        Err(_) => bail!("remote SSH command timed out after {timeout_secs} seconds"),
    }
}

async fn resolve_secret_material(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    secret_ref: &str,
) -> Result<ResolvedSecretMaterial> {
    if let Some(env_name) = secret_ref.strip_prefix("env:") {
        return Ok(ResolvedSecretMaterial {
            source: "env",
            value: std::env::var(env_name).ok(),
        });
    }

    let storage_key = secret_ref.strip_prefix("storage:").unwrap_or(secret_ref);
    let value = storage
        .get_secret_value(user_id, storage_key.to_string())
        .await
        .map_err(|err| anyhow!("failed to load secret ref '{storage_key}': {err}"))?;
    Ok(ResolvedSecretMaterial {
        source: "storage",
        value,
    })
}

fn secret_source(secret_ref: &str) -> &'static str {
    if secret_ref.starts_with("env:") {
        "env"
    } else {
        "storage"
    }
}

fn validate_opaque_secret(secret_ref: &str, source: &str, value: &str) -> SecretProbeReport {
    if value.trim().is_empty() {
        return SecretProbeReport::invalid(
            secret_ref,
            source,
            SecretProbeKind::Opaque,
            "secret payload is empty".to_string(),
        );
    }

    SecretProbeReport::valid(secret_ref, source, SecretProbeKind::Opaque)
}

async fn validate_ssh_private_key(
    secret_ref: &str,
    source: &str,
    private_key: &str,
) -> SecretProbeReport {
    if private_key.trim().is_empty() {
        return SecretProbeReport::invalid(
            secret_ref,
            source,
            SecretProbeKind::SshPrivateKey,
            "secret payload is empty".to_string(),
        );
    }

    let key_file = match write_private_key_tempfile(private_key) {
        Ok(file) => file,
        Err(error) => {
            return SecretProbeReport::invalid(
                secret_ref,
                source,
                SecretProbeKind::SshPrivateKey,
                error.to_string(),
            );
        }
    };
    let key_path = key_file.path();

    let mut public_command = Command::new("ssh-keygen");
    public_command.arg("-y").arg("-f").arg(key_path);
    let public_result = run_command_with_timeout(public_command, KEY_PROBE_TIMEOUT_SECS).await;

    match public_result {
        Ok(output) if output.exit_code == 0 => {
            let mut listing_command = Command::new("ssh-keygen");
            listing_command.arg("-l").arg("-f").arg(key_path);
            match run_command_with_timeout(listing_command, KEY_PROBE_TIMEOUT_SECS).await {
                Ok(listing_output) if listing_output.exit_code == 0 => {
                    let mut report = SecretProbeReport::valid(
                        secret_ref,
                        source,
                        SecretProbeKind::SshPrivateKey,
                    );
                    let listing = listing_output.stdout.trim();
                    if let Some((fingerprint, key_type, comment)) =
                        parse_ssh_keygen_listing(listing, key_path)
                    {
                        report.fingerprint = Some(fingerprint);
                        report.key_type = key_type;
                        report.comment = comment;
                    }
                    report
                }
                Ok(listing_output) => SecretProbeReport::invalid(
                    secret_ref,
                    source,
                    SecretProbeKind::SshPrivateKey,
                    format!(
                        "ssh-keygen -l failed with exit code {}{}",
                        listing_output.exit_code,
                        format_stderr_suffix(&listing_output.stderr)
                    ),
                ),
                Err(error) => SecretProbeReport::invalid(
                    secret_ref,
                    source,
                    SecretProbeKind::SshPrivateKey,
                    format!("ssh-keygen -l failed: {error}"),
                ),
            }
        }
        Ok(output) => SecretProbeReport::invalid(
            secret_ref,
            source,
            SecretProbeKind::SshPrivateKey,
            format!(
                "ssh-keygen -y failed with exit code {}{}",
                output.exit_code,
                format_stderr_suffix(&output.stderr)
            ),
        ),
        Err(error) => SecretProbeReport::invalid(
            secret_ref,
            source,
            SecretProbeKind::SshPrivateKey,
            format!("ssh-keygen -y failed: {error}"),
        ),
    }
}

fn format_stderr_suffix(stderr: &str) -> String {
    let trimmed = stderr.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!(": {trimmed}")
    }
}

fn parse_ssh_keygen_listing(
    listing: &str,
    key_path: &Path,
) -> Option<(String, Option<String>, Option<String>)> {
    let tokens = listing.split_whitespace().collect::<Vec<_>>();
    if tokens.len() < 2 {
        return None;
    }

    let fingerprint = tokens.get(1)?.to_string();
    let key_type = tokens
        .last()
        .filter(|token| token.starts_with('(') && token.ends_with(')'))
        .map(|token| token.trim_matches(|c| c == '(' || c == ')').to_string());
    let comment_end = if key_type.is_some() {
        tokens.len().saturating_sub(1)
    } else {
        tokens.len()
    };
    let raw_comment = if comment_end > 2 {
        Some(tokens[2..comment_end].join(" "))
    } else {
        None
    };
    let temp_path = key_path.display().to_string();
    let comment = raw_comment.and_then(|comment| {
        let trimmed = comment.trim();
        if trimmed.is_empty() || trimmed == temp_path {
            None
        } else {
            Some(trimmed.to_string())
        }
    });

    Some((fingerprint, key_type, comment))
}

fn validate_non_empty(value: String, field_name: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        bail!("{field_name} must not be empty");
    }
    Ok(trimmed.to_string())
}

fn validate_chat_file_name(value: String) -> Result<String> {
    let trimmed = validate_non_empty(value, "file_name")?;
    if trimmed.contains('/') || trimmed.contains('\\') {
        bail!("file_name must not contain path separators");
    }
    if trimmed.chars().any(char::is_control) {
        bail!("file_name must not contain control characters");
    }
    Ok(trimmed)
}

fn default_remote_file_name(remote_path: &str) -> String {
    Path::new(remote_path)
        .file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "file".to_string())
}

fn sanitize_transfer_local_component(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| match ch {
            '/' | '\\' => '_',
            _ if ch.is_control() => '_',
            _ => ch,
        })
        .collect::<String>();
    let trimmed = sanitized.trim_matches('.').trim();
    if trimmed.is_empty() {
        "file".to_string()
    } else {
        trimmed.to_string()
    }
}

fn unique_transfer_local_path(file_name: &str) -> String {
    format!(
        "{}-{}",
        Uuid::new_v4().simple(),
        sanitize_transfer_local_component(file_name)
    )
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut truncated = value.chars().take(max_chars).collect::<String>();
    truncated.push_str("...");
    truncated
}

fn escape_shell_argument(value: &str) -> String {
    escape(Cow::Borrowed(value)).into_owned()
}

/// Remove any stale SSH private key temp files left from previous process runs.
pub(crate) fn cleanup_stale_private_key_tempfiles() -> Result<usize> {
    cleanup_stale_private_key_tempfiles_in(&std::env::temp_dir())
}

fn cleanup_stale_private_key_tempfiles_in(temp_dir: &Path) -> Result<usize> {
    let mut removed = 0_usize;
    let entries = fs::read_dir(temp_dir)
        .with_context(|| format!("failed to read temp directory {}", temp_dir.display()))?;

    for entry in entries {
        let entry = entry.with_context(|| {
            format!(
                "failed to inspect temp directory entry in {}",
                temp_dir.display()
            )
        })?;
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        if !file_name.starts_with(PRIVATE_KEY_TEMPFILE_PREFIX) {
            continue;
        }

        match fs::remove_file(entry.path()) {
            Ok(()) => removed += 1,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to remove stale private key temp file {}",
                        entry.path().display()
                    )
                });
            }
        }
    }

    Ok(removed)
}

fn write_private_key_tempfile(private_key: &str) -> Result<NamedTempFile> {
    write_private_key_tempfile_in(&std::env::temp_dir(), private_key)
}

fn write_private_key_tempfile_in(temp_dir: &Path, private_key: &str) -> Result<NamedTempFile> {
    let mut file = Builder::new()
        .prefix(PRIVATE_KEY_TEMPFILE_PREFIX)
        .tempfile_in(temp_dir)
        .with_context(|| {
            format!(
                "failed to create private key temp file in {}",
                temp_dir.display()
            )
        })?;
    file.write_all(private_key.as_bytes()).with_context(|| {
        format!(
            "failed to write private key temp file at {}",
            file.path().display()
        )
    })?;
    file.flush().with_context(|| {
        format!(
            "failed to flush private key temp file at {}",
            file.path().display()
        )
    })?;
    Ok(file)
}

/// Build a safe system message describing SSH preflight status for the current topic.
pub fn inject_topic_infra_preflight_system_message(
    report: &TopicInfraPreflightReport,
) -> AgentMessage {
    AgentMessage::infra_status(format!(
        "Topic-scoped SSH preflight status: {} Never request, reveal, or print the underlying secret material.",
        report.summary
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        RemoteOutput, SecretProbeKind, SecretProbeReport, TopicInfraPreflightReport,
        UpstreamApplyFileEditResponse, UpstreamReadFileResponse, WrappedCommandMarkers,
        cleanup_stale_private_key_tempfiles_in, decode_hex, default_remote_file_name,
        inject_topic_infra_preflight_system_message, normalize_check_process_args,
        parse_ssh_keygen_listing, parse_wrapped_remote_output, typed_apply_file_edit_payload,
        typed_read_file_payload, typed_ssh_exec_output, unique_transfer_local_path,
        validate_chat_file_name, write_private_key_tempfile_in,
    };
    use crate::agent::identity::SessionId;
    use crate::agent::tool_runtime::{
        CleanupStatus, ModelMetadata, ProviderMetadata, ToolBatchId, ToolCallId,
        ToolExecutionContext, ToolInvocation, ToolName, ToolOutputStatus, ToolTimeoutConfig,
        TurnId,
    };
    use crate::llm::InvocationId;
    use crate::storage::{TopicInfraAuthMode, TopicInfraConfigRecord, TopicInfraToolMode};
    use chrono::Utc;
    use serde_json::json;
    use std::{fs, path::Path, path::PathBuf};
    use tempfile::tempdir;

    #[test]
    fn default_remote_file_name_uses_basename() {
        assert_eq!(default_remote_file_name("/var/log/app.log"), "app.log");
        assert_eq!(default_remote_file_name("/"), "file");
    }

    #[test]
    fn transfer_local_path_is_sanitized_and_unique() {
        let path = unique_transfer_local_path("../app.log");
        assert!(!path.contains('/'));
        assert!(!path.contains(".."));
    }

    #[test]
    fn chat_file_name_rejects_path_separators() {
        assert!(validate_chat_file_name("app.log".to_string()).is_ok());
        assert!(validate_chat_file_name("foo/bar.log".to_string()).is_err());
    }

    #[test]
    fn ssh_keygen_listing_parser_extracts_safe_metadata() {
        let parsed = parse_ssh_keygen_listing(
            "256 SHA256:abc123 deploy@example (ED25519)",
            Path::new("/tmp/oxide-agent-ssh-key-test"),
        )
        .expect("listing should parse");
        assert_eq!(parsed.0, "SHA256:abc123");
        assert_eq!(parsed.1.as_deref(), Some("ED25519"));
        assert_eq!(parsed.2.as_deref(), Some("deploy@example"));
    }

    #[test]
    fn topic_infra_preflight_system_message_never_contains_secret_material() {
        let report = TopicInfraPreflightReport {
            topic_id: "topic-a".to_string(),
            target_name: "prod-app".to_string(),
            host: "prod.example.com".to_string(),
            port: 22,
            remote_user: "deploy".to_string(),
            auth_mode: TopicInfraAuthMode::PrivateKey,
            provider_enabled: false,
            auth_secret: Some(SecretProbeReport {
                secret_ref: "storage:vds".to_string(),
                source: "storage".to_string(),
                kind: SecretProbeKind::SshPrivateKey,
                present: true,
                usable: false,
                status: "invalid".to_string(),
                fingerprint: None,
                key_type: None,
                comment: None,
                error: Some("ssh-keygen -y failed".to_string()),
            }),
            sudo_secret: None,
            summary: "safe summary only".to_string(),
        };

        let message = inject_topic_infra_preflight_system_message(&report);
        assert!(message.content.contains("safe summary only"));
        assert!(!message.content.contains("BEGIN OPENSSH PRIVATE KEY"));
    }

    #[test]
    fn decode_hex_round_trips_ascii_payload() {
        let decoded = decode_hex("68656c6c6f20776f726c64").expect("hex should decode");
        assert_eq!(String::from_utf8_lossy(&decoded), "hello world");
    }

    #[test]
    fn wrapped_remote_output_parser_extracts_sections() {
        let markers = WrappedCommandMarkers {
            stdout_begin: "__OUT_BEGIN__".to_string(),
            stdout_end: "__OUT_END__".to_string(),
            stderr_begin: "__ERR_BEGIN__".to_string(),
            stderr_end: "__ERR_END__".to_string(),
            exit_prefix: "__EXIT__=".to_string(),
        };
        let raw = "__OUT_BEGIN__\n68656c6c6f\n__OUT_END__\n__ERR_BEGIN__\n7761726e\n__ERR_END__\n__EXIT__=7\n";
        let parsed =
            parse_wrapped_remote_output(&markers, raw).expect("wrapped output should parse");

        assert_eq!(parsed.stdout, "hello");
        assert_eq!(parsed.stderr, "warn");
        assert_eq!(parsed.exit_code, 7);
    }

    #[test]
    fn check_process_tool_schema_uses_chatgpt_compatible_top_level_object() {
        let tool = super::SshMcpProvider::runtime_tool_definitions()
            .into_iter()
            .find(|tool| tool.name == "ssh_check_process")
            .expect("ssh_check_process tool definition should exist");

        let parameters = &tool.parameters;
        assert_eq!(parameters["type"], json!("object"));
        for keyword in ["oneOf", "anyOf", "allOf", "enum", "not"] {
            assert!(
                parameters.get(keyword).is_none(),
                "top-level {keyword} should not be present"
            );
        }

        let properties = parameters["properties"]
            .as_object()
            .expect("tool parameters should define properties");
        for property in ["pattern", "job_id", "tail_lines"] {
            assert!(
                properties.contains_key(property),
                "{property} property should be present"
            );
        }
    }

    #[test]
    fn typed_ssh_exec_output_preserves_exit_code_and_remote_cleanup_status() {
        let invocation = test_invocation("ssh_exec");
        let config = test_topic_config();
        let output = typed_ssh_exec_output(
            &invocation,
            &config,
            "false",
            false,
            RemoteOutput {
                stdout: "out".to_string(),
                stderr: "err".to_string(),
                exit_code: 7,
            },
        );

        assert_eq!(output.status, ToolOutputStatus::Failure);
        assert_eq!(output.exit_code, Some(7));
        assert_eq!(output.stdout.text.as_deref(), Some("out"));
        assert_eq!(output.stderr.text.as_deref(), Some("err"));
        assert_eq!(
            output.cleanup_status,
            CleanupStatus::BestEffortRemoteCleanup
        );
        assert_eq!(
            output
                .structured_payload
                .as_ref()
                .and_then(|payload: &serde_json::Value| payload.get("target_name"))
                .and_then(serde_json::Value::as_str),
            Some("prod-app")
        );
    }

    #[test]
    fn normalize_check_process_args_accepts_job_id_or_pattern() {
        let job = normalize_check_process_args(super::CheckProcessArgs {
            pattern: None,
            job_id: Some("job-1".to_string()),
            tail_lines: Some(20),
        })
        .expect("job args should validate");
        assert!(matches!(
            job,
            super::CheckProcessRequest::JobId {
                job_id,
                tail_lines: 20
            } if job_id == "job-1"
        ));

        let pattern = normalize_check_process_args(super::CheckProcessArgs {
            pattern: Some("nginx".to_string()),
            job_id: None,
            tail_lines: None,
        })
        .expect("pattern args should validate");
        assert!(matches!(
            pattern,
            super::CheckProcessRequest::Pattern(value) if value == "nginx"
        ));
    }

    #[test]
    fn normalize_check_process_args_rejects_missing_or_ambiguous_input() {
        let missing = normalize_check_process_args(super::CheckProcessArgs {
            pattern: None,
            job_id: None,
            tail_lines: None,
        })
        .expect_err("missing args should fail");
        assert!(missing.to_string().contains("either pattern or job_id"));

        let ambiguous = normalize_check_process_args(super::CheckProcessArgs {
            pattern: Some("nginx".to_string()),
            job_id: Some("job-1".to_string()),
            tail_lines: None,
        })
        .expect_err("ambiguous args should fail");
        assert!(ambiguous.to_string().contains("either pattern or job_id"));
    }

    fn test_topic_config() -> TopicInfraConfigRecord {
        TopicInfraConfigRecord {
            schema_version: 1,
            version: 1,
            user_id: 42,
            topic_id: "topic-a".to_string(),
            target_name: "prod-app".to_string(),
            host: "prod.example.com".to_string(),
            port: 22,
            remote_user: "deploy".to_string(),
            auth_mode: TopicInfraAuthMode::PrivateKey,
            secret_ref: Some("storage:ssh/key".to_string()),
            sudo_secret_ref: None,
            environment: Some("prod".to_string()),
            tags: Vec::new(),
            allowed_tool_modes: vec![
                TopicInfraToolMode::Exec,
                TopicInfraToolMode::SudoExec,
                TopicInfraToolMode::CheckProcess,
            ],
            created_at: 0,
            updated_at: 0,
        }
    }

    fn test_invocation(tool_name: &str) -> ToolInvocation {
        let now = Utc::now();
        ToolInvocation {
            session_id: SessionId::from(42),
            turn_id: TurnId::from("turn_ssh"),
            batch_id: ToolBatchId::from("batch_ssh"),
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

    #[test]
    fn typed_read_file_payload_preserves_metadata_and_local_truncation() {
        let response = UpstreamReadFileResponse {
            path: "/etc/app.conf".to_string(),
            content: "abcdef".to_string(),
            mode: Some("full".to_string()),
            returned_lines: Some(1),
            truncated: Some(false),
            approx_tokens_returned: Some(2),
            approx_tokens_total_estimate: Some(2),
            hint: Some("hint".to_string()),
            sha256: Some("deadbeef".to_string()),
            read_ticket: Some("ticket-1".to_string()),
        };

        let (value, content) = typed_read_file_payload(response, 4);
        assert_eq!(content, "abcd");
        assert_eq!(value.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(value.get("content").and_then(|v| v.as_str()), Some("abcd"));
        assert_eq!(value.get("truncated").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(
            value.get("read_ticket").and_then(|v| v.as_str()),
            Some("ticket-1")
        );
    }

    #[test]
    fn typed_apply_file_edit_payload_maps_status() {
        let response = UpstreamApplyFileEditResponse {
            path: "/etc/app.conf".to_string(),
            previous_sha256: "old".to_string(),
            new_sha256: "new".to_string(),
            bytes_written: 42,
            changed: true,
        };

        let value = typed_apply_file_edit_payload(response, false);
        assert_eq!(
            value.get("status").and_then(|v| v.as_str()),
            Some("updated")
        );
        assert_eq!(value.get("changed").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn private_key_tempfile_is_removed_on_drop() {
        let temp_dir = tempdir().expect("temp dir must be created");
        let path = {
            let key_file = write_private_key_tempfile_in(temp_dir.path(), "secret")
                .expect("temp key file must be created");
            let path = key_file.path().to_path_buf();
            assert!(path.exists());
            path
        };

        assert!(!path.exists());
    }

    #[test]
    fn cleanup_stale_private_key_tempfiles_removes_only_matching_files() {
        let temp_dir = tempdir().expect("temp dir must be created");
        let stale_path = temp_dir.path().join("oxide-agent-ssh-key-stale-test");
        let keep_path = temp_dir.path().join("keep-me.txt");

        fs::write(&stale_path, "secret").expect("stale file must be written");
        fs::write(&keep_path, "safe").expect("control file must be written");

        let removed =
            cleanup_stale_private_key_tempfiles_in(temp_dir.path()).expect("cleanup must succeed");

        assert_eq!(removed, 1);
        assert!(!stale_path.exists());
        assert!(keep_path.exists());
    }
}
