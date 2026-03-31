#![allow(missing_docs)]

//! Docker sandbox manager using Bollard
//!
//! Manages Docker containers for isolated code execution.

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use bollard::errors::Error as DockerError;
use bollard::exec::{CreateExecOptions, StartExecResults};
use bollard::models::{ContainerCreateBody, HostConfig};
use bollard::query_parameters::{
    CreateContainerOptions, DownloadFromContainerOptions, InspectContainerOptions,
    RemoveContainerOptions, StartContainerOptions, UploadToContainerOptions,
};
use bollard::Docker;
use bytes::Bytes;
use futures_util::{StreamExt, TryStreamExt};
use http_body_util::{Either, Full};
use serde::{Deserialize, Serialize};
use shell_escape::escape;
use std::collections::HashMap;
use std::io::Read;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, info, instrument, warn};

use crate::config::{
    get_sandbox_image, sandbox_uses_broker, SANDBOX_CPU_PERIOD, SANDBOX_CPU_QUOTA,
    SANDBOX_EXEC_TIMEOUT_SECS, SANDBOX_MEMORY_LIMIT,
};
use crate::sandbox::broker::SandboxBrokerClient;
use crate::sandbox::SandboxScope;

/// Result of executing a command in the sandbox
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecResult {
    /// Standard output of the command
    pub stdout: String,
    /// Standard error of the command
    pub stderr: String,
    /// Exit code of the command
    pub exit_code: i64,
}

/// Docker metadata for a user-owned sandbox container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxContainerRecord {
    /// Docker container id.
    pub container_id: String,
    /// Stable Docker container name.
    pub container_name: String,
    /// Docker image reference.
    pub image: Option<String>,
    /// Container creation timestamp reported by Docker.
    pub created_at: Option<i64>,
    /// Low-level Docker state such as `running` or `exited`.
    pub state: Option<String>,
    /// Human-readable Docker status string.
    pub status: Option<String>,
    /// Whether Docker currently reports the container as running.
    pub running: bool,
    /// Owning user id from Docker labels.
    pub user_id: Option<i64>,
    /// Sandbox namespace / topic scope from Docker labels.
    pub scope: Option<String>,
    /// Optional transport chat id from Docker labels.
    pub chat_id: Option<i64>,
    /// Optional transport thread id from Docker labels.
    pub thread_id: Option<i64>,
    /// Full Docker labels attached to the container.
    pub labels: HashMap<String, String>,
}

#[derive(Clone)]
pub struct SandboxManager {
    inner: SandboxManagerInner,
}

#[derive(Clone)]
enum SandboxManagerInner {
    Docker(DockerSandboxManager),
    Broker(BrokerSandboxManager),
}

#[derive(Clone)]
struct BrokerSandboxManager {
    client: SandboxBrokerClient,
    container_id: Option<String>,
    image_name: String,
    scope: SandboxScope,
}

impl ExecResult {
    /// Check if the command succeeded (exit code 0)
    #[must_use]
    pub const fn success(&self) -> bool {
        self.exit_code == 0
    }

    /// Get combined output (stdout + stderr)
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

impl BrokerSandboxManager {
    fn new(scope: SandboxScope) -> Self {
        Self {
            client: SandboxBrokerClient::from_env(),
            container_id: None,
            image_name: get_sandbox_image(),
            scope,
        }
    }

    const fn is_running(&self) -> bool {
        self.container_id.is_some()
    }

    fn container_id(&self) -> Option<&str> {
        self.container_id.as_deref()
    }

    const fn scope(&self) -> &SandboxScope {
        &self.scope
    }

    async fn create_sandbox(&mut self) -> Result<()> {
        self.container_id = self
            .client
            .create_sandbox(self.scope.clone(), self.image_name.clone())
            .await?
            .or_else(|| Some(self.scope.container_name()));
        Ok(())
    }

    async fn exec_command(
        &mut self,
        cmd: &str,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<ExecResult> {
        let result = self
            .client
            .exec_command(
                self.scope.clone(),
                self.image_name.clone(),
                cmd,
                cancellation_token,
            )
            .await?;
        self.container_id
            .get_or_insert_with(|| self.scope.container_name());
        Ok(result)
    }

    async fn write_file(&mut self, path: &str, content: &[u8]) -> Result<()> {
        if self.container_id.is_none() {
            return Err(anyhow!("Sandbox not running"));
        }
        self.client
            .write_file(self.scope.clone(), self.image_name.clone(), path, content)
            .await
    }

    async fn read_file(&mut self, path: &str) -> Result<Vec<u8>> {
        let result = self
            .client
            .read_file(self.scope.clone(), self.image_name.clone(), path)
            .await?;
        self.container_id
            .get_or_insert_with(|| self.scope.container_name());
        Ok(result)
    }

    async fn upload_file(&mut self, container_path: &str, content: &[u8]) -> Result<()> {
        if self.container_id.is_none() {
            return Err(anyhow!("Sandbox not running"));
        }
        self.client
            .upload_file(
                self.scope.clone(),
                self.image_name.clone(),
                container_path,
                content,
            )
            .await
    }

    async fn download_file(&mut self, container_path: &str) -> Result<Vec<u8>> {
        if self.container_id.is_none() {
            return Err(anyhow!("Sandbox not running"));
        }
        self.client
            .download_file(self.scope.clone(), self.image_name.clone(), container_path)
            .await
    }

    async fn get_uploads_size(&mut self) -> Result<u64> {
        let size = self
            .client
            .get_uploads_size(self.scope.clone(), self.image_name.clone())
            .await?;
        self.container_id
            .get_or_insert_with(|| self.scope.container_name());
        Ok(size)
    }

    async fn cleanup_old_downloads(&mut self) -> Result<u64> {
        let count = self
            .client
            .cleanup_old_downloads(self.scope.clone(), self.image_name.clone())
            .await?;
        self.container_id
            .get_or_insert_with(|| self.scope.container_name());
        Ok(count)
    }

    async fn destroy(&mut self) -> Result<()> {
        self.client
            .destroy(self.scope.clone(), self.image_name.clone())
            .await?;
        self.container_id = None;
        Ok(())
    }

    async fn recreate(&mut self) -> Result<()> {
        self.client
            .recreate(self.scope.clone(), self.image_name.clone())
            .await?;
        self.container_id = Some(self.scope.container_name());
        Ok(())
    }

    async fn file_size_bytes(
        &mut self,
        container_path: &str,
        _cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<u64> {
        let size = self
            .client
            .file_size_bytes(self.scope.clone(), self.image_name.clone(), container_path)
            .await?;
        self.container_id
            .get_or_insert_with(|| self.scope.container_name());
        Ok(size)
    }
}

impl SandboxManager {
    #[instrument(skip_all)]
    pub async fn new(scope: impl Into<SandboxScope>) -> Result<Self> {
        let scope = scope.into();
        let inner = if sandbox_uses_broker() {
            SandboxManagerInner::Broker(BrokerSandboxManager::new(scope))
        } else {
            SandboxManagerInner::Docker(DockerSandboxManager::new(scope).await?)
        };
        Ok(Self { inner })
    }

    pub async fn list_user_sandboxes(user_id: i64) -> Result<Vec<SandboxContainerRecord>> {
        if sandbox_uses_broker() {
            SandboxBrokerClient::from_env()
                .list_user_sandboxes(user_id)
                .await
        } else {
            DockerSandboxManager::list_user_sandboxes(user_id).await
        }
    }

    pub async fn inspect_sandbox_by_name(
        user_id: i64,
        container_name: &str,
    ) -> Result<Option<SandboxContainerRecord>> {
        if sandbox_uses_broker() {
            SandboxBrokerClient::from_env()
                .inspect_sandbox_by_name(user_id, container_name)
                .await
        } else {
            DockerSandboxManager::inspect_sandbox_by_name(user_id, container_name).await
        }
    }

    pub async fn ensure_scope_sandbox(scope: SandboxScope) -> Result<SandboxContainerRecord> {
        if sandbox_uses_broker() {
            SandboxBrokerClient::from_env()
                .ensure_scope_sandbox(scope, get_sandbox_image())
                .await
        } else {
            DockerSandboxManager::ensure_scope_sandbox(scope).await
        }
    }

    pub async fn recreate_scope_sandbox(scope: SandboxScope) -> Result<SandboxContainerRecord> {
        if sandbox_uses_broker() {
            SandboxBrokerClient::from_env()
                .recreate_scope_sandbox(scope, get_sandbox_image())
                .await
        } else {
            DockerSandboxManager::recreate_scope_sandbox(scope).await
        }
    }

    pub async fn delete_sandbox_by_name(user_id: i64, container_name: &str) -> Result<bool> {
        if sandbox_uses_broker() {
            SandboxBrokerClient::from_env()
                .delete_sandbox_by_name(user_id, container_name)
                .await
        } else {
            DockerSandboxManager::delete_sandbox_by_name(user_id, container_name).await
        }
    }

    #[must_use]
    pub fn is_running(&self) -> bool {
        match &self.inner {
            SandboxManagerInner::Docker(manager) => manager.is_running(),
            SandboxManagerInner::Broker(manager) => manager.is_running(),
        }
    }

    #[must_use]
    pub fn container_id(&self) -> Option<&str> {
        match &self.inner {
            SandboxManagerInner::Docker(manager) => manager.container_id(),
            SandboxManagerInner::Broker(manager) => manager.container_id(),
        }
    }

    #[must_use]
    pub fn scope(&self) -> &SandboxScope {
        match &self.inner {
            SandboxManagerInner::Docker(manager) => manager.scope(),
            SandboxManagerInner::Broker(manager) => manager.scope(),
        }
    }

    pub async fn create_sandbox(&mut self) -> Result<()> {
        match &mut self.inner {
            SandboxManagerInner::Docker(manager) => manager.create_sandbox().await,
            SandboxManagerInner::Broker(manager) => manager.create_sandbox().await,
        }
    }

    pub async fn exec_command(
        &mut self,
        cmd: &str,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<ExecResult> {
        match &mut self.inner {
            SandboxManagerInner::Docker(manager) => {
                manager.exec_command(cmd, cancellation_token).await
            }
            SandboxManagerInner::Broker(manager) => {
                manager.exec_command(cmd, cancellation_token).await
            }
        }
    }

    pub async fn write_file(&mut self, path: &str, content: &[u8]) -> Result<()> {
        match &mut self.inner {
            SandboxManagerInner::Docker(manager) => manager.write_file(path, content).await,
            SandboxManagerInner::Broker(manager) => manager.write_file(path, content).await,
        }
    }

    pub async fn read_file(&mut self, path: &str) -> Result<Vec<u8>> {
        match &mut self.inner {
            SandboxManagerInner::Docker(manager) => manager.read_file(path).await,
            SandboxManagerInner::Broker(manager) => manager.read_file(path).await,
        }
    }

    pub async fn upload_file(&mut self, container_path: &str, content: &[u8]) -> Result<()> {
        match &mut self.inner {
            SandboxManagerInner::Docker(manager) => {
                manager.upload_file(container_path, content).await
            }
            SandboxManagerInner::Broker(manager) => {
                manager.upload_file(container_path, content).await
            }
        }
    }

    pub async fn download_file(&mut self, container_path: &str) -> Result<Vec<u8>> {
        match &mut self.inner {
            SandboxManagerInner::Docker(manager) => manager.download_file(container_path).await,
            SandboxManagerInner::Broker(manager) => manager.download_file(container_path).await,
        }
    }

    pub async fn get_uploads_size(&mut self) -> Result<u64> {
        match &mut self.inner {
            SandboxManagerInner::Docker(manager) => manager.get_uploads_size().await,
            SandboxManagerInner::Broker(manager) => manager.get_uploads_size().await,
        }
    }

    pub async fn cleanup_old_downloads(&mut self) -> Result<u64> {
        match &mut self.inner {
            SandboxManagerInner::Docker(manager) => manager.cleanup_old_downloads().await,
            SandboxManagerInner::Broker(manager) => manager.cleanup_old_downloads().await,
        }
    }

    pub async fn destroy(&mut self) -> Result<()> {
        match &mut self.inner {
            SandboxManagerInner::Docker(manager) => manager.destroy().await,
            SandboxManagerInner::Broker(manager) => manager.destroy().await,
        }
    }

    pub async fn recreate(&mut self) -> Result<()> {
        match &mut self.inner {
            SandboxManagerInner::Docker(manager) => manager.recreate().await,
            SandboxManagerInner::Broker(manager) => manager.recreate().await,
        }
    }

    pub async fn file_size_bytes(
        &mut self,
        container_path: &str,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<u64> {
        match &mut self.inner {
            SandboxManagerInner::Docker(manager) => {
                manager
                    .file_size_bytes(container_path, cancellation_token)
                    .await
            }
            SandboxManagerInner::Broker(manager) => {
                manager
                    .file_size_bytes(container_path, cancellation_token)
                    .await
            }
        }
    }
}

/// Docker sandbox manager for isolated code execution
#[derive(Clone)]
pub(crate) struct DockerSandboxManager {
    docker: Docker,
    container_id: Option<String>,
    image_name: String,
    scope: SandboxScope,
}

const RECREATE_REMOVE_MAX_ATTEMPTS: usize = 8;
const RECREATE_REMOVE_INITIAL_BACKOFF_MS: u64 = 50;
const RECREATE_REMOVE_MAX_BACKOFF_MS: u64 = 800;

impl DockerSandboxManager {
    fn parse_label_i64(labels: &HashMap<String, String>, key: &str) -> Option<i64> {
        labels.get(key).and_then(|value| value.parse::<i64>().ok())
    }

    fn normalize_container_name(names: Option<&Vec<String>>, fallback: &str) -> String {
        names
            .and_then(|names| names.first())
            .map(|name| name.trim_start_matches('/').to_string())
            .unwrap_or_else(|| fallback.to_string())
    }

    fn record_from_container_summary(
        summary: &bollard::models::ContainerSummary,
    ) -> Option<SandboxContainerRecord> {
        let container_id = summary.id.clone()?;
        let labels = summary.labels.clone().unwrap_or_default();
        let state_raw = summary.state;
        let state = state_raw
            .as_ref()
            .map(|state| format!("{state:?}").to_ascii_lowercase());
        let status = summary.status.clone();
        let running = matches!(
            state_raw,
            Some(bollard::models::ContainerSummaryStateEnum::RUNNING)
        ) || status
            .as_deref()
            .is_some_and(|status| status.starts_with("Up"));

        Some(SandboxContainerRecord {
            container_name: Self::normalize_container_name(summary.names.as_ref(), &container_id),
            container_id,
            image: summary.image.clone(),
            created_at: summary.created,
            state,
            status,
            running,
            user_id: Self::parse_label_i64(&labels, "agent.user_id"),
            scope: labels.get("agent.scope").cloned(),
            chat_id: Self::parse_label_i64(&labels, "agent.chat_id"),
            thread_id: Self::parse_label_i64(&labels, "agent.thread_id"),
            labels,
        })
    }

    fn sandbox_filters(user_id: i64) -> HashMap<String, Vec<String>> {
        HashMap::from([(
            "label".to_string(),
            vec![
                "agent.sandbox=true".to_string(),
                format!("agent.user_id={user_id}"),
            ],
        )])
    }

    async fn connect_and_ping() -> Result<Docker> {
        let docker =
            Docker::connect_with_local_defaults().context("Failed to connect to Docker daemon")?;
        docker
            .ping()
            .await
            .context("Failed to ping Docker daemon")?;
        Ok(docker)
    }

    fn is_not_found_error(error: &DockerError) -> bool {
        matches!(
            error,
            DockerError::DockerResponseServerError {
                status_code: 404,
                ..
            }
        )
    }

    fn is_conflict_error(error: &DockerError) -> bool {
        matches!(
            error,
            DockerError::DockerResponseServerError {
                status_code: 409,
                ..
            }
        )
    }

    fn is_image_not_found_error(error: &DockerError, image_name: &str) -> bool {
        if !Self::is_not_found_error(error) {
            return false;
        }

        let error_message = error.to_string().to_ascii_lowercase();
        let image_name = image_name.to_ascii_lowercase();

        error_message.contains("no such image") && error_message.contains(&image_name)
    }

    async fn get_container_id_by_name(&self, container_name: &str) -> Result<Option<String>> {
        let mut filters = HashMap::new();
        filters.insert("name".to_string(), vec![container_name.to_string()]);

        let containers = self
            .docker
            .list_containers(Some(bollard::query_parameters::ListContainersOptions {
                all: true,
                filters: Some(filters),
                ..Default::default()
            }))
            .await
            .context("Failed to list containers by name")?;

        Ok(containers
            .first()
            .and_then(|container| container.id.clone()))
    }

    /// Create a new sandbox manager
    ///
    /// # Errors
    ///
    /// Returns an error if connection to Docker daemon fails or ping fails.
    #[instrument(skip_all)]
    pub(crate) async fn new(scope: impl Into<SandboxScope>) -> Result<Self> {
        Self::new_with_image(scope, get_sandbox_image()).await
    }

    #[instrument(skip_all)]
    pub(crate) async fn new_with_image(
        scope: impl Into<SandboxScope>,
        image_name: String,
    ) -> Result<Self> {
        let scope = scope.into();
        let docker = Self::connect_and_ping().await?;

        debug!(owner_id = scope.owner_id(), scope = %scope.namespace(), "Docker connection established");

        Ok(Self {
            docker,
            container_id: None,
            image_name,
            scope,
        })
    }

    #[instrument(skip(self), fields(owner_id = self.scope.owner_id(), scope = %self.scope.namespace()))]
    pub(crate) async fn attach_existing_container(&mut self) -> Result<bool> {
        if self.refresh_container_liveness().await {
            return Ok(true);
        }

        let container_name = self.scope.container_name();
        let Some(container_id) = self.get_container_id_by_name(&container_name).await? else {
            return Ok(false);
        };

        self.container_id = Some(container_id.clone());

        if let Err(error) = self
            .docker
            .start_container(&container_id, None::<StartContainerOptions>)
            .await
        {
            debug!(
                owner_id = self.scope.owner_id(),
                scope = %self.scope.namespace(),
                container_id = %container_id,
                error = %error,
                "Tried to start existing sandbox container while reattaching (might already be running)"
            );
        }

        info!(
            owner_id = self.scope.owner_id(),
            scope = %self.scope.namespace(),
            container_id = %container_id,
            "Reattached sandbox manager to existing container"
        );

        Ok(true)
    }

    /// List all sandbox containers owned by a user.
    pub async fn list_user_sandboxes(user_id: i64) -> Result<Vec<SandboxContainerRecord>> {
        let docker = Self::connect_and_ping().await?;
        let containers = docker
            .list_containers(Some(bollard::query_parameters::ListContainersOptions {
                all: true,
                filters: Some(Self::sandbox_filters(user_id)),
                ..Default::default()
            }))
            .await
            .context("Failed to list sandbox containers")?;

        let mut records = containers
            .iter()
            .filter_map(Self::record_from_container_summary)
            .collect::<Vec<_>>();
        records.sort_by(|left, right| left.container_name.cmp(&right.container_name));
        Ok(records)
    }

    /// Inspect a sandbox container by its Docker name.
    pub async fn inspect_sandbox_by_name(
        user_id: i64,
        container_name: &str,
    ) -> Result<Option<SandboxContainerRecord>> {
        let docker = Self::connect_and_ping().await?;
        let mut filters = Self::sandbox_filters(user_id);
        filters.insert("name".to_string(), vec![container_name.to_string()]);
        let containers = docker
            .list_containers(Some(bollard::query_parameters::ListContainersOptions {
                all: true,
                filters: Some(filters),
                ..Default::default()
            }))
            .await
            .context("Failed to inspect sandbox container by name")?;

        Ok(containers
            .iter()
            .filter_map(Self::record_from_container_summary)
            .find(|record| record.container_name == container_name))
    }

    /// Ensure a sandbox exists for the provided scope and return its Docker metadata.
    pub async fn ensure_scope_sandbox(scope: SandboxScope) -> Result<SandboxContainerRecord> {
        Self::ensure_scope_sandbox_with_image(scope, get_sandbox_image()).await
    }

    /// Ensure a sandbox exists for the provided scope and image and return its Docker metadata.
    pub(crate) async fn ensure_scope_sandbox_with_image(
        scope: SandboxScope,
        image_name: String,
    ) -> Result<SandboxContainerRecord> {
        let container_name = scope.container_name();
        let owner_id = scope.owner_id();
        let mut sandbox = Self::new_with_image(scope, image_name).await?;
        sandbox.create_sandbox().await?;
        Self::inspect_sandbox_by_name(owner_id, &container_name)
            .await?
            .ok_or_else(|| {
                anyhow!("sandbox container '{container_name}' was not found after create")
            })
    }

    /// Recreate a sandbox for the provided scope and return its Docker metadata.
    pub async fn recreate_scope_sandbox(scope: SandboxScope) -> Result<SandboxContainerRecord> {
        Self::recreate_scope_sandbox_with_image(scope, get_sandbox_image()).await
    }

    /// Recreate a sandbox for the provided scope and image and return its Docker metadata.
    pub(crate) async fn recreate_scope_sandbox_with_image(
        scope: SandboxScope,
        image_name: String,
    ) -> Result<SandboxContainerRecord> {
        let container_name = scope.container_name();
        let owner_id = scope.owner_id();
        let mut sandbox = Self::new_with_image(scope, image_name).await?;
        sandbox.recreate().await?;
        Self::inspect_sandbox_by_name(owner_id, &container_name)
            .await?
            .ok_or_else(|| {
                anyhow!("sandbox container '{container_name}' was not found after recreate")
            })
    }

    /// Delete a user-owned sandbox by Docker container name.
    pub async fn delete_sandbox_by_name(user_id: i64, container_name: &str) -> Result<bool> {
        let Some(_) = Self::inspect_sandbox_by_name(user_id, container_name).await? else {
            return Ok(false);
        };

        let docker = Self::connect_and_ping().await?;
        let options = RemoveContainerOptions {
            force: true,
            ..Default::default()
        };
        match docker.remove_container(container_name, Some(options)).await {
            Ok(()) => Ok(true),
            Err(error) if Self::is_not_found_error(&error) => Ok(false),
            Err(error) => Err(error).context("Failed to delete sandbox container by name"),
        }
    }

    /// Check if sandbox container is running
    #[must_use]
    pub const fn is_running(&self) -> bool {
        self.container_id.is_some()
    }

    /// Validate tracked container liveness and clear stale state.
    async fn refresh_container_liveness(&mut self) -> bool {
        let Some(container_id) = self.container_id.clone() else {
            return false;
        };

        match self
            .docker
            .inspect_container(&container_id, None::<InspectContainerOptions>)
            .await
        {
            Ok(_) => true,
            Err(error) => {
                if Self::is_not_found_error(&error) {
                    warn!(
                        owner_id = self.scope.owner_id(),
                        scope = %self.scope.namespace(),
                        container_id = %container_id,
                        error = %error,
                        "Sandbox container not found, resetting tracked container_id"
                    );
                    self.container_id = None;
                    false
                } else {
                    warn!(
                        owner_id = self.scope.owner_id(),
                        scope = %self.scope.namespace(),
                        container_id = %container_id,
                        error = %error,
                        "Sandbox container inspect failed, preserving tracked container_id"
                    );
                    true
                }
            }
        }
    }

    /// Get container ID if running
    #[must_use]
    pub fn container_id(&self) -> Option<&str> {
        self.container_id.as_deref()
    }

    /// Sandbox scope used for persistent container identity.
    #[must_use]
    pub fn scope(&self) -> &SandboxScope {
        &self.scope
    }

    async fn has_container_with_name(&self, container_name: &str) -> Result<bool> {
        let mut filters = HashMap::new();
        filters.insert("name".to_string(), vec![container_name.to_string()]);

        let containers = self
            .docker
            .list_containers(Some(bollard::query_parameters::ListContainersOptions {
                all: true,
                filters: Some(filters),
                ..Default::default()
            }))
            .await
            .context("Failed to list containers by name")?;

        Ok(!containers.is_empty())
    }

    async fn wait_for_container_removal_by_name(&self, container_name: &str) -> Result<()> {
        let mut backoff_ms = RECREATE_REMOVE_INITIAL_BACKOFF_MS;

        for attempt in 1..=RECREATE_REMOVE_MAX_ATTEMPTS {
            if !self.has_container_with_name(container_name).await? {
                debug!(
                    owner_id = self.scope.owner_id(),
                    scope = %self.scope.namespace(),
                    container_name, attempt, "Container name is free for recreate"
                );
                return Ok(());
            }

            warn!(
                owner_id = self.scope.owner_id(),
                scope = %self.scope.namespace(),
                container_name,
                attempt,
                backoff_ms,
                "Container still exists after remove request, waiting before recreate"
            );
            sleep(Duration::from_millis(backoff_ms)).await;
            backoff_ms = (backoff_ms.saturating_mul(2)).min(RECREATE_REMOVE_MAX_BACKOFF_MS);
        }

        Err(anyhow!(
            "Timed out waiting for container removal: {container_name}"
        ))
    }

    /// Create and start a new sandbox container
    ///
    /// # Errors
    ///
    /// Returns an error if container creation or starting fails.
    #[instrument(skip(self), fields(owner_id = self.scope.owner_id(), scope = %self.scope.namespace()))]
    pub async fn create_sandbox(&mut self) -> Result<()> {
        if self.refresh_container_liveness().await {
            // Already tracked in this object
            return Ok(());
        }

        let container_name = self.scope.container_name();

        // Check if container already exists
        if let Some(id) = self.get_container_id_by_name(&container_name).await? {
            info!(owner_id = self.scope.owner_id(), scope = %self.scope.namespace(), container_id = %id, "Found existing sandbox container");
            self.container_id = Some(id.clone());

            // Simpler: Just try to start it.
            if let Err(e) = self
                .docker
                .start_container(&id, None::<StartContainerOptions>)
                .await
            {
                // If it's already running, this might error or might not.
                // We'll log debug and proceed.
                debug!(error = %e, "Tried to start existing container (might already be running)");
            }
            return Ok(());
        }

        // Container configuration with resource limits
        let host_config = HostConfig {
            memory: Some(SANDBOX_MEMORY_LIMIT),
            cpu_period: Some(SANDBOX_CPU_PERIOD),
            cpu_quota: Some(SANDBOX_CPU_QUOTA),
            // Network access enabled (bridge mode)
            network_mode: Some("bridge".to_string()),
            // Auto-remove on stop (safety net)
            auto_remove: Some(true),
            ..Default::default()
        };

        let config = ContainerCreateBody {
            image: Some(self.image_name.clone()),
            hostname: Some("sandbox".to_string()),
            working_dir: Some("/workspace".to_string()),
            host_config: Some(host_config),
            labels: Some(self.scope.docker_labels()),
            // Keep container running
            cmd: Some(vec!["sleep".to_string(), "infinity".to_string()]),
            ..Default::default()
        };

        let options = CreateContainerOptions {
            name: Some(container_name.clone()),
            ..Default::default()
        };

        // Create container
        let container_id = match self.docker.create_container(Some(options), config).await {
            Ok(response) => {
                let container_id = response.id;
                info!(container_id = %container_id, "Sandbox container created");
                container_id
            }
            Err(error) if Self::is_conflict_error(&error) => {
                warn!(
                    owner_id = self.scope.owner_id(),
                    scope = %self.scope.namespace(),
                    container_name = %container_name,
                    error = %error,
                    "Sandbox create conflicted, resolving existing container by name"
                );

                let resolved_id = self
                    .get_container_id_by_name(&container_name)
                    .await?
                    .ok_or_else(|| {
                        anyhow!(
                            "Sandbox create conflicted but no container found by name: {container_name}"
                        )
                    })?;

                info!(container_id = %resolved_id, "Resolved sandbox container after create conflict");
                resolved_id
            }
            Err(error) if Self::is_image_not_found_error(&error, &self.image_name) => {
                return Err(error).with_context(|| {
                    format!(
                        "Sandbox image '{}' not found. Build it with `docker compose --profile build build sandbox_image`",
                        self.image_name
                    )
                });
            }
            Err(error) => return Err(error).context("Failed to create sandbox container"),
        };

        // Start container
        self.docker
            .start_container(&container_id, None::<StartContainerOptions>)
            .await
            .context("Failed to start sandbox container")?;

        self.container_id = Some(container_id.clone());
        info!(container_id = %container_id, "Sandbox container started");

        Ok(())
    }

    /// Kill all processes in the container (SIGKILL)
    ///
    /// Used when cancelling an ongoing command execution.
    /// Returns Ok even if kill fails (best effort cleanup).
    async fn kill_processes(&self) {
        if let Some(container_id) = &self.container_id {
            // Best effort kill: send SIGKILL to all processes
            // We use killall5 which sends signal to all processes except kernel threads
            let kill_cmd = "killall5 -9 2>/dev/null || true";

            debug!(
                container_id = %container_id,
                "Attempting to kill all processes in container"
            );

            // Execute without recursion (use internal Docker API directly to avoid deadlock)
            let exec_options = CreateExecOptions {
                attach_stdout: Some(false),
                attach_stderr: Some(false),
                cmd: Some(vec!["sh", "-c", kill_cmd]),
                ..Default::default()
            };

            if let Ok(exec) = self.docker.create_exec(container_id, exec_options).await {
                // Fire and forget - don't wait for completion
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    self.docker.start_exec(&exec.id, None),
                )
                .await;

                info!(container_id = %container_id, "Process kill signal sent");
            } else {
                warn!(container_id = %container_id, "Failed to create kill exec");
            }
        }
    }

    /// Execute a command in the sandbox
    ///
    /// # Arguments
    ///
    /// * `cmd` - The command to execute
    /// * `cancellation_token` - Optional token to allow cancellation of long-running commands
    ///
    /// # Errors
    ///
    /// Returns an error if sandbox is not running, exec creation fails, execution times out, or is cancelled.
    #[instrument(skip(self, cancellation_token), fields(container_id = ?self.container_id))]
    pub async fn exec_command(
        &mut self,
        cmd: &str,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<ExecResult> {
        if !self.refresh_container_liveness().await {
            self.create_sandbox().await?;
        }

        let container_id = self
            .container_id
            .as_ref()
            .cloned()
            .ok_or_else(|| anyhow!("Sandbox not running"))?;

        debug!(cmd = %cmd, "Executing command in sandbox");

        let exec_options = CreateExecOptions {
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            cmd: Some(vec!["sh", "-c", cmd]),
            working_dir: Some("/workspace"),
            ..Default::default()
        };

        let exec = self
            .docker
            .create_exec(&container_id, exec_options)
            .await
            .context("Failed to create exec")?;

        // If cancellation_token is provided, use select! to handle cancellation
        let result = if let Some(token) = cancellation_token {
            use tokio::select;
            select! {
                res = tokio::time::timeout(
                    std::time::Duration::from_secs(SANDBOX_EXEC_TIMEOUT_SECS),
                    self.run_exec(&exec.id),
                ) => {
                    res.map_err(|_| anyhow!("Command execution timed out after {SANDBOX_EXEC_TIMEOUT_SECS}s"))?
                        .context("Command execution failed")?
                },
                _ = token.cancelled() => {
                    warn!(exec_id = %exec.id, cmd = %cmd, "Command cancelled by user, killing processes");

                    // Kill all processes in the container
                    self.kill_processes().await;

                    return Err(anyhow!("Command execution cancelled by user"));
                }
            }
        } else {
            // No cancellation token: use original timeout logic
            tokio::time::timeout(
                std::time::Duration::from_secs(SANDBOX_EXEC_TIMEOUT_SECS),
                self.run_exec(&exec.id),
            )
            .await
            .map_err(|_| anyhow!("Command execution timed out after {SANDBOX_EXEC_TIMEOUT_SECS}s"))?
            .context("Command execution failed")?
        };

        debug!(
            exit_code = result.exit_code,
            stdout_len = result.stdout.len(),
            stderr_len = result.stderr.len(),
            "Command completed"
        );

        Ok(result)
    }

    /// Run the exec and collect output
    async fn run_exec(&self, exec_id: &str) -> Result<ExecResult> {
        let output = self.docker.start_exec(exec_id, None).await?;

        let mut stdout = String::new();
        let mut stderr = String::new();

        if let StartExecResults::Attached { mut output, .. } = output {
            while let Some(msg) = output.next().await {
                match msg? {
                    bollard::container::LogOutput::StdOut { message } => {
                        stdout.push_str(&String::from_utf8_lossy(&message));
                    }
                    bollard::container::LogOutput::StdErr { message } => {
                        stderr.push_str(&String::from_utf8_lossy(&message));
                    }
                    _ => {}
                }
            }
        }

        // Get exit code
        let inspect = self.docker.inspect_exec(exec_id).await?;
        let exit_code = inspect.exit_code.unwrap_or(-1);

        Ok(ExecResult {
            stdout,
            stderr,
            exit_code,
        })
    }

    /// Write content to a file in the sandbox
    ///
    /// # Errors
    ///
    /// Returns an error if sandbox is not running or file writing fails.
    #[instrument(skip(self, content), fields(path = %path, content_len = content.len()))]
    pub async fn write_file(&mut self, path: &str, content: &[u8]) -> Result<()> {
        if self.container_id.is_none() {
            return Err(anyhow!("Sandbox not running"));
        }

        // Use base64 to safely transfer binary content
        let encoded = base64::engine::general_purpose::STANDARD.encode(content);

        let cmd = format!(
            "echo '{}' | base64 -d > {}",
            encoded,
            shell_escape::escape(path.into())
        );

        let result = self.exec_command(&cmd, None).await?;

        if !result.success() {
            return Err(anyhow!("Failed to write file: {}", result.stderr));
        }

        debug!(path = %path, "File written to sandbox");
        Ok(())
    }

    /// Read content from a file in the sandbox.
    ///
    /// # Errors
    ///
    /// Returns an error if file reading fails.
    #[instrument(skip(self), fields(path = %path))]
    pub async fn read_file(&mut self, path: &str) -> Result<Vec<u8>> {
        self.download_file_via_docker_api(path, None).await
    }

    /// Upload a file to the container using Docker's copy API
    ///
    /// Uses tar archive format as required by Docker API.
    /// Creates parent directories automatically.
    ///
    /// # Errors
    ///
    /// Returns an error if sandbox is not running, directory creation fails, or upload fails.
    #[instrument(skip(self, content), fields(path = %container_path, content_len = content.len()))]
    pub async fn upload_file(&mut self, container_path: &str, content: &[u8]) -> Result<()> {
        let container_id = self
            .container_id
            .as_ref()
            .cloned()
            .ok_or_else(|| anyhow!("Sandbox not running"))?;

        let path = std::path::Path::new(container_path);
        let parent = path.parent().map_or_else(
            || "/workspace".to_string(),
            |p| p.to_string_lossy().to_string(),
        );
        let file_name = path
            .file_name()
            .map_or_else(|| "file".to_string(), |n| n.to_string_lossy().to_string());

        // Ensure parent directory exists
        self.exec_command(&format!("mkdir -p '{parent}'"), None)
            .await?;

        // Create tar archive in memory
        let mut tar_buffer = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_buffer);
            let mut header = tar::Header::new_gnu();
            header.set_path(&file_name)?;
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            );
            header.set_cksum();
            builder.append(&header, content)?;
            builder.finish()?;
        }

        self.docker
            .upload_to_container(
                &container_id,
                Some(UploadToContainerOptions {
                    path: parent,
                    ..Default::default()
                }),
                Either::Left(Full::new(Bytes::from(tar_buffer))),
            )
            .await
            .context("Failed to upload file to container")?;

        info!(
            container_id = %container_id,
            path = %container_path,
            size = content.len(),
            "File uploaded to sandbox"
        );

        Ok(())
    }

    /// Download a file from the container
    ///
    /// Returns the raw file content as bytes.
    /// Uses Docker's download API with tar extraction.
    ///
    /// # Errors
    ///
    /// Returns an error if sandbox is not running, file doesn't exist, file is too large, or download/extraction fails.
    #[instrument(skip(self), fields(path = %container_path))]
    pub async fn download_file(&mut self, container_path: &str) -> Result<Vec<u8>> {
        const MAX_FILE_SIZE: u64 = 50 * 1024 * 1024; // 50 MB
        self.download_file_via_docker_api(container_path, Some(MAX_FILE_SIZE))
            .await
    }

    async fn download_file_via_docker_api(
        &mut self,
        container_path: &str,
        max_file_size: Option<u64>,
    ) -> Result<Vec<u8>> {
        // Reuse the existing size check to self-heal stale container IDs and verify the path exists.
        let file_size = self.file_size_bytes(container_path, None).await?;

        if let Some(max_file_size) = max_file_size {
            if file_size > max_file_size {
                anyhow::bail!(
                    "File too large: {} bytes (max {} MB)",
                    file_size,
                    max_file_size / 1024 / 1024
                );
            }
        }

        let container_id = self
            .container_id
            .as_ref()
            .cloned()
            .ok_or_else(|| anyhow!("Sandbox not running"))?;

        let stream = self
            .docker
            .download_from_container(
                &container_id,
                Some(DownloadFromContainerOptions {
                    path: container_path.to_string(),
                }),
            )
            .try_collect::<Vec<_>>()
            .await
            .context("Failed to download file from container")?;

        let tar_data: Vec<u8> = stream.into_iter().flatten().collect();
        let mut archive = tar::Archive::new(tar_data.as_slice());
        let mut entries = archive.entries()?;

        if let Some(entry_result) = entries.next() {
            let mut entry = entry_result?;
            let mut content = Vec::new();
            entry.read_to_end(&mut content)?;

            info!(
                container_id = %container_id,
                path = %container_path,
                size = content.len(),
                "File downloaded from sandbox"
            );

            Ok(content)
        } else {
            anyhow::bail!("Empty tar archive received")
        }
    }

    /// Get total size of uploaded files in /workspace/uploads/
    ///
    /// # Errors
    ///
    /// Returns an error if the command execution fails or the output cannot be parsed.
    #[instrument(skip(self))]
    pub async fn get_uploads_size(&mut self) -> Result<u64> {
        let result = self
            .exec_command("du -sb /workspace/uploads 2>/dev/null || echo '0'", None)
            .await?;

        let size_str = result.stdout.split_whitespace().next().unwrap_or("0");
        size_str
            .parse::<u64>()
            .map_err(|e| anyhow!("Failed to parse uploads size: {e}"))
    }

    /// Clean up old media files in /workspace/downloads/ (older than 7 days)
    ///
    /// This helps prevent accumulation of orphaned media files from ytdlp downloads.
    /// Files are considered orphaned if delivery failed or was interrupted.
    ///
    /// # Errors
    ///
    /// Returns an error if the cleanup command fails.
    #[instrument(skip(self))]
    pub async fn cleanup_old_downloads(&mut self) -> Result<u64> {
        // Find and count files older than 7 days
        let count_cmd = "find /workspace/downloads -type f -mtime +7 2>/dev/null | wc -l";
        let count_result = self.exec_command(count_cmd, None).await?;
        let count: u64 = count_result.stdout.trim().parse().unwrap_or(0);

        if count > 0 {
            // Delete files older than 7 days
            let cleanup_cmd = "find /workspace/downloads -type f -mtime +7 -delete 2>/dev/null";
            self.exec_command(cleanup_cmd, None).await?;
            info!(files_deleted = count, "Cleaned up old download files");
        }

        Ok(count)
    }

    /// Destroy the sandbox container
    ///
    /// # Errors
    ///
    /// Returns an error if container removal fails.
    #[instrument(skip(self), fields(container_id = ?self.container_id))]
    pub async fn destroy(&mut self) -> Result<()> {
        let container_ref = if let Some(container_id) = self.container_id.take() {
            Some(container_id)
        } else {
            self.get_container_id_by_name(&self.scope.container_name())
                .await?
        };

        if let Some(container_ref) = container_ref {
            info!(container_ref = %container_ref, scope = %self.scope.namespace(), "Destroying sandbox container");

            let options = RemoveContainerOptions {
                force: true,
                ..Default::default()
            };

            if let Err(e) = self
                .docker
                .remove_container(&container_ref, Some(options))
                .await
            {
                // Container might already be removed (auto_remove)
                warn!(container_ref = %container_ref, error = %e, "Failed to remove container (may already be removed)");
            } else {
                info!(container_ref = %container_ref, "Sandbox container destroyed");
            }
        } else {
            debug!(scope = %self.scope.namespace(), "No sandbox container found for destroy");
        }

        Ok(())
    }

    /// Recreate the sandbox container (wipe data)
    ///
    /// # Errors
    ///
    /// Returns an error if destruction or creation fails.
    #[instrument(skip(self), fields(owner_id = self.scope.owner_id(), scope = %self.scope.namespace()))]
    pub async fn recreate(&mut self) -> Result<()> {
        info!("Recreating sandbox");

        // Clear stale in-memory ID before recreation attempts.
        self.refresh_container_liveness().await;

        let container_name = self.scope.container_name();

        // Force destroy current container
        if self.container_id.is_some() {
            self.destroy().await?;
        }

        // Best effort cleanup by name in case ID was stale/lost or removal is still converging.
        if let Err(error) = self
            .docker
            .remove_container(
                &container_name,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await
        {
            debug!(
                owner_id = self.scope.owner_id(),
                scope = %self.scope.namespace(),
                container_name = %container_name,
                error = %error,
                "Remove-by-name before recreate returned error"
            );
        }

        self.wait_for_container_removal_by_name(&container_name)
            .await?;

        // Create new one
        self.create_sandbox().await
    }

    /// Get a file size from inside the sandbox in bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if sandbox is not running, file doesn't exist, or output can't be parsed.
    #[instrument(skip(self, cancellation_token), fields(path = %container_path))]
    pub async fn file_size_bytes(
        &mut self,
        container_path: &str,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<u64> {
        let escaped_path = escape(container_path.into());

        let check_cmd = format!("test -f {escaped_path} && echo 'exists'");
        let check = self.exec_command(&check_cmd, cancellation_token).await?;
        if !check.stdout.contains("exists") {
            anyhow::bail!("File not found: {container_path}");
        }

        let size_cmd = format!("stat -c %s {escaped_path}");
        let size_check = self.exec_command(&size_cmd, cancellation_token).await?;
        let file_size: u64 = size_check
            .stdout
            .trim()
            .parse()
            .context("Failed to parse file size")?;

        Ok(file_size)
    }
}

impl Drop for DockerSandboxManager {
    fn drop(&mut self) {
        if let Some(ref id) = self.container_id {
            info!(
                container_id = %id,
                "DockerSandboxManager dropped. Container persists in Docker (intentional)."
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Integration test - requires Docker
    #[tokio::test]
    #[ignore = "Requires Docker daemon"]
    async fn test_sandbox_lifecycle() -> Result<(), Box<dyn std::error::Error>> {
        let mut sandbox = DockerSandboxManager::new(12345).await?;

        // Create sandbox
        sandbox.create_sandbox().await?;
        assert!(sandbox.is_running());

        // Execute command
        let result = sandbox.exec_command("echo 'Hello, World!'", None).await?;
        assert!(result.success());
        assert!(result.stdout.contains("Hello, World!"));

        // Python test
        let result = sandbox
            .exec_command("python3 -c \"print(2 + 2)\"", None)
            .await?;
        assert!(result.success());
        assert!(result.stdout.contains('4'));

        // Cleanup
        sandbox.destroy().await?;
        assert!(!sandbox.is_running());
        Ok(())
    }

    #[tokio::test]
    #[ignore = "Requires Docker daemon"]
    async fn test_exec_self_heals_stale_container_id() -> Result<(), Box<dyn std::error::Error>> {
        let mut sandbox = DockerSandboxManager::new(12346).await?;
        sandbox.create_sandbox().await?;

        sandbox.container_id = Some("stale-container-id".to_string());

        let result = sandbox.exec_command("echo 'healed'", None).await?;
        assert!(result.success());
        assert!(result.stdout.contains("healed"));

        sandbox.destroy().await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "Requires Docker daemon"]
    async fn test_recreate_clears_workspace() -> Result<(), Box<dyn std::error::Error>> {
        let mut sandbox = DockerSandboxManager::new(12347).await?;
        sandbox.create_sandbox().await?;

        sandbox
            .write_file("/workspace/recreate-me.txt", b"before recreate")
            .await?;
        let before = sandbox.read_file("/workspace/recreate-me.txt").await?;
        assert_eq!(before, b"before recreate");

        sandbox.recreate().await?;

        let after = sandbox.read_file("/workspace/recreate-me.txt").await;
        assert!(
            after.is_err(),
            "workspace file should be removed after recreate"
        );

        sandbox.destroy().await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "Requires Docker daemon"]
    async fn test_read_file_round_trips_binary_content() -> Result<(), Box<dyn std::error::Error>> {
        let mut sandbox = DockerSandboxManager::new(12348).await?;
        sandbox.create_sandbox().await?;

        let payload = (0..128).map(|value| value as u8).collect::<Vec<_>>();
        sandbox
            .upload_file("/workspace/binary-roundtrip.bin", &payload)
            .await?;

        let content = sandbox.read_file("/workspace/binary-roundtrip.bin").await?;
        assert_eq!(content, payload);

        sandbox.destroy().await?;
        Ok(())
    }
}
