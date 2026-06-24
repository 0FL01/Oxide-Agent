//! Life principal profile envelope.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::{PrincipalUserId, TimestampMillis};

/// Canonical life principal row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LifePrincipal {
    /// Canonical principal id.
    pub principal_user_id: PrincipalUserId,
    /// Deterministic user defaults and profile state.
    pub profile_state: Value,
    /// Confirmed AuDHD operating contract.
    pub operating_profile: Value,
    /// Life-mode settings envelope.
    pub settings: Value,
    /// Schema version for profile/settings JSON payloads.
    pub schema_version: i32,
    /// Creation timestamp.
    pub created_at: TimestampMillis,
    /// Last update timestamp.
    pub updated_at: TimestampMillis,
}
