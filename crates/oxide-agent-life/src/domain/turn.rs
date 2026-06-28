//! Canonical life transcript records.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::{LifeTransportId, PrincipalUserId, RunId, TimestampMillis, TurnId};

/// Role stored in `life_turns`.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LifeTurnRole {
    /// User-originated message.
    User,
    /// Assistant response.
    Assistant,
    /// System/internal note.
    System,
    /// Tool observation.
    Tool,
}

/// Redaction state for transcript content.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RedactionState {
    /// Content is safe for durable memory processing.
    Clean,
    /// Content was redacted.
    Redacted,
    /// Content was blocked as secret-like.
    SecretBlocked,
}

/// Append-only canonical life turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LifeTurn {
    /// Turn id.
    pub turn_id: TurnId,
    /// Principal owner.
    pub principal_user_id: PrincipalUserId,
    /// Optional run id that consumed/produced this turn.
    pub run_id: Option<RunId>,
    /// Turn role.
    pub role: LifeTurnRole,
    /// Source transport.
    pub source_transport: LifeTransportId,
    /// Transport-local reference.
    pub source_ref: Option<String>,
    /// Turn content.
    pub content: String,
    /// Attachment references, not raw bytes.
    pub attachments: Value,
    /// Transport metadata captured by the gateway without letting transports own life state.
    pub transport_metadata: Value,
    /// Redaction state.
    pub redaction_state: RedactionState,
    /// Creation timestamp.
    pub created_at: TimestampMillis,
}
