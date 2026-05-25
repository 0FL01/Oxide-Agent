#![allow(missing_docs)]

//! Sandbox manager facade used when no sandbox backend feature is compiled.

use super::{SandboxFileListing, SandboxScope};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

fn unavailable() -> anyhow::Error {
    anyhow::anyhow!(
        "sandbox support is not compiled; enable sandbox-backend-docker-direct, sandbox-backend-bwrap, or sandbox-daemon"
    )
}

/// Result of executing a command in the sandbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i64,
}

impl ExecResult {
    #[must_use]
    pub const fn success(&self) -> bool {
        self.exit_code == 0
    }

    #[must_use]
    pub fn combined_output(&self) -> String {
        if self.stderr.is_empty() {
            self.stdout.clone()
        } else if self.stdout.is_empty() {
            self.stderr.clone()
        } else {
            format!("{}\n{}", self.stdout, self.stderr)
        }
    }
}

/// Backend-neutral metadata for a user-owned sandbox instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxInstanceRecord {
    pub backend: String,
    pub instance_id: String,
    pub instance_name: String,
    pub scope_id: Option<String>,
    pub image_id: Option<String>,
    pub rootfs_path: Option<String>,
    pub state_dir: Option<String>,
    pub workspace_dir: Option<String>,
    pub root_mode: Option<String>,
    pub network_mode: Option<String>,
    pub created_at: Option<i64>,
    pub last_used_at: Option<i64>,
    pub state: Option<String>,
    pub status: Option<String>,
    pub running: bool,
    pub user_id: Option<i64>,
    pub chat_id: Option<i64>,
    pub thread_id: Option<i64>,
    pub labels: HashMap<String, String>,
    pub container_id: String,
    pub container_name: String,
}

/// Docker-compatible metadata for a user-owned sandbox container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxContainerRecord {
    pub container_id: String,
    pub container_name: String,
    pub image: Option<String>,
    pub created_at: Option<i64>,
    pub state: Option<String>,
    pub status: Option<String>,
    pub running: bool,
    pub user_id: Option<i64>,
    pub scope: Option<String>,
    pub chat_id: Option<i64>,
    pub thread_id: Option<i64>,
    pub labels: HashMap<String, String>,
}

impl From<SandboxContainerRecord> for SandboxInstanceRecord {
    fn from(record: SandboxContainerRecord) -> Self {
        Self {
            backend: record
                .labels
                .get("agent.sandbox_backend")
                .cloned()
                .unwrap_or_else(|| "docker".to_string()),
            instance_id: record.container_id.clone(),
            instance_name: record.container_name.clone(),
            scope_id: record.scope.clone(),
            image_id: record.image.clone(),
            rootfs_path: record.labels.get("agent.rootfs").cloned(),
            state_dir: record.labels.get("agent.state_dir").cloned(),
            workspace_dir: record.labels.get("agent.workspace_dir").cloned(),
            root_mode: record.labels.get("agent.root_mode").cloned(),
            network_mode: record.labels.get("agent.network_mode").cloned(),
            created_at: record.created_at,
            last_used_at: record
                .labels
                .get("agent.updated_at")
                .and_then(|value| value.parse::<i64>().ok()),
            state: record.state.clone(),
            status: record.status.clone(),
            running: record.running,
            user_id: record.user_id,
            chat_id: record.chat_id,
            thread_id: record.thread_id,
            labels: record.labels,
            container_id: record.container_id,
            container_name: record.container_name,
        }
    }
}

#[derive(Clone)]
pub struct SandboxManager {
    scope: SandboxScope,
}

impl SandboxManager {
    pub async fn new(scope: impl Into<SandboxScope>) -> Result<Self> {
        Ok(Self {
            scope: scope.into(),
        })
    }

    pub async fn list_user_sandboxes(_user_id: i64) -> Result<Vec<SandboxContainerRecord>> {
        Err(unavailable())
    }

    pub async fn inspect_sandbox_by_name(
        _user_id: i64,
        _container_name: &str,
    ) -> Result<Option<SandboxContainerRecord>> {
        Err(unavailable())
    }

    pub async fn ensure_scope_sandbox(_scope: SandboxScope) -> Result<SandboxContainerRecord> {
        Err(unavailable())
    }

    pub async fn recreate_scope_sandbox(_scope: SandboxScope) -> Result<SandboxContainerRecord> {
        Err(unavailable())
    }

    pub async fn delete_sandbox_by_name(_user_id: i64, _container_name: &str) -> Result<bool> {
        Err(unavailable())
    }

    #[must_use]
    pub const fn is_running(&self) -> bool {
        false
    }

    #[must_use]
    pub const fn container_id(&self) -> Option<&str> {
        None
    }

    #[must_use]
    pub const fn scope(&self) -> &SandboxScope {
        &self.scope
    }

    pub async fn create_sandbox(&mut self) -> Result<()> {
        Err(unavailable())
    }

    pub async fn exec_command(
        &mut self,
        _cmd: &str,
        _cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<ExecResult> {
        Err(unavailable())
    }

    pub async fn write_file(&mut self, _path: &str, _content: &[u8]) -> Result<()> {
        Err(unavailable())
    }

    pub async fn read_file(&mut self, _path: &str) -> Result<Vec<u8>> {
        Err(unavailable())
    }

    pub async fn upload_file(&mut self, _container_path: &str, _content: &[u8]) -> Result<()> {
        Err(unavailable())
    }

    pub async fn download_file(&mut self, _container_path: &str) -> Result<Vec<u8>> {
        Err(unavailable())
    }

    pub async fn get_uploads_size(&mut self) -> Result<u64> {
        Err(unavailable())
    }

    pub async fn cleanup_old_downloads(&mut self) -> Result<u64> {
        Err(unavailable())
    }

    pub async fn destroy(&mut self) -> Result<()> {
        Err(unavailable())
    }

    pub async fn recreate(&mut self) -> Result<()> {
        Err(unavailable())
    }

    pub async fn file_size_bytes(
        &mut self,
        _container_path: &str,
        _cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<u64> {
        Err(unavailable())
    }

    pub async fn list_files(&mut self, _path: &str) -> Result<SandboxFileListing> {
        Err(unavailable())
    }
}
