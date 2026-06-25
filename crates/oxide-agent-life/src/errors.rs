//! Error types for life-mode domain contracts.

use thiserror::Error;

use crate::domain::{MemoryGenerationId, PrincipalUserId};

/// Result alias for life-mode domain and service contracts.
pub type LifeResult<T> = Result<T, LifeDomainError>;

/// Domain-level errors before storage/provider-specific errors are introduced.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LifeDomainError {
    /// A required identifier or descriptor was empty.
    #[error("life field '{field}' must not be empty")]
    EmptyField { field: &'static str },
    /// Principal ids are internal positive `users.user_id` values.
    #[error("life principal_user_id must be positive, got {0}")]
    InvalidPrincipalUserId(i64),
    /// A row from another principal was used with this principal scope.
    #[error("life principal mismatch: expected {expected}, got {actual}")]
    PrincipalMismatch {
        expected: PrincipalUserId,
        actual: PrincipalUserId,
    },
    /// A memory-owned row from a stale generation tried to enter the active read path.
    #[error("life memory generation mismatch: expected {expected}, got {actual}")]
    GenerationMismatch {
        expected: MemoryGenerationId,
        actual: MemoryGenerationId,
    },
    /// Unknown wire value for a closed domain enum.
    #[error("unknown {type_name} value '{value}'")]
    UnknownEnumValue {
        type_name: &'static str,
        value: String,
    },
}
