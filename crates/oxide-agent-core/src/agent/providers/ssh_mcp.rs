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
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use shell_escape::unix::escape;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::process::Command;
use tokio::sync::Mutex;
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

/// Topic-scoped SSH tool provider backed by the local `ssh` CLI.
#[derive(Clone)]
pub struct SshMcpProvider {
    storage: Arc<dyn StorageProvider>,
    user_id: i64,
    topic_id: String,
    config: TopicInfraConfigRecord,
    approvals: SshApprovalRegistry,
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
            storage,
            user_id,
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

    async fn execute_exec(&self, arguments: &str, sudo: bool) -> Result<String> {
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

        let remote_script = if sudo {
            self.wrap_sudo_command(&command).await?
        } else {
            format!("sh -lc {}", escape_shell_argument(&command))
        };
        let output = self
            .run_remote_script(
                remote_script,
                args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS),
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
            .run_remote_script(remote_script, DEFAULT_TIMEOUT_SECS)
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
            .run_remote_script(remote_script, DEFAULT_TIMEOUT_SECS)
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
            .run_remote_script(
                remote_script,
                args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS),
            )
            .await?;
        serde_json::to_string(&json!({
            "ok": true,
            "path": path,
            "status": output.stdout.trim()
        }))
        .map_err(Into::into)
    }

    async fn wrap_sudo_command(&self, command: &str) -> Result<String> {
        let escaped_command = escape_shell_argument(command);
        if let Some(secret_ref) = self.config.sudo_secret_ref.as_deref() {
            let sudo_password = self.resolve_secret_ref(secret_ref).await?;
            return Ok(format!(
                "printf '%s\\n' {} | sudo -S -p '' sh -lc {}",
                escape_shell_argument(&sudo_password),
                escaped_command,
            ));
        }

        Ok(format!("sudo -n sh -lc {}", escaped_command))
    }

    async fn resolve_secret_ref(&self, secret_ref: &str) -> Result<String> {
        if let Some(env_name) = secret_ref.strip_prefix("env:") {
            return std::env::var(env_name)
                .with_context(|| format!("missing environment secret '{env_name}'"));
        }

        let storage_key = secret_ref.strip_prefix("storage:").unwrap_or(secret_ref);
        self.storage
            .get_secret_value(self.user_id, storage_key.to_string())
            .await
            .map_err(|err| anyhow!("failed to load secret ref '{storage_key}': {err}"))?
            .ok_or_else(|| anyhow!("secret ref '{storage_key}' is not provisioned"))
    }

    async fn run_remote_script(
        &self,
        remote_script: String,
        timeout_secs: u64,
    ) -> Result<RemoteOutput> {
        let mut cleanup_path = None;
        let mut command = match self.config.auth_mode {
            TopicInfraAuthMode::Password => {
                let secret_ref = self
                    .config
                    .secret_ref
                    .as_deref()
                    .ok_or_else(|| anyhow!("password auth requires secret_ref"))?;
                let password = self.resolve_secret_ref(secret_ref).await?;
                let mut command = Command::new("sshpass");
                command.arg("-e").arg("ssh");
                command.env("SSHPASS", password);
                command
            }
            TopicInfraAuthMode::PrivateKey => {
                let secret_ref = self
                    .config
                    .secret_ref
                    .as_deref()
                    .ok_or_else(|| anyhow!("private_key auth requires secret_ref"))?;
                let private_key = self.resolve_secret_ref(secret_ref).await?;
                let key_path = write_private_key_tempfile(&private_key)?;
                cleanup_path = Some(key_path.clone());
                let mut command = Command::new("ssh");
                command.arg("-i").arg(&key_path);
                command
            }
            TopicInfraAuthMode::None => Command::new("ssh"),
        };

        command
            .arg("-p")
            .arg(self.config.port.to_string())
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("StrictHostKeyChecking=accept-new")
            .arg(format!("{}@{}", self.config.remote_user, self.config.host))
            .arg(remote_script)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let output = run_command_with_timeout(command, timeout_secs).await;

        if let Some(path) = cleanup_path {
            let _ = std::fs::remove_file(path);
        }

        let output = output?;
        if output.exit_code != 0 {
            bail!(
                "remote SSH command failed (exit {}): {}",
                output.exit_code,
                output.stderr
            );
        }

        Ok(output)
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
        _cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        match tool_name {
            TOOL_SSH_EXEC => self.execute_exec(arguments, false).await,
            TOOL_SSH_SUDO_EXEC => self.execute_exec(arguments, true).await,
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

fn write_private_key_tempfile(private_key: &str) -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!("oxide-agent-ssh-key-{}", Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&path)
        .with_context(|| {
            format!(
                "failed to create private key temp file at {}",
                path.display()
            )
        })?;
    file.write_all(private_key.as_bytes()).with_context(|| {
        format!(
            "failed to write private key temp file at {}",
            path.display()
        )
    })?;
    Ok(path)
}

/// Build a system message instructing the agent to replay an approved SSH tool call.
pub fn inject_ssh_approval_system_message(grant: &SshApprovalGrant) -> AgentMessage {
    AgentMessage::system(format!(
        "A human operator approved the pending SSH action for target '{}' in topic '{}'. Retry the exact same SSH tool call and include approval_request_id='{}' and approval_token='{}'. Do not change any other tool arguments.",
        grant.target_name, grant.topic_id, grant.request_id, grant.approval_token
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        fingerprint_for_request, inject_ssh_approval_system_message, is_dangerous_command,
        is_sensitive_path, SshApprovalRegistry,
    };

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
}
