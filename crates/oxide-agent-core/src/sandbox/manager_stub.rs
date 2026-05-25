#![allow(missing_docs)]

//! Sandbox manager facade used when no sandbox backend feature is compiled.

use super::SandboxScope;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

fn unavailable() -> anyhow::Error {
    anyhow::anyhow!(
        "sandbox support is not compiled; enable sandbox-backend-docker-direct or sandbox-daemon"
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

/// Docker metadata for a user-owned sandbox container.
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
}
