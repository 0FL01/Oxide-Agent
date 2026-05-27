use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct OutputTruncation {
    pub(super) original_bytes: usize,
    pub(super) captured_bytes: usize,
}

#[derive(Debug)]
pub(super) struct CappedOutput {
    bytes: Vec<u8>,
    original_bytes: usize,
    max_bytes: usize,
}

impl CappedOutput {
    const fn empty() -> Self {
        Self {
            bytes: Vec::new(),
            original_bytes: 0,
            max_bytes: 0,
        }
    }

    pub(super) fn into_output(self) -> (String, Option<OutputTruncation>) {
        let truncation = (self.original_bytes > self.bytes.len()).then_some(OutputTruncation {
            original_bytes: self.original_bytes,
            captured_bytes: self.max_bytes,
        });
        (
            String::from_utf8_lossy(&self.bytes).into_owned(),
            truncation,
        )
    }
}

pub(super) async fn read_capped_counted<R>(mut reader: R, max_bytes: usize) -> CappedOutput
where
    R: AsyncRead + Unpin,
{
    let mut bytes = Vec::with_capacity(max_bytes.min(8192));
    let mut original_bytes = 0usize;
    let mut chunk = [0_u8; 8192];

    loop {
        let Ok(read) = reader.read(&mut chunk).await else {
            break;
        };
        if read == 0 {
            break;
        }
        original_bytes = original_bytes.saturating_add(read);
        let remaining = max_bytes.saturating_sub(bytes.len());
        if remaining > 0 {
            bytes.extend_from_slice(&chunk[..read.min(remaining)]);
        }
    }

    CappedOutput {
        bytes,
        original_bytes,
        max_bytes,
    }
}

pub(super) async fn await_capped_output(
    task: Option<tokio::task::JoinHandle<CappedOutput>>,
) -> CappedOutput {
    match task {
        Some(task) => task.await.unwrap_or_else(|_| CappedOutput::empty()),
        None => CappedOutput::empty(),
    }
}

pub(super) async fn cleanup_bwrap_child(
    child: &mut tokio::process::Child,
    pid: Option<u32>,
) -> &'static str {
    match child.try_wait() {
        Ok(Some(_)) => return "process already exited",
        Ok(None) => {}
        Err(_) => return "process cleanup status could not be inspected",
    }

    if let Some(pid) = pid {
        let _ = send_process_group_signal(pid, "TERM").await;
        if wait_for_bwrap_child(child, Duration::from_secs(2)).await {
            return "process group terminated";
        }

        if send_process_group_signal(pid, "KILL").await
            && wait_for_bwrap_child(child, Duration::from_secs(2)).await
        {
            return "process group was killed";
        }
    }

    if child.kill().await.is_ok() && wait_for_bwrap_child(child, Duration::from_secs(2)).await {
        return "process was killed";
    }

    "process cleanup failed"
}

async fn wait_for_bwrap_child(child: &mut tokio::process::Child, duration: Duration) -> bool {
    matches!(
        tokio::time::timeout(duration, child.wait()).await,
        Ok(Ok(_))
    )
}

async fn send_process_group_signal(pid: u32, signal: &str) -> bool {
    #[cfg(unix)]
    {
        Command::new("kill")
            .arg(format!("-{signal}"))
            .arg("--")
            .arg(format!("-{pid}"))
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
