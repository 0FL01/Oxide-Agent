//! Post-run memory curator contracts.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::{MemoryScope, RunId, TurnId};

/// Typed candidate kind produced by the post-run curator.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CuratorCandidateKind {
    /// Durable fact candidate.
    DurableFact,
    /// Project decision candidate.
    ProjectDecision,
    /// Operating profile update candidate.
    OperatingProfileUpdate,
    /// Task state update candidate.
    TaskStateUpdate,
    /// Friction pattern candidate.
    FrictionPattern,
    /// Support protocol candidate.
    SupportProtocol,
    /// Ephemeral observation; do not store durably.
    Ephemeral,
    /// Secret-like candidate requiring deny/redaction.
    SecretCandidate,
    /// Skip candidate.
    Skip,
}

/// Curator output candidate. This is not a source-of-truth write by itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CuratorCandidate {
    /// Active memory scope for any eventual canonical write.
    pub memory_scope: MemoryScope,
    /// Run that produced this candidate.
    pub run_id: RunId,
    /// Candidate kind.
    pub kind: CuratorCandidateKind,
    /// Evidence turns.
    pub evidence_turn_ids: Vec<TurnId>,
    /// Structured payload.
    pub payload: Value,
}
