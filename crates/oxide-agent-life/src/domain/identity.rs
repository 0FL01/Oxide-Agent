//! Transport-neutral life identity links.

use std::fmt;

use serde::{Deserialize, Serialize};

use serde_json::Value;

use crate::domain::{BindingId, PrincipalUserId, ProviderSubject, TimestampMillis};
use crate::errors::{LifeDomainError, LifeResult};

/// Canonical Web transport id.
pub const WEB_TRANSPORT_ID: &str = "web";
/// Canonical Telegram transport id.
pub const TELEGRAM_TRANSPORT_ID: &str = "telegram";
/// Canonical internal/system source id.
pub const INTERNAL_TRANSPORT_ID: &str = "internal";

/// Open transport namespace used for life identity links and turn provenance.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LifeTransportId(String);

impl LifeTransportId {
    /// Creates a non-empty transport id.
    pub fn new(value: impl Into<String>) -> LifeResult<Self> {
        let value = value.into().trim().to_owned();
        if value.is_empty() {
            Err(LifeDomainError::EmptyField {
                field: "transport_id",
            })
        } else {
            Ok(Self(value))
        }
    }

    /// Returns the canonical transport id string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LifeTransportId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Link from a transport-local subject to a canonical life principal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifeIdentityLink {
    /// Transport namespace.
    pub transport_id: LifeTransportId,
    /// Transport-local user id/subject.
    pub provider_subject: ProviderSubject,
    /// Canonical life principal.
    pub principal_user_id: PrincipalUserId,
    /// Optional verification timestamp.
    pub verified_at: Option<TimestampMillis>,
    /// Creation timestamp.
    pub created_at: TimestampMillis,
    /// Last update timestamp.
    pub updated_at: TimestampMillis,
}

/// Durable owner-approved route between a transport address and a life principal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LifeTransportBinding {
    /// Stable binding row id.
    pub binding_id: BindingId,
    /// Canonical life principal.
    pub principal_user_id: PrincipalUserId,
    /// Transport namespace.
    pub transport_id: LifeTransportId,
    /// Address observed on inbound messages; object JSON, never a secret.
    pub inbound_address: Value,
    /// Address used by delivery workers; object JSON, never a token/secret.
    pub delivery_address: Value,
    /// Disabled bindings do not accept inbound submissions or outbound delivery.
    pub enabled: bool,
    /// Creation timestamp.
    pub created_at: TimestampMillis,
    /// Last update timestamp.
    pub updated_at: TimestampMillis,
}
