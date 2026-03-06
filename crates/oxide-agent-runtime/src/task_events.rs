//! Transport-agnostic runtime task event publishing.

use async_trait::async_trait;
use oxide_agent_core::agent::TaskEvent;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;

/// Sink for runtime-published task lifecycle events.
#[async_trait]
pub trait TaskEventPublisher: Send + Sync + 'static {
    /// Publish a task event to the next runtime consumer.
    async fn publish(&self, event: TaskEvent);
}

/// No-op task event publisher used when no sink is configured.
#[derive(Debug, Default)]
pub struct NoopTaskEventPublisher;

#[async_trait]
impl TaskEventPublisher for NoopTaskEventPublisher {
    async fn publish(&self, _event: TaskEvent) {}
}

/// Channel-backed task event publisher for tests and adapter integration.
#[derive(Debug, Clone)]
pub struct ChannelTaskEventPublisher {
    sender: UnboundedSender<TaskEvent>,
}

impl ChannelTaskEventPublisher {
    /// Create a new publisher that forwards task events into a channel.
    #[must_use]
    pub fn new(sender: UnboundedSender<TaskEvent>) -> Self {
        Self { sender }
    }
}

#[async_trait]
impl TaskEventPublisher for ChannelTaskEventPublisher {
    async fn publish(&self, event: TaskEvent) {
        let _ = self.sender.send(event);
    }
}

/// Shared task event publisher trait object.
pub type SharedTaskEventPublisher = Arc<dyn TaskEventPublisher>;
