//! Derived Engram recall/index contracts.

use serde::{Deserialize, Serialize};

use crate::domain::{MemoryGenerationId, MemoryItemId, PrincipalUserId};

/// Derived Engram namespace. It is scoped by principal and memory generation.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EngramNamespace {
    /// Principal owner.
    pub principal_user_id: PrincipalUserId,
    /// Generation namespace.
    pub memory_generation_id: MemoryGenerationId,
}

impl EngramNamespace {
    /// Creates a namespace for derived recall.
    #[must_use]
    pub const fn new(
        principal_user_id: PrincipalUserId,
        memory_generation_id: MemoryGenerationId,
    ) -> Self {
        Self {
            principal_user_id,
            memory_generation_id,
        }
    }
}

/// Candidate returned by derived recall. Canonical data must be dereferenced in Postgres.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecallCandidate {
    /// Canonical memory id to dereference.
    pub memory_id: MemoryItemId,
    /// Derived relevance score.
    pub score: f32,
    /// Short explanation/evidence from the backend.
    pub rationale: Option<String>,
}
