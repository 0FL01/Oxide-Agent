use super::r2_base::ControlPlaneLocks;
use aws_sdk_s3::Client;
use moka::future::Cache;
use std::sync::Arc;

/// R2-backed storage implementation
pub struct R2Storage {
    pub(super) client: Client,
    pub(super) bucket: String,
    pub(super) cache: Cache<String, Arc<Vec<u8>>>,
    pub(super) control_plane_locks: ControlPlaneLocks,
}
