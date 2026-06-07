//! Minimal local Unix process manager for typed tool execution.

use super::config::ToolRuntimeConfig;
use super::invocation::ToolInvocation;
use super::normalizer::OutputNormalizer;
use super::output::{CleanupStatus, ToolOutput};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::time::{Duration, timeout};

/// Local shell command process manager.
#[derive(Debug, Clone)]
pub struct ProcessManager {
    config: ToolRuntimeConfig,
    normalizer: OutputNormalizer,
}

impl ProcessManager {
    /// Build a process manager from runtime config.
    #[must_use]
    pub fn new(config: ToolRuntimeConfig) -> Self {
        Self {
            normalizer: OutputNormalizer::new(config.clone()),
            config,
        }
    }

    /// Execute a shell command and normalize the terminal state into `ToolOutput`.
    #[must_use]
    pub async fn execute_shell(&self, invocation: &ToolInvocation, command: &str) -> ToolOutput {
        let mut child = match self.spawn_shell(command, invocation.working_directory.clone()) {
            Ok(child) => child,
            Err(error) => {
                return self
                    .normalizer
                    .failure(invocation, format!("process spawn failed: {error}"))
                    .with_cleanup_status(CleanupStatus::NotStarted);
            }
        };

        let pid = child.id();
        let stdout_task = child.stdout.take().map(|stdout| {
            tokio::spawn(read_capped(
                stdout,
                self.config
                    .output
                    .max_captured_stdout_bytes
                    .saturating_add(1),
            ))
        });
        let stderr_task = child.stderr.take().map(|stderr| {
            tokio::spawn(read_capped(
                stderr,
                self.config
                    .output
                    .max_captured_stderr_bytes
                    .saturating_add(1),
            ))
        });

        let terminal = tokio::select! {
            () = invocation.cancellation_token.cancelled() => {
                let cleanup = self.cleanup_child(&mut child, pid).await;
                ProcessTerminal::Cancelled(cleanup)
            }
            result = timeout(invocation.timeout.per_tool_hard_timeout, child.wait()) => {
                match result {
                    Ok(Ok(status)) => ProcessTerminal::Exited(status.code().unwrap_or(-1)),
                    Ok(Err(error)) => ProcessTerminal::SpawnFailure(error.to_string()),
                    Err(_) => {
                        invocation.cancellation_token.cancel();
                        let cleanup = self.cleanup_child(&mut child, pid).await;
                        ProcessTerminal::Timeout(cleanup)
                    }
                }
            }
        };

        let stdout = await_capture(stdout_task).await;
        let stderr = await_capture(stderr_task).await;
        self.output_for_terminal(invocation, terminal, &stdout, &stderr)
    }

    fn spawn_shell(&self, command: &str, cwd: Option<PathBuf>) -> std::io::Result<Child> {
        let mut process = Command::new("sh");
        process
            .arg("-c")
            .arg(command)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = cwd {
            process.current_dir(cwd);
        }
        #[cfg(unix)]
        {
            process.process_group(0);
        }
        process.spawn()
    }

    async fn cleanup_child(&self, child: &mut Child, pid: Option<u32>) -> CleanupStatus {
        match child.try_wait() {
            Ok(Some(_)) => return CleanupStatus::AlreadyExited,
            Ok(None) => {}
            Err(_) => return CleanupStatus::Failed,
        }

        if let Some(pid) = pid {
            let _sent = send_group_signal(pid, "TERM").await;
            if wait_for_child(child, self.config.timeout.terminate_grace_period).await {
                return CleanupStatus::TerminatedGracefully;
            }

            let killed_group = send_group_signal(pid, "KILL").await;
            if wait_for_child(child, self.config.timeout.kill_grace_period).await {
                return if killed_group {
                    CleanupStatus::KilledProcessGroup
                } else {
                    CleanupStatus::KilledProcess
                };
            }
        }

        if child.kill().await.is_ok()
            && wait_for_child(child, self.config.timeout.kill_grace_period).await
        {
            return CleanupStatus::KilledProcess;
        }

        CleanupStatus::Failed
    }

    fn output_for_terminal(
        &self,
        invocation: &ToolInvocation,
        terminal: ProcessTerminal,
        stdout: &str,
        stderr: &str,
    ) -> ToolOutput {
        let stdout_preview = self.normalizer.stdout_preview(stdout);
        let stderr_preview = self.normalizer.stderr_preview(stderr);
        match terminal {
            ProcessTerminal::Exited(code) if code == 0 => self
                .normalizer
                .success(invocation, stdout, stderr)
                .with_exit_code(code),
            ProcessTerminal::Exited(code) => self
                .normalizer
                .failure(invocation, format!("process exited with code {code}"))
                .with_streams(stdout_preview, stderr_preview)
                .with_exit_code(code),
            ProcessTerminal::Timeout(cleanup) => self
                .normalizer
                .timeout(invocation, cleanup)
                .with_streams(stdout_preview, stderr_preview),
            ProcessTerminal::Cancelled(cleanup) => self
                .normalizer
                .cancelled(invocation, super::output::CancellationReason::User, cleanup)
                .with_streams(stdout_preview, stderr_preview),
            ProcessTerminal::SpawnFailure(message) => self
                .normalizer
                .failure(invocation, message)
                .with_cleanup_status(CleanupStatus::NotStarted),
        }
    }
}

enum ProcessTerminal {
    Exited(i32),
    Timeout(CleanupStatus),
    Cancelled(CleanupStatus),
    SpawnFailure(String),
}

async fn read_capped<R>(mut reader: R, max_bytes: usize) -> String
where
    R: AsyncRead + Unpin,
{
    let mut captured = Vec::with_capacity(max_bytes.min(8192));
    let mut chunk = [0_u8; 8192];

    loop {
        let Ok(read) = reader.read(&mut chunk).await else {
            break;
        };
        if read == 0 {
            break;
        }

        let remaining = max_bytes.saturating_sub(captured.len());
        if remaining > 0 {
            captured.extend_from_slice(&chunk[..read.min(remaining)]);
        }
    }

    String::from_utf8_lossy(&captured).into_owned()
}

async fn await_capture(handle: Option<tokio::task::JoinHandle<String>>) -> String {
    match handle {
        Some(handle) => handle.await.unwrap_or_default(),
        None => String::new(),
    }
}

async fn wait_for_child(child: &mut Child, duration: Duration) -> bool {
    matches!(timeout(duration, child.wait()).await, Ok(Ok(_)))
}

async fn send_group_signal(pid: u32, signal: &str) -> bool {
    #[cfg(unix)]
    {
        let target = format!("-{pid}");
        Command::new("kill")
            .arg(format!("-{signal}"))
            .arg("--")
            .arg(target)
            .status()
            .await
            .map(|status| status.success())
            .unwrap_or(false)
    }

    #[cfg(not(unix))]
    {
        let _ = (pid, signal);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::identity::SessionId;
    use crate::agent::tool_runtime::config::ToolOutputBudget;
    use crate::agent::tool_runtime::invocation::{
        ModelMetadata, ProviderMetadata, ToolExecutionContext,
    };
    use crate::agent::tool_runtime::output::ToolOutputStatus;
    use crate::agent::tool_runtime::types::{ToolBatchId, ToolCallId, ToolName, TurnId};
    use crate::llm::InvocationId;
    use chrono::Utc;
    use serde_json::json;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn shell_success_returns_stdout_and_exit_zero() {
        let config = ToolRuntimeConfig::default();
        let manager = ProcessManager::new(config.clone());
        let invocation = test_invocation(&config);

        let output = manager.execute_shell(&invocation, "printf ok").await;

        assert_eq!(output.status, ToolOutputStatus::Success);
        assert_eq!(output.exit_code, Some(0));
        assert_eq!(output.stdout.text.as_deref(), Some("ok"));
    }

    #[tokio::test]
    async fn nonzero_exit_is_failure_with_exit_code() {
        let config = ToolRuntimeConfig::default();
        let manager = ProcessManager::new(config.clone());
        let invocation = test_invocation(&config);

        let output = manager.execute_shell(&invocation, "exit 7").await;

        assert_eq!(output.status, ToolOutputStatus::Failure);
        assert_eq!(output.exit_code, Some(7));
    }

    #[tokio::test]
    async fn timeout_kills_process_group() {
        let mut config = ToolRuntimeConfig::default();
        config.timeout.per_tool_hard_timeout = Duration::from_millis(40);
        config.timeout.terminate_grace_period = Duration::from_millis(20);
        config.timeout.kill_grace_period = Duration::from_millis(80);
        let manager = ProcessManager::new(config.clone());
        let invocation = test_invocation(&config);

        let output = manager.execute_shell(&invocation, "sleep 30").await;

        assert_eq!(output.status, ToolOutputStatus::Timeout);
        assert!(matches!(
            output.cleanup_status,
            CleanupStatus::TerminatedGracefully | CleanupStatus::KilledProcessGroup
        ));
    }

    #[tokio::test]
    async fn command_ignoring_sigterm_is_killed() {
        let mut config = ToolRuntimeConfig::default();
        config.timeout.per_tool_hard_timeout = Duration::from_millis(40);
        config.timeout.terminate_grace_period = Duration::from_millis(20);
        config.timeout.kill_grace_period = Duration::from_millis(120);
        let manager = ProcessManager::new(config.clone());
        let invocation = test_invocation(&config);

        let output = manager
            .execute_shell(&invocation, "trap '' TERM; while true; do sleep 1; done")
            .await;

        assert_eq!(output.status, ToolOutputStatus::Timeout);
        assert_eq!(output.cleanup_status, CleanupStatus::KilledProcessGroup);
    }

    #[tokio::test]
    async fn timeout_cleans_up_child_process() {
        let temp = tempdir().expect("temp dir");
        let pid_file = temp.path().join("child.pid");
        let mut config = ToolRuntimeConfig::default();
        config.timeout.per_tool_hard_timeout = Duration::from_millis(50);
        config.timeout.terminate_grace_period = Duration::from_millis(20);
        config.timeout.kill_grace_period = Duration::from_millis(120);
        let manager = ProcessManager::new(config.clone());
        let invocation = test_invocation(&config);
        let command = format!("sleep 30 & echo $! > {}; wait", pid_file.display());

        let output = manager.execute_shell(&invocation, &command).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let child_pid = fs::read_to_string(pid_file).expect("pid file");

        assert_eq!(output.status, ToolOutputStatus::Timeout);
        assert!(!pid_is_alive(child_pid.trim()).await);
    }

    #[tokio::test]
    async fn large_stdout_is_truncated_before_model_context() {
        let mut config = ToolRuntimeConfig {
            output: ToolOutputBudget {
                max_captured_stdout_bytes: 32,
                output_head_bytes: 8,
                output_tail_bytes: 8,
                ..ToolOutputBudget::default()
            },
            ..ToolRuntimeConfig::default()
        };
        config.timeout.per_tool_hard_timeout = Duration::from_secs(2);
        let manager = ProcessManager::new(config.clone());
        let invocation = test_invocation(&config);

        let output = manager
            .execute_shell(&invocation, "yes x | head -c 200")
            .await;

        assert_eq!(output.status, ToolOutputStatus::Success);
        assert!(output.stdout.truncated);
        assert_eq!(output.truncation.max_stdout_bytes, 32);
    }

    async fn pid_is_alive(pid: &str) -> bool {
        Command::new("kill")
            .arg("-0")
            .arg(pid)
            .stderr(Stdio::null())
            .status()
            .await
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn test_invocation(config: &ToolRuntimeConfig) -> ToolInvocation {
        let now = Utc::now();
        ToolInvocation {
            session_id: SessionId::from(42),
            turn_id: TurnId::from("turn_process"),
            batch_id: ToolBatchId::from("batch_process"),
            batch_index: 0,
            invocation_id: InvocationId::from("invocation_process"),
            tool_call_id: ToolCallId::from("call_process"),
            provider_tool_call_id: None,
            tool_name: ToolName::from("execute_command"),
            raw_provider_payload: json!({}),
            raw_arguments: json!({ "command": "true" }).to_string(),
            normalized_arguments: json!({ "command": "true" }),
            cancellation_token: tokio_util::sync::CancellationToken::new(),
            timeout: config.timeout.clone(),
            execution_context: ToolExecutionContext::new(config.artifact_dir.clone()),
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
}
