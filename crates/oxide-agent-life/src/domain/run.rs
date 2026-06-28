//! Life run lifecycle records.

use serde::{Deserialize, Serialize};

use crate::domain::{PrincipalUserId, RunId, TimestampMillis};

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

/// Life run record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifeRun {
    /// Run id.
    pub run_id: RunId,
    /// Principal owner.
    pub principal_user_id: PrincipalUserId,
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
