//! Durable outbound delivery work for Life bridge transports.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::{
    BindingId, DeliveryId, LifeTransportId, PrincipalUserId, TimestampMillis, TurnId,
};
use crate::errors::{LifeDomainError, LifeResult};

/// Durable delivery state for one assistant turn and one enabled transport binding.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LifeDeliveryStatus {
    /// Ready to be claimed by a delivery worker.
    Queued,
    /// Claimed by a worker. `claim_expires_at` makes crashed claims recoverable.
    Claimed,
    /// Successfully sent or acknowledged by a transport adapter.
    Delivered,
    /// Failed but retryable after `next_attempt_at`.
    Failed,
    /// Permanently failed after retry exhaustion or unrecoverable validation.
    Dead,
}

/// One durable outbound delivery row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifeDeliveryOutbox {
    /// Delivery row id.
    pub delivery_id: DeliveryId,
    /// Assistant transcript turn to deliver.
    pub turn_id: TurnId,
    /// Binding that produced this delivery row.
    pub binding_id: BindingId,
    /// Canonical life principal.
    pub principal_user_id: PrincipalUserId,
    /// Transport namespace for adapter dispatch.
    pub transport_id: LifeTransportId,
    /// Non-secret delivery address snapshot from the binding.
    pub delivery_address: Value,
    /// Durable status.
    pub status: LifeDeliveryStatus,
    /// Number of claim attempts.
    pub attempt_count: i32,
    /// Worker id currently owning the row, if claimed.
    pub claimed_by: Option<String>,
    /// Claim timestamp.
    pub claimed_at: Option<TimestampMillis>,
    /// Claim visibility timeout; expired claimed rows can be reclaimed.
    pub claim_expires_at: Option<TimestampMillis>,
    /// Earliest retry time for queued/failed rows.
    pub next_attempt_at: TimestampMillis,
    /// Last transport/storage error, never a secret.
    pub last_error: Option<String>,
    /// Creation timestamp.
    pub created_at: TimestampMillis,
    /// Last mutation timestamp.
    pub updated_at: TimestampMillis,
}

/// Claimed delivery row plus assistant content loaded from `life_turns`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimedLifeDelivery {
    /// Claimed delivery row.
    pub delivery: LifeDeliveryOutbox,
    /// Assistant content to send.
    pub content: String,
}

/// Non-empty worker identifier used for delivery claims.
pub fn validate_delivery_worker_id(worker_id: &str) -> LifeResult<()> {
    if worker_id.trim().is_empty() {
        Err(LifeDomainError::EmptyField {
            field: "delivery_worker_id",
        })
    } else {
        Ok(())
    }
}
