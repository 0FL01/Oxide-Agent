//! Transport-neutral life progress/event stream records.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::{EventId, RunId, TimestampMillis};

/// Life event stream row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LifeEvent {
    /// Event id.
    pub event_id: EventId,
    /// Run id.
    pub run_id: RunId,
    /// Monotonic sequence per run.
    pub seq: i64,
    /// Event kind.
    pub kind: String,
    /// Event payload.
    pub payload: Value,
    /// Creation timestamp.
    pub created_at: TimestampMillis,
}
