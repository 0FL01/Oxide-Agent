//! Temporary context overrides.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::{ContextOverrideId, PrincipalUserId, TimestampMillis};

/// TTL-scoped temporary context override.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LifeContextOverride {
    /// Override id.
    pub override_id: ContextOverrideId,
    /// Principal owner.
    pub principal_user_id: PrincipalUserId,
    /// Override key.
    pub key: String,
    /// Override value.
    pub value: Value,
    /// Optional human-readable reason.
    pub reason: Option<String>,
    /// Expiration timestamp.
    pub expires_at: Option<TimestampMillis>,
    /// Creation timestamp.
    pub created_at: TimestampMillis,
    /// Last update timestamp.
    pub updated_at: TimestampMillis,
}
