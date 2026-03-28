use anyhow::Result;
use async_trait::async_trait;
use oxide_agent_core::agent::loop_detection::LoopType;
use oxide_agent_core::agent::progress::{AgentEvent, FileDeliveryKind, ProgressState};
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
    async fn deliver_file(
        &self,
        mode: DeliveryMode,
        kind: FileDeliveryKind,
        file_name: &str,
        content: &[u8],
    ) -> Result<()>;

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
            AgentEvent::FileToSend {
                kind,
                file_name,
                content,
            } => {
                if let Err(e) = transport
                    .deliver_file(DeliveryMode::BestEffort, *kind, file_name, content)
                    .await
                {
                    warn!(file_name = %file_name, error = %e, "File delivery failed");
                }
            }
            AgentEvent::FileToSendWithConfirmation { .. } => {
                // Destructure to move `confirmation_tx` out.
                if let AgentEvent::FileToSendWithConfirmation {
                    kind,
                    file_name,
                    content,
                    source_path,
                    confirmation_tx,
                } = event
                {
                    let result = transport
                        .deliver_file(DeliveryMode::Confirmed, kind, &file_name, &content)
                        .await;

                    match result {
                        Ok(_) => {
                            let _ = confirmation_tx.send(Ok(()));
                        }
                        Err(e) => {
                            error!(
                                file_name = %file_name,
                                source_path = %source_path,
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
            // Rate limit events must update the UI immediately regardless of throttle,
            // so the user sees the retry status without delay. After the forced update
            // we continue to skip the normal throttle check.
            AgentEvent::RateLimitRetrying { .. } => {
                state.update(event);
                if let Err(e) = transport.update_progress(&state).await {
                    warn!(error = %e, "Rate limit progress update failed");
                }
                last_update = Instant::now();
                needs_update = false;
                continue;
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
    use oxide_agent_core::agent::compaction::BudgetState;
    use oxide_agent_core::agent::progress::TokenSnapshot;
    use oxide_agent_core::llm::TokenUsage;
    use std::sync::Arc;
    use tokio::sync::{mpsc, oneshot, Mutex};

    type DeliveredFileRecord = (DeliveryMode, FileDeliveryKind, String, usize);

    fn sample_snapshot() -> TokenSnapshot {
        TokenSnapshot {
            hot_memory_tokens: 1,
            system_prompt_tokens: 2,
            tool_schema_tokens: 3,
            loaded_skill_tokens: 0,
            total_input_tokens: 6,
            reserved_output_tokens: 4,
            hard_reserve_tokens: 2,
            projected_total_tokens: 12,
            context_window_tokens: 100,
            headroom_tokens: 88,
            budget_state: BudgetState::Healthy,
            last_api_usage: Some(TokenUsage {
                prompt_tokens: 6,
                completion_tokens: 4,
                total_tokens: 10,
            }),
        }
    }

    #[derive(Clone, Default)]
    struct DummyTransport {
        updates: Arc<Mutex<usize>>,
        delivered: Arc<Mutex<Vec<DeliveredFileRecord>>>,
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
            kind: FileDeliveryKind,
            file_name: &str,
            content: &[u8],
        ) -> Result<()> {
            if self.fail_deliver {
                anyhow::bail!("simulated deliver failure");
            }

            let mut delivered = self.delivered.lock().await;
            delivered.push((mode, kind, file_name.to_string(), content.len()));
            Ok(())
        }
    }

    #[tokio::test]
    async fn progress_updates_on_events() {
        let (tx, rx) = mpsc::channel(8);
        let transport = DummyTransport::default();

        let cfg = ProgressRuntimeConfig::new(3).with_throttle(Duration::from_millis(0));
        let handle = spawn_progress_runtime(transport.clone(), rx, cfg);

        let send_result = tx
            .send(AgentEvent::Thinking {
                snapshot: sample_snapshot(),
            })
            .await;
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
    async fn best_effort_delivery_preserves_file_kind() {
        let (tx, rx) = mpsc::channel(8);
        let transport = DummyTransport::default();

        let cfg = ProgressRuntimeConfig::new(3).with_throttle(Duration::from_millis(0));
        let handle = spawn_progress_runtime(transport.clone(), rx, cfg);

        tx.send(AgentEvent::FileToSend {
            kind: FileDeliveryKind::VoiceNote,
            file_name: "speech.ogg".to_string(),
            content: vec![1, 2, 3],
        })
        .await
        .expect("failed to send file event");

        drop(tx);

        let _state = handle.await.expect("progress runtime join failed");
        let delivered = transport.delivered.lock().await;

        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].0, DeliveryMode::BestEffort);
        assert_eq!(delivered[0].1, FileDeliveryKind::VoiceNote);
        assert_eq!(delivered[0].2, "speech.ogg");
    }

    /// Regression test: RateLimitRetrying must trigger an immediate UI update
    /// even when the throttle is large (the user must see the retry banner
    /// without delay). A subsequent Thinking event must also clear the
    /// rate_limit_retry state from the rendered output.
    #[tokio::test]
    async fn rate_limit_retrying_forces_immediate_update() {
        let (tx, rx) = mpsc::channel(8);
        let transport = DummyTransport::default();

        // Use a large throttle so the bypass is exercised.
        let cfg = ProgressRuntimeConfig::new(10).with_throttle(Duration::from_secs(60));
        let handle = spawn_progress_runtime(transport.clone(), rx, cfg);

        // 1. Send RateLimitRetrying — must trigger immediate update.
        tx.send(AgentEvent::RateLimitRetrying {
            attempt: 2,
            max_attempts: 5,
            wait_secs: Some(20),
            provider: "minimax".to_string(),
        })
        .await
        .expect("send succeeds");

        // Give the runtime a moment to process the event.
        tokio::time::sleep(Duration::from_millis(20)).await;

        let updates_after_rate_limit = *transport.updates.lock().await;
        assert!(
            updates_after_rate_limit >= 1,
            "RateLimitRetrying must force at least one immediate update"
        );

        // 2. Send Thinking — must clear rate_limit_retry in state and
        // trigger a normal throttled update.
        tx.send(AgentEvent::Thinking {
            snapshot: sample_snapshot(),
        })
        .await
        .expect("send succeeds");

        // Wait for the channel to be drained.
        drop(tx);

        let _state = match handle.await {
            Ok(state) => state,
            Err(err) => panic!("progress runtime join failed: {err}"),
        };

        let final_updates = *transport.updates.lock().await;
        assert!(
            final_updates >= 2,
            "Expected at least 2 updates: one for RateLimitRetrying, one for Thinking"
        );

        // Verify rate_limit_retry was cleared after Thinking.
        assert!(
            _state.rate_limit_retry.is_none(),
            "rate_limit_retry must be cleared after Thinking event"
        );
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
                kind: FileDeliveryKind::Auto,
                file_name: "out.txt".to_string(),
                content: vec![1, 2, 3],
                source_path: "/workspace/out.txt".to_string(),
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
        assert_eq!(delivered[0].1, FileDeliveryKind::Auto);
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
                kind: FileDeliveryKind::Auto,
                file_name: "out.txt".to_string(),
                content: vec![1, 2, 3],
                source_path: "/workspace/out.txt".to_string(),
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
