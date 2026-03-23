use super::r2_base::ControlPlaneLocks;
use aws_sdk_s3::Client;
use moka::future::Cache;
use std::sync::Arc;

/// Reference to a persisted topic-scoped agent memory record in R2 storage.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PersistedAgentMemoryRef {
    /// User owning the memory record.
    pub user_id: i64,
    /// Transport context key associated with the memory record.
    pub context_key: String,
    /// Optional flow identifier when the memory belongs to a detached flow.
    pub flow_id: Option<String>,
}

/// R2-backed storage implementation
pub struct R2Storage {
    pub(super) client: Client,
    pub(super) bucket: String,
    pub(super) cache: Cache<String, Arc<Vec<u8>>>,
    pub(super) control_plane_locks: ControlPlaneLocks,
}
