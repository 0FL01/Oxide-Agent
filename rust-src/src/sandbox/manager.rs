//! Docker sandbox manager using Bollard
//!
//! Manages Docker containers for isolated code execution.

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use bollard::exec::{CreateExecOptions, StartExecResults};
use bollard::models::{ContainerCreateBody, HostConfig};
use bollard::query_parameters::{
    CreateContainerOptions, RemoveContainerOptions, StartContainerOptions,
};
use bollard::Docker;
use futures_util::StreamExt;
use std::collections::HashMap;
use tracing::{debug, error, info, instrument, warn};

use crate::config::{
    SANDBOX_CPU_PERIOD, SANDBOX_CPU_QUOTA, SANDBOX_EXEC_TIMEOUT_SECS, SANDBOX_IMAGE,
    SANDBOX_MEMORY_LIMIT,
};

/// Result of executing a command in the sandbox
#[derive(Debug, Clone)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i64,
}

impl ExecResult {
    /// Check if the command succeeded (exit code 0)
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }

    /// Get combined output (stdout + stderr)
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
pub struct SandboxManager {
    docker: Docker,
    container_id: Option<String>,
    image_name: String,
    user_id: i64,
}

impl SandboxManager {
    /// Create a new sandbox manager
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
    pub fn is_running(&self) -> bool {
        self.container_id.is_some()
    }

    /// Get container ID if running
    pub fn container_id(&self) -> Option<&str> {
        self.container_id.as_deref()
    }

    /// Create and start a new sandbox container
    #[instrument(skip(self), fields(user_id = self.user_id))]
    pub async fn create_sandbox(&mut self) -> Result<()> {
        if self.container_id.is_some() {
            // Already tracked in this object
            return Ok(());
        }

        let container_name = format!("agent-sandbox-{}", self.user_id);

        // Check if container already exists
        let filter = HashMap::from([("name", vec![container_name.as_str()])]);
        let containers = self
            .docker
            .list_containers(Some(bollard::container::ListContainersOptions {
                all: true,
                filters: filter,
                ..Default::default()
            }))
            .await
            .context("Failed to list containers")?;

        if let Some(container) = containers.first() {
            let id = container.id.clone().unwrap_or_default();
            info!(user_id = self.user_id, container_id = %id, "Found existing sandbox container");
            self.container_id = Some(id.clone());

            // Start if not running
            // We use format! debug since StateEnum might not implement Display/AsRef<str> easily or we don't want to import the enum
            let _state_str = format!("{:?}", container.state);

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

    /// Execute a command in the sandbox
    #[instrument(skip(self), fields(container_id = ?self.container_id))]
    pub async fn exec_command(&self, cmd: &str) -> Result<ExecResult> {
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

        // Start exec with timeout
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(SANDBOX_EXEC_TIMEOUT_SECS),
            self.run_exec(&exec.id),
        )
        .await
        .map_err(|_| {
            anyhow!(
                "Command execution timed out after {}s",
                SANDBOX_EXEC_TIMEOUT_SECS
            )
        })?
        .context("Command execution failed")?;

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

        let result = self.exec_command(&cmd).await?;

        if !result.success() {
            return Err(anyhow!("Failed to write file: {}", result.stderr));
        }

        debug!(path = %path, "File written to sandbox");
        Ok(())
    }

    /// Read content from a file in the sandbox
    #[instrument(skip(self), fields(path = %path))]
    pub async fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        let cmd = format!("base64 {}", shell_escape::escape(path.into()));

        let result = self.exec_command(&cmd).await?;

        if !result.success() {
            return Err(anyhow!("Failed to read file: {}", result.stderr));
        }

        let content = base64::engine::general_purpose::STANDARD
            .decode(result.stdout.trim())
            .context("Failed to decode file content")?;

        debug!(path = %path, size = content.len(), "File read from sandbox");
        Ok(content)
    }

    /// Destroy the sandbox container
    #[instrument(skip(self), fields(container_id = ?self.container_id))]
    pub async fn destroy(&mut self) -> Result<()> {
        if let Some(container_id) = self.container_id.take() {
            info!(container_id = %container_id, "Destroying sandbox container");

            let options = RemoveContainerOptions {
                force: true,
                ..Default::default()
            };

            match self
                .docker
                .remove_container(&container_id, Some(options))
                .await
            {
                Ok(_) => {
                    info!(container_id = %container_id, "Sandbox container destroyed");
                }
                Err(e) => {
                    // Container might already be removed (auto_remove)
                    warn!(container_id = %container_id, error = %e, "Failed to remove container (may already be removed)");
                }
            }
        }

        Ok(())
    }

    /// Recreate the sandbox container (wipe data)
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
        if self.container_id.is_some() {
            error!(
                container_id = ?self.container_id,
                "SandboxManager dropped with active container! Call destroy() explicitly."
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
    async fn test_sandbox_lifecycle() {
        let mut sandbox = SandboxManager::new(12345).await.unwrap();

        // Create sandbox
        sandbox.create_sandbox().await.unwrap();
        assert!(sandbox.is_running());

        // Execute command
        let result = sandbox.exec_command("echo 'Hello, World!'").await.unwrap();
        assert!(result.success());
        assert!(result.stdout.contains("Hello, World!"));

        // Python test
        let result = sandbox
            .exec_command("python3 -c \"print(2 + 2)\"")
            .await
            .unwrap();
        assert!(result.success());
        assert!(result.stdout.contains("4"));

        // Cleanup
        sandbox.destroy().await.unwrap();
        assert!(!sandbox.is_running());
    }
}
