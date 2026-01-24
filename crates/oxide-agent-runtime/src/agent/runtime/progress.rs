use anyhow::Result;
use async_trait::async_trait;
use oxide_agent_core::agent::loop_detection::LoopType;
use oxide_agent_core::agent::progress::{AgentEvent, ProgressState};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::Receiver;
use tokio::task::JoinHandle;
use tracing::{error, warn};

/// File delivery semantics for progress runtime handlers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryMode {
    /// Best-effort delivery: runtime should continue even if delivery fails.
    BestEffort,
    /// Delivery is observed/confirmed by the agent tool (via oneshot channel).
    Confirmed,
}

/// Transport adapter used by the progress runtime loop.
#[async_trait]
pub trait AgentTransport: Send + Sync + 'static {
    /// Update the progress message based on current state.
    async fn update_progress(&self, state: &ProgressState) -> Result<()>;

    /// Deliver a file emitted by the agent.
    async fn deliver_file(&self, mode: DeliveryMode, file_name: &str, content: &[u8])
        -> Result<()>;

    /// Notify the user about loop detection and prompt for an action.
    async fn notify_loop_detected(&self, _loop_type: LoopType, _iteration: usize) -> Result<()> {
        Ok(())
    }
}

/// Runtime configuration for progress updates.
#[derive(Debug, Clone, Copy)]
pub struct ProgressRuntimeConfig {
    /// Minimum duration between progress updates.
    pub throttle: Duration,
    /// Maximum iterations for initializing progress state.
    pub max_iterations: usize,
}

impl ProgressRuntimeConfig {
    /// Create a new config with the default throttle.
    pub fn new(max_iterations: usize) -> Self {
        Self {
            throttle: Duration::from_millis(1500),
            max_iterations,
        }
    }

    #[cfg(test)]
    /// Override the throttle interval for tests.
    pub fn with_throttle(mut self, throttle: Duration) -> Self {
        self.throttle = throttle;
        self
    }
}

/// Spawn the progress runtime loop on the Tokio runtime.
pub fn spawn_progress_runtime<T: AgentTransport>(
    transport: T,
    rx: Receiver<AgentEvent>,
    config: ProgressRuntimeConfig,
) -> JoinHandle<ProgressState> {
    tokio::spawn(run_progress_loop(transport, rx, config))
}

/// Run the progress update loop until the channel is closed.
pub async fn run_progress_loop<T: AgentTransport>(
    transport: T,
    mut rx: Receiver<AgentEvent>,
    config: ProgressRuntimeConfig,
) -> ProgressState {
    let mut state = ProgressState::new(config.max_iterations);
    let mut last_update = Instant::now();
    let mut needs_update = false;

    while let Some(event) = rx.recv().await {
        // File delivery is a side-effect and should not block state updates more than necessary.
        match &event {
            AgentEvent::FileToSend { file_name, content } => {
                if let Err(e) = transport
                    .deliver_file(DeliveryMode::BestEffort, file_name, content)
                    .await
                {
                    warn!(file_name = %file_name, error = %e, "File delivery failed");
                }
            }
            AgentEvent::FileToSendWithConfirmation { .. } => {
                // Destructure to move `confirmation_tx` out.
                if let AgentEvent::FileToSendWithConfirmation {
                    file_name,
                    content,
                    sandbox_path,
                    confirmation_tx,
                } = event
                {
                    let result = transport
                        .deliver_file(DeliveryMode::Confirmed, &file_name, &content)
                        .await;

                    match result {
                        Ok(_) => {
                            let _ = confirmation_tx.send(Ok(()));
                        }
                        Err(e) => {
                            error!(
                                file_name = %file_name,
                                sandbox_path = %sandbox_path,
                                error = %e,
                                "Confirmed file delivery failed"
                            );
                            let _ = confirmation_tx.send(Err(e.to_string()));
                        }
                    }

                    // Preserve existing semantics: do not update progress state for this variant.
                    needs_update = true;
                    continue;
                }
            }
            AgentEvent::LoopDetected {
                loop_type,
                iteration,
            } => {
                if let Err(e) = transport.notify_loop_detected(*loop_type, *iteration).await {
                    warn!(error = %e, "Loop detection notification failed");
                }
            }
            _ => {}
        }

        state.update(event);
        needs_update = true;

        if needs_update && last_update.elapsed() >= config.throttle {
            if let Err(e) = transport.update_progress(&state).await {
                warn!(error = %e, "Progress update failed");
            }
            last_update = Instant::now();
            needs_update = false;
        }
    }

    if needs_update {
        if let Err(e) = transport.update_progress(&state).await {
            warn!(error = %e, "Final progress update failed");
        }
    }

    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::{mpsc, oneshot, Mutex};

    #[derive(Clone, Default)]
    struct DummyTransport {
        updates: Arc<Mutex<usize>>,
        delivered: Arc<Mutex<Vec<(DeliveryMode, String, usize)>>>,
        fail_deliver: bool,
    }

    #[async_trait]
    impl AgentTransport for DummyTransport {
        async fn update_progress(&self, _state: &ProgressState) -> Result<()> {
            let mut updates = self.updates.lock().await;
            *updates += 1;
            Ok(())
        }

        async fn deliver_file(
            &self,
            mode: DeliveryMode,
            file_name: &str,
            content: &[u8],
        ) -> Result<()> {
            if self.fail_deliver {
                anyhow::bail!("simulated deliver failure");
            }

            let mut delivered = self.delivered.lock().await;
            delivered.push((mode, file_name.to_string(), content.len()));
            Ok(())
        }
    }

    #[tokio::test]
    async fn progress_updates_on_events() {
        let (tx, rx) = mpsc::channel(8);
        let transport = DummyTransport::default();

        let cfg = ProgressRuntimeConfig::new(3).with_throttle(Duration::from_millis(0));
        let handle = spawn_progress_runtime(transport.clone(), rx, cfg);

        let send_result = tx.send(AgentEvent::Thinking { tokens: 1 }).await;
        assert!(
            send_result.is_ok(),
            "failed to send event to runtime channel"
        );
        drop(tx);

        let _state = match handle.await {
            Ok(state) => state,
            Err(err) => panic!("progress runtime join failed: {err}"),
        };

        let updates = *transport.updates.lock().await;
        assert!(updates >= 1);
    }

    #[tokio::test]
    async fn confirmed_delivery_ack_success() {
        let (tx, rx) = mpsc::channel(8);
        let transport = DummyTransport::default();

        let cfg = ProgressRuntimeConfig::new(3).with_throttle(Duration::from_millis(0));
        let handle = spawn_progress_runtime(transport.clone(), rx, cfg);

        let (ack_tx, ack_rx) = oneshot::channel();
        let send_result = tx
            .send(AgentEvent::FileToSendWithConfirmation {
                file_name: "out.txt".to_string(),
                content: vec![1, 2, 3],
                sandbox_path: "/workspace/out.txt".to_string(),
                confirmation_tx: ack_tx,
            })
            .await;
        assert!(send_result.is_ok(), "failed to send file event");

        drop(tx);

        let ack = match ack_rx.await {
            Ok(ack) => ack,
            Err(err) => panic!("ack channel closed: {err}"),
        };
        assert!(ack.is_ok());

        let _state = match handle.await {
            Ok(state) => state,
            Err(err) => panic!("progress runtime join failed: {err}"),
        };
        let delivered = transport.delivered.lock().await;
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].0, DeliveryMode::Confirmed);
    }

    #[tokio::test]
    async fn confirmed_delivery_ack_failure() {
        let (tx, rx) = mpsc::channel(8);

        let transport = DummyTransport {
            fail_deliver: true,
            ..DummyTransport::default()
        };

        let cfg = ProgressRuntimeConfig::new(3).with_throttle(Duration::from_millis(0));
        let handle = spawn_progress_runtime(transport, rx, cfg);

        let (ack_tx, ack_rx) = oneshot::channel();
        let send_result = tx
            .send(AgentEvent::FileToSendWithConfirmation {
                file_name: "out.txt".to_string(),
                content: vec![1, 2, 3],
                sandbox_path: "/workspace/out.txt".to_string(),
                confirmation_tx: ack_tx,
            })
            .await;
        assert!(send_result.is_ok(), "failed to send file event");

        drop(tx);

        let ack = match ack_rx.await {
            Ok(ack) => ack,
            Err(err) => panic!("ack channel closed: {err}"),
        };
        assert!(ack.is_err());

        let _state = match handle.await {
            Ok(state) => state,
            Err(err) => panic!("progress runtime join failed: {err}"),
        };
    }
}
