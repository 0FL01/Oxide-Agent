//! Reminder scheduling provider.

use crate::agent::provider::ToolProvider;
use crate::llm::ToolDefinition;
use crate::storage::{
    AppendAuditEventOptions, CreateReminderJobOptions, ReminderJobRecord, ReminderJobStatus,
    ReminderScheduleKind, ReminderThreadKind, StorageProvider,
};
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
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
}

/// Provider that allows the agent to schedule reminder jobs.
pub struct ReminderProvider {
    context: ReminderContext,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct ReminderScheduleArgs {
    kind: ReminderScheduleKind,
    task: String,
    run_at_unix: Option<i64>,
    delay_secs: Option<u64>,
    interval_secs: Option<u64>,
    first_run_at_unix: Option<i64>,
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

    async fn execute_schedule(&self, arguments: &str) -> Result<String> {
        let args: ReminderScheduleArgs = serde_json::from_str(arguments)?;
        let now = now_unix_secs();
        let task_prompt = args.task.trim();
        if task_prompt.is_empty() {
            bail!("task must not be empty");
        }

        let (next_run_at, interval_secs) = match args.kind {
            ReminderScheduleKind::Once => {
                let next_run_at = resolve_once_next_run_at(&args, now)?;
                (next_run_at, None)
            }
            ReminderScheduleKind::Interval => {
                let interval_secs = args
                    .interval_secs
                    .filter(|interval| *interval > 0)
                    .ok_or_else(|| {
                        anyhow!("interval_secs must be provided for interval reminders")
                    })?;
                let next_run_at = args.first_run_at_unix.unwrap_or_else(|| {
                    now.saturating_add(i64::try_from(interval_secs).unwrap_or(i64::MAX))
                });
                if next_run_at <= now {
                    bail!("first_run_at_unix must be in the future");
                }
                (next_run_at, Some(interval_secs))
            }
        };

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
                next_run_at,
                interval_secs,
            })
            .await?;

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
                }),
            })
            .await;

        Ok(format_reminder_created(&record))
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
        let next_run_at = resolve_retry_next_run_at(args.run_at_unix, args.delay_secs, now)?;
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

fn resolve_once_next_run_at(args: &ReminderScheduleArgs, now: i64) -> Result<i64> {
    let from_delay = args
        .delay_secs
        .map(|delay_secs| now.saturating_add(i64::try_from(delay_secs).unwrap_or(i64::MAX)));
    let next_run_at = args.run_at_unix.or(from_delay).ok_or_else(|| {
        anyhow!("run_at_unix or delay_secs must be provided for one-time reminders")
    })?;
    if next_run_at <= now {
        bail!("one-time reminders must be scheduled in the future");
    }
    Ok(next_run_at)
}

fn reminder_schedule_definition() -> ToolDefinition {
    ToolDefinition {
        name: TOOL_REMINDER_SCHEDULE.to_string(),
        description: "Schedule a one-time or recurring wake-up task. The agent will wake up later, execute the task, and post a report to the user in the same topic.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "kind": {
                    "type": "string",
                    "enum": ["once", "interval"],
                    "description": "Schedule type: one-time or recurring interval"
                },
                "task": {
                    "type": "string",
                    "description": "Task the agent should execute when the reminder wakes up"
                },
                "run_at_unix": {
                    "type": "integer",
                    "description": "Unix timestamp for a one-time reminder"
                },
                "delay_secs": {
                    "type": "integer",
                    "description": "Alternative to run_at_unix for one-time reminders"
                },
                "interval_secs": {
                    "type": "integer",
                    "description": "Repeat interval in seconds for recurring reminders"
                },
                "first_run_at_unix": {
                    "type": "integer",
                    "description": "Optional first execution timestamp for recurring reminders; defaults to now + interval_secs"
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
        None => {
            if record.next_run_at > now {
                Ok(record.next_run_at)
            } else if let Some(interval_secs) = record.interval_secs {
                Ok(now.saturating_add(i64::try_from(interval_secs).unwrap_or(i64::MAX)))
            } else {
                Ok(now.saturating_add(1))
            }
        }
    }
}

fn resolve_retry_next_run_at(
    run_at_unix: Option<i64>,
    delay_secs: Option<u64>,
    now: i64,
) -> Result<i64> {
    Ok(resolve_override_next_run_at(run_at_unix, delay_secs, now)?
        .unwrap_or_else(|| now.saturating_add(1)))
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

fn format_reminder_created(record: &ReminderJobRecord) -> String {
    let cadence = match record.schedule_kind {
        ReminderScheduleKind::Once => "one-time".to_string(),
        ReminderScheduleKind::Interval => format!(
            "recurring every {} seconds",
            record.interval_secs.unwrap_or_default()
        ),
    };
    format!(
        "Reminder scheduled. ID: {}. Type: {}. Next run at unix {}.",
        record.reminder_id, cadence, record.next_run_at
    )
}

fn format_reminder_line(record: &ReminderJobRecord) -> String {
    let interval = record
        .interval_secs
        .map(|value| format!(", interval={}s", value))
        .unwrap_or_default();
    let last_error = record
        .last_error
        .as_deref()
        .map(|value| format!(", last_error={value}"))
        .unwrap_or_default();
    format!(
        "- id={}, status={:?}, kind={:?}, next_run_at={}{}{}",
        record.reminder_id,
        record.status,
        record.schedule_kind,
        record.next_run_at,
        interval,
        last_error
    )
}

fn now_unix_secs() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_secs()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}
