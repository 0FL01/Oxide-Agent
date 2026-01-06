//! Docker sandbox manager using Bollard
//!
//! Manages Docker containers for isolated code execution.

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use bollard::exec::{CreateExecOptions, StartExecResults};
use bollard::models::{ContainerCreateBody, HostConfig};
use bollard::query_parameters::{
    CreateContainerOptions, DownloadFromContainerOptions, RemoveContainerOptions,
    StartContainerOptions, UploadToContainerOptions,
};
use bollard::Docker;
use bytes::Bytes;
use futures_util::{StreamExt, TryStreamExt};
use http_body_util::{Either, Full};
use std::collections::HashMap;
use std::io::Read;
use tracing::{debug, info, instrument, warn};

use crate::config::{
    SANDBOX_CPU_PERIOD, SANDBOX_CPU_QUOTA, SANDBOX_EXEC_TIMEOUT_SECS, SANDBOX_IMAGE,
    SANDBOX_MEMORY_LIMIT,
};

/// Result of executing a command in the sandbox
#[derive(Debug, Clone)]
pub struct ExecResult {
    /// Standard output of the command
    pub stdout: String,
    /// Standard error of the command
    pub stderr: String,
    /// Exit code of the command
    pub exit_code: i64,
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

/// Docker sandbox manager for isolated code execution
#[derive(Clone)]
pub struct SandboxManager {
    docker: Docker,
    container_id: Option<String>,
    image_name: String,
    user_id: i64,
}

impl SandboxManager {
    /// Create a new sandbox manager
    ///
    /// # Errors
    ///
    /// Returns an error if connection to Docker daemon fails or ping fails.
    #[instrument(skip_all, fields(user_id))]
    pub async fn new(user_id: i64) -> Result<Self> {
        let docker =
            Docker::connect_with_local_defaults().context("Failed to connect to Docker daemon")?;

        // Verify Docker connection
        docker
            .ping()
            .await
            .context("Failed to ping Docker daemon")?;

        debug!(user_id, "Docker connection established");

        Ok(Self {
            docker,
            container_id: None,
            image_name: SANDBOX_IMAGE.to_string(),
            user_id,
        })
    }

    /// Check if sandbox container is running
    #[must_use]
    pub const fn is_running(&self) -> bool {
        self.container_id.is_some()
    }

    /// Get container ID if running
    #[must_use]
    pub fn container_id(&self) -> Option<&str> {
        self.container_id.as_deref()
    }

    /// Create and start a new sandbox container
    ///
    /// # Errors
    ///
    /// Returns an error if container creation or starting fails.
    #[instrument(skip(self), fields(user_id = self.user_id))]
    pub async fn create_sandbox(&mut self) -> Result<()> {
        if self.container_id.is_some() {
            // Already tracked in this object
            return Ok(());
        }

        let container_name = format!("agent-sandbox-{}", self.user_id);

        // Check if container already exists
        let mut filters = HashMap::new();
        filters.insert("name".to_string(), vec![container_name.clone()]);

        let containers = self
            .docker
            .list_containers(Some(bollard::query_parameters::ListContainersOptions {
                all: true,
                filters: Some(filters),
                ..Default::default()
            }))
            .await
            .context("Failed to list containers")?;

        if let Some(container) = containers.first() {
            let id = container.id.clone().unwrap_or_default();
            info!(user_id = self.user_id, container_id = %id, "Found existing sandbox container");
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
            labels: Some(HashMap::from([
                ("agent.user_id".to_string(), self.user_id.to_string()),
                ("agent.sandbox".to_string(), "true".to_string()),
            ])),
            // Keep container running
            cmd: Some(vec!["sleep".to_string(), "infinity".to_string()]),
            ..Default::default()
        };

        let options = CreateContainerOptions {
            name: Some(container_name.clone()),
            ..Default::default()
        };

        // Create container
        let response = self
            .docker
            .create_container(Some(options), config)
            .await
            .context("Failed to create sandbox container")?;

        let container_id = response.id;
        info!(container_id = %container_id, "Sandbox container created");

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
        &self,
        cmd: &str,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<ExecResult> {
        let container_id = self
            .container_id
            .as_ref()
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
            .create_exec(container_id, exec_options)
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

                    return Err(anyhow!("Выполнение команды прервано пользователем"));
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
    pub async fn write_file(&self, path: &str, content: &[u8]) -> Result<()> {
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

    /// Read content from a file in the sandbox
    ///
    /// # Errors
    ///
    /// Returns an error if file reading or decoding fails.
    #[instrument(skip(self), fields(path = %path))]
    pub async fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        let cmd = format!("base64 {}", shell_escape::escape(path.into()));

        let result = self.exec_command(&cmd, None).await?;

        if !result.success() {
            return Err(anyhow!("Failed to read file: {}", result.stderr));
        }

        let content = base64::engine::general_purpose::STANDARD
            .decode(result.stdout.trim())
            .context("Failed to decode file content")?;

        debug!(path = %path, size = content.len(), "File read from sandbox");
        Ok(content)
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
    pub async fn upload_file(&self, container_path: &str, content: &[u8]) -> Result<()> {
        let container_id = self
            .container_id
            .as_ref()
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
                container_id,
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
    pub async fn download_file(&self, container_path: &str) -> Result<Vec<u8>> {
        let container_id = self
            .container_id
            .as_ref()
            .ok_or_else(|| anyhow!("Sandbox not running"))?;

        // Check if file exists first
        let check = self
            .exec_command(
                &format!("test -f '{container_path}' && echo 'exists'"),
                None,
            )
            .await?;
        if !check.stdout.contains("exists") {
            anyhow::bail!("File not found: {container_path}");
        }

        // Get file size to check limits (50MB max for Telegram)
        let size_check = self
            .exec_command(&format!("stat -c %s '{container_path}'"), None)
            .await?;
        let file_size: u64 = size_check.stdout.trim().parse().unwrap_or(0);

        const MAX_FILE_SIZE: u64 = 50 * 1024 * 1024; // 50 MB
        if file_size > MAX_FILE_SIZE {
            anyhow::bail!(
                "File too large: {} bytes (max {} MB)",
                file_size,
                MAX_FILE_SIZE / 1024 / 1024
            );
        }

        // Download file as tar archive
        let stream = self
            .docker
            .download_from_container(
                container_id,
                Some(DownloadFromContainerOptions {
                    path: container_path.to_string(),
                }),
            )
            .try_collect::<Vec<_>>()
            .await
            .context("Failed to download file from container")?;

        // Combine chunks into single buffer
        let tar_data: Vec<u8> = stream.into_iter().flatten().collect();

        // Extract file from tar
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
    pub async fn get_uploads_size(&self) -> Result<u64> {
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
    /// Files are considered orphaned if delivery to Telegram failed or was interrupted.
    ///
    /// # Errors
    ///
    /// Returns an error if the cleanup command fails.
    #[instrument(skip(self))]
    pub async fn cleanup_old_downloads(&self) -> Result<u64> {
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
        if let Some(container_id) = self.container_id.take() {
            info!(container_id = %container_id, "Destroying sandbox container");

            let options = RemoveContainerOptions {
                force: true,
                ..Default::default()
            };

            if let Err(e) = self
                .docker
                .remove_container(&container_id, Some(options))
                .await
            {
                // Container might already be removed (auto_remove)
                warn!(container_id = %container_id, error = %e, "Failed to remove container (may already be removed)");
            } else {
                info!(container_id = %container_id, "Sandbox container destroyed");
            }
        }

        Ok(())
    }

    /// Recreate the sandbox container (wipe data)
    ///
    /// # Errors
    ///
    /// Returns an error if destruction or creation fails.
    #[instrument(skip(self), fields(user_id = self.user_id))]
    pub async fn recreate(&mut self) -> Result<()> {
        info!("Recreating sandbox");

        // Force destroy current container
        if self.container_id.is_some() {
            self.destroy().await?;
        } else {
            // Even if not in memory, check docker for the named container
            let container_name = format!("agent-sandbox-{}", self.user_id);
            // Best effort cleanup by name if we lost the ID
            let _ = self
                .docker
                .remove_container(
                    &container_name,
                    Some(RemoveContainerOptions {
                        force: true,
                        ..Default::default()
                    }),
                )
                .await;
        }

        // Create new one
        self.create_sandbox().await
    }
}

impl Drop for SandboxManager {
    fn drop(&mut self) {
        if let Some(ref id) = self.container_id {
            info!(
                container_id = %id,
                "SandboxManager dropped. Container persists in Docker (intentional)."
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
        let mut sandbox = SandboxManager::new(12345).await?;

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
}
