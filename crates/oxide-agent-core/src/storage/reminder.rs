use super::StorageError;
use chrono::{FixedOffset, LocalResult, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use cron::Schedule;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReminderTimezoneKind {
    Named(Tz),
    Fixed(FixedOffset),
}

/// Parsed reminder timezone used for cron and local wall-clock scheduling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReminderTimezone {
    kind: ReminderTimezoneKind,
    label: String,
}

impl ReminderTimezone {
    /// Returns the original timezone label.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.label
    }

    fn localize(&self, unix: i64) -> Result<String, StorageError> {
        match &self.kind {
            ReminderTimezoneKind::Named(named) => named
                .timestamp_opt(unix, 0)
                .single()
                .map(|value| value.format("%Y-%m-%d %H:%M:%S %Z").to_string())
                .ok_or_else(|| {
                    StorageError::InvalidInput(format!(
                        "cannot resolve reminder timestamp {unix} in timezone {}",
                        self.name()
                    ))
                }),
            ReminderTimezoneKind::Fixed(offset) => offset
                .timestamp_opt(unix, 0)
                .single()
                .map(|value| value.format("%Y-%m-%d %H:%M:%S %:z").to_string())
                .ok_or_else(|| {
                    StorageError::InvalidInput(format!(
                        "cannot resolve reminder timestamp {unix} in timezone {}",
                        self.name()
                    ))
                }),
        }
    }

    fn local_datetime_to_unix(
        &self,
        date: NaiveDate,
        time: NaiveTime,
    ) -> Result<i64, StorageError> {
        let local = date.and_time(time);
        match &self.kind {
            ReminderTimezoneKind::Named(named) => match named.from_local_datetime(&local) {
                LocalResult::Single(value) => Ok(value.with_timezone(&Utc).timestamp()),
                LocalResult::Ambiguous(first, second) => {
                    Ok(first.min(second).with_timezone(&Utc).timestamp())
                }
                LocalResult::None => Err(StorageError::InvalidInput(format!(
                    "cannot resolve local reminder time {} {} in timezone {}",
                    date,
                    time.format("%H:%M:%S"),
                    self.name()
                ))),
            },
            ReminderTimezoneKind::Fixed(offset) => match offset.from_local_datetime(&local) {
                LocalResult::Single(value) => Ok(value.with_timezone(&Utc).timestamp()),
                LocalResult::Ambiguous(first, second) => {
                    Ok(first.min(second).with_timezone(&Utc).timestamp())
                }
                LocalResult::None => Err(StorageError::InvalidInput(format!(
                    "cannot resolve local reminder time {} {} in timezone {}",
                    date,
                    time.format("%H:%M:%S"),
                    self.name()
                ))),
            },
        }
    }
}

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
/// Falls back to `UTC` when not specified. Accepts IANA names such as
/// `Europe/Moscow` and fixed UTC offsets such as `UTC+3` or `+03:00`.
pub fn parse_reminder_timezone(timezone: Option<&str>) -> Result<ReminderTimezone, StorageError> {
    let timezone = timezone.unwrap_or("UTC").trim();
    if let Ok(named) = Tz::from_str(timezone) {
        return Ok(ReminderTimezone {
            kind: ReminderTimezoneKind::Named(named),
            label: timezone.to_string(),
        });
    }

    if let Some(offset) = parse_fixed_offset(timezone) {
        return Ok(ReminderTimezone {
            kind: ReminderTimezoneKind::Fixed(offset),
            label: timezone.to_string(),
        });
    }

    Err(StorageError::InvalidInput(format!(
        "invalid reminder timezone '{timezone}'"
    )))
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
    match &timezone.kind {
        ReminderTimezoneKind::Named(named) => {
            let after = named.timestamp_opt(after_unix, 0).single().ok_or_else(|| {
                StorageError::InvalidInput(format!(
                    "cannot resolve reminder timestamp {after_unix} in timezone {}",
                    timezone.name()
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
        ReminderTimezoneKind::Fixed(offset) => {
            let after = offset
                .timestamp_opt(after_unix, 0)
                .single()
                .ok_or_else(|| {
                    StorageError::InvalidInput(format!(
                        "cannot resolve reminder timestamp {after_unix} in timezone {}",
                        timezone.name()
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
    }
}

/// Convert a local calendar date/time into a unix timestamp using reminder timezone rules.
pub fn resolve_reminder_local_datetime(
    date: &str,
    time: &str,
    timezone: Option<&str>,
) -> Result<i64, StorageError> {
    let timezone = parse_reminder_timezone(timezone)?;
    let date = NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d").map_err(|error| {
        StorageError::InvalidInput(format!("invalid reminder date '{}': {error}", date.trim()))
    })?;
    let time = parse_reminder_clock_time(time)?;
    timezone.local_datetime_to_unix(date, time)
}

/// Format a unix timestamp in reminder timezone.
pub fn format_reminder_unix_in_timezone(
    unix: i64,
    timezone: Option<&str>,
) -> Result<String, StorageError> {
    parse_reminder_timezone(timezone)?.localize(unix)
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

fn parse_fixed_offset(raw: &str) -> Option<FixedOffset> {
    let trimmed = raw.trim();
    let candidate = trimmed
        .strip_prefix("UTC")
        .or_else(|| trimmed.strip_prefix("utc"))
        .unwrap_or(trimmed)
        .trim();
    if candidate == "Z" {
        return FixedOffset::east_opt(0);
    }
    let mut chars = candidate.chars();
    let sign = match chars.next()? {
        '+' => 1,
        '-' => -1,
        _ => return None,
    };
    let rest = chars.as_str();
    let (hours, minutes) = if let Some((hours, minutes)) = rest.split_once(':') {
        (hours.parse::<i32>().ok()?, minutes.parse::<i32>().ok()?)
    } else if rest.len() <= 2 {
        (rest.parse::<i32>().ok()?, 0)
    } else if rest.len() == 4 {
        let (hours, minutes) = rest.split_at(2);
        (hours.parse::<i32>().ok()?, minutes.parse::<i32>().ok()?)
    } else {
        return None;
    };
    if hours > 23 || minutes > 59 {
        return None;
    }
    let seconds = sign * ((hours * 3600) + (minutes * 60));
    FixedOffset::east_opt(seconds)
}

fn parse_reminder_clock_time(raw: &str) -> Result<NaiveTime, StorageError> {
    let trimmed = raw.trim();
    NaiveTime::parse_from_str(trimmed, "%H:%M:%S")
        .or_else(|_| NaiveTime::parse_from_str(trimmed, "%H:%M"))
        .map_err(|error| {
            StorageError::InvalidInput(format!("invalid reminder time '{}': {error}", trimmed))
        })
}
