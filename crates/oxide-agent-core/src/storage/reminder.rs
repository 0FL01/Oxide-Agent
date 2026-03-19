use super::StorageError;
use chrono::{TimeZone, Utc};
use chrono_tz::Tz;
use cron::Schedule;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Thread routing kind persisted for reminder delivery.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReminderThreadKind {
    /// Direct/private chat context.
    Dm,
    /// Telegram forum topic context.
    Forum,
    /// Non-threaded group or fallback context.
    None,
}

/// Reminder schedule kind persisted in storage.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReminderScheduleKind {
    /// Execute once and then finish.
    Once,
    /// Execute repeatedly using a fixed interval.
    Interval,
    /// Execute on a cron schedule in a specific timezone.
    Cron,
}

/// Reminder lifecycle status.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReminderJobStatus {
    /// Reminder is scheduled and may be claimed by a worker.
    Scheduled,
    /// Reminder is temporarily paused and should not be executed.
    Paused,
    /// Reminder completed successfully and will not run again.
    Completed,
    /// Reminder was cancelled by the user.
    Cancelled,
    /// Reminder stopped after an unrecoverable error.
    Failed,
}

/// Reminder job metadata persisted in control-plane storage.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ReminderJobRecord {
    /// Record schema version for forward-compatible evolution.
    pub schema_version: u32,
    /// Logical record revision incremented on each mutation.
    pub version: u64,
    /// Stable reminder identifier.
    pub reminder_id: String,
    /// User owning this reminder.
    pub user_id: i64,
    /// Transport context key the reminder belongs to.
    pub context_key: String,
    /// Stable agent flow identifier used for wake-up execution.
    pub flow_id: String,
    /// Destination Telegram chat identifier.
    pub chat_id: i64,
    /// Destination Telegram thread/topic identifier when available.
    pub thread_id: Option<i64>,
    /// Logical thread kind required to reconstruct delivery routing.
    pub thread_kind: ReminderThreadKind,
    /// Agent task to execute on wake-up.
    pub task_prompt: String,
    /// Schedule kind.
    pub schedule_kind: ReminderScheduleKind,
    /// Current lifecycle status.
    pub status: ReminderJobStatus,
    /// Next scheduled execution timestamp (unix seconds).
    pub next_run_at: i64,
    /// Fixed interval for recurring reminders (seconds).
    pub interval_secs: Option<u64>,
    /// Cron expression for cron-based reminders.
    pub cron_expression: Option<String>,
    /// IANA timezone identifier for cron-based reminders.
    pub timezone: Option<String>,
    /// Temporary lease expiry while a worker is processing the reminder.
    pub lease_until: Option<i64>,
    /// Last successful or attempted execution timestamp (unix seconds).
    pub last_run_at: Option<i64>,
    /// Last execution error, if any.
    pub last_error: Option<String>,
    /// Number of completed executions.
    pub run_count: u64,
    /// Creation timestamp (unix seconds).
    pub created_at: i64,
    /// Last update timestamp (unix seconds).
    pub updated_at: i64,
}

impl ReminderJobRecord {
    /// Returns true when the reminder is due and available for claiming.
    #[must_use]
    pub fn is_due(&self, now: i64) -> bool {
        self.status == ReminderJobStatus::Scheduled
            && self.next_run_at <= now
            && !self.is_leased(now)
    }

    /// Returns true when the reminder has an active worker lease.
    #[must_use]
    pub fn is_leased(&self, now: i64) -> bool {
        self.lease_until
            .is_some_and(|lease_until| lease_until > now)
    }

    /// Returns true when the reminder repeats indefinitely.
    #[must_use]
    pub const fn is_recurring(&self) -> bool {
        matches!(
            self.schedule_kind,
            ReminderScheduleKind::Interval | ReminderScheduleKind::Cron
        )
    }
}

/// Parameters used when creating a reminder job.
#[derive(Debug, Clone)]
pub struct CreateReminderJobOptions {
    /// User owning this reminder.
    pub user_id: i64,
    /// Transport context key.
    pub context_key: String,
    /// Agent flow identifier used for wake-up execution.
    pub flow_id: String,
    /// Destination chat identifier.
    pub chat_id: i64,
    /// Destination thread/topic identifier.
    pub thread_id: Option<i64>,
    /// Delivery thread kind.
    pub thread_kind: ReminderThreadKind,
    /// Task the agent must execute on wake-up.
    pub task_prompt: String,
    /// Schedule kind.
    pub schedule_kind: ReminderScheduleKind,
    /// First execution timestamp (unix seconds).
    pub next_run_at: i64,
    /// Fixed interval for recurring reminders (seconds).
    pub interval_secs: Option<u64>,
    /// Cron expression for cron-based reminders.
    pub cron_expression: Option<String>,
    /// IANA timezone identifier for cron-based reminders.
    pub timezone: Option<String>,
}

/// Parse a reminder timezone identifier.
///
/// Falls back to `UTC` when not specified.
pub fn parse_reminder_timezone(timezone: Option<&str>) -> Result<Tz, StorageError> {
    let timezone = timezone.unwrap_or("UTC").trim();
    Tz::from_str(timezone).map_err(|error| {
        StorageError::InvalidInput(format!("invalid reminder timezone '{timezone}': {error}"))
    })
}

/// Compute the next cron wake-up timestamp after `after_unix`.
pub fn compute_cron_next_run_at(
    cron_expression: &str,
    timezone: Option<&str>,
    after_unix: i64,
) -> Result<i64, StorageError> {
    let timezone = parse_reminder_timezone(timezone)?;
    let schedule = Schedule::from_str(cron_expression).map_err(|error| {
        StorageError::InvalidInput(format!(
            "invalid reminder cron expression '{cron_expression}': {error}"
        ))
    })?;
    let after = timezone
        .timestamp_opt(after_unix, 0)
        .single()
        .ok_or_else(|| {
            StorageError::InvalidInput(format!(
                "cannot resolve reminder timestamp {after_unix} in timezone {timezone}"
            ))
        })?;
    schedule
        .after(&after)
        .next()
        .map(|next| next.with_timezone(&Utc).timestamp())
        .ok_or_else(|| {
            StorageError::InvalidInput(format!(
                "cron expression '{cron_expression}' does not yield future occurrences"
            ))
        })
}

/// Compute the next recurring execution timestamp for a reminder.
pub fn compute_next_reminder_run_at(
    record: &ReminderJobRecord,
    after_unix: i64,
) -> Result<Option<i64>, StorageError> {
    match record.schedule_kind {
        ReminderScheduleKind::Once => Ok(None),
        ReminderScheduleKind::Interval => {
            let interval_secs = record.interval_secs.ok_or_else(|| {
                StorageError::InvalidInput(format!(
                    "interval reminder '{}' is missing interval_secs",
                    record.reminder_id
                ))
            })?;
            Ok(Some(after_unix.saturating_add(
                i64::try_from(interval_secs).unwrap_or(i64::MAX),
            )))
        }
        ReminderScheduleKind::Cron => {
            let cron_expression = record.cron_expression.as_deref().ok_or_else(|| {
                StorageError::InvalidInput(format!(
                    "cron reminder '{}' is missing cron_expression",
                    record.reminder_id
                ))
            })?;
            Ok(Some(compute_cron_next_run_at(
                cron_expression,
                record.timezone.as_deref(),
                after_unix,
            )?))
        }
    }
}
