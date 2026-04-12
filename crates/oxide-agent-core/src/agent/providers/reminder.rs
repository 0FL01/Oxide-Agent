//! Reminder scheduling provider.

use crate::agent::provider::ToolProvider;
use crate::llm::ToolDefinition;
use crate::storage::{
    compute_cron_next_run_at, compute_next_reminder_run_at, format_reminder_unix_in_timezone,
    resolve_reminder_local_datetime, AppendAuditEventOptions, CreateReminderJobOptions,
    ReminderJobRecord, ReminderJobStatus, ReminderScheduleKind, ReminderThreadKind,
    StorageProvider,
};
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use chrono::Local;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

const TOOL_REMINDER_SCHEDULE: &str = "reminder_schedule";
const TOOL_REMINDER_LIST: &str = "reminder_list";
const TOOL_REMINDER_CANCEL: &str = "reminder_cancel";
const TOOL_REMINDER_PAUSE: &str = "reminder_pause";
const TOOL_REMINDER_RESUME: &str = "reminder_resume";
const TOOL_REMINDER_RETRY: &str = "reminder_retry";

/// Returns the built-in reminder tool names.
#[must_use]
pub fn reminder_tool_names() -> Vec<String> {
    vec![
        TOOL_REMINDER_SCHEDULE.to_string(),
        TOOL_REMINDER_LIST.to_string(),
        TOOL_REMINDER_CANCEL.to_string(),
        TOOL_REMINDER_PAUSE.to_string(),
        TOOL_REMINDER_RESUME.to_string(),
        TOOL_REMINDER_RETRY.to_string(),
    ]
}

/// Reminder provider context bound to a concrete transport destination.
#[derive(Clone)]
pub struct ReminderContext {
    /// Storage backend used for persistence.
    pub storage: Arc<dyn StorageProvider>,
    /// User owning the reminder job.
    pub user_id: i64,
    /// Transport context key.
    pub context_key: String,
    /// Flow identifier used for wake-up execution.
    pub flow_id: String,
    /// Destination chat identifier.
    pub chat_id: i64,
    /// Destination thread identifier.
    pub thread_id: Option<i64>,
    /// Delivery thread kind.
    pub thread_kind: ReminderThreadKind,
    /// Optional notifier used by transports to maintain an in-memory due queue.
    pub notifier: Option<Arc<dyn ReminderScheduleNotifier>>,
}

/// Reminder change event emitted after successful storage mutations.
#[derive(Debug, Clone)]
pub enum ReminderScheduleEvent {
    /// Insert or replace the latest state for a reminder record.
    Upsert(Box<ReminderJobRecord>),
    /// Remove a reminder record from any in-memory due index.
    Delete {
        /// User owning the reminder.
        user_id: i64,
        /// Stable reminder identifier.
        reminder_id: String,
    },
}

/// Transport hook that tracks reminder mutations outside persistent storage.
#[async_trait]
pub trait ReminderScheduleNotifier: Send + Sync {
    /// Consume a reminder mutation event.
    async fn notify(&self, event: ReminderScheduleEvent);
}

/// Provider that allows the agent to schedule reminder jobs.
pub struct ReminderProvider {
    context: ReminderContext,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
struct ReminderScheduleArgs {
    kind: ReminderScheduleKind,
    task: String,
    date: Option<String>,
    time: Option<String>,
    every_minutes: Option<u64>,
    every_hours: Option<u64>,
    first_date: Option<String>,
    first_time: Option<String>,
    timezone: Option<String>,
    weekdays: Option<Vec<ReminderWeekday>>,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ReminderWeekday {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

impl ReminderWeekday {
    const fn cron_value(self) -> &'static str {
        match self {
            Self::Sun => "Sun",
            Self::Mon => "Mon",
            Self::Tue => "Tue",
            Self::Wed => "Wed",
            Self::Thu => "Thu",
            Self::Fri => "Fri",
            Self::Sat => "Sat",
        }
    }
}

struct CompiledReminderSchedule {
    next_run_at: i64,
    interval_secs: Option<u64>,
    cron_expression: Option<String>,
    timezone: Option<String>,
    preview: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct ReminderListArgs {
    statuses: Option<Vec<ReminderJobStatus>>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct ReminderCancelArgs {
    reminder_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct ReminderPauseArgs {
    reminder_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct ReminderResumeArgs {
    reminder_id: String,
    run_at_unix: Option<i64>,
    delay_secs: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct ReminderRetryArgs {
    reminder_id: String,
    run_at_unix: Option<i64>,
    delay_secs: Option<u64>,
}

impl ReminderProvider {
    /// Create a new reminder provider.
    #[must_use]
    pub const fn new(context: ReminderContext) -> Self {
        Self { context }
    }

    async fn notify_schedule_event(&self, event: ReminderScheduleEvent) {
        if let Some(notifier) = &self.context.notifier {
            notifier.notify(event).await;
        }
    }

    async fn execute_schedule(&self, arguments: &str) -> Result<String> {
        let args: ReminderScheduleArgs = serde_json::from_str(arguments)?;
        let now = now_unix_secs();
        let task_prompt = args.task.trim();
        if task_prompt.is_empty() {
            bail!("task must not be empty");
        }

        let compiled = compile_schedule(&args, now)?;

        let record = self
            .context
            .storage
            .create_reminder_job(CreateReminderJobOptions {
                user_id: self.context.user_id,
                context_key: self.context.context_key.clone(),
                flow_id: self.context.flow_id.clone(),
                chat_id: self.context.chat_id,
                thread_id: self.context.thread_id,
                thread_kind: self.context.thread_kind,
                task_prompt: task_prompt.to_string(),
                schedule_kind: args.kind,
                next_run_at: compiled.next_run_at,
                interval_secs: compiled.interval_secs,
                cron_expression: compiled.cron_expression.clone(),
                timezone: compiled.timezone.clone(),
            })
            .await?;
        self.notify_schedule_event(ReminderScheduleEvent::Upsert(Box::new(record.clone())))
            .await;

        let _ = self
            .context
            .storage
            .append_audit_event(AppendAuditEventOptions {
                user_id: self.context.user_id,
                topic_id: Some(self.context.context_key.clone()),
                agent_id: None,
                action: "reminder_job_scheduled".to_string(),
                payload: json!({
                    "reminder_id": record.reminder_id.clone(),
                    "flow_id": record.flow_id.clone(),
                    "schedule_kind": record.schedule_kind,
                    "next_run_at": record.next_run_at,
                    "interval_secs": record.interval_secs,
                    "cron_expression": record.cron_expression,
                    "timezone": record.timezone,
                }),
            })
            .await;

        Ok(format_reminder_created(&record, &compiled.preview))
    }

    async fn execute_list(&self, arguments: &str) -> Result<String> {
        let args = if arguments.trim().is_empty() {
            ReminderListArgs {
                statuses: None,
                limit: None,
            }
        } else {
            serde_json::from_str(arguments)?
        };

        let limit = args.limit.unwrap_or(20).clamp(1, 100);
        let records = self
            .context
            .storage
            .list_reminder_jobs(
                self.context.user_id,
                Some(self.context.context_key.clone()),
                args.statuses,
                limit,
            )
            .await?;
        if records.is_empty() {
            return Ok("No reminders found for the current topic.".to_string());
        }

        let mut lines = vec![format!("Found {} reminder(s):", records.len())];
        for record in records {
            lines.push(format_reminder_line(&record));
        }
        Ok(lines.join("\n"))
    }

    async fn execute_cancel(&self, arguments: &str) -> Result<String> {
        let args: ReminderCancelArgs = serde_json::from_str(arguments)?;
        let record = self
            .context
            .storage
            .get_reminder_job(self.context.user_id, args.reminder_id.clone())
            .await?
            .ok_or_else(|| anyhow!("reminder '{}' was not found", args.reminder_id))?;
        if record.context_key != self.context.context_key {
            bail!(
                "reminder '{}' does not belong to the current topic",
                args.reminder_id
            );
        }

        let now = now_unix_secs();
        let cancelled = self
            .context
            .storage
            .cancel_reminder_job(self.context.user_id, args.reminder_id.clone(), now)
            .await?
            .ok_or_else(|| anyhow!("reminder '{}' could not be cancelled", args.reminder_id))?;
        let cancelled_id = cancelled.reminder_id.clone();
        self.notify_schedule_event(ReminderScheduleEvent::Upsert(Box::new(cancelled.clone())))
            .await;

        let _ = self
            .context
            .storage
            .append_audit_event(AppendAuditEventOptions {
                user_id: self.context.user_id,
                topic_id: Some(self.context.context_key.clone()),
                agent_id: None,
                action: "reminder_job_cancelled".to_string(),
                payload: json!({
                    "reminder_id": cancelled_id.clone(),
                    "status": cancelled.status,
                }),
            })
            .await;

        Ok(format!(
            "Reminder cancelled. ID: {}. Status: {:?}.",
            cancelled_id, cancelled.status
        ))
    }

    async fn execute_pause(&self, arguments: &str) -> Result<String> {
        let args: ReminderPauseArgs = serde_json::from_str(arguments)?;
        let record = self
            .load_current_topic_reminder(&args.reminder_id)
            .await?
            .ok_or_else(|| anyhow!("reminder '{}' was not found", args.reminder_id))?;
        if record.status != ReminderJobStatus::Scheduled {
            bail!(
                "reminder '{}' cannot be paused from status {:?}",
                args.reminder_id,
                record.status
            );
        }

        let now = now_unix_secs();
        let paused = self
            .context
            .storage
            .pause_reminder_job(self.context.user_id, args.reminder_id.clone(), now)
            .await?
            .ok_or_else(|| anyhow!("reminder '{}' could not be paused", args.reminder_id))?;
        self.notify_schedule_event(ReminderScheduleEvent::Upsert(Box::new(paused.clone())))
            .await;

        self.append_audit(
            "reminder_job_paused",
            json!({
                "reminder_id": paused.reminder_id,
                "status": paused.status,
            }),
        )
        .await;

        Ok(format!(
            "Reminder paused. ID: {}. Status: {:?}.",
            paused.reminder_id, paused.status
        ))
    }

    async fn execute_resume(&self, arguments: &str) -> Result<String> {
        let args: ReminderResumeArgs = serde_json::from_str(arguments)?;
        let record = self
            .load_current_topic_reminder(&args.reminder_id)
            .await?
            .ok_or_else(|| anyhow!("reminder '{}' was not found", args.reminder_id))?;
        if record.status != ReminderJobStatus::Paused {
            bail!(
                "reminder '{}' cannot be resumed from status {:?}",
                args.reminder_id,
                record.status
            );
        }

        let now = now_unix_secs();
        let next_run_at =
            resolve_resume_next_run_at(&record, args.run_at_unix, args.delay_secs, now)?;
        let resumed = self
            .context
            .storage
            .resume_reminder_job(
                self.context.user_id,
                args.reminder_id.clone(),
                next_run_at,
                now,
            )
            .await?
            .ok_or_else(|| anyhow!("reminder '{}' could not be resumed", args.reminder_id))?;
        self.notify_schedule_event(ReminderScheduleEvent::Upsert(Box::new(resumed.clone())))
            .await;

        self.append_audit(
            "reminder_job_resumed",
            json!({
                "reminder_id": resumed.reminder_id,
                "status": resumed.status,
                "next_run_at": resumed.next_run_at,
            }),
        )
        .await;

        Ok(format!(
            "Reminder resumed. ID: {}. Next run at unix {}.",
            resumed.reminder_id, resumed.next_run_at
        ))
    }

    async fn execute_retry(&self, arguments: &str) -> Result<String> {
        let args: ReminderRetryArgs = serde_json::from_str(arguments)?;
        let record = self
            .load_current_topic_reminder(&args.reminder_id)
            .await?
            .ok_or_else(|| anyhow!("reminder '{}' was not found", args.reminder_id))?;
        if record.status != ReminderJobStatus::Failed {
            bail!(
                "reminder '{}' cannot be retried from status {:?}",
                args.reminder_id,
                record.status
            );
        }

        let now = now_unix_secs();
        let next_run_at =
            resolve_retry_next_run_at(&record, args.run_at_unix, args.delay_secs, now)?;
        let retried = self
            .context
            .storage
            .retry_reminder_job(
                self.context.user_id,
                args.reminder_id.clone(),
                next_run_at,
                now,
            )
            .await?
            .ok_or_else(|| anyhow!("reminder '{}' could not be retried", args.reminder_id))?;
        self.notify_schedule_event(ReminderScheduleEvent::Upsert(Box::new(retried.clone())))
            .await;

        self.append_audit(
            "reminder_job_retried",
            json!({
                "reminder_id": retried.reminder_id,
                "status": retried.status,
                "next_run_at": retried.next_run_at,
            }),
        )
        .await;

        Ok(format!(
            "Reminder retried. ID: {}. Next run at unix {}.",
            retried.reminder_id, retried.next_run_at
        ))
    }

    async fn load_current_topic_reminder(
        &self,
        reminder_id: &str,
    ) -> Result<Option<ReminderJobRecord>> {
        let record = self
            .context
            .storage
            .get_reminder_job(self.context.user_id, reminder_id.to_string())
            .await?;
        Ok(record.filter(|record| record.context_key == self.context.context_key))
    }

    async fn append_audit(&self, action: &str, payload: serde_json::Value) {
        let _ = self
            .context
            .storage
            .append_audit_event(AppendAuditEventOptions {
                user_id: self.context.user_id,
                topic_id: Some(self.context.context_key.clone()),
                agent_id: None,
                action: action.to_string(),
                payload,
            })
            .await;
    }
}

#[async_trait]
impl ToolProvider for ReminderProvider {
    fn name(&self) -> &'static str {
        "reminder"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![
            reminder_schedule_definition(),
            reminder_list_definition(),
            reminder_cancel_definition(),
            reminder_pause_definition(),
            reminder_resume_definition(),
            reminder_retry_definition(),
        ]
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        matches!(
            tool_name,
            TOOL_REMINDER_SCHEDULE
                | TOOL_REMINDER_LIST
                | TOOL_REMINDER_CANCEL
                | TOOL_REMINDER_PAUSE
                | TOOL_REMINDER_RESUME
                | TOOL_REMINDER_RETRY
        )
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        _progress_tx: Option<&tokio::sync::mpsc::Sender<crate::agent::progress::AgentEvent>>,
        _cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        match tool_name {
            TOOL_REMINDER_SCHEDULE => self.execute_schedule(arguments).await,
            TOOL_REMINDER_LIST => self.execute_list(arguments).await,
            TOOL_REMINDER_CANCEL => self.execute_cancel(arguments).await,
            TOOL_REMINDER_PAUSE => self.execute_pause(arguments).await,
            TOOL_REMINDER_RESUME => self.execute_resume(arguments).await,
            TOOL_REMINDER_RETRY => self.execute_retry(arguments).await,
            _ => bail!("Unknown reminder tool: {tool_name}"),
        }
    }
}

fn compile_schedule(args: &ReminderScheduleArgs, now: i64) -> Result<CompiledReminderSchedule> {
    match args.kind {
        ReminderScheduleKind::Once => compile_once_schedule(args, now),
        ReminderScheduleKind::Interval => compile_interval_schedule(args, now),
        ReminderScheduleKind::Cron => compile_cron_schedule(args, now),
    }
}

fn compile_once_schedule(
    args: &ReminderScheduleArgs,
    now: i64,
) -> Result<CompiledReminderSchedule> {
    let next_run_at = resolve_once_next_run_at(args, now)?;
    let timezone = effective_timezone(args.timezone.as_deref());
    let preview = format!(
        "One-time reminder. Next run: unix {} ({}).",
        next_run_at,
        format_display_time(next_run_at, timezone.as_deref())?
    );
    Ok(CompiledReminderSchedule {
        next_run_at,
        interval_secs: None,
        cron_expression: None,
        timezone: None,
        preview,
    })
}

fn compile_interval_schedule(
    args: &ReminderScheduleArgs,
    now: i64,
) -> Result<CompiledReminderSchedule> {
    let interval_secs = resolve_interval_secs(args)?;
    let next_run_at = resolve_interval_first_run_at(args, now, interval_secs)?;
    let timezone = effective_timezone(args.timezone.as_deref());
    let second_run_at =
        next_run_at.saturating_add(i64::try_from(interval_secs).unwrap_or(i64::MAX));
    let preview = format!(
        "Fixed-delay reminder every {} seconds. Next runs: unix {} ({}), then unix {} ({}). Use cron for wall-clock schedules like every day at 09:00.",
        interval_secs,
        next_run_at,
        format_display_time(next_run_at, timezone.as_deref())?,
        second_run_at,
        format_display_time(second_run_at, timezone.as_deref())?
    );
    Ok(CompiledReminderSchedule {
        next_run_at,
        interval_secs: Some(interval_secs),
        cron_expression: None,
        timezone: None,
        preview,
    })
}

fn compile_cron_schedule(
    args: &ReminderScheduleArgs,
    now: i64,
) -> Result<CompiledReminderSchedule> {
    let timezone = effective_timezone(args.timezone.as_deref());
    let cron_expression = resolve_cron_expression(args)?;
    let mut after = now;
    let mut previews = Vec::with_capacity(3);
    for _ in 0..3 {
        let next_run_at = compute_cron_next_run_at(&cron_expression, timezone.as_deref(), after)?;
        previews.push(format!(
            "unix {} ({})",
            next_run_at,
            format_display_time(next_run_at, timezone.as_deref())?
        ));
        after = next_run_at;
    }
    let next_run_at = compute_cron_next_run_at(&cron_expression, timezone.as_deref(), now)?;
    let preview = format!(
        "Wall-clock reminder with cron '{}' in timezone {}. Next runs: {}.",
        cron_expression,
        timezone.as_deref().unwrap_or("UTC"),
        previews.join(", ")
    );
    Ok(CompiledReminderSchedule {
        next_run_at,
        interval_secs: None,
        cron_expression: Some(cron_expression),
        timezone,
        preview,
    })
}

fn resolve_once_next_run_at(args: &ReminderScheduleArgs, now: i64) -> Result<i64> {
    let next_run_at = if let Some(local_run_at) = resolve_local_datetime(
        args.date.as_deref(),
        args.time.as_deref(),
        args.timezone.as_deref(),
    )? {
        local_run_at
    } else {
        bail!("one-time reminders require date + time");
    };
    if next_run_at <= now {
        bail!("one-time reminders must be scheduled in the future");
    }
    Ok(next_run_at)
}

fn resolve_interval_secs(args: &ReminderScheduleArgs) -> Result<u64> {
    if let Some(every_hours) = args.every_hours.filter(|hours| *hours > 0) {
        return Ok(every_hours.saturating_mul(3600));
    }
    if let Some(every_minutes) = args.every_minutes.filter(|minutes| *minutes > 0) {
        return Ok(every_minutes.saturating_mul(60));
    }
    bail!("interval reminders require every_minutes or every_hours")
}

fn resolve_interval_first_run_at(
    args: &ReminderScheduleArgs,
    now: i64,
    interval_secs: u64,
) -> Result<i64> {
    let next_run_at = if let Some(local_run_at) = resolve_local_datetime(
        args.first_date.as_deref(),
        args.first_time.as_deref(),
        args.timezone.as_deref(),
    )? {
        local_run_at
    } else {
        now.saturating_add(i64::try_from(interval_secs).unwrap_or(i64::MAX))
    };
    if next_run_at <= now {
        bail!("interval reminders must start in the future");
    }
    Ok(next_run_at)
}

fn resolve_cron_expression(args: &ReminderScheduleArgs) -> Result<String> {
    let time = args
        .time
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("cron reminders require time with an optional weekdays list"))?;
    let (hour, minute, second) = parse_clock_components(time)?;
    let weekday_field = match args.weekdays.as_deref() {
        Some(weekdays) if !weekdays.is_empty() => weekdays
            .iter()
            .map(|weekday| weekday.cron_value())
            .collect::<Vec<_>>()
            .join(","),
        _ => "*".to_string(),
    };
    Ok(format!("{second} {minute} {hour} * * {weekday_field} *"))
}

fn resolve_local_datetime(
    date: Option<&str>,
    time: Option<&str>,
    timezone: Option<&str>,
) -> Result<Option<i64>> {
    let date = date.map(str::trim).filter(|value| !value.is_empty());
    let time = time.map(str::trim).filter(|value| !value.is_empty());
    let tz = effective_timezone(timezone);

    match (date, time) {
        (Some(date), Some(time)) => Ok(Some(resolve_reminder_local_datetime(
            date,
            time,
            tz.as_deref(),
        )?)),
        (None, Some(time)) => {
            // Use today's date when only time is specified
            let today = Local::now().format("%Y-%m-%d").to_string();
            Ok(Some(resolve_reminder_local_datetime(
                &today,
                time,
                tz.as_deref(),
            )?))
        }
        (Some(date), None) => {
            // Use midnight when only date is specified
            Ok(Some(resolve_reminder_local_datetime(
                date,
                "00:00:00",
                tz.as_deref(),
            )?))
        }
        (None, None) => Ok(None),
    }
}

fn parse_clock_components(raw: &str) -> Result<(u32, u32, u32)> {
    let parts = raw.split(':').collect::<Vec<_>>();
    match parts.as_slice() {
        [hour, minute] => Ok((hour.parse()?, minute.parse()?, 0)),
        [hour, minute, second] => Ok((hour.parse()?, minute.parse()?, second.parse()?)),
        _ => bail!("time must use HH:MM or HH:MM:SS format"),
    }
}

fn normalized_timezone(timezone: Option<&str>) -> Option<String> {
    timezone
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn effective_timezone(timezone: Option<&str>) -> Option<String> {
    normalized_timezone(timezone).or_else(|| Some(local_offset_timezone()))
}

fn local_offset_timezone() -> String {
    let seconds = Local::now().offset().local_minus_utc();
    let sign = if seconds >= 0 { '+' } else { '-' };
    let absolute = seconds.unsigned_abs();
    let hours = absolute / 3600;
    let minutes = (absolute % 3600) / 60;
    if minutes == 0 {
        format!("UTC{sign}{hours}")
    } else {
        format!("UTC{sign}{hours:02}:{minutes:02}")
    }
}

fn format_display_time(unix: i64, timezone: Option<&str>) -> Result<String> {
    Ok(format_reminder_unix_in_timezone(unix, timezone)?)
}

fn reminder_schedule_definition() -> ToolDefinition {
    ToolDefinition {
        name: TOOL_REMINDER_SCHEDULE.to_string(),
        description: "Schedule a wake-up task. Prefer date + time for one-time reminders, interval only for fixed-delay repetition, and cron for wall-clock schedules like every day at 09:00 or weekdays at 18:30. The agent will wake up later, execute the task, and post a report in the same topic.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "kind": {
                    "type": "string",
                    "enum": ["once", "interval", "cron"],
                    "description": "Schedule type: one-time, fixed-delay interval, or wall-clock cron"
                },
                "task": {
                    "type": "string",
                    "description": "Task the agent should execute when the reminder wakes up"
                },
                "date": {
                    "type": "string",
                    "description": "Local calendar date in YYYY-MM-DD for one-time reminders. If timezone is omitted, the current local timezone from the prompt is used."
                },
                "time": {
                    "type": "string",
                    "description": "Local wall-clock time in HH:MM or HH:MM:SS. With kind=once use together with date. With kind=cron use it for daily or weekly schedules. If timezone is omitted, the current local timezone from the prompt is used."
                },
                "every_minutes": {
                    "type": "integer",
                    "description": "Fixed-delay reminder cadence in minutes. Use this only for repeat-after-N-minutes schedules, not for every day at 09:00."
                },
                "every_hours": {
                    "type": "integer",
                    "description": "Fixed-delay reminder cadence in hours. Use this only for repeat-after-N-hours schedules, not for every day at 09:00."
                },
                "first_date": {
                    "type": "string",
                    "description": "Optional first local date in YYYY-MM-DD for interval reminders. If omitted, the first run is now plus the interval."
                },
                "first_time": {
                    "type": "string",
                    "description": "Optional first local time in HH:MM or HH:MM:SS for interval reminders. Use together with first_date."
                },
                "timezone": {
                    "type": "string",
                    "description": "Optional timezone for local wall-clock scheduling. Accepts IANA names like Europe/Moscow or offsets like UTC+3. When omitted, the tool uses the current local timezone from the prompt."
                },
                "weekdays": {
                    "type": "array",
                    "items": {
                        "type": "string",
                        "enum": ["mon", "tue", "wed", "thu", "fri", "sat", "sun"]
                    },
                    "description": "Optional weekdays for cron reminders. If omitted and kind=cron with time, the reminder runs every day at that time."
                }
            },
            "required": ["kind", "task"]
        }),
    }
}

fn reminder_list_definition() -> ToolDefinition {
    ToolDefinition {
        name: TOOL_REMINDER_LIST.to_string(),
        description: "List reminder jobs already scheduled for the current topic.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "statuses": {
                    "type": "array",
                    "items": {
                        "type": "string",
                        "enum": ["scheduled", "paused", "completed", "cancelled", "failed"]
                    },
                    "description": "Optional reminder statuses to include"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of reminders to return"
                }
            }
        }),
    }
}

fn reminder_cancel_definition() -> ToolDefinition {
    simple_reminder_id_tool_definition(
        TOOL_REMINDER_CANCEL,
        "Cancel an existing reminder in the current topic by id.",
        "Reminder identifier returned by reminder_schedule or reminder_list",
    )
}

fn reminder_pause_definition() -> ToolDefinition {
    simple_reminder_id_tool_definition(
        TOOL_REMINDER_PAUSE,
        "Pause a scheduled reminder in the current topic without deleting it.",
        "Reminder identifier returned by reminder_schedule or reminder_list",
    )
}

fn reminder_resume_definition() -> ToolDefinition {
    ToolDefinition {
        name: TOOL_REMINDER_RESUME.to_string(),
        description: "Resume a paused reminder. You may optionally override its next run time."
            .to_string(),
        parameters: reminder_override_parameters("Paused reminder identifier"),
    }
}

fn reminder_retry_definition() -> ToolDefinition {
    ToolDefinition {
        name: TOOL_REMINDER_RETRY.to_string(),
        description: "Retry a failed reminder by scheduling it again.".to_string(),
        parameters: reminder_override_parameters("Failed reminder identifier"),
    }
}

fn simple_reminder_id_tool_definition(
    name: &str,
    description: &str,
    id_description: &str,
) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: description.to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "reminder_id": {
                    "type": "string",
                    "description": id_description
                }
            },
            "required": ["reminder_id"]
        }),
    }
}

fn reminder_override_parameters(id_description: &str) -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "reminder_id": {
                "type": "string",
                "description": id_description
            },
            "run_at_unix": {
                "type": "integer",
                "description": "Optional explicit next execution timestamp"
            },
            "delay_secs": {
                "type": "integer",
                "description": "Optional delay before the next execution"
            }
        },
        "required": ["reminder_id"]
    })
}

fn resolve_resume_next_run_at(
    record: &ReminderJobRecord,
    run_at_unix: Option<i64>,
    delay_secs: Option<u64>,
    now: i64,
) -> Result<i64> {
    match resolve_override_next_run_at(run_at_unix, delay_secs, now)? {
        Some(next_run_at) => Ok(next_run_at),
        None if record.next_run_at > now => Ok(record.next_run_at),
        None => compute_next_reminder_run_at(record, now)?.ok_or_else(|| {
            anyhow!(
                "reminder '{}' does not have a recurring schedule to resume",
                record.reminder_id
            )
        }),
    }
}

fn resolve_retry_next_run_at(
    record: &ReminderJobRecord,
    run_at_unix: Option<i64>,
    delay_secs: Option<u64>,
    now: i64,
) -> Result<i64> {
    match resolve_override_next_run_at(run_at_unix, delay_secs, now)? {
        Some(next_run_at) => Ok(next_run_at),
        None => {
            Ok(compute_next_reminder_run_at(record, now)?.unwrap_or_else(|| now.saturating_add(1)))
        }
    }
}

fn resolve_override_next_run_at(
    run_at_unix: Option<i64>,
    delay_secs: Option<u64>,
    now: i64,
) -> Result<Option<i64>> {
    let next_run_at = run_at_unix.or_else(|| {
        delay_secs
            .map(|delay_secs| now.saturating_add(i64::try_from(delay_secs).unwrap_or(i64::MAX)))
    });
    if next_run_at.is_some_and(|next_run_at| next_run_at <= now) {
        bail!("next execution time must be in the future");
    }
    Ok(next_run_at)
}

fn format_reminder_created(record: &ReminderJobRecord, preview: &str) -> String {
    let cadence = match record.schedule_kind {
        ReminderScheduleKind::Once => "one-time".to_string(),
        ReminderScheduleKind::Interval => format!(
            "recurring every {} seconds",
            record.interval_secs.unwrap_or_default()
        ),
        ReminderScheduleKind::Cron => format!(
            "cron '{}' ({})",
            record.cron_expression.as_deref().unwrap_or("?"),
            record.timezone.as_deref().unwrap_or("UTC")
        ),
    };
    format!(
        "Reminder scheduled. ID: {}. Type: {}. Next run at unix {}. {}",
        record.reminder_id, cadence, record.next_run_at, preview
    )
}

fn format_reminder_line(record: &ReminderJobRecord) -> String {
    let interval = record
        .interval_secs
        .map(|value| format!(", interval={}s", value))
        .unwrap_or_default();
    let cron = record
        .cron_expression
        .as_deref()
        .map(|value| format!(", cron={value}"))
        .unwrap_or_default();
    let timezone = record
        .timezone
        .as_deref()
        .map(|value| format!(", timezone={value}"))
        .unwrap_or_default();
    let last_error = record
        .last_error
        .as_deref()
        .map(|value| format!(", last_error={value}"))
        .unwrap_or_default();
    format!(
        "- id={}, status={:?}, kind={:?}, next_run_at={}{}{}{}{}",
        record.reminder_id,
        record.status,
        record.schedule_kind,
        record.next_run_at,
        interval,
        cron,
        timezone,
        last_error
    )
}

fn now_unix_secs() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_secs()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn base_args(kind: ReminderScheduleKind) -> ReminderScheduleArgs {
        ReminderScheduleArgs {
            kind,
            task: "Ping".to_string(),
            date: None,
            time: None,
            every_minutes: None,
            every_hours: None,
            first_date: None,
            first_time: None,
            timezone: None,
            weekdays: None,
        }
    }

    #[test]
    fn resolves_one_time_local_datetime_with_offset_timezone() {
        let mut args = base_args(ReminderScheduleKind::Once);
        args.date = Some("2026-03-24".to_string());
        args.time = Some("09:00".to_string());
        args.timezone = Some("UTC+3".to_string());

        let next_run_at = resolve_once_next_run_at(&args, 0).expect("local datetime should parse");
        let expected = Utc
            .with_ymd_and_hms(2026, 3, 24, 6, 0, 0)
            .single()
            .expect("valid datetime")
            .timestamp();
        assert_eq!(next_run_at, expected);
    }

    #[test]
    fn builds_daily_cron_from_wall_clock_time() {
        let mut args = base_args(ReminderScheduleKind::Cron);
        args.time = Some("09:15".to_string());
        args.timezone = Some("UTC+3".to_string());

        let expression = resolve_cron_expression(&args).expect("cron expression should build");
        assert_eq!(expression, "0 15 9 * * * *");
    }

    #[test]
    fn builds_weekly_cron_from_weekdays() {
        let mut args = base_args(ReminderScheduleKind::Cron);
        args.time = Some("18:30".to_string());
        args.timezone = Some("UTC+3".to_string());
        args.weekdays = Some(vec![ReminderWeekday::Mon, ReminderWeekday::Fri]);

        let expression = resolve_cron_expression(&args).expect("cron expression should build");
        assert_eq!(expression, "0 30 18 * * Mon,Fri *");
    }

    #[test]
    fn resolves_interval_seconds_from_hours() {
        let mut args = base_args(ReminderScheduleKind::Interval);
        args.every_hours = Some(2);

        let interval_secs = resolve_interval_secs(&args).expect("interval should resolve");
        assert_eq!(interval_secs, 7200);
    }

    #[test]
    fn schedule_args_reject_legacy_fields() {
        let parsed = serde_json::from_value::<ReminderScheduleArgs>(serde_json::json!({
            "kind": "once",
            "task": "Ping",
            "run_at_unix": 123
        }));

        assert!(parsed.is_err());
    }

    #[test]
    fn resolves_interval_first_run_at_with_only_time() {
        use chrono::TimeZone;

        let mut args = base_args(ReminderScheduleKind::Interval);
        args.every_hours = Some(24);
        args.first_time = Some("09:00".to_string());
        args.timezone = Some("UTC+3".to_string());

        // Set `now` to yesterday 08:00 UTC (before 09:00 UTC+3)
        let now = Utc
            .with_ymd_and_hms(2026, 3, 23, 5, 0, 0)
            .single()
            .expect("valid date for test")
            .timestamp();

        let interval_secs = resolve_interval_secs(&args).expect("interval should resolve");
        let next_run_at = resolve_interval_first_run_at(&args, now, interval_secs)
            .expect("should resolve with only time");

        // Should use today's date (via Local::now()) with 09:00 UTC+3
        let today = Local::now().format("%Y-%m-%d").to_string();
        let expected = resolve_reminder_local_datetime(&today, "09:00", Some("UTC+3"))
            .expect("valid datetime");
        assert_eq!(next_run_at, expected);
    }

    #[test]
    fn resolves_interval_first_run_at_uses_timezone_offset() {
        use chrono::TimeZone;

        let mut args = base_args(ReminderScheduleKind::Interval);
        args.every_minutes = Some(60);
        args.first_time = Some("12:00".to_string());
        args.timezone = Some("UTC+5".to_string());

        // Set `now` to 2026-03-23 05:00 UTC (before 12:00 UTC+5 = 07:00 UTC)
        let now = Utc
            .with_ymd_and_hms(2026, 3, 23, 5, 0, 0)
            .single()
            .expect("valid date for test")
            .timestamp();

        let interval_secs = resolve_interval_secs(&args).expect("interval should resolve");
        let next_run_at = resolve_interval_first_run_at(&args, now, interval_secs)
            .expect("should resolve with timezone offset");

        // Should use today's date (via Local::now()) with 12:00 UTC+5
        let today = Local::now().format("%Y-%m-%d").to_string();
        let expected = resolve_reminder_local_datetime(&today, "12:00", Some("UTC+5"))
            .expect("valid datetime");
        assert_eq!(next_run_at, expected);
    }

    #[test]
    fn resolve_local_datetime_with_only_time_uses_today_date() {
        let result = resolve_local_datetime(None, Some("09:00"), Some("UTC+3"))
            .expect("should resolve with only time")
            .expect("valid date for test");

        // Should use today's date with the specified time
        let today = Local::now().format("%Y-%m-%d").to_string();
        let expected = resolve_reminder_local_datetime(&today, "09:00", Some("UTC+3"))
            .expect("valid datetime");

        assert_eq!(result, expected);
    }

    #[test]
    fn resolve_local_datetime_with_only_date_uses_midnight() {
        let result = resolve_local_datetime(Some("2026-03-24"), None, Some("UTC+3"))
            .expect("should resolve with only date")
            .expect("valid date for test");

        // Should use midnight with the specified date
        let expected = resolve_reminder_local_datetime("2026-03-24", "00:00:00", Some("UTC+3"))
            .expect("valid datetime");

        assert_eq!(result, expected);
    }
}
