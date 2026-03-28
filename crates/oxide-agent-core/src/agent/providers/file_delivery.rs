use crate::agent::progress::AgentEvent;
use tokio::sync::mpsc::Sender;
use tracing::{info, warn};

const CHAT_DELIVERY_CONFIRMATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

pub(crate) const CHAT_DELIVERY_MAX_FILE_SIZE_BYTES: u64 = 50 * 1024 * 1024;

pub(crate) struct FileDeliveryRequest {
    pub(crate) file_name: String,
    pub(crate) content: Vec<u8>,
    pub(crate) source_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FileDeliveryStatus {
    Delivered,
    DeliveryFailed(String),
    ConfirmationChannelClosed,
    TimedOut,
    QueueUnavailable(String),
    EmptyContent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileDeliveryReport {
    pub(crate) file_name: String,
    pub(crate) source_path: String,
    pub(crate) size_bytes: usize,
    pub(crate) status: FileDeliveryStatus,
}

impl FileDeliveryReport {
    pub(crate) fn size_mb(&self) -> f64 {
        self.size_bytes as f64 / 1024.0 / 1024.0
    }
}

pub(crate) async fn deliver_file_via_progress(
    progress_tx: Option<&Sender<AgentEvent>>,
    request: FileDeliveryRequest,
) -> FileDeliveryReport {
    let FileDeliveryRequest {
        file_name,
        content,
        source_path,
    } = request;

    let size_bytes = content.len();
    if content.is_empty() {
        return FileDeliveryReport {
            file_name,
            source_path,
            size_bytes,
            status: FileDeliveryStatus::EmptyContent,
        };
    }

    let Some(tx) = progress_tx else {
        warn!(file_name = %file_name, source_path = %source_path, "Progress channel not available");
        return FileDeliveryReport {
            file_name,
            source_path,
            size_bytes,
            status: FileDeliveryStatus::QueueUnavailable(
                "send channel is not available".to_string(),
            ),
        };
    };

    let (confirm_tx, confirm_rx) = tokio::sync::oneshot::channel();
    if let Err(error) = tx
        .send(AgentEvent::FileToSendWithConfirmation {
            file_name: file_name.clone(),
            content,
            source_path: source_path.clone(),
            confirmation_tx: confirm_tx,
        })
        .await
    {
        warn!(file_name = %file_name, source_path = %source_path, error = %error, "Failed to send FileToSendWithConfirmation event");
        return FileDeliveryReport {
            file_name,
            source_path,
            size_bytes,
            status: FileDeliveryStatus::QueueUnavailable(error.to_string()),
        };
    }

    let status = match tokio::time::timeout(CHAT_DELIVERY_CONFIRMATION_TIMEOUT, confirm_rx).await {
        Ok(Ok(Ok(()))) => {
            info!(file_name = %file_name, source_path = %source_path, "File delivered successfully");
            FileDeliveryStatus::Delivered
        }
        Ok(Ok(Err(error))) => {
            warn!(file_name = %file_name, source_path = %source_path, error = %error, "File delivery failed");
            FileDeliveryStatus::DeliveryFailed(error)
        }
        Ok(Err(_)) => {
            warn!(file_name = %file_name, source_path = %source_path, "Confirmation channel closed unexpectedly");
            FileDeliveryStatus::ConfirmationChannelClosed
        }
        Err(_) => {
            warn!(file_name = %file_name, source_path = %source_path, "File delivery confirmation timeout");
            FileDeliveryStatus::TimedOut
        }
    };

    FileDeliveryReport {
        file_name,
        source_path,
        size_bytes,
        status,
    }
}

pub(crate) fn format_generic_delivery_report(report: &FileDeliveryReport) -> String {
    match &report.status {
        FileDeliveryStatus::Delivered => {
            format!("✅ File '{}' delivered to user", report.file_name)
        }
        FileDeliveryStatus::DeliveryFailed(error) => format!(
            "❌ Failed to send file '{}' to the user: {}\nSource path: {}",
            report.file_name, error, report.source_path
        ),
        FileDeliveryStatus::ConfirmationChannelClosed => format!(
            "⚠️ Status of file '{}' delivery unknown (confirmation channel closed).\nSource path: {}",
            report.file_name, report.source_path
        ),
        FileDeliveryStatus::TimedOut => format!(
            "⚠️ File '{}' delivery confirmation timeout (2 minutes).\nSource path: {}",
            report.file_name, report.source_path
        ),
        FileDeliveryStatus::QueueUnavailable(error) => format!(
            "⚠️ File '{}' read ({:.2} MB), but failed to queue for sending: {}\nSource path: {}",
            report.file_name,
            report.size_mb(),
            error,
            report.source_path
        ),
        FileDeliveryStatus::EmptyContent => format!(
            "❌ ERROR: File '{}' is empty (0 bytes) and cannot be sent.\nSource path: {}",
            report.file_name, report.source_path
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn deliver_file_returns_success_only_after_confirmation() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(1);

        tokio::spawn(async move {
            if let Some(AgentEvent::FileToSendWithConfirmation {
                confirmation_tx, ..
            }) = rx.recv().await
            {
                let _ = confirmation_tx.send(Ok(()));
            }
        });

        let report = deliver_file_via_progress(
            Some(&tx),
            FileDeliveryRequest {
                file_name: "ok.txt".to_string(),
                content: b"hello".to_vec(),
                source_path: "/workspace/ok.txt".to_string(),
            },
        )
        .await;

        assert_eq!(report.status, FileDeliveryStatus::Delivered);
    }

    #[tokio::test]
    async fn deliver_file_propagates_delivery_error() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(1);

        tokio::spawn(async move {
            if let Some(AgentEvent::FileToSendWithConfirmation {
                confirmation_tx, ..
            }) = rx.recv().await
            {
                let _ =
                    confirmation_tx.send(Err("Bad Request: file must be non-empty".to_string()));
            }
        });

        let report = deliver_file_via_progress(
            Some(&tx),
            FileDeliveryRequest {
                file_name: "empty.txt".to_string(),
                content: b"x".to_vec(),
                source_path: "/workspace/empty.txt".to_string(),
            },
        )
        .await;

        assert_eq!(
            report.status,
            FileDeliveryStatus::DeliveryFailed("Bad Request: file must be non-empty".to_string())
        );
    }

    #[tokio::test]
    async fn deliver_file_fails_when_queue_is_unavailable() {
        let (tx, rx) = tokio::sync::mpsc::channel::<AgentEvent>(1);
        drop(rx);

        let report = deliver_file_via_progress(
            Some(&tx),
            FileDeliveryRequest {
                file_name: "file.txt".to_string(),
                content: b"hello".to_vec(),
                source_path: "/workspace/file.txt".to_string(),
            },
        )
        .await;

        assert!(matches!(
            report.status,
            FileDeliveryStatus::QueueUnavailable(_)
        ));
    }

    #[tokio::test]
    async fn deliver_file_rejects_empty_content() {
        let report = deliver_file_via_progress(
            None,
            FileDeliveryRequest {
                file_name: "empty.bin".to_string(),
                content: Vec::new(),
                source_path: "/workspace/empty.bin".to_string(),
            },
        )
        .await;

        assert_eq!(report.status, FileDeliveryStatus::EmptyContent);
    }
}
