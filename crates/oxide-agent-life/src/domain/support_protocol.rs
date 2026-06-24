//! Reusable support protocols for life-mode prompt context.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::{
    GenerationScoped, MemoryAuthority, MemoryGenerationId, PrincipalUserId, SupportProtocolId,
    SupportStateStatus, TimestampMillis, TurnId,
};

/// User-confirmed support protocol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LifeSupportProtocol {
    /// Protocol id.
    pub protocol_id: SupportProtocolId,
    /// Principal owner.
    pub principal_user_id: PrincipalUserId,
    /// Memory generation owner.
    pub memory_generation_id: MemoryGenerationId,
    /// Protocol name.
    pub name: String,
    /// Trigger descriptor.
    pub trigger_descriptor: String,
    /// Ordered protocol steps.
    pub steps: Value,
    /// Higher values sort earlier within relevant protocols.
    pub priority: i32,
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

impl GenerationScoped for LifeSupportProtocol {
    fn principal_user_id(&self) -> PrincipalUserId {
        self.principal_user_id
    }

    fn memory_generation_id(&self) -> MemoryGenerationId {
        self.memory_generation_id
    }
}
