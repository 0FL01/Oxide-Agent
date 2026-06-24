//! Life prompt-context assembly contracts.

use serde::{Deserialize, Serialize};

use crate::domain::{MemoryScope, PrincipalUserId};

/// Request to assemble life prompt context for a principal/run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifeContextRequest {
    /// Principal owner.
    pub principal_user_id: PrincipalUserId,
    /// Active memory generation scope.
    pub memory_scope: MemoryScope,
    /// Optional project key to bias task resume lookup.
    pub project_key: Option<String>,
    /// User query/task text used for recall and relevance filtering.
    pub query: String,
}

/// PRD-defined prompt block names emitted by the future life context provider.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LifeContextBlockKind {
    /// Life defaults/profile block.
    LifeDefaults,
    /// Confirmed AuDHD operating contract.
    OperatingContract,
    /// Current task resume/open-loop block.
    CurrentTaskResume,
    /// Temporary active overrides.
    ActiveOverrides,
    /// Support protocols and friction patterns.
    SupportProtocols,
    /// Hot checkpoint handoff.
    HotHandoff,
    /// Long-term memory evidence.
    LongTermMemoryEvidence,
}
