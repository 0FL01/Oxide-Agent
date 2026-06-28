//! Strong identifiers for life-mode rows.

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::errors::{LifeDomainError, LifeResult};

/// Internal principal id. This is the canonical life identity and references `users.user_id`.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PrincipalUserId(i64);

impl PrincipalUserId {
    /// Creates a positive principal id.
    pub fn new(value: i64) -> LifeResult<Self> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(LifeDomainError::InvalidPrincipalUserId(value))
        }
    }

    /// Returns the raw id value.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

impl fmt::Display for PrincipalUserId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

macro_rules! uuid_id {
    ($name:ident) => {
        #[derive(
            Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        pub struct $name(Uuid);

        impl $name {
            /// Creates a new random v4 identifier.
            #[must_use]
            pub fn new_v4() -> Self {
                Self(Uuid::new_v4())
            }

            /// Wraps an existing UUID.
            #[must_use]
            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            /// Returns the raw UUID.
            #[must_use]
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

uuid_id!(TurnId);
uuid_id!(InputId);
uuid_id!(RunId);
uuid_id!(EventId);
uuid_id!(BindingId);

/// Non-empty provider-local identity subject.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProviderSubject(String);

impl ProviderSubject {
    /// Creates a non-empty provider subject.
    pub fn new(value: impl Into<String>) -> LifeResult<Self> {
        let value = value.into().trim().to_owned();
        if value.is_empty() {
            Err(LifeDomainError::EmptyField {
                field: "provider_subject",
            })
        } else {
            Ok(Self(value))
        }
    }

    /// Returns the canonical subject string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderSubject {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}
