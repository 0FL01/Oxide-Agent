//! Derived Engram outbox rows.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::{MemoryGenerationId, MemoryItemId, OutboxId, PrincipalUserId, TimestampMillis};

/// Delivery status for derived long-term memory projections.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LifeEngramOutboxStatus {
    /// Ready to be flushed by an outbox worker.
    Pending,
    /// Currently being flushed.
    Flushing,
    /// Successfully flushed.
    Flushed,
    /// Permanently failed after retries.
    Dead,
}

/// A pending derived-memory projection into Engram.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LifeEngramOutboxRow {
    /// Outbox row id.
    pub outbox_id: OutboxId,
    /// Principal owner.
    pub principal_user_id: PrincipalUserId,
    /// Memory generation owner.
    pub memory_generation_id: MemoryGenerationId,
    /// Canonical memory source row, when this projection is memory-backed.
    pub source_memory_id: Option<MemoryItemId>,
    /// Idempotency key for the derived backend.
    pub idempotency_key: String,
    /// Backend-specific payload.
    pub payload: Value,
    /// Delivery status.
    pub status: LifeEngramOutboxStatus,
    /// Retry attempts.
    pub attempts: i32,
    /// Earliest next retry timestamp.
    pub next_attempt_at: TimestampMillis,
    /// Last delivery error.
    pub last_error: Option<String>,
    /// Creation timestamp.
    pub created_at: TimestampMillis,
    /// Last update timestamp.
    pub updated_at: TimestampMillis,
}
