//! Transport-neutral life identity links.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::domain::{PrincipalUserId, ProviderSubject, TimestampMillis};
use crate::errors::{LifeDomainError, LifeResult};

/// Transport/provider namespace used for explicit life identity links.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LifeIdentityProvider {
    /// Web console user id namespace.
    Web,
    /// Telegram user id namespace.
    Telegram,
}

impl LifeIdentityProvider {
    /// Canonical DB/wire value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Web => "web",
            Self::Telegram => "telegram",
        }
    }
}

impl fmt::Display for LifeIdentityProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for LifeIdentityProvider {
    type Err = LifeDomainError;

    fn from_str(value: &str) -> LifeResult<Self> {
        match value {
            "web" => Ok(Self::Web),
            "telegram" => Ok(Self::Telegram),
            other => Err(LifeDomainError::UnknownEnumValue {
                type_name: "life identity provider",
                value: other.to_owned(),
            }),
        }
    }
}

/// Link from a provider-local subject to a canonical life principal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifeIdentityLink {
    /// Provider namespace.
    pub provider: LifeIdentityProvider,
    /// Provider-local user id/subject.
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
