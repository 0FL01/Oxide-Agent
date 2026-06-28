//! Transport-neutral delivery outbox worker.

use async_trait::async_trait;
use thiserror::Error;

use crate::domain::{
    ClaimedLifeDelivery, DeliveryId, LifeTransportId, TimestampMillis, validate_delivery_worker_id,
};
use crate::storage::{LifeStorageError, LifeStorageRepository, LifeStorageResult};

/// Sender failure classification returned by concrete transport adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifeDeliverySendFailure {
    /// Retryable transport/API failure.
    Retryable(String),
    /// Non-retryable validation or permanent transport failure.
    Permanent(String),
}

/// Delivery worker error.
#[derive(Debug, Error)]
pub enum LifeDeliveryWorkerError {
    /// Storage failed.
    #[error(transparent)]
    Storage(#[from] LifeStorageError),
    /// Sender failed after the row was claimed.
    #[error("life delivery send failed: {0}")]
    Sender(String),
    /// Worker id was invalid.
    #[error(transparent)]
    Domain(#[from] crate::errors::LifeDomainError),
}

/// Result alias for delivery worker operations.
pub type LifeDeliveryWorkerResult<T> = Result<T, LifeDeliveryWorkerError>;

/// Outcome of one delivery worker poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifeDeliveryWorkerOutcome {
    /// No due delivery row exists for the worker transport.
    Idle,
    /// A row was delivered.
    Delivered { delivery_id: DeliveryId },
    /// A row failed and was scheduled for retry.
    Failed {
        delivery_id: DeliveryId,
        next_attempt_at: TimestampMillis,
    },
    /// A row was permanently dead-lettered.
    Dead { delivery_id: DeliveryId },
}

/// Minimal storage seam required by the delivery worker.
#[async_trait]
pub trait LifeDeliveryStore: Send + Sync {
    /// Claims the next due delivery row for a transport.
    async fn claim_next_delivery(
        &self,
        transport_id: &LifeTransportId,
        worker_id: &str,
        now: TimestampMillis,
    ) -> LifeStorageResult<Option<ClaimedLifeDelivery>>;

    /// Marks the claimed row delivered.
    async fn mark_delivery_delivered(
        &self,
        delivery_id: DeliveryId,
        now: TimestampMillis,
    ) -> LifeStorageResult<()>;

    /// Marks the claimed row retryable.
    async fn mark_delivery_failed(
        &self,
        delivery_id: DeliveryId,
        error_text: &str,
        next_attempt_at: TimestampMillis,
        now: TimestampMillis,
    ) -> LifeStorageResult<()>;

    /// Marks the claimed row permanently dead.
    async fn mark_delivery_dead(
        &self,
        delivery_id: DeliveryId,
        error_text: &str,
        now: TimestampMillis,
    ) -> LifeStorageResult<()>;
}

#[async_trait]
impl<T> LifeDeliveryStore for T
where
    T: LifeStorageRepository,
{
    async fn claim_next_delivery(
        &self,
        transport_id: &LifeTransportId,
        worker_id: &str,
        now: TimestampMillis,
    ) -> LifeStorageResult<Option<ClaimedLifeDelivery>> {
        LifeStorageRepository::claim_next_delivery(self, transport_id, worker_id, now).await
    }

    async fn mark_delivery_delivered(
        &self,
        delivery_id: DeliveryId,
        now: TimestampMillis,
    ) -> LifeStorageResult<()> {
        LifeStorageRepository::mark_delivery_delivered(self, delivery_id, now).await
    }

    async fn mark_delivery_failed(
        &self,
        delivery_id: DeliveryId,
        error_text: &str,
        next_attempt_at: TimestampMillis,
        now: TimestampMillis,
    ) -> LifeStorageResult<()> {
        LifeStorageRepository::mark_delivery_failed(
            self,
            delivery_id,
            error_text,
            next_attempt_at,
            now,
        )
        .await
    }

    async fn mark_delivery_dead(
        &self,
        delivery_id: DeliveryId,
        error_text: &str,
        now: TimestampMillis,
    ) -> LifeStorageResult<()> {
        LifeStorageRepository::mark_delivery_dead(self, delivery_id, error_text, now).await
    }
}

/// Transport adapter seam. Implementations own API details and secrets.
#[async_trait]
pub trait LifeDeliverySender: Send + Sync {
    /// Sends one claimed delivery.
    async fn send(&self, delivery: &ClaimedLifeDelivery) -> Result<(), LifeDeliverySendFailure>;
}

/// Transport-neutral outbox worker.
pub struct LifeDeliveryWorker<S, T> {
    store: S,
    sender: T,
    transport_id: LifeTransportId,
    worker_id: String,
    max_attempts: i32,
    retry_delay_millis: i64,
}

impl<S, T> LifeDeliveryWorker<S, T> {
    /// Creates a worker for one transport sender.
    pub fn new(
        store: S,
        sender: T,
        transport_id: LifeTransportId,
        worker_id: impl Into<String>,
    ) -> crate::LifeResult<Self> {
        let worker_id = worker_id.into();
        validate_delivery_worker_id(&worker_id)?;
        Ok(Self {
            store,
            sender,
            transport_id,
            worker_id,
            max_attempts: 5,
            retry_delay_millis: 30_000,
        })
    }

    /// Overrides retry policy. Intended for tests and small deployments.
    #[must_use]
    pub const fn with_retry_policy(mut self, max_attempts: i32, retry_delay_millis: i64) -> Self {
        self.max_attempts = max_attempts;
        self.retry_delay_millis = retry_delay_millis;
        self
    }
}

impl<S, T> LifeDeliveryWorker<S, T>
where
    S: LifeDeliveryStore,
    T: LifeDeliverySender,
{
    /// Claims and processes at most one due delivery row.
    pub async fn process_one(
        &self,
        now: TimestampMillis,
    ) -> LifeDeliveryWorkerResult<LifeDeliveryWorkerOutcome> {
        let Some(claimed) = self
            .store
            .claim_next_delivery(&self.transport_id, &self.worker_id, now)
            .await?
        else {
            return Ok(LifeDeliveryWorkerOutcome::Idle);
        };

        let delivery_id = claimed.delivery.delivery_id;
        match self.sender.send(&claimed).await {
            Ok(()) => {
                self.store.mark_delivery_delivered(delivery_id, now).await?;
                Ok(LifeDeliveryWorkerOutcome::Delivered { delivery_id })
            }
            Err(LifeDeliverySendFailure::Permanent(error)) => {
                self.store
                    .mark_delivery_dead(delivery_id, &error, now)
                    .await?;
                Ok(LifeDeliveryWorkerOutcome::Dead { delivery_id })
            }
            Err(LifeDeliverySendFailure::Retryable(error)) => {
                if claimed.delivery.attempt_count >= self.max_attempts {
                    self.store
                        .mark_delivery_dead(delivery_id, &error, now)
                        .await?;
                    Ok(LifeDeliveryWorkerOutcome::Dead { delivery_id })
                } else {
                    let next_attempt_at = TimestampMillis::new(now.get() + self.retry_delay_millis);
                    self.store
                        .mark_delivery_failed(delivery_id, &error, next_attempt_at, now)
                        .await?;
                    Ok(LifeDeliveryWorkerOutcome::Failed {
                        delivery_id,
                        next_attempt_at,
                    })
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use serde_json::json;

    use crate::domain::{
        BindingId, ClaimedLifeDelivery, DeliveryId, LifeDeliveryOutbox, LifeDeliveryStatus,
        LifeTransportId, PrincipalUserId, TimestampMillis, TurnId,
    };
    use crate::storage::LifeStorageResult;

    use super::*;

    #[derive(Clone, Default)]
    struct FakeDeliveryStore {
        rows: Arc<Mutex<Vec<ClaimedLifeDelivery>>>,
        transitions: Arc<Mutex<Vec<String>>>,
    }

    impl FakeDeliveryStore {
        fn with_row(row: ClaimedLifeDelivery) -> Self {
            Self {
                rows: Arc::new(Mutex::new(vec![row])),
                transitions: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn transitions(&self) -> Vec<String> {
            self.transitions.lock().expect("transitions mutex").clone()
        }
    }

    #[async_trait]
    impl LifeDeliveryStore for FakeDeliveryStore {
        async fn claim_next_delivery(
            &self,
            _transport_id: &LifeTransportId,
            _worker_id: &str,
            _now: TimestampMillis,
        ) -> LifeStorageResult<Option<ClaimedLifeDelivery>> {
            Ok(self.rows.lock().expect("rows mutex").pop())
        }

        async fn mark_delivery_delivered(
            &self,
            delivery_id: DeliveryId,
            _now: TimestampMillis,
        ) -> LifeStorageResult<()> {
            self.transitions
                .lock()
                .expect("transitions mutex")
                .push(format!("delivered:{delivery_id}"));
            Ok(())
        }

        async fn mark_delivery_failed(
            &self,
            delivery_id: DeliveryId,
            error_text: &str,
            next_attempt_at: TimestampMillis,
            _now: TimestampMillis,
        ) -> LifeStorageResult<()> {
            self.transitions
                .lock()
                .expect("transitions mutex")
                .push(format!(
                    "failed:{delivery_id}:{error_text}:{}",
                    next_attempt_at.get()
                ));
            Ok(())
        }

        async fn mark_delivery_dead(
            &self,
            delivery_id: DeliveryId,
            error_text: &str,
            _now: TimestampMillis,
        ) -> LifeStorageResult<()> {
            self.transitions
                .lock()
                .expect("transitions mutex")
                .push(format!("dead:{delivery_id}:{error_text}"));
            Ok(())
        }
    }

    struct FakeSender {
        result: Result<(), LifeDeliverySendFailure>,
    }

    #[async_trait]
    impl LifeDeliverySender for FakeSender {
        async fn send(
            &self,
            delivery: &ClaimedLifeDelivery,
        ) -> Result<(), LifeDeliverySendFailure> {
            assert_eq!(delivery.content, "assistant response");
            self.result.clone()
        }
    }

    fn claimed(attempt_count: i32) -> ClaimedLifeDelivery {
        let now = TimestampMillis::new(1_700_000_000_000);
        ClaimedLifeDelivery {
            delivery: LifeDeliveryOutbox {
                delivery_id: DeliveryId::new_v4(),
                turn_id: TurnId::new_v4(),
                binding_id: BindingId::new_v4(),
                principal_user_id: PrincipalUserId::new(42).expect("principal"),
                transport_id: LifeTransportId::new("telegram").expect("transport"),
                delivery_address: json!({ "chat_id": "424242" }),
                status: LifeDeliveryStatus::Claimed,
                attempt_count,
                claimed_by: Some("worker".to_owned()),
                claimed_at: Some(now),
                claim_expires_at: Some(TimestampMillis::new(now.get() + 1_000)),
                next_attempt_at: now,
                last_error: None,
                created_at: now,
                updated_at: now,
            },
            content: "assistant response".to_owned(),
        }
    }

    #[tokio::test]
    async fn delivery_worker_marks_success_delivered() {
        let row = claimed(1);
        let delivery_id = row.delivery.delivery_id;
        let store = FakeDeliveryStore::with_row(row);
        let worker = LifeDeliveryWorker::new(
            store.clone(),
            FakeSender { result: Ok(()) },
            LifeTransportId::new("telegram").expect("transport"),
            "worker",
        )
        .expect("worker");

        let outcome = worker
            .process_one(TimestampMillis::new(1_700_000_000_001))
            .await
            .expect("process one");
        assert_eq!(
            outcome,
            LifeDeliveryWorkerOutcome::Delivered { delivery_id }
        );
        assert_eq!(
            store.transitions(),
            vec![format!("delivered:{delivery_id}")]
        );
    }

    #[tokio::test]
    async fn delivery_worker_retries_then_dead_letters() {
        let retry_row = claimed(1);
        let retry_id = retry_row.delivery.delivery_id;
        let retry_store = FakeDeliveryStore::with_row(retry_row);
        let retry_worker = LifeDeliveryWorker::new(
            retry_store.clone(),
            FakeSender {
                result: Err(LifeDeliverySendFailure::Retryable("temporary".to_owned())),
            },
            LifeTransportId::new("telegram").expect("transport"),
            "worker",
        )
        .expect("worker")
        .with_retry_policy(3, 10);
        assert_eq!(
            retry_worker
                .process_one(TimestampMillis::new(100))
                .await
                .expect("retry outcome"),
            LifeDeliveryWorkerOutcome::Failed {
                delivery_id: retry_id,
                next_attempt_at: TimestampMillis::new(110)
            }
        );

        let dead_row = claimed(3);
        let dead_id = dead_row.delivery.delivery_id;
        let dead_store = FakeDeliveryStore::with_row(dead_row);
        let dead_worker = LifeDeliveryWorker::new(
            dead_store.clone(),
            FakeSender {
                result: Err(LifeDeliverySendFailure::Retryable("again".to_owned())),
            },
            LifeTransportId::new("telegram").expect("transport"),
            "worker",
        )
        .expect("worker")
        .with_retry_policy(3, 10);
        assert_eq!(
            dead_worker
                .process_one(TimestampMillis::new(100))
                .await
                .expect("dead outcome"),
            LifeDeliveryWorkerOutcome::Dead {
                delivery_id: dead_id
            }
        );
    }
}
