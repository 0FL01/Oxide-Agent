use super::AuditEventRecord;
#[cfg(feature = "storage-s3-r2")]
use aws_sdk_s3::error::SdkError;
#[cfg(feature = "storage-s3-r2")]
use aws_sdk_s3::operation::put_object::PutObjectError;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, OwnedMutexGuard};

#[allow(dead_code)]
pub(crate) const CONTROL_PLANE_RMW_MAX_RETRIES: usize = 5;
#[allow(dead_code)]
pub(crate) const CONTROL_PLANE_RMW_RETRY_BACKOFF_MS: u64 = 25;

/// Process-local per-key lock registry for control-plane RMW operations.
///
/// Limitation: this lock only serializes operations inside a single process.
/// It does not provide cross-process or cross-instance mutual exclusion.
#[derive(Default)]
#[allow(dead_code)]
pub(super) struct ControlPlaneLocks {
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl ControlPlaneLocks {
    #[allow(dead_code)]
    pub(super) fn new() -> Self {
        Self::default()
    }

    #[allow(dead_code)]
    pub(super) async fn acquire(&self, key: String) -> OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self.locks.lock().await;
            Arc::clone(locks.entry(key).or_insert_with(|| Arc::new(Mutex::new(()))))
        };

        lock.lock_owned().await
    }
}

#[must_use]
#[allow(dead_code)]
pub(crate) fn select_audit_events_page(
    events: Vec<AuditEventRecord>,
    before_version: Option<u64>,
    limit: usize,
) -> Vec<AuditEventRecord> {
    events
        .into_iter()
        .rev()
        .filter(|event| before_version.is_none_or(|cursor| event.version < cursor))
        .take(limit)
        .collect()
}

#[must_use]
#[allow(dead_code)]
pub(crate) fn should_retry_control_plane_rmw(attempt: usize) -> bool {
    attempt < CONTROL_PLANE_RMW_MAX_RETRIES
}

#[must_use]
pub(crate) fn current_timestamp_unix_secs() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs() as i64,
        Err(_) => 0,
    }
}

#[must_use]
#[cfg(feature = "storage-s3-r2")]
pub(crate) fn is_precondition_failed_put_error(err: &SdkError<PutObjectError>) -> bool {
    match err {
        SdkError::ServiceError(service_err) => service_err.raw().status().as_u16() == 412,
        _ => false,
    }
}
