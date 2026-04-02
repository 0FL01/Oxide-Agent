#![allow(missing_docs)]

//! Docker sandbox manager using Bollard
//!
//! Manages Docker containers for isolated code execution.

use anyhow::{anyhow, Context, Result};
use bollard::container::LogOutput;
use bollard::errors::Error as DockerError;
use bollard::exec::{CreateExecOptions, StartExecResults};
use bollard::models::{
    ContainerCreateBody, ContainerSummary, ContainerSummaryStateEnum, HostConfig,
};
use bollard::query_parameters::{
    CreateContainerOptions, DownloadFromContainerOptions, InspectContainerOptions,
    LogsOptionsBuilder, RemoveContainerOptions, StartContainerOptions, UploadToContainerOptions,
};
use bollard::Docker;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_util::{StreamExt, TryStreamExt};
use http_body_util::{Either, Full};
use serde::{Deserialize, Serialize};
use shell_escape::escape;
use std::collections::{BTreeSet, HashMap};
use std::io::Read;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, info, instrument, warn};

use crate::config::{
    get_sandbox_image, get_stack_logs_project, sandbox_uses_broker, SANDBOX_CPU_PERIOD,
    SANDBOX_CPU_QUOTA, SANDBOX_EXEC_TIMEOUT_SECS, SANDBOX_MEMORY_LIMIT,
};
use crate::sandbox::broker::{
    ResolvedStackLogsSelector, SandboxBrokerClient, StackLogCursor, StackLogEntry, StackLogSource,
    StackLogSuppression, StackLogsFetchRequest, StackLogsFetchResponse,
    StackLogsListSourcesRequest, StackLogsListSourcesResponse, StackLogsSelector, StackLogsWindow,
};
use crate::sandbox::SandboxScope;

const DOCKER_COMPOSE_PROJECT_LABEL: &str = "com.docker.compose.project";
const DOCKER_COMPOSE_SERVICE_LABEL: &str = "com.docker.compose.service";
const STACK_LOGS_PROJECT_ENV: &str = "STACK_LOGS_PROJECT";
const UNKNOWN_STACK_LOG_STATE: &str = "unknown";
const STACK_LOG_STREAM_STDOUT: &str = "stdout";
const STACK_LOG_STREAM_STDERR: &str = "stderr";
const STACK_LOGS_HARD_MAX_ENTRIES: usize = 500;

#[derive(Default)]
struct StackLogBufferState {
    buffer: String,
    raw_ordinal: u64,
}

/// Result of executing a command in the sandbox
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    fn normalize_non_empty(value: &str) -> Option<String> {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }

    fn normalize_optional_string(value: Option<&str>) -> Option<String> {
        value.and_then(Self::normalize_non_empty)
    }

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

    fn normalize_requested_stack_log_services(services: &[String]) -> BTreeSet<String> {
        services
            .iter()
            .filter_map(|service| Self::normalize_non_empty(service))
            .collect()
    }

    fn stack_logs_max_entries(max_entries: u32, warnings: &mut Vec<String>) -> usize {
        let requested = usize::try_from(max_entries).unwrap_or(usize::MAX);
        if requested == 0 {
            warnings.push("Requested max_entries=0 is invalid; using 1".to_string());
            return 1;
        }

        let effective = requested.min(STACK_LOGS_HARD_MAX_ENTRIES);
        if requested > STACK_LOGS_HARD_MAX_ENTRIES {
            warnings.push(format!(
                "Requested max_entries={requested} exceeds hard limit {STACK_LOGS_HARD_MAX_ENTRIES}; using {STACK_LOGS_HARD_MAX_ENTRIES}"
            ));
        }
        effective
    }

    fn stack_logs_query_timestamp(value: Option<DateTime<Utc>>) -> i32 {
        value.map_or(0, |value| {
            i32::try_from(value.timestamp()).unwrap_or_else(|_| {
                if value.timestamp().is_negative() {
                    i32::MIN
                } else {
                    i32::MAX
                }
            })
        })
    }

    fn validate_stack_logs_window(
        since: Option<DateTime<Utc>>,
        until: Option<DateTime<Utc>>,
    ) -> Result<()> {
        if let (Some(since), Some(until)) = (since, until) {
            if since > until {
                return Err(anyhow!(
                    "Invalid stack log time window: 'since' must be earlier than or equal to 'until'"
                ));
            }
        }

        Ok(())
    }

    fn resolve_stack_logs_selector(
        request_selector: &StackLogsSelector,
        env_compose_project: Option<String>,
        runtime_compose_project: Option<String>,
    ) -> Result<ResolvedStackLogsSelector> {
        if let Some(compose_project) =
            Self::normalize_optional_string(request_selector.compose_project.as_deref())
        {
            return Ok(ResolvedStackLogsSelector { compose_project });
        }

        if let Some(compose_project) = env_compose_project
            .as_deref()
            .and_then(Self::normalize_non_empty)
        {
            return Ok(ResolvedStackLogsSelector { compose_project });
        }

        if let Some(compose_project) = runtime_compose_project
            .as_deref()
            .and_then(Self::normalize_non_empty)
        {
            return Ok(ResolvedStackLogsSelector { compose_project });
        }

        Err(anyhow!(
            "Unable to resolve compose project for stack log discovery; set {STACK_LOGS_PROJECT_ENV} or run sandboxd inside a Docker Compose deployment"
        ))
    }

    async fn detect_runtime_compose_project(docker: &Docker) -> Result<String> {
        let hostname = Self::normalize_optional_string(std::env::var("HOSTNAME").ok().as_deref())
            .ok_or_else(|| {
                anyhow!(
                    "Unable to resolve compose project for stack log discovery automatically: HOSTNAME is unavailable; set {STACK_LOGS_PROJECT_ENV}"
                )
            })?;

        let inspect = docker
            .inspect_container(&hostname, None::<InspectContainerOptions>)
            .await
            .context("Failed to inspect current sandboxd container for stack log discovery")?;

        inspect
            .config
            .as_ref()
            .and_then(|config| config.labels.as_ref())
            .and_then(|labels| {
                Self::normalize_optional_string(
                    labels.get(DOCKER_COMPOSE_PROJECT_LABEL).map(String::as_str),
                )
            })
            .ok_or_else(|| {
                anyhow!(
                    "Unable to resolve compose project for stack log discovery automatically: current sandboxd container is missing label '{DOCKER_COMPOSE_PROJECT_LABEL}'; set {STACK_LOGS_PROJECT_ENV}"
                )
            })
    }

    fn container_summary_is_running(summary: &ContainerSummary) -> bool {
        matches!(summary.state, Some(ContainerSummaryStateEnum::RUNNING))
            || summary
                .status
                .as_deref()
                .is_some_and(|status| status.starts_with("Up"))
    }

    fn container_summary_state(summary: &ContainerSummary) -> String {
        summary
            .state
            .map(|state| state.to_string())
            .filter(|state| !state.is_empty())
            .or_else(|| {
                summary
                    .status
                    .as_deref()
                    .and_then(Self::normalize_non_empty)
            })
            .unwrap_or_else(|| UNKNOWN_STACK_LOG_STATE.to_string())
    }

    fn parse_stack_log_started_at(started_at: Option<&str>) -> Option<DateTime<Utc>> {
        started_at
            .and_then(Self::normalize_non_empty)
            .and_then(|started_at| DateTime::parse_from_rfc3339(&started_at).ok())
            .map(|started_at| started_at.with_timezone(&Utc))
    }

    fn parse_timestamped_stack_log_line(line: &str) -> Option<(DateTime<Utc>, String)> {
        let line = line.trim_end_matches('\r');
        let (timestamp, message) = line.split_once(' ')?;
        let timestamp = DateTime::parse_from_rfc3339(timestamp).ok()?;
        Some((timestamp.with_timezone(&Utc), message.to_string()))
    }

    fn drain_complete_stack_log_lines(buffer: &mut String) -> Vec<String> {
        let mut lines = Vec::new();
        while let Some(newline_index) = buffer.find('\n') {
            let line = buffer.drain(..=newline_index).collect::<String>();
            lines.push(line.trim_end_matches('\n').to_string());
        }
        lines
    }

    fn push_stack_log_line(
        source: &StackLogSource,
        stream: &str,
        line: String,
        raw_ordinal: &mut u64,
        entries: &mut Vec<StackLogEntry>,
        unparsable_lines: &mut u64,
    ) {
        let Some((ts, message)) = Self::parse_timestamped_stack_log_line(&line) else {
            *unparsable_lines += 1;
            return;
        };

        entries.push(StackLogEntry {
            ts,
            service: source.service.clone(),
            container_name: source.container_name.clone(),
            stream: stream.to_string(),
            ordinal: *raw_ordinal,
            message,
        });
        *raw_ordinal += 1;
    }

    fn ingest_stack_log_chunk(
        source: &StackLogSource,
        stream: &str,
        chunk: &[u8],
        state: &mut StackLogBufferState,
        entries: &mut Vec<StackLogEntry>,
        unparsable_lines: &mut u64,
    ) {
        state.buffer.push_str(&String::from_utf8_lossy(chunk));
        for line in Self::drain_complete_stack_log_lines(&mut state.buffer) {
            Self::push_stack_log_line(
                source,
                stream,
                line,
                &mut state.raw_ordinal,
                entries,
                unparsable_lines,
            );
        }
    }

    fn finalize_stack_log_buffer(
        source: &StackLogSource,
        stream: &str,
        state: &mut StackLogBufferState,
        entries: &mut Vec<StackLogEntry>,
        unparsable_lines: &mut u64,
    ) {
        if state.buffer.is_empty() {
            return;
        }

        let trailing_line = std::mem::take(&mut state.buffer);
        Self::push_stack_log_line(
            source,
            stream,
            trailing_line,
            &mut state.raw_ordinal,
            entries,
            unparsable_lines,
        );
    }

    fn assign_stack_log_ordinals(entries: &mut [StackLogEntry]) {
        let mut next_ordinals: HashMap<(String, String), u64> = HashMap::new();
        for entry in entries {
            let next_ordinal = next_ordinals
                .entry((entry.service.clone(), entry.stream.clone()))
                .or_insert(0);
            entry.ordinal = *next_ordinal;
            *next_ordinal += 1;
        }
    }

    fn record_stack_log_suppression(
        suppressed: &mut Vec<StackLogSuppression>,
        reason: &str,
        count: u64,
    ) {
        if count == 0 {
            return;
        }

        if let Some(existing) = suppressed.iter_mut().find(|item| item.reason == reason) {
            existing.count += count;
            return;
        }

        suppressed.push(StackLogSuppression {
            reason: reason.to_string(),
            count,
        });
    }

    fn is_stack_log_health_probe_chatter(message: &str) -> bool {
        let normalized = message.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return false;
        }

        let mentions_endpoint = [
            "/health",
            "/healthz",
            "/ready",
            "/readyz",
            "/readiness",
            "/live",
            "/livez",
        ]
        .iter()
        .any(|endpoint| normalized.contains(endpoint));
        if !mentions_endpoint {
            return false;
        }

        ["get ", "head ", "healthcheck", "kube-probe", "probe"]
            .iter()
            .any(|needle| normalized.contains(needle))
    }

    fn is_stack_log_exact_duplicate_burst(
        previous: &StackLogEntry,
        current: &StackLogEntry,
    ) -> bool {
        previous.service == current.service
            && previous.container_name == current.container_name
            && previous.stream == current.stream
            && previous.message == current.message
            && current.ts >= previous.ts
            && (current.ts - previous.ts).num_seconds() <= 1
    }

    fn apply_stack_log_noise_filter(
        entries: Vec<StackLogEntry>,
        include_noise: bool,
    ) -> (Vec<StackLogEntry>, Vec<StackLogSuppression>) {
        if include_noise {
            return (entries, Vec::new());
        }

        let mut filtered = Vec::with_capacity(entries.len());
        let mut suppressed = Vec::new();
        let mut last_kept: Option<&StackLogEntry> = None;

        for entry in entries {
            if entry.message.trim().is_empty() {
                Self::record_stack_log_suppression(&mut suppressed, "empty_line", 1);
                continue;
            }

            if Self::is_stack_log_health_probe_chatter(&entry.message) {
                Self::record_stack_log_suppression(&mut suppressed, "health_probe_chatter", 1);
                continue;
            }

            if let Some(previous) = last_kept {
                if Self::is_stack_log_exact_duplicate_burst(previous, &entry) {
                    Self::record_stack_log_suppression(&mut suppressed, "exact_duplicate_burst", 1);
                    continue;
                }
            }

            filtered.push(entry);
            last_kept = filtered.last();
        }

        (filtered, suppressed)
    }

    fn stack_log_cursor_from_entry(entry: &StackLogEntry) -> StackLogCursor {
        StackLogCursor {
            ts: entry.ts,
            service: entry.service.clone(),
            stream: entry.stream.clone(),
            ordinal: entry.ordinal,
        }
    }

    fn stack_log_entry_is_after_cursor(entry: &StackLogEntry, cursor: &StackLogCursor) -> bool {
        entry.ts > cursor.ts
            || (entry.ts == cursor.ts
                && (entry.service > cursor.service
                    || (entry.service == cursor.service
                        && (entry.stream > cursor.stream
                            || (entry.stream == cursor.stream && entry.ordinal > cursor.ordinal)))))
    }

    fn apply_stack_log_cursor(
        entries: Vec<StackLogEntry>,
        cursor: Option<&StackLogCursor>,
    ) -> Vec<StackLogEntry> {
        let Some(cursor) = cursor else {
            return entries;
        };

        entries
            .into_iter()
            .filter(|entry| Self::stack_log_entry_is_after_cursor(entry, cursor))
            .collect()
    }

    fn paginate_stack_log_entries(
        mut entries: Vec<StackLogEntry>,
        max_entries: usize,
    ) -> (Vec<StackLogEntry>, bool, Option<StackLogCursor>) {
        let truncated = entries.len() > max_entries;
        if !truncated {
            return (entries, false, None);
        }

        entries.truncate(max_entries);
        let next_cursor = entries.last().map(Self::stack_log_cursor_from_entry);
        (entries, true, next_cursor)
    }

    fn stack_log_source_from_summary(
        summary: &ContainerSummary,
        requested_services: &BTreeSet<String>,
        include_stopped: bool,
    ) -> Option<StackLogSource> {
        let labels = summary.labels.as_ref()?;
        let service = Self::normalize_optional_string(
            labels.get(DOCKER_COMPOSE_SERVICE_LABEL).map(String::as_str),
        )?;

        if !requested_services.is_empty() && !requested_services.contains(&service) {
            return None;
        }

        if !include_stopped && !Self::container_summary_is_running(summary) {
            return None;
        }

        let container_id = summary.id.clone()?;
        Some(StackLogSource {
            service,
            container_name: Self::normalize_container_name(summary.names.as_ref(), &container_id),
            container_id,
            state: Self::container_summary_state(summary),
            started_at: None,
        })
    }

    async fn enrich_stack_log_source_from_inspect(docker: &Docker, source: &mut StackLogSource) {
        match docker
            .inspect_container(&source.container_id, None::<InspectContainerOptions>)
            .await
        {
            Ok(inspect) => {
                if let Some(container_name) = inspect
                    .name
                    .as_deref()
                    .map(|name| name.trim_start_matches('/'))
                    .and_then(Self::normalize_non_empty)
                {
                    source.container_name = container_name;
                }

                if let Some(state) = inspect
                    .state
                    .as_ref()
                    .and_then(|state| state.status.map(|status| status.to_string()))
                    .filter(|state| !state.is_empty())
                {
                    source.state = state;
                }

                source.started_at = inspect.state.as_ref().and_then(|state| {
                    Self::parse_stack_log_started_at(state.started_at.as_deref())
                });
            }
            Err(error) => {
                warn!(
                    container_id = %source.container_id,
                    error = %error,
                    "Failed to inspect compose container during stack log source discovery; returning summary metadata only"
                );
            }
        }
    }

    async fn discover_stack_log_sources(
        docker: &Docker,
        selector: &StackLogsSelector,
        services: &[String],
        include_stopped: bool,
    ) -> Result<(ResolvedStackLogsSelector, Vec<StackLogSource>)> {
        let env_compose_project = get_stack_logs_project();
        let runtime_compose_project =
            if selector.compose_project.is_some() || env_compose_project.is_some() {
                None
            } else {
                Some(Self::detect_runtime_compose_project(docker).await?)
            };
        let resolved_selector = Self::resolve_stack_logs_selector(
            selector,
            env_compose_project,
            runtime_compose_project,
        )?;

        let filters = HashMap::from([(
            "label".to_string(),
            vec![format!(
                "{DOCKER_COMPOSE_PROJECT_LABEL}={}",
                resolved_selector.compose_project
            )],
        )]);
        let containers = docker
            .list_containers(Some(bollard::query_parameters::ListContainersOptions {
                all: true,
                filters: Some(filters),
                ..Default::default()
            }))
            .await
            .context("Failed to list compose stack containers for stack log discovery")?;

        let requested_services = Self::normalize_requested_stack_log_services(services);
        let mut sources = Vec::new();
        for summary in &containers {
            let Some(mut source) =
                Self::stack_log_source_from_summary(summary, &requested_services, include_stopped)
            else {
                continue;
            };

            Self::enrich_stack_log_source_from_inspect(docker, &mut source).await;
            sources.push(source);
        }

        sources.sort_by(|left, right| {
            left.service
                .cmp(&right.service)
                .then(left.container_name.cmp(&right.container_name))
                .then(left.container_id.cmp(&right.container_id))
        });

        Ok((resolved_selector, sources))
    }

    async fn collect_stack_log_entries_for_source(
        docker: &Docker,
        source: &StackLogSource,
        request: &StackLogsFetchRequest,
        max_entries: usize,
    ) -> Result<(Vec<StackLogEntry>, u64)> {
        let tail = max_entries.to_string();
        let options = LogsOptionsBuilder::new()
            .follow(false)
            .stdout(true)
            .stderr(request.include_stderr)
            .since(Self::stack_logs_query_timestamp(request.since))
            .until(Self::stack_logs_query_timestamp(request.until))
            .timestamps(true)
            .tail(&tail)
            .build();

        let mut output = docker.logs(&source.container_id, Some(options));
        let mut entries = Vec::new();
        let mut stdout_state = StackLogBufferState::default();
        let mut stderr_state = StackLogBufferState::default();
        let mut unparsable_lines = 0_u64;

        while let Some(message) = output.next().await {
            match message.context("Failed to stream Docker container logs")? {
                LogOutput::StdOut { message } => Self::ingest_stack_log_chunk(
                    source,
                    STACK_LOG_STREAM_STDOUT,
                    &message,
                    &mut stdout_state,
                    &mut entries,
                    &mut unparsable_lines,
                ),
                LogOutput::StdErr { message } => Self::ingest_stack_log_chunk(
                    source,
                    STACK_LOG_STREAM_STDERR,
                    &message,
                    &mut stderr_state,
                    &mut entries,
                    &mut unparsable_lines,
                ),
                _ => {}
            }
        }

        Self::finalize_stack_log_buffer(
            source,
            STACK_LOG_STREAM_STDOUT,
            &mut stdout_state,
            &mut entries,
            &mut unparsable_lines,
        );
        Self::finalize_stack_log_buffer(
            source,
            STACK_LOG_STREAM_STDERR,
            &mut stderr_state,
            &mut entries,
            &mut unparsable_lines,
        );

        Ok((entries, unparsable_lines))
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

    /// List compose-stack containers that can be used as stack log sources.
    pub(crate) async fn list_stack_log_sources(
        request: StackLogsListSourcesRequest,
    ) -> Result<StackLogsListSourcesResponse> {
        let docker = Self::connect_and_ping().await?;
        let (resolved_selector, sources) = Self::discover_stack_log_sources(
            &docker,
            &request.selector,
            &request.services,
            request.include_stopped,
        )
        .await?;

        Ok(StackLogsListSourcesResponse {
            stack_selector: resolved_selector,
            containers: sources,
        })
    }

    /// Fetch raw compose-stack log entries for the selected services and time window.
    pub(crate) async fn fetch_stack_logs(
        request: StackLogsFetchRequest,
    ) -> Result<StackLogsFetchResponse> {
        Self::validate_stack_logs_window(request.since, request.until)?;

        let docker = Self::connect_and_ping().await?;
        let mut warnings = Vec::new();
        let max_entries = Self::stack_logs_max_entries(request.max_entries, &mut warnings);

        let (_resolved_selector, sources) =
            Self::discover_stack_log_sources(&docker, &request.selector, &request.services, true)
                .await?;

        let mut entries = Vec::new();
        let mut source_failures = Vec::new();
        for source in &sources {
            match Self::collect_stack_log_entries_for_source(&docker, source, &request, max_entries)
                .await
            {
                Ok((mut source_entries, unparsable_lines)) => {
                    if unparsable_lines > 0 {
                        warnings.push(format!(
                            "Skipped {unparsable_lines} unparsable timestamped log lines from {} ({})",
                            source.service, source.container_name
                        ));
                    }
                    entries.append(&mut source_entries);
                }
                Err(error) => {
                    let message = format!(
                        "Failed to fetch logs for {} ({}): {error}",
                        source.service, source.container_name
                    );
                    warn!(container_id = %source.container_id, error = %error, "{message}");
                    source_failures.push(message);
                }
            }
        }

        if !sources.is_empty() && entries.is_empty() && !source_failures.is_empty() {
            return Err(anyhow!(source_failures.join("; ")));
        }
        warnings.extend(source_failures);

        entries.sort_by(|left, right| {
            left.ts
                .cmp(&right.ts)
                .then(left.service.cmp(&right.service))
                .then(left.stream.cmp(&right.stream))
                .then(left.container_name.cmp(&right.container_name))
                .then(left.ordinal.cmp(&right.ordinal))
        });
        let (mut entries, suppressed) =
            Self::apply_stack_log_noise_filter(entries, request.include_noise);
        Self::assign_stack_log_ordinals(&mut entries);
        let entries = Self::apply_stack_log_cursor(entries, request.cursor.as_ref());
        let (entries, truncated, next_cursor) =
            Self::paginate_stack_log_entries(entries, max_entries);

        Ok(StackLogsFetchResponse {
            window: StackLogsWindow {
                since: request.since,
                until: request.until,
            },
            entries,
            suppressed,
            truncated,
            next_cursor,
            warnings,
        })
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
                        "Sandbox image '{}' not found. Build it with `docker compose build sandbox_image` or start the full stack with `docker compose up --build -d`",
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
        self.upload_file(path, content).await
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
    use crate::sandbox::broker::{
        StackLogCursor, StackLogEntry, StackLogSuppression, StackLogsSelector,
    };
    use chrono::{TimeZone, Utc};

    fn test_stack_log_entry(
        ts: DateTime<Utc>,
        service: &str,
        container_name: &str,
        stream: &str,
        ordinal: u64,
        message: &str,
    ) -> StackLogEntry {
        StackLogEntry {
            ts,
            service: service.to_string(),
            container_name: container_name.to_string(),
            stream: stream.to_string(),
            ordinal,
            message: message.to_string(),
        }
    }

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

    #[tokio::test]
    #[ignore = "Requires Docker daemon"]
    async fn test_write_file_round_trips_large_binary_content(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut sandbox = DockerSandboxManager::new(12349).await?;
        sandbox.create_sandbox().await?;

        let payload = (0..200_000)
            .map(|value| (value % 251) as u8)
            .collect::<Vec<_>>();
        sandbox
            .write_file("/workspace/large-binary-roundtrip.bin", &payload)
            .await?;

        let content = sandbox
            .read_file("/workspace/large-binary-roundtrip.bin")
            .await?;
        assert_eq!(content, payload);

        sandbox.destroy().await?;
        Ok(())
    }

    #[test]
    fn resolve_stack_logs_selector_prefers_request_over_env_and_runtime() {
        let resolved = DockerSandboxManager::resolve_stack_logs_selector(
            &StackLogsSelector {
                compose_project: Some("request-project".to_string()),
            },
            Some("env-project".to_string()),
            Some("runtime-project".to_string()),
        )
        .expect("resolve selector");

        assert_eq!(resolved.compose_project, "request-project");
    }

    #[test]
    fn resolve_stack_logs_selector_uses_env_override_before_runtime() {
        let resolved = DockerSandboxManager::resolve_stack_logs_selector(
            &StackLogsSelector::default(),
            Some("env-project".to_string()),
            Some("runtime-project".to_string()),
        )
        .expect("resolve selector");

        assert_eq!(resolved.compose_project, "env-project");
    }

    #[test]
    fn resolve_stack_logs_selector_errors_when_project_is_unavailable() {
        let error = DockerSandboxManager::resolve_stack_logs_selector(
            &StackLogsSelector::default(),
            None,
            None,
        )
        .expect_err("selector should fail without compose project");

        assert!(error
            .to_string()
            .contains("Unable to resolve compose project for stack log discovery"));
    }

    #[test]
    fn stack_log_source_from_summary_filters_by_service_and_running_state() {
        let summary = ContainerSummary {
            id: Some("abc123def456".to_string()),
            names: Some(vec!["/oxide_agent".to_string()]),
            labels: Some(HashMap::from([
                (
                    DOCKER_COMPOSE_PROJECT_LABEL.to_string(),
                    "oxide-agent".to_string(),
                ),
                (
                    DOCKER_COMPOSE_SERVICE_LABEL.to_string(),
                    "oxide_agent".to_string(),
                ),
            ])),
            state: Some(ContainerSummaryStateEnum::RUNNING),
            status: Some("Up 2 minutes".to_string()),
            ..Default::default()
        };

        let filtered = DockerSandboxManager::stack_log_source_from_summary(
            &summary,
            &BTreeSet::from(["browser_use".to_string()]),
            false,
        );
        assert!(
            filtered.is_none(),
            "non-matching services should be skipped"
        );

        let source = DockerSandboxManager::stack_log_source_from_summary(
            &summary,
            &BTreeSet::from(["oxide_agent".to_string()]),
            false,
        )
        .expect("matching running service should be included");

        assert_eq!(source.service, "oxide_agent");
        assert_eq!(source.container_name, "oxide_agent");
        assert_eq!(source.container_id, "abc123def456");
        assert_eq!(source.state, "running");
        assert_eq!(source.started_at, None);
    }

    #[test]
    fn stack_log_source_from_summary_excludes_stopped_by_default_and_parses_started_at() {
        let stopped = ContainerSummary {
            id: Some("abc123def456".to_string()),
            names: Some(vec!["/oxide_agent".to_string()]),
            labels: Some(HashMap::from([(
                DOCKER_COMPOSE_SERVICE_LABEL.to_string(),
                "oxide_agent".to_string(),
            )])),
            state: Some(ContainerSummaryStateEnum::EXITED),
            status: Some("Exited (0) 5 seconds ago".to_string()),
            ..Default::default()
        };

        assert!(DockerSandboxManager::stack_log_source_from_summary(
            &stopped,
            &BTreeSet::new(),
            false,
        )
        .is_none());

        let included =
            DockerSandboxManager::stack_log_source_from_summary(&stopped, &BTreeSet::new(), true)
                .expect("include_stopped should keep exited containers");
        assert_eq!(included.state, "exited");

        let started_at = DockerSandboxManager::parse_stack_log_started_at(Some(
            "2026-04-02T10:11:12.000000000Z",
        ))
        .expect("parse started_at");
        assert_eq!(
            started_at,
            Utc.with_ymd_and_hms(2026, 4, 2, 10, 11, 12).unwrap()
        );
    }

    #[test]
    fn stack_logs_max_entries_clamps_to_hard_limit() {
        let mut warnings = Vec::new();

        let effective = DockerSandboxManager::stack_logs_max_entries(999, &mut warnings);

        assert_eq!(effective, STACK_LOGS_HARD_MAX_ENTRIES);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("hard limit 500"));
    }

    #[test]
    fn stack_logs_max_entries_rejects_zero() {
        let mut warnings = Vec::new();

        let effective = DockerSandboxManager::stack_logs_max_entries(0, &mut warnings);

        assert_eq!(effective, 1);
        assert_eq!(
            warnings,
            vec!["Requested max_entries=0 is invalid; using 1"]
        );
    }

    #[test]
    fn validate_stack_logs_window_rejects_inverted_range() {
        let since = Utc.with_ymd_and_hms(2026, 4, 2, 10, 11, 13).unwrap();
        let until = Utc.with_ymd_and_hms(2026, 4, 2, 10, 11, 12).unwrap();

        let error = DockerSandboxManager::validate_stack_logs_window(Some(since), Some(until))
            .expect_err("inverted range should fail");

        assert!(error.to_string().contains("Invalid stack log time window"));
    }

    #[test]
    fn parse_timestamped_stack_log_line_extracts_timestamp_and_message() {
        let (ts, message) = DockerSandboxManager::parse_timestamped_stack_log_line(
            "2026-04-02T10:11:12.000000000Z provider failover activated\r",
        )
        .expect("parse timestamped log line");

        assert_eq!(ts, Utc.with_ymd_and_hms(2026, 4, 2, 10, 11, 12).unwrap());
        assert_eq!(message, "provider failover activated");
    }

    #[test]
    fn ingest_stack_log_chunk_buffers_partial_lines_until_newline() {
        let source = StackLogSource {
            service: "oxide_agent".to_string(),
            container_name: "oxide_agent".to_string(),
            container_id: "abc123def456".to_string(),
            state: "running".to_string(),
            started_at: None,
        };
        let mut state = StackLogBufferState::default();
        let mut entries = Vec::new();
        let mut unparsable_lines = 0_u64;

        DockerSandboxManager::ingest_stack_log_chunk(
            &source,
            STACK_LOG_STREAM_STDOUT,
            b"2026-04-02T10:11:12.000000000Z part",
            &mut state,
            &mut entries,
            &mut unparsable_lines,
        );
        assert!(entries.is_empty());
        assert_eq!(state.buffer, "2026-04-02T10:11:12.000000000Z part");

        DockerSandboxManager::ingest_stack_log_chunk(
            &source,
            STACK_LOG_STREAM_STDOUT,
            b"ial\n2026-04-02T10:11:13.000000000Z done\n",
            &mut state,
            &mut entries,
            &mut unparsable_lines,
        );

        assert!(state.buffer.is_empty());
        assert_eq!(unparsable_lines, 0);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].message, "partial");
        assert_eq!(entries[0].ordinal, 0);
        assert_eq!(entries[1].message, "done");
        assert_eq!(entries[1].ordinal, 1);
    }

    #[test]
    fn assign_stack_log_ordinals_counts_per_service_and_stream() {
        let ts = Utc.with_ymd_and_hms(2026, 4, 2, 10, 11, 12).unwrap();
        let mut entries = vec![
            StackLogEntry {
                ts,
                service: "oxide_agent".to_string(),
                container_name: "oxide_agent".to_string(),
                stream: STACK_LOG_STREAM_STDOUT.to_string(),
                ordinal: 11,
                message: "first".to_string(),
            },
            StackLogEntry {
                ts,
                service: "oxide_agent".to_string(),
                container_name: "oxide_agent-2".to_string(),
                stream: STACK_LOG_STREAM_STDOUT.to_string(),
                ordinal: 7,
                message: "second".to_string(),
            },
            StackLogEntry {
                ts,
                service: "oxide_agent".to_string(),
                container_name: "oxide_agent".to_string(),
                stream: STACK_LOG_STREAM_STDERR.to_string(),
                ordinal: 4,
                message: "stderr".to_string(),
            },
        ];

        DockerSandboxManager::assign_stack_log_ordinals(&mut entries);

        assert_eq!(entries[0].ordinal, 0);
        assert_eq!(entries[1].ordinal, 1);
        assert_eq!(entries[2].ordinal, 0);
    }

    #[test]
    fn apply_stack_log_noise_filter_suppresses_expected_noise_classes() {
        let ts = Utc.with_ymd_and_hms(2026, 4, 2, 10, 11, 12).unwrap();
        let entries = vec![
            test_stack_log_entry(ts, "oxide_agent", "oxide_agent", "stdout", 0, "useful"),
            test_stack_log_entry(ts, "oxide_agent", "oxide_agent", "stdout", 1, ""),
            test_stack_log_entry(
                ts,
                "oxide_agent",
                "oxide_agent",
                "stdout",
                2,
                "GET /health HTTP/1.1 200 OK",
            ),
            test_stack_log_entry(ts, "oxide_agent", "oxide_agent", "stdout", 3, "duplicate"),
            test_stack_log_entry(ts, "oxide_agent", "oxide_agent", "stdout", 4, "duplicate"),
        ];

        let (filtered, suppressed) =
            DockerSandboxManager::apply_stack_log_noise_filter(entries, false);

        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].message, "useful");
        assert_eq!(filtered[1].message, "duplicate");
        assert_eq!(
            suppressed,
            vec![
                StackLogSuppression {
                    reason: "empty_line".to_string(),
                    count: 1,
                },
                StackLogSuppression {
                    reason: "health_probe_chatter".to_string(),
                    count: 1,
                },
                StackLogSuppression {
                    reason: "exact_duplicate_burst".to_string(),
                    count: 1,
                },
            ]
        );
    }

    #[test]
    fn apply_stack_log_cursor_returns_entries_after_cursor() {
        let ts = Utc.with_ymd_and_hms(2026, 4, 2, 10, 11, 12).unwrap();
        let entries = vec![
            test_stack_log_entry(ts, "oxide_agent", "oxide_agent", "stderr", 0, "stderr"),
            test_stack_log_entry(ts, "oxide_agent", "oxide_agent", "stdout", 0, "first"),
            test_stack_log_entry(ts, "oxide_agent", "oxide_agent", "stdout", 1, "second"),
        ];

        let filtered = DockerSandboxManager::apply_stack_log_cursor(
            entries,
            Some(&StackLogCursor {
                ts,
                service: "oxide_agent".to_string(),
                stream: "stdout".to_string(),
                ordinal: 0,
            }),
        );

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].message, "second");
    }

    #[test]
    fn paginate_stack_log_entries_sets_next_cursor_from_last_returned_entry() {
        let ts = Utc.with_ymd_and_hms(2026, 4, 2, 10, 11, 12).unwrap();
        let entries = vec![
            test_stack_log_entry(ts, "oxide_agent", "oxide_agent", "stdout", 0, "first"),
            test_stack_log_entry(ts, "oxide_agent", "oxide_agent", "stdout", 1, "second"),
            test_stack_log_entry(ts, "oxide_agent", "oxide_agent", "stdout", 2, "third"),
        ];

        let (page, truncated, next_cursor) =
            DockerSandboxManager::paginate_stack_log_entries(entries, 2);

        assert!(truncated);
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].message, "first");
        assert_eq!(page[1].message, "second");
        assert_eq!(
            next_cursor,
            Some(StackLogCursor {
                ts,
                service: "oxide_agent".to_string(),
                stream: "stdout".to_string(),
                ordinal: 1,
            })
        );
    }
}
