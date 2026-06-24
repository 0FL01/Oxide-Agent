//! DB-backed life worker contracts.

use serde::{Deserialize, Serialize};

use crate::domain::{InputId, MemoryScope, PrincipalUserId, RunId};

/// Command to process a queued principal input.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessPrincipalInput {
    /// Principal owner.
    pub principal_user_id: PrincipalUserId,
    /// Input id to process.
    pub input_id: InputId,
}

/// Claimed run context after the worker has loaded the active generation.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimedLifeRun {
    /// Run id.
    pub run_id: RunId,
    /// Active scope for memory reads in this run.
    pub memory_scope: MemoryScope,
}
