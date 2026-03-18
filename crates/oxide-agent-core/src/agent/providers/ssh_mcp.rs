//! Topic-scoped SSH infrastructure provider with approval gating.

use crate::agent::memory::AgentMessage;
use crate::agent::progress::AgentEvent;
use crate::agent::provider::ToolProvider;
use crate::llm::ToolDefinition;
use crate::storage::{
    StorageProvider, TopicInfraAuthMode, TopicInfraConfigRecord, TopicInfraToolMode,
};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use rmcp::{
    model::CallToolRequestParams,
    service::{Peer, RoleClient, RunningService, ServiceError},
    transport::{ConfigureCommandExt, TokioChildProcess},
    ServiceExt,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use shell_escape::unix::escape;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::{Builder, NamedTempFile};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::{timeout, Duration};
use uuid::Uuid;

const TOOL_SSH_EXEC: &str = "ssh_exec";
const TOOL_SSH_SUDO_EXEC: &str = "ssh_sudo_exec";
const TOOL_SSH_READ_FILE: &str = "ssh_read_file";
const TOOL_SSH_APPLY_FILE_EDIT: &str = "ssh_apply_file_edit";
const TOOL_SSH_CHECK_PROCESS: &str = "ssh_check_process";

const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_REMOTE_OUTPUT_CHARS: usize = 16_000;
const APPROVAL_TTL_SECS: i64 = 600;
const KEY_PROBE_TIMEOUT_SECS: u64 = 10;
const DEFAULT_UPSTREAM_SSH_MCP_BINARY_PATH: &str = "/usr/local/bin/ssh-mcp";
const UPSTREAM_SSH_MCP_BINARY_ENV: &str = "OXIDE_SSH_MCP_BINARY";
const UPSTREAM_TOOL_EXEC: &str = "exec";
const UPSTREAM_TOOL_SUDO_EXEC: &str = "sudo-exec";
const UPSTREAM_TIMEOUT_GRACE_MS: u64 = 30_000;
const UPSTREAM_MAX_OUTPUT_TOKENS: usize = 12_000;
const PRIVATE_KEY_TEMPFILE_PREFIX: &str = "oxide-agent-ssh-key-";

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

/// Transport-facing view of a pending SSH approval request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshApprovalRequestView {
    /// Stable approval request identifier.
    pub request_id: String,
    /// Tool name awaiting approval.
    pub tool_name: String,
    /// Topic associated with the request.
    pub topic_id: String,
    /// Human-readable infra target name.
    pub target_name: String,
    /// Operator-facing summary of the pending action.
    pub summary: String,
    /// Creation timestamp (unix seconds).
    pub created_at: i64,
    /// Expiry timestamp (unix seconds).
    pub expires_at: i64,
}

/// Granted SSH approval token returned after operator confirmation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshApprovalGrant {
    /// Stable approval request identifier.
    pub request_id: String,
    /// Single-use approval token.
    pub approval_token: String,
    /// Tool name that may now be replayed.
    pub tool_name: String,
    /// Topic associated with the request.
    pub topic_id: String,
    /// Human-readable infra target name.
    pub target_name: String,
    /// Operator-facing summary of the pending action.
    pub summary: String,
    /// Expiry timestamp (unix seconds).
    pub expires_at: i64,
}

/// In-memory short-lived approval registry for topic-scoped SSH actions.
#[derive(Clone, Default)]
pub struct SshApprovalRegistry {
    inner: Arc<Mutex<HashMap<String, ApprovalEntry>>>,
}

#[derive(Clone)]
struct ApprovalEntry {
    view: SshApprovalRequestView,
    fingerprint: String,
    state: ApprovalState,
    announced: bool,
}

#[derive(Clone)]
enum ApprovalState {
    Pending,
    Approved { token: String, expires_at: i64 },
    Rejected,
    Consumed,
}

impl SshApprovalRegistry {
    /// Create an empty approval registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new pending SSH approval request.
    pub async fn register(
        &self,
        tool_name: &str,
        topic_id: &str,
        target_name: &str,
        summary: String,
        fingerprint: String,
    ) -> SshApprovalRequestView {
        let now = now_unix_secs();
        let view = SshApprovalRequestView {
            request_id: Uuid::new_v4().to_string(),
            tool_name: tool_name.to_string(),
            topic_id: topic_id.to_string(),
            target_name: target_name.to_string(),
            summary,
            created_at: now,
            expires_at: now + APPROVAL_TTL_SECS,
        };
        let entry = ApprovalEntry {
            view: view.clone(),
            fingerprint,
            state: ApprovalState::Pending,
            announced: false,
        };
        let mut guard = self.inner.lock().await;
        purge_expired_entries(&mut guard, now);
        guard.insert(view.request_id.clone(), entry);
        view
    }

    /// Return pending approvals that have not yet been surfaced to the transport.
    pub async fn take_unannounced(&self) -> Vec<SshApprovalRequestView> {
        let now = now_unix_secs();
        let mut guard = self.inner.lock().await;
        purge_expired_entries(&mut guard, now);
        let mut pending = Vec::new();
        for entry in guard.values_mut() {
            if !matches!(entry.state, ApprovalState::Pending) || entry.announced {
                continue;
            }
            entry.announced = true;
            pending.push(entry.view.clone());
        }
        pending.sort_by(|left, right| left.created_at.cmp(&right.created_at));
        pending
    }

    /// Mark a pending approval request as granted and mint a replay token.
    pub async fn grant(&self, request_id: &str) -> Option<SshApprovalGrant> {
        let now = now_unix_secs();
        let mut guard = self.inner.lock().await;
        purge_expired_entries(&mut guard, now);
        let entry = guard.get_mut(request_id)?;
        if !matches!(entry.state, ApprovalState::Pending) {
            return None;
        }
        let token = Uuid::new_v4().to_string();
        let expires_at = now + APPROVAL_TTL_SECS;
        entry.state = ApprovalState::Approved {
            token: token.clone(),
            expires_at,
        };
        Some(SshApprovalGrant {
            request_id: entry.view.request_id.clone(),
            approval_token: token,
            tool_name: entry.view.tool_name.clone(),
            topic_id: entry.view.topic_id.clone(),
            target_name: entry.view.target_name.clone(),
            summary: entry.view.summary.clone(),
            expires_at,
        })
    }

    /// Mark an existing approval request as rejected.
    pub async fn reject(&self, request_id: &str) -> Option<SshApprovalRequestView> {
        let now = now_unix_secs();
        let mut guard = self.inner.lock().await;
        purge_expired_entries(&mut guard, now);
        let entry = guard.get_mut(request_id)?;
        if !matches!(
            entry.state,
            ApprovalState::Pending | ApprovalState::Approved { .. }
        ) {
            return None;
        }
        entry.state = ApprovalState::Rejected;
        Some(entry.view.clone())
    }

    /// Consume a granted approval token for a specific replayed SSH action fingerprint.
    pub async fn consume(
        &self,
        request_id: &str,
        approval_token: &str,
        fingerprint: &str,
    ) -> Result<()> {
        let now = now_unix_secs();
        let mut guard = self.inner.lock().await;
        purge_expired_entries(&mut guard, now);
        let entry = guard
            .get_mut(request_id)
            .ok_or_else(|| anyhow!("approval request not found or expired"))?;
        if entry.fingerprint != fingerprint {
            bail!("approval token does not match the original SSH action");
        }
        match &entry.state {
            ApprovalState::Approved { token, expires_at } => {
                if token != approval_token {
                    bail!("approval token is invalid");
                }
                if *expires_at < now {
                    bail!("approval token has expired");
                }
                entry.state = ApprovalState::Consumed;
                Ok(())
            }
            ApprovalState::Pending => bail!("approval has not been granted yet"),
            ApprovalState::Rejected => bail!("approval request was rejected"),
            ApprovalState::Consumed => bail!("approval token has already been used"),
        }
    }
}

fn purge_expired_entries(entries: &mut HashMap<String, ApprovalEntry>, now: i64) {
    entries.retain(|_, entry| match entry.state {
        ApprovalState::Pending => entry.view.expires_at >= now,
        ApprovalState::Approved { expires_at, .. } => expires_at >= now,
        ApprovalState::Rejected | ApprovalState::Consumed => false,
    });
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
        if let Ok(mut stderr_task) = self.stderr_task.try_lock() {
            if let Some(task) = stderr_task.take() {
                task.abort();
            }
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
                .arg("--log-level=warn");

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
    config: TopicInfraConfigRecord,
    approvals: SshApprovalRegistry,
    backend: SshExecutionBackend,
}

impl SshMcpProvider {
    /// Create a new topic-scoped SSH provider instance.
    #[must_use]
    pub fn new(
        storage: Arc<dyn StorageProvider>,
        user_id: i64,
        topic_id: String,
        config: TopicInfraConfigRecord,
        approvals: SshApprovalRegistry,
    ) -> Self {
        Self {
            backend: SshExecutionBackend::Upstream(UpstreamSshMcpBackend::new(
                Arc::clone(&storage),
                user_id,
                config.clone(),
            )),
            topic_id,
            config,
            approvals,
        }
    }

    /// Shared approval registry used by this provider.
    #[must_use]
    pub fn approvals(&self) -> SshApprovalRegistry {
        self.approvals.clone()
    }

    fn tool_definitions() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: TOOL_SSH_EXEC.to_string(),
                description: "Run a remote SSH command on the topic infra target".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Remote shell command" },
                        "timeout_secs": { "type": "integer", "description": "Optional timeout in seconds" },
                        "approval_request_id": { "type": "string", "description": "Approval request id for replay after operator confirmation" },
                        "approval_token": { "type": "string", "description": "Approval token issued by operator confirmation" }
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
                        "timeout_secs": { "type": "integer", "description": "Optional timeout in seconds" },
                        "approval_request_id": { "type": "string", "description": "Approval request id for replay after operator confirmation" },
                        "approval_token": { "type": "string", "description": "Approval token issued by operator confirmation" }
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
                        "max_bytes": { "type": "integer", "description": "Optional maximum bytes to read" },
                        "approval_request_id": { "type": "string", "description": "Approval request id for replay after operator confirmation" },
                        "approval_token": { "type": "string", "description": "Approval token issued by operator confirmation" }
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
                        "timeout_secs": { "type": "integer", "description": "Optional timeout in seconds" },
                        "approval_request_id": { "type": "string", "description": "Approval request id for replay after operator confirmation" },
                        "approval_token": { "type": "string", "description": "Approval token issued by operator confirmation" }
                    },
                    "required": ["path", "search", "replace"]
                }),
            },
            ToolDefinition {
                name: TOOL_SSH_CHECK_PROCESS.to_string(),
                description: "Check remote processes on the topic infra target by pattern"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Substring or process pattern to inspect" },
                        "approval_request_id": { "type": "string", "description": "Approval request id for replay after operator confirmation" },
                        "approval_token": { "type": "string", "description": "Approval token issued by operator confirmation" }
                    },
                    "required": ["pattern"]
                }),
            },
        ]
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

    async fn approval_or_continue(
        &self,
        tool_name: &str,
        mode: TopicInfraToolMode,
        arguments: &str,
        summary: String,
        approval_request_id: Option<&str>,
        approval_token: Option<&str>,
    ) -> Result<Option<String>> {
        let fingerprint = fingerprint_for_request(tool_name, arguments)?;
        if !self.requires_approval(mode, &summary) {
            return Ok(None);
        }

        if let (Some(request_id), Some(token)) = (approval_request_id, approval_token) {
            self.approvals
                .consume(request_id, token, &fingerprint)
                .await?;
            return Ok(None);
        }

        let request = self
            .approvals
            .register(
                tool_name,
                &self.topic_id,
                &self.config.target_name,
                summary,
                fingerprint,
            )
            .await;

        Ok(Some(serde_json::to_string(&json!({
            "ok": false,
            "approval_required": true,
            "request_id": request.request_id,
            "tool_name": request.tool_name,
            "topic_id": request.topic_id,
            "target_name": request.target_name,
            "summary": request.summary,
            "expires_at": request.expires_at
        }))?))
    }

    fn requires_approval(&self, mode: TopicInfraToolMode, summary: &str) -> bool {
        if self.config.approval_required_modes.contains(&mode) {
            return true;
        }

        match mode {
            TopicInfraToolMode::SudoExec | TopicInfraToolMode::ApplyFileEdit => true,
            TopicInfraToolMode::Exec => is_dangerous_command(summary),
            TopicInfraToolMode::ReadFile => is_sensitive_path(summary),
            TopicInfraToolMode::CheckProcess => false,
        }
    }

    async fn execute_exec(
        &self,
        arguments: &str,
        sudo: bool,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        let args: CommandArgs = serde_json::from_str(arguments)
            .map_err(|err| anyhow!("invalid ssh command args: {err}"))?;
        let command = validate_non_empty(args.command, "command")?;
        let mode = if sudo {
            TopicInfraToolMode::SudoExec
        } else {
            TopicInfraToolMode::Exec
        };
        self.ensure_mode_allowed(mode)?;

        let summary = if sudo {
            format!(
                "sudo exec on {}: {}",
                self.config.target_name,
                truncate(&command, 120)
            )
        } else {
            format!(
                "exec on {}: {}",
                self.config.target_name,
                truncate(&command, 120)
            )
        };
        if let Some(response) = self
            .approval_or_continue(
                if sudo {
                    TOOL_SSH_SUDO_EXEC
                } else {
                    TOOL_SSH_EXEC
                },
                mode,
                arguments,
                summary,
                args.approval_request_id.as_deref(),
                args.approval_token.as_deref(),
            )
            .await?
        {
            return Ok(response);
        }

        let remote_script = command;
        let output = self
            .backend
            .execute(
                &remote_script,
                args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS),
                sudo,
                cancellation_token,
            )
            .await?;

        serde_json::to_string(&json!({
            "ok": true,
            "target_name": self.config.target_name,
            "host": self.config.host,
            "stdout": output.stdout,
            "stderr": output.stderr,
            "exit_code": output.exit_code,
            "sudo": sudo
        }))
        .map_err(Into::into)
    }

    async fn execute_read_file(&self, arguments: &str) -> Result<String> {
        let args: ReadFileArgs = serde_json::from_str(arguments)
            .map_err(|err| anyhow!("invalid ssh read file args: {err}"))?;
        let path = validate_non_empty(args.path, "path")?;
        self.ensure_mode_allowed(TopicInfraToolMode::ReadFile)?;

        let summary = format!("read file on {}: {}", self.config.target_name, path);
        if let Some(response) = self
            .approval_or_continue(
                TOOL_SSH_READ_FILE,
                TopicInfraToolMode::ReadFile,
                arguments,
                summary,
                args.approval_request_id.as_deref(),
                args.approval_token.as_deref(),
            )
            .await?
        {
            return Ok(response);
        }

        let max_bytes = args.max_bytes.unwrap_or(16_384).max(1);
        let remote_script = format!(
            "python3 - <<'PY'\nfrom pathlib import Path\npath = Path({})\ncontent = path.read_bytes()[:{}]\nimport sys\nsys.stdout.buffer.write(content)\nPY",
            python_string_literal(&path),
            max_bytes,
        );
        let output = self
            .backend
            .execute(&remote_script, DEFAULT_TIMEOUT_SECS, false, None)
            .await?;
        serde_json::to_string(&json!({
            "ok": true,
            "path": path,
            "content": output.stdout
        }))
        .map_err(Into::into)
    }

    async fn execute_check_process(&self, arguments: &str) -> Result<String> {
        let args: CheckProcessArgs = serde_json::from_str(arguments)
            .map_err(|err| anyhow!("invalid ssh check process args: {err}"))?;
        let pattern = validate_non_empty(args.pattern, "pattern")?;
        self.ensure_mode_allowed(TopicInfraToolMode::CheckProcess)?;

        let summary = format!(
            "check process on {}: {}",
            self.config.target_name,
            truncate(&pattern, 120)
        );
        if let Some(response) = self
            .approval_or_continue(
                TOOL_SSH_CHECK_PROCESS,
                TopicInfraToolMode::CheckProcess,
                arguments,
                summary,
                args.approval_request_id.as_deref(),
                args.approval_token.as_deref(),
            )
            .await?
        {
            return Ok(response);
        }

        let remote_script = format!("pgrep -af -- {} || true", escape_shell_argument(&pattern),);
        let output = self
            .backend
            .execute(&remote_script, DEFAULT_TIMEOUT_SECS, false, None)
            .await?;
        serde_json::to_string(&json!({
            "ok": true,
            "pattern": pattern,
            "matches": output.stdout
        }))
        .map_err(Into::into)
    }

    async fn execute_apply_file_edit(&self, arguments: &str) -> Result<String> {
        let args: ApplyFileEditArgs = serde_json::from_str(arguments)
            .map_err(|err| anyhow!("invalid ssh apply file edit args: {err}"))?;
        let path = validate_non_empty(args.path, "path")?;
        self.ensure_mode_allowed(TopicInfraToolMode::ApplyFileEdit)?;

        let summary = format!("edit file on {}: {}", self.config.target_name, path);
        if let Some(response) = self
            .approval_or_continue(
                TOOL_SSH_APPLY_FILE_EDIT,
                TopicInfraToolMode::ApplyFileEdit,
                arguments,
                summary,
                args.approval_request_id.as_deref(),
                args.approval_token.as_deref(),
            )
            .await?
        {
            return Ok(response);
        }

        let remote_script = format!(
            "python3 - <<'PY'\nimport base64\nfrom pathlib import Path\npath = Path(base64.b64decode({}).decode())\nsearch = base64.b64decode({}).decode()\nreplace = base64.b64decode({}).decode()\ncreate_if_missing = {}\nif not path.exists():\n    if not create_if_missing:\n        raise SystemExit(f'file not found: {path}')\n    path.parent.mkdir(parents=True, exist_ok=True)\n    path.write_text(replace)\n    print('created')\n    raise SystemExit(0)\ntext = path.read_text()\nif search not in text:\n    raise SystemExit('search text not found in remote file')\npath.write_text(text.replace(search, replace, 1))\nprint('updated')\nPY",
            python_string_literal(&BASE64_STANDARD.encode(path.as_bytes())),
            python_string_literal(&BASE64_STANDARD.encode(args.search.as_bytes())),
            python_string_literal(&BASE64_STANDARD.encode(args.replace.as_bytes())),
            if args.create_if_missing { "True" } else { "False" },
        );
        let output = self
            .backend
            .execute(
                &remote_script,
                args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS),
                false,
                None,
            )
            .await?;
        serde_json::to_string(&json!({
            "ok": true,
            "path": path,
            "status": output.stdout.trim()
        }))
        .map_err(Into::into)
    }
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

#[async_trait]
impl ToolProvider for SshMcpProvider {
    fn name(&self) -> &'static str {
        "ssh_mcp"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        Self::tool_definitions()
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        matches!(
            tool_name,
            TOOL_SSH_EXEC
                | TOOL_SSH_SUDO_EXEC
                | TOOL_SSH_READ_FILE
                | TOOL_SSH_APPLY_FILE_EDIT
                | TOOL_SSH_CHECK_PROCESS
        )
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        _progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        match tool_name {
            TOOL_SSH_EXEC => {
                self.execute_exec(arguments, false, cancellation_token)
                    .await
            }
            TOOL_SSH_SUDO_EXEC => self.execute_exec(arguments, true, cancellation_token).await,
            TOOL_SSH_READ_FILE => self.execute_read_file(arguments).await,
            TOOL_SSH_APPLY_FILE_EDIT => self.execute_apply_file_edit(arguments).await,
            TOOL_SSH_CHECK_PROCESS => self.execute_check_process(arguments).await,
            _ => bail!("unknown ssh_mcp tool: {tool_name}"),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandArgs {
    command: String,
    #[serde(default)]
    timeout_secs: Option<u64>,
    #[serde(default)]
    approval_request_id: Option<String>,
    #[serde(default)]
    approval_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadFileArgs {
    path: String,
    #[serde(default)]
    max_bytes: Option<usize>,
    #[serde(default)]
    approval_request_id: Option<String>,
    #[serde(default)]
    approval_token: Option<String>,
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
    #[serde(default)]
    approval_request_id: Option<String>,
    #[serde(default)]
    approval_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckProcessArgs {
    pattern: String,
    #[serde(default)]
    approval_request_id: Option<String>,
    #[serde(default)]
    approval_token: Option<String>,
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
            )
        }
    };
    let key_path = key_file.path();

    let mut public_command = Command::new("ssh-keygen");
    public_command.arg("-y").arg("-f").arg(key_path);
    let public_result = run_command_with_timeout(public_command, KEY_PROBE_TIMEOUT_SECS).await;

    let report = match public_result {
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
    };

    report
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

fn fingerprint_for_request(tool_name: &str, arguments: &str) -> Result<String> {
    let mut value = serde_json::from_str::<serde_json::Value>(arguments)
        .map_err(|err| anyhow!("invalid approval fingerprint payload: {err}"))?;
    if let Some(object) = value.as_object_mut() {
        object.remove("approval_request_id");
        object.remove("approval_token");
    }
    let canonical = serde_json::to_string(&value)?;
    let mut digest = Sha256::new();
    digest.update(tool_name.as_bytes());
    digest.update(b":");
    digest.update(canonical.as_bytes());
    Ok(format!("{:x}", digest.finalize()))
}

/// Inject approval replay credentials into the original SSH tool arguments.
pub fn inject_approval_credentials(
    arguments: &str,
    request_id: &str,
    approval_token: &str,
) -> Result<String> {
    let mut value = serde_json::from_str::<serde_json::Value>(arguments)
        .map_err(|err| anyhow!("invalid approval replay payload: {err}"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| anyhow!("approval replay payload must be a JSON object"))?;
    object.insert(
        "approval_request_id".to_string(),
        serde_json::Value::String(request_id.to_string()),
    );
    object.insert(
        "approval_token".to_string(),
        serde_json::Value::String(approval_token.to_string()),
    );
    serde_json::to_string(&value).map_err(Into::into)
}

fn validate_non_empty(value: String, field_name: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        bail!("{field_name} must not be empty");
    }
    Ok(trimmed.to_string())
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut truncated = value.chars().take(max_chars).collect::<String>();
    truncated.push_str("...");
    truncated
}

fn is_dangerous_command(summary: &str) -> bool {
    let lowered = summary.to_ascii_lowercase();
    [
        "rm -rf",
        "shutdown",
        "reboot",
        "systemctl stop",
        "systemctl restart",
        "docker compose down",
        "kubectl delete",
        "terraform apply",
        "terraform destroy",
    ]
    .iter()
    .any(|pattern| lowered.contains(pattern))
}

fn is_sensitive_path(summary: &str) -> bool {
    let lowered = summary.to_ascii_lowercase();
    [
        "/etc/",
        "/root/",
        "/home/",
        ".ssh",
        "systemd",
        "nginx",
        "postgresql",
    ]
    .iter()
    .any(|pattern| lowered.contains(pattern))
}

fn escape_shell_argument(value: &str) -> String {
    escape(Cow::Borrowed(value)).into_owned()
}

fn python_string_literal(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

fn now_unix_secs() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs() as i64,
        Err(_) => 0,
    }
}

/// Remove any stale SSH private key temp files left from previous process runs.
pub fn cleanup_stale_private_key_tempfiles() -> Result<usize> {
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

/// Build a system message instructing the agent to replay an approved SSH tool call.
pub fn inject_ssh_approval_system_message(grant: &SshApprovalGrant) -> AgentMessage {
    AgentMessage::approval_replay(format!(
        "A human operator approved the pending SSH action for target '{}' in topic '{}'. Retry the exact same SSH tool call and include approval_request_id='{}' and approval_token='{}'. Do not change any other tool arguments.",
        grant.target_name, grant.topic_id, grant.request_id, grant.approval_token
    ))
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
        cleanup_stale_private_key_tempfiles_in, decode_hex, fingerprint_for_request,
        inject_approval_credentials, inject_ssh_approval_system_message,
        inject_topic_infra_preflight_system_message, is_dangerous_command, is_sensitive_path,
        parse_ssh_keygen_listing, parse_wrapped_remote_output, write_private_key_tempfile_in,
        SecretProbeKind, SecretProbeReport, SshApprovalRegistry, TopicInfraPreflightReport,
        WrappedCommandMarkers,
    };
    use crate::storage::TopicInfraAuthMode;
    use std::{fs, path::Path};
    use tempfile::tempdir;

    #[tokio::test]
    async fn approval_registry_grants_and_consumes_matching_request() {
        let registry = SshApprovalRegistry::new();
        let request = registry
            .register(
                "ssh_exec",
                "topic-a",
                "prod-app",
                "exec on prod-app: systemctl restart api".to_string(),
                "fp-1".to_string(),
            )
            .await;

        let grant = registry
            .grant(&request.request_id)
            .await
            .expect("grant must succeed");
        registry
            .consume(&request.request_id, &grant.approval_token, "fp-1")
            .await
            .expect("consume must succeed");
    }

    #[test]
    fn fingerprint_ignores_approval_fields() {
        let first = fingerprint_for_request(
            "ssh_exec",
            r#"{"command":"uname -a","approval_request_id":"a","approval_token":"b"}"#,
        )
        .expect("fingerprint must succeed");
        let second = fingerprint_for_request("ssh_exec", r#"{"command":"uname -a"}"#)
            .expect("fingerprint must succeed");
        assert_eq!(first, second);
    }

    #[test]
    fn inject_approval_credentials_preserves_original_fingerprint() {
        let original = r#"{"command":"uname -a","timeout_secs":30}"#;
        let replay = inject_approval_credentials(original, "req-1", "token-1")
            .expect("approval credentials must inject");

        let original_fingerprint =
            fingerprint_for_request("ssh_exec", original).expect("fingerprint must succeed");
        let replay_fingerprint =
            fingerprint_for_request("ssh_exec", &replay).expect("fingerprint must succeed");

        assert_eq!(original_fingerprint, replay_fingerprint);
        assert!(replay.contains("approval_request_id"));
        assert!(replay.contains("approval_token"));
    }

    #[test]
    fn dangerous_command_detection_flags_high_risk_operations() {
        assert!(is_dangerous_command(
            "exec on prod: terraform apply -auto-approve"
        ));
        assert!(!is_dangerous_command("exec on prod: uname -a"));
    }

    #[test]
    fn sensitive_path_detection_flags_system_locations() {
        assert!(is_sensitive_path(
            "read file on prod: /etc/nginx/nginx.conf"
        ));
        assert!(!is_sensitive_path("read file on prod: /tmp/app.log"));
    }

    #[test]
    fn approval_system_message_contains_replay_tokens() {
        let grant = super::SshApprovalGrant {
            request_id: "req-1".to_string(),
            approval_token: "token-1".to_string(),
            tool_name: "ssh_exec".to_string(),
            topic_id: "topic-a".to_string(),
            target_name: "prod-app".to_string(),
            summary: "restart api".to_string(),
            expires_at: 100,
        };
        let message = inject_ssh_approval_system_message(&grant);
        assert!(message.content.contains("approval_request_id='req-1'"));
        assert!(message.content.contains("approval_token='token-1'"));
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
