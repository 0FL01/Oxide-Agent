//! Narrow sandbox backend capability traits.

#[cfg(feature = "tool-stack-logs")]
use super::broker::{
    StackLogsFetchRequest, StackLogsFetchResponse, StackLogsListSourcesRequest,
    StackLogsListSourcesResponse,
};
use super::ExecResult;
use super::{SandboxContainerRecord, SandboxScope};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::str;

/// Stable identifier for a sandbox backend implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct SandboxBackendId(&'static str);

impl SandboxBackendId {
    /// Creates a stable sandbox backend identifier.
    #[must_use]
    pub const fn new(id: &'static str) -> Self {
        Self(id)
    }

    /// Returns the string form of the backend identifier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// Coarse capability exposed by a sandbox backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SandboxCapability {
    /// File read/write/list support.
    FileOps,
    /// Command execution support.
    Exec,
    /// Sandbox lifecycle management support.
    Lifecycle,
    /// Operational diagnostics support, such as stack logs.
    Diagnostics,
    /// User/topic-level sandbox inventory and lifecycle administration.
    Admin,
}

/// Shared metadata for sandbox backend capability traits.
pub trait SandboxBackend: Send + Sync {
    /// Stable backend ID.
    fn id(&self) -> SandboxBackendId;

    /// Capabilities exposed by this backend facade.
    fn capabilities(&self) -> &'static [SandboxCapability];
}

/// Result of listing files in a sandbox workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxFileListing {
    /// Path that was listed.
    pub path: String,
    /// Listing text returned by the backend.
    pub listing: String,
    /// Stderr text returned by the backend.
    pub stderr: String,
    /// Process exit code from the backend listing command.
    pub exit_code: i64,
}

/// Request for a targeted sandbox text edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxFileEdit {
    /// Exact text fragment to replace.
    pub search: String,
    /// Replacement text.
    pub replace: String,
    /// Exact number of replacements expected. Defaults are owned by the tool layer.
    pub expected_replacements: usize,
}

/// Previously observed file state required before applying a guarded edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxEditReadGuard {
    /// SHA-256 observed by the last successful read.
    pub sha256: String,
    /// Byte length observed by the last successful read.
    pub bytes: usize,
}

/// Result of applying a targeted sandbox text edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxApplyFileEditResult {
    /// Edited file path.
    pub path: String,
    /// `updated` when bytes changed, `unchanged` when the replacement was identical.
    pub status: String,
    /// Number of exact fragment replacements applied.
    pub replacements: usize,
    /// SHA-256 of the file before the edit.
    pub previous_sha256: String,
    /// SHA-256 of the file after the edit.
    pub new_sha256: String,
    /// File size before the edit.
    pub bytes_before: usize,
    /// File size after the edit.
    pub bytes_written: usize,
    /// Whether file bytes changed.
    pub changed: bool,
}

/// Internal result of applying a sandbox text edit to in-memory bytes.
#[cfg_attr(
    not(any(
        feature = "sandbox-backend-docker-direct",
        feature = "sandbox-backend-sandboxd-client",
        feature = "sandbox-backend-bwrap"
    )),
    allow(dead_code)
)]
pub(crate) struct SandboxAppliedFileEdit {
    /// Updated bytes to write when the edit changed the file.
    pub updated: Vec<u8>,
    /// Structured edit result.
    pub result: SandboxApplyFileEditResult,
}

#[cfg_attr(
    not(any(
        feature = "sandbox-backend-docker-direct",
        feature = "sandbox-backend-sandboxd-client",
        feature = "sandbox-backend-bwrap"
    )),
    allow(dead_code)
)]
pub(crate) fn apply_sandbox_file_edit(
    path: &str,
    current: &[u8],
    edit: &SandboxFileEdit,
    read_guard: Option<&SandboxEditReadGuard>,
) -> Result<SandboxAppliedFileEdit> {
    let previous_sha256 = sha256_hex(current);

    if !current.is_empty() {
        validate_edit_read_guard(path, &previous_sha256, current.len(), read_guard)?;
    }

    let (updated, replacements) = apply_exact_text_edit(current, edit)?;
    let new_sha256 = sha256_hex(&updated);
    let changed = current != updated;

    Ok(SandboxAppliedFileEdit {
        result: SandboxApplyFileEditResult {
            path: path.to_string(),
            status: if changed { "updated" } else { "unchanged" }.to_string(),
            replacements,
            previous_sha256,
            new_sha256,
            bytes_before: current.len(),
            bytes_written: updated.len(),
            changed,
        },
        updated,
    })
}

#[cfg_attr(
    not(any(
        feature = "sandbox-backend-docker-direct",
        feature = "sandbox-backend-sandboxd-client",
        feature = "sandbox-backend-bwrap"
    )),
    allow(dead_code)
)]
fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg_attr(
    not(any(
        feature = "sandbox-backend-docker-direct",
        feature = "sandbox-backend-sandboxd-client",
        feature = "sandbox-backend-bwrap"
    )),
    allow(dead_code)
)]
fn validate_edit_read_guard(
    path: &str,
    current_sha256: &str,
    current_bytes: usize,
    read_guard: Option<&SandboxEditReadGuard>,
) -> Result<()> {
    if current_bytes == 0 {
        return Ok(());
    }

    let read_guard = read_guard.ok_or_else(|| {
        anyhow::anyhow!(
            "file must be read with read_file before apply_file_edit; empty files are exempt: {path}"
        )
    })?;

    if read_guard.sha256 != current_sha256 {
        anyhow::bail!(
            "file changed after last read; call read_file again before editing: {path} (last_read_sha256={}, current_sha256={}, last_read_bytes={}, current_bytes={})",
            read_guard.sha256,
            current_sha256,
            read_guard.bytes,
            current_bytes
        );
    }

    Ok(())
}

#[cfg_attr(
    not(any(
        feature = "sandbox-backend-docker-direct",
        feature = "sandbox-backend-sandboxd-client",
        feature = "sandbox-backend-bwrap"
    )),
    allow(dead_code)
)]
fn apply_exact_text_edit(current: &[u8], edit: &SandboxFileEdit) -> Result<(Vec<u8>, usize)> {
    if bytes_look_binary(current) {
        anyhow::bail!("apply_file_edit only supports text files; binary content was detected");
    }

    let current_text = str::from_utf8(current)
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

#[cfg_attr(
    not(any(
        feature = "sandbox-backend-docker-direct",
        feature = "sandbox-backend-sandboxd-client",
        feature = "sandbox-backend-bwrap"
    )),
    allow(dead_code)
)]
fn bytes_look_binary(bytes: &[u8]) -> bool {
    let inspected = &bytes[..bytes.len().min(8192)];
    inspected.contains(&0)
}

impl SandboxFileListing {
    /// Whether the list operation exited successfully.
    #[must_use]
    pub const fn success(&self) -> bool {
        self.exit_code == 0
    }

    /// Whether the listing output is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.listing.is_empty()
    }
}

/// Sandbox command-execution capability.
#[async_trait]
pub trait SandboxExec: SandboxBackend {
    /// Execute a shell command in the current sandbox scope.
    async fn exec(
        &self,
        command: &str,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<ExecResult>;
}

/// Sandbox file operation capability.
#[async_trait]
pub trait SandboxFileOps: SandboxBackend {
    /// Write bytes to a file in the current sandbox scope.
    async fn write_file(&self, path: &str, bytes: &[u8]) -> Result<()>;

    /// Read bytes from a file in the current sandbox scope.
    async fn read_file(&self, path: &str) -> Result<Vec<u8>>;

    /// Return file size in bytes without reading the whole file.
    async fn file_size_bytes(
        &self,
        path: &str,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<u64>;

    /// List files below a path in the current sandbox scope.
    async fn list_files(&self, path: &str) -> Result<SandboxFileListing>;

    /// Apply a targeted text edit to a file in the current sandbox scope.
    async fn apply_file_edit(
        &self,
        path: &str,
        edit: SandboxFileEdit,
    ) -> Result<SandboxApplyFileEditResult>;
}

/// Sandbox lifecycle capability.
#[async_trait]
pub trait SandboxLifecycle: SandboxBackend {
    /// Recreate the current sandbox scope.
    async fn recreate(&self) -> Result<()>;
}

/// Sandbox inventory and lifecycle administration capability.
#[async_trait]
pub trait SandboxAdmin: SandboxBackend {
    /// Destroy sandbox resources for a logical scope.
    async fn destroy_scope(&self, scope: SandboxScope) -> Result<()>;

    /// List all sandbox instances owned by a user.
    async fn list_user_sandboxes(&self, user_id: i64) -> Result<Vec<SandboxContainerRecord>>;

    /// Inspect a user-owned sandbox instance by backend instance name.
    async fn inspect_sandbox_by_name(
        &self,
        user_id: i64,
        container_name: &str,
    ) -> Result<Option<SandboxContainerRecord>>;

    /// Ensure a sandbox exists for a logical scope.
    async fn ensure_scope_sandbox(&self, scope: SandboxScope) -> Result<SandboxContainerRecord>;

    /// Recreate the sandbox for a logical scope.
    async fn recreate_scope_sandbox(&self, scope: SandboxScope) -> Result<SandboxContainerRecord>;

    /// Delete a user-owned sandbox by backend instance name.
    async fn delete_sandbox_by_name(&self, user_id: i64, container_name: &str) -> Result<bool>;
}

/// Sandbox diagnostics capability.
#[cfg(feature = "tool-stack-logs")]
#[async_trait]
pub trait SandboxDiagnostics: SandboxBackend {
    /// List compose-stack log sources available to the diagnostics backend.
    async fn list_stack_log_sources(
        &self,
        request: StackLogsListSourcesRequest,
    ) -> Result<StackLogsListSourcesResponse>;

    /// Fetch bounded compose-stack logs from the diagnostics backend.
    async fn fetch_stack_logs(
        &self,
        request: StackLogsFetchRequest,
    ) -> Result<StackLogsFetchResponse>;
}
