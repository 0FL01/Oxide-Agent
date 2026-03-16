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

/// Returns the built-in reminder tool names.
#[must_use]
pub fn reminder_tool_names() -> Vec<String> {
    vec![
        TOOL_REMINDER_SCHEDULE.to_string(),
        TOOL_REMINDER_LIST.to_string(),
        TOOL_REMINDER_CANCEL.to_string(),
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
}

#[async_trait]
impl ToolProvider for ReminderProvider {
    fn name(&self) -> &'static str {
        "reminder"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![
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
            },
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
                                "enum": ["scheduled", "completed", "cancelled", "failed"]
                            },
                            "description": "Optional reminder statuses to include"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of reminders to return"
                        }
                    }
                }),
            },
            ToolDefinition {
                name: TOOL_REMINDER_CANCEL.to_string(),
                description: "Cancel an existing reminder in the current topic by id.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "reminder_id": {
                            "type": "string",
                            "description": "Reminder identifier returned by reminder_schedule or reminder_list"
                        }
                    },
                    "required": ["reminder_id"]
                }),
            },
        ]
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        matches!(
            tool_name,
            TOOL_REMINDER_SCHEDULE | TOOL_REMINDER_LIST | TOOL_REMINDER_CANCEL
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
