//! Memory generation lifecycle contracts.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::{GenerationScoped, MemoryGenerationId, PrincipalUserId, TimestampMillis};

/// Lifecycle status for a memory generation.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryGenerationStatus {
    /// Generation is being built and must not be used by prompt reads.
    Building,
    /// Generation is active for prompt/recall reads.
    Active,
    /// Generation is retained for rollback/audit.
    Archived,
    /// Generation build failed.
    Failed,
    /// Generation was deleted/wiped.
    Deleted,
}

/// Rebuildable memory generation row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LifeMemoryGeneration {
    /// Generation id.
    pub memory_generation_id: MemoryGenerationId,
    /// Principal owner.
    pub principal_user_id: PrincipalUserId,
    /// Monotonic generation number per principal.
    pub generation_number: i64,
    /// Generation lifecycle status.
    pub status: MemoryGenerationStatus,
    /// Optional source generation.
    pub source_generation_id: Option<MemoryGenerationId>,
    /// Human-readable build reason.
    pub build_reason: String,
    /// Curator/prompt/sensitivity/projection policy metadata.
    pub build_policy: Value,
    /// Transcript/source range and seed memory metadata.
    pub source_scope: Value,
    /// Diff/report against the source generation.
    pub comparison_report: Value,
    /// Activation timestamp.
    pub activated_at: Option<TimestampMillis>,
    /// Creation timestamp.
    pub created_at: TimestampMillis,
    /// Last update timestamp.
    pub updated_at: TimestampMillis,
}

impl GenerationScoped for LifeMemoryGeneration {
    fn principal_user_id(&self) -> PrincipalUserId {
        self.principal_user_id
    }

    fn memory_generation_id(&self) -> MemoryGenerationId {
        self.memory_generation_id
    }
}
