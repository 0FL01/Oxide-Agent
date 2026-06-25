//! DB-backed life input queue records.

use serde::{Deserialize, Serialize};

use crate::domain::{InputId, PrincipalUserId, TimestampMillis, TurnId};

/// Status for queued life inputs.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LifeInputStatus {
    /// Waiting to be processed.
    Queued,
    /// Claimed by a worker.
    Claimed,
    /// Consumed into a run/continuation.
    Consumed,
    /// Permanently failed.
    Dead,
}

/// DB-backed input queue row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifeInput {
    /// Input id.
    pub input_id: InputId,
    /// Principal owner.
    pub principal_user_id: PrincipalUserId,
    /// Canonical user turn for this input.
    pub turn_id: TurnId,
    /// Queue status.
    pub status: LifeInputStatus,
    /// Optional worker claim id.
    pub claimed_by: Option<String>,
    /// Optional claim timestamp.
    pub claimed_at: Option<TimestampMillis>,
    /// Creation timestamp.
    pub created_at: TimestampMillis,
    /// Last update timestamp.
    pub updated_at: TimestampMillis,
}
