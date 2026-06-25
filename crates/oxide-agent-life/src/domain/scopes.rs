//! Scoping contracts that prevent stale memory generations entering prompt context.

use serde::{Deserialize, Serialize};

use crate::domain::{MemoryGenerationId, PrincipalUserId, TimestampMillis};
use crate::errors::{LifeDomainError, LifeResult};

/// Active memory scope for every rebuildable memory read.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MemoryScope {
    /// Canonical principal.
    pub principal_user_id: PrincipalUserId,
    /// Active memory generation.
    pub memory_generation_id: MemoryGenerationId,
}

impl MemoryScope {
    /// Creates a memory scope.
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

/// Active pointer row for a principal.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveMemoryGeneration {
    /// Active read scope.
    pub scope: MemoryScope,
    /// Timestamp at which this generation became active.
    pub activated_at: TimestampMillis,
}

/// Trait implemented by rows that are generation-owned.
pub trait GenerationScoped {
    /// Principal that owns this row.
    fn principal_user_id(&self) -> PrincipalUserId;

    /// Memory generation that owns this row.
    fn memory_generation_id(&self) -> MemoryGenerationId;

    /// Verifies that this row belongs to the active scope.
    fn assert_in_scope(&self, scope: &MemoryScope) -> LifeResult<()> {
        let actual_principal = self.principal_user_id();
        if actual_principal != scope.principal_user_id {
            return Err(LifeDomainError::PrincipalMismatch {
                expected: scope.principal_user_id,
                actual: actual_principal,
            });
        }

        let actual_generation = self.memory_generation_id();
        if actual_generation != scope.memory_generation_id {
            return Err(LifeDomainError::GenerationMismatch {
                expected: scope.memory_generation_id,
                actual: actual_generation,
            });
        }

        Ok(())
    }
}
