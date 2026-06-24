//! Canonical durable life memory ledger rows.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::{
    GenerationScoped, MemoryGenerationId, MemoryItemId, PrincipalUserId, TimestampMillis, TurnId,
};

/// Canonical memory item category.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryItemKind {
    /// Biographical fact.
    Biography,
    /// User preference.
    Preference,
    /// Project principle.
    ProjectPrinciple,
    /// Procedure or process memory.
    Procedure,
    /// Decision memory.
    Decision,
    /// Episodic memory.
    Episode,
    /// Operating rule.
    OperatingRule,
    /// Friction pattern.
    FrictionPattern,
    /// Support protocol.
    SupportProtocol,
}

/// Memory authority/provenance class.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryAuthority {
    /// User explicitly asserted the memory.
    UserAsserted,
    /// User confirmed a candidate.
    UserConfirmed,
    /// Curator suggested candidate; not authoritative by itself.
    CuratorSuggested,
    /// System-derived state.
    SystemDerived,
}

/// Canonical memory status.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryItemStatus {
    /// Active memory visible to prompt/recall.
    Active,
    /// Superseded by a newer memory.
    Superseded,
    /// Deleted/forgotten.
    Deleted,
    /// Candidate pending confirmation.
    Candidate,
}

/// Sensitivity class for durable memory.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemorySensitivity {
    /// Clean memory.
    Clean,
    /// Personal but allowed memory.
    Personal,
    /// Redacted memory.
    Redacted,
    /// Secret-like content blocked.
    SecretBlocked,
}

/// Canonical durable memory row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LifeMemoryItem {
    /// Memory id.
    pub memory_id: MemoryItemId,
    /// Principal owner.
    pub principal_user_id: PrincipalUserId,
    /// Memory generation owner.
    pub memory_generation_id: MemoryGenerationId,
    /// Memory kind.
    pub kind: MemoryItemKind,
    /// Authority class.
    pub authority: MemoryAuthority,
    /// Item status.
    pub status: MemoryItemStatus,
    /// Canonical memory text.
    pub text: String,
    /// Structured metadata.
    pub structured: Value,
    /// Tags for filtering.
    pub tags: Vec<String>,
    /// Evidence turn ids.
    pub evidence_turn_ids: Vec<TurnId>,
    /// Sensitivity class.
    pub sensitivity: MemorySensitivity,
    /// Valid-time start.
    pub valid_from: Option<TimestampMillis>,
    /// Valid-time end.
    pub valid_to: Option<TimestampMillis>,
    /// Superseded memory id.
    pub supersedes_memory_id: Option<MemoryItemId>,
    /// Creation timestamp.
    pub created_at: TimestampMillis,
    /// Last update timestamp.
    pub updated_at: TimestampMillis,
}

impl GenerationScoped for LifeMemoryItem {
    fn principal_user_id(&self) -> PrincipalUserId {
        self.principal_user_id
    }

    fn memory_generation_id(&self) -> MemoryGenerationId {
        self.memory_generation_id
    }
}
