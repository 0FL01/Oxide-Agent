//! Life run lifecycle records.

use serde::{Deserialize, Serialize};

use crate::domain::{
    GenerationScoped, MemoryGenerationId, PrincipalUserId, RunId, TimestampMillis,
};

/// Life run status.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LifeRunStatus {
    /// Run exists but has not started.
    Queued,
    /// Run is executing.
    Running,
    /// Run completed successfully.
    Completed,
    /// Run failed.
    Failed,
    /// Run was cancelled.
    Cancelled,
}

/// Life run record. The generation id is the generation under which prompt context was built.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifeRun {
    /// Run id.
    pub run_id: RunId,
    /// Principal owner.
    pub principal_user_id: PrincipalUserId,
    /// Memory generation used by this run.
    pub memory_generation_id: MemoryGenerationId,
    /// Run status.
    pub status: LifeRunStatus,
    /// Start timestamp.
    pub started_at: Option<TimestampMillis>,
    /// Finish timestamp.
    pub finished_at: Option<TimestampMillis>,
    /// Last checkpoint timestamp.
    pub last_checkpoint_at: Option<TimestampMillis>,
    /// Error text for failed runs.
    pub error_text: Option<String>,
    /// Creation timestamp.
    pub created_at: TimestampMillis,
    /// Last update timestamp.
    pub updated_at: TimestampMillis,
}

impl GenerationScoped for LifeRun {
    fn principal_user_id(&self) -> PrincipalUserId {
        self.principal_user_id
    }

    fn memory_generation_id(&self) -> MemoryGenerationId {
        self.memory_generation_id
    }
}
