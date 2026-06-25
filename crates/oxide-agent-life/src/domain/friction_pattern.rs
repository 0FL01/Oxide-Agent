//! User-confirmed friction patterns for AuDHD support.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::{
    FrictionPatternId, GenerationScoped, MemoryAuthority, MemoryGenerationId, PrincipalUserId,
    TimestampMillis, TurnId,
};

/// Friction pattern kind.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FrictionPatternKind {
    /// Overload trigger.
    OverloadTrigger,
    /// Task initiation barrier.
    TaskInitiationBarrier,
    /// Context loss pattern.
    ContextLoss,
    /// Communication mismatch.
    CommunicationMismatch,
    /// Sensory or energy constraint.
    SensoryOrEnergyConstraint,
}

/// Lifecycle status for friction/protocol rows.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SupportStateStatus {
    /// Active and eligible for prompt context.
    Active,
    /// Superseded by another row.
    Superseded,
    /// Deleted/forgotten.
    Deleted,
    /// Candidate pending confirmation.
    Candidate,
}

/// Concrete friction pattern for this user.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LifeFrictionPattern {
    /// Pattern id.
    pub pattern_id: FrictionPatternId,
    /// Principal owner.
    pub principal_user_id: PrincipalUserId,
    /// Memory generation owner.
    pub memory_generation_id: MemoryGenerationId,
    /// Pattern kind.
    pub kind: FrictionPatternKind,
    /// Trigger descriptor.
    pub trigger_descriptor: String,
    /// Preferred response payload.
    pub preferred_response: Value,
    /// Evidence turn ids.
    pub evidence_turn_ids: Vec<TurnId>,
    /// Authority class.
    pub authority: MemoryAuthority,
    /// Lifecycle status.
    pub status: SupportStateStatus,
    /// Creation timestamp.
    pub created_at: TimestampMillis,
    /// Last update timestamp.
    pub updated_at: TimestampMillis,
}

impl GenerationScoped for LifeFrictionPattern {
    fn principal_user_id(&self) -> PrincipalUserId {
        self.principal_user_id
    }

    fn memory_generation_id(&self) -> MemoryGenerationId {
        self.memory_generation_id
    }
}
