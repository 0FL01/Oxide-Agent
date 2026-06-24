//! Storage repository contracts.

use crate::domain::{ActiveMemoryGeneration, MemoryScope, PrincipalUserId};

/// Minimal repository boundary shared by future storage services.
pub trait LifeGenerationReader {
    /// Returns the active memory generation for a principal.
    fn active_generation(
        &self,
        principal_user_id: PrincipalUserId,
    ) -> Option<ActiveMemoryGeneration>;

    /// Converts a principal id to the mandatory active memory scope.
    fn active_memory_scope(&self, principal_user_id: PrincipalUserId) -> Option<MemoryScope> {
        self.active_generation(principal_user_id)
            .map(|active| active.scope)
    }
}
