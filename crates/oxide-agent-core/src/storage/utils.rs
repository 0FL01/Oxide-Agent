use super::AuditEventRecord;
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::put_object::PutObjectError;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const CONTROL_PLANE_RMW_MAX_RETRIES: usize = 5;
pub(crate) const CONTROL_PLANE_RMW_RETRY_BACKOFF_MS: u64 = 25;

#[must_use]
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
pub(crate) fn is_precondition_failed_put_error(err: &SdkError<PutObjectError>) -> bool {
    match err {
        SdkError::ServiceError(service_err) => service_err.raw().status().as_u16() == 412,
        _ => false,
    }
}
