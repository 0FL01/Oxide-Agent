use super::{
    agent_mode_session_keys, apply_execution_profile, apply_reminder_context,
    apply_topic_infra_config, ensure_session_exists, install_reminder_scheduler,
    is_agent_task_running, manager_control_plane_enabled, manager_default_chat_id,
    renew_cancellation_token, resolve_execution_profile, resolve_topic_infra_config,
    run_agent_task_with_text, use_inline_flow_controls, use_inline_topic_controls,
    EnsureSessionContext, RunAgentTaskTextContext, SessionTransportContext,
};
use crate::bot::context::sandbox_scope;
use crate::bot::topic_route::{touch_dynamic_binding_activity_if_needed, TopicRouteDecision};
use crate::bot::{build_outbound_thread_params, TelegramThreadKind, TelegramThreadSpec};
use crate::config::BotSettings;
use crate::reminder_scheduler::ReminderSchedulerHandle;
use anyhow::{Error, Result};
use oxide_agent_core::agent::SessionId;
use oxide_agent_core::llm::LlmClient;
use oxide_agent_core::storage::{
    compute_next_reminder_run_at, resolve_active_topic_binding, ReminderJobRecord,
    ReminderThreadKind, StorageProvider,
};
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{MessageId, ThreadId};
use tokio::time::{Duration, MissedTickBehavior};
use tracing::warn;

const REMINDER_BATCH_LIMIT: usize = 16;
const REMINDER_LEASE_SECS: i64 = 300;
const REMINDER_BUSY_BACKOFF_SECS: i64 = 30;
const REMINDER_RECONCILE_INTERVAL_SECS: u64 = 300;
const REMINDER_IDLE_WAIT_SECS: u64 = 60;

fn telegram_thread_kind(kind: ReminderThreadKind) -> TelegramThreadKind {
    match kind {
        ReminderThreadKind::Dm => TelegramThreadKind::Dm,
        ReminderThreadKind::Forum => TelegramThreadKind::Forum,
        ReminderThreadKind::None => TelegramThreadKind::None,
    }
}

fn thread_spec_from_reminder(record: &ReminderJobRecord) -> TelegramThreadSpec {
    TelegramThreadSpec::new(
        telegram_thread_kind(record.thread_kind),
        record
            .thread_id
            .and_then(|thread_id| i32::try_from(thread_id).ok())
            .map(|thread_id| ThreadId(MessageId(thread_id))),
    )
}

pub(crate) fn spawn_reminder_scheduler(
    bot: Bot,
    storage: Arc<dyn StorageProvider>,
    llm: Arc<LlmClient>,
    persistent_memory_store: Arc<dyn oxide_agent_core::agent::PersistentMemoryStore>,
    settings: Arc<BotSettings>,
) {
    tokio::spawn(async move {
        let scheduler = Arc::new(ReminderSchedulerHandle::new(
            settings.telegram.agent_allowed_users(),
        ));
        install_reminder_scheduler(scheduler.clone()).await;
        if let Err(error) = scheduler.bootstrap_from_storage(&storage).await {
            warn!(error = %error, "Reminder scheduler bootstrap failed");
        }

        let mut reconcile =
            tokio::time::interval(Duration::from_secs(REMINDER_RECONCILE_INTERVAL_SECS));
        reconcile.set_missed_tick_behavior(MissedTickBehavior::Skip);
        reconcile.tick().await;

        loop {
            if let Err(error) = process_due_reminders(
                &bot,
                &storage,
                &llm,
                &persistent_memory_store,
                &settings,
                &scheduler,
            )
            .await
            {
                warn!(error = %error, "Reminder scheduler due-batch failed");
            }

            let wait_duration = scheduler_wait_duration(&scheduler).await;
            tokio::select! {
                _ = scheduler.wait_for_change() => {}
                _ = tokio::time::sleep(wait_duration) => {}
                _ = reconcile.tick() => {
                    if let Err(error) = scheduler.bootstrap_from_storage(&storage).await {
                        warn!(error = %error, "Reminder scheduler reconcile failed");
                    }
                }
            }
        }
    });
}

async fn scheduler_wait_duration(scheduler: &Arc<ReminderSchedulerHandle>) -> Duration {
    let now = current_timestamp_unix_secs();
    match scheduler.next_due_at().await {
        Some(next_run_at) if next_run_at <= now => Duration::ZERO,
        Some(next_run_at) => Duration::from_secs(next_run_at.saturating_sub(now) as u64),
        None => Duration::from_secs(REMINDER_IDLE_WAIT_SECS),
    }
}

async fn process_due_reminders(
    bot: &Bot,
    storage: &Arc<dyn StorageProvider>,
    llm: &Arc<LlmClient>,
    persistent_memory_store: &Arc<dyn oxide_agent_core::agent::PersistentMemoryStore>,
    settings: &Arc<BotSettings>,
    scheduler: &Arc<ReminderSchedulerHandle>,
) -> Result<()> {
    let now = current_timestamp_unix_secs();
    let reminders = scheduler.take_due_batch(now, REMINDER_BATCH_LIMIT).await;

    for reminder in reminders {
        let user_id = reminder.user_id;
        let reminder_id = reminder.reminder_id.clone();
        if let Err(error) = process_due_reminder(
            bot,
            storage,
            llm,
            persistent_memory_store,
            settings,
            scheduler,
            reminder,
        )
        .await
        {
            warn!(error = %error, reminder_id = %reminder_id, "Failed to execute due reminder");
            if let Err(reconcile_error) = scheduler
                .reconcile_user_from_storage(storage, user_id)
                .await
            {
                warn!(
                    error = %reconcile_error,
                    user_id,
                    "Failed to reconcile reminder scheduler after execution error"
                );
            }
        }
    }

    Ok(())
}

async fn process_due_reminder(
    bot: &Bot,
    storage: &Arc<dyn StorageProvider>,
    llm: &Arc<LlmClient>,
    persistent_memory_store: &Arc<dyn oxide_agent_core::agent::PersistentMemoryStore>,
    settings: &Arc<BotSettings>,
    scheduler: &Arc<ReminderSchedulerHandle>,
    reminder: ReminderJobRecord,
) -> Result<()> {
    let Some(prepared) = prepare_due_reminder_execution(
        bot,
        storage,
        llm,
        persistent_memory_store,
        settings,
        scheduler,
        reminder,
    )
    .await?
    else {
        return Ok(());
    };

    let route = resolve_scheduled_topic_route(
        storage,
        prepared.reminder.user_id,
        settings,
        &prepared.reminder.context_key,
        prepared.chat_id,
        prepared.thread_spec,
    )
    .await;
    let execution_profile = resolve_execution_profile(
        storage,
        prepared.reminder.user_id,
        &prepared.reminder.context_key,
        &route,
        prepared.manager_enabled,
        prepared.thread_spec,
    )
    .await;
    let topic_infra_config = resolve_topic_infra_config(
        storage,
        prepared.reminder.user_id,
        &prepared.reminder.context_key,
    )
    .await;

    apply_execution_profile(prepared.session_id, execution_profile).await;
    apply_topic_infra_config(
        prepared.session_id,
        storage.clone(),
        prepared.reminder.user_id,
        prepared.reminder.context_key.clone(),
        topic_infra_config,
    )
    .await;
    apply_reminder_context(
        prepared.session_id,
        storage.clone(),
        prepared.reminder.user_id,
        prepared.reminder.context_key.clone(),
        prepared.reminder.flow_id.clone(),
        prepared.chat_id,
        prepared.thread_spec,
    )
    .await;
    renew_cancellation_token(prepared.session_id).await;

    let result = run_agent_task_with_text(RunAgentTaskTextContext {
        bot: bot.clone(),
        chat_id: prepared.chat_id,
        session_id: prepared.session_id,
        user_id: prepared.reminder.user_id,
        task_text: scheduled_reminder_task_text(&prepared.reminder),
        storage: storage.clone(),
        context_key: prepared.reminder.context_key.clone(),
        agent_flow_id: prepared.reminder.flow_id.clone(),
        message_thread_id: build_outbound_thread_params(prepared.thread_spec).message_thread_id,
        use_inline_progress_controls: use_inline_topic_controls(prepared.thread_spec),
        use_inline_flow_controls: use_inline_flow_controls(prepared.thread_spec),
    })
    .await;

    finalize_reminder_execution(storage, scheduler, &prepared.reminder, result.as_ref()).await;
    touch_dynamic_binding_activity_if_needed(storage.as_ref(), prepared.reminder.user_id, &route)
        .await;
    result
}

struct PreparedReminderExecution {
    reminder: ReminderJobRecord,
    chat_id: ChatId,
    thread_spec: TelegramThreadSpec,
    session_id: SessionId,
    manager_enabled: bool,
}

async fn prepare_due_reminder_execution(
    bot: &Bot,
    storage: &Arc<dyn StorageProvider>,
    llm: &Arc<LlmClient>,
    persistent_memory_store: &Arc<dyn oxide_agent_core::agent::PersistentMemoryStore>,
    settings: &Arc<BotSettings>,
    scheduler: &Arc<ReminderSchedulerHandle>,
    reminder: ReminderJobRecord,
) -> Result<Option<PreparedReminderExecution>> {
    let now = current_timestamp_unix_secs();
    let Some(reminder) = storage
        .claim_reminder_job(
            reminder.user_id,
            reminder.reminder_id.clone(),
            now.saturating_add(REMINDER_LEASE_SECS),
            now,
        )
        .await?
    else {
        let _ = scheduler
            .reconcile_user_from_storage(storage, reminder.user_id)
            .await;
        return Ok(None);
    };
    scheduler.upsert_record(reminder.clone()).await;

    let chat_id = ChatId(reminder.chat_id);
    let thread_spec = thread_spec_from_reminder(&reminder);
    let session_keys = agent_mode_session_keys(
        reminder.user_id,
        chat_id,
        thread_spec.thread_id,
        &reminder.flow_id,
    );
    let manager_enabled =
        manager_control_plane_enabled(settings, reminder.user_id, chat_id, thread_spec);
    let session_id = ensure_session_exists(EnsureSessionContext {
        session_keys,
        context_key: reminder.context_key.clone(),
        agent_flow_id: reminder.flow_id.clone(),
        agent_flow_created: false,
        sandbox_scope: sandbox_scope(reminder.user_id, chat_id, thread_spec),
        user_id: reminder.user_id,
        bot,
        transport_ctx: SessionTransportContext {
            chat_id,
            manager_default_chat_id: manager_default_chat_id(settings, chat_id, thread_spec),
            thread_spec,
        },
        llm,
        storage,
        persistent_memory_store,
        settings,
    })
    .await;

    if is_agent_task_running(session_id).await {
        defer_busy_reminder(storage, scheduler, &reminder).await;
        return Ok(None);
    }

    Ok(Some(PreparedReminderExecution {
        reminder,
        chat_id,
        thread_spec,
        session_id,
        manager_enabled,
    }))
}

async fn resolve_scheduled_topic_route(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    settings: &Arc<BotSettings>,
    context_key: &str,
    chat_id: ChatId,
    thread_spec: TelegramThreadSpec,
) -> TopicRouteDecision {
    let now = current_timestamp_unix_secs();
    let binding = match storage
        .get_topic_binding(user_id, context_key.to_string())
        .await
    {
        Ok(record) => resolve_active_topic_binding(record, now),
        Err(error) => {
            warn!(error = %error, user_id, topic_id = %context_key, "Failed to resolve binding for scheduled reminder");
            None
        }
    };

    if let Some(binding) = binding {
        return TopicRouteDecision {
            enabled: true,
            require_mention: false,
            mention_satisfied: true,
            system_prompt_override: None,
            agent_id: Some(binding.agent_id),
            dynamic_binding_topic_id: Some(binding.topic_id),
        };
    }

    let thread_id = thread_spec.thread_id.map(|thread_id| thread_id.0 .0);
    match settings.telegram.resolve_topic_config(chat_id.0, thread_id) {
        Some(topic) => TopicRouteDecision {
            enabled: topic.enabled,
            require_mention: topic.require_mention,
            mention_satisfied: true,
            system_prompt_override: topic.system_prompt.clone(),
            agent_id: topic.agent_id.clone(),
            dynamic_binding_topic_id: None,
        },
        None => TopicRouteDecision {
            enabled: true,
            require_mention: false,
            mention_satisfied: true,
            system_prompt_override: None,
            agent_id: None,
            dynamic_binding_topic_id: None,
        },
    }
}

async fn defer_busy_reminder(
    storage: &Arc<dyn StorageProvider>,
    scheduler: &Arc<ReminderSchedulerHandle>,
    reminder: &ReminderJobRecord,
) {
    let next_run_at = current_timestamp_unix_secs().saturating_add(REMINDER_BUSY_BACKOFF_SECS);
    match storage
        .reschedule_reminder_job(
            reminder.user_id,
            reminder.reminder_id.clone(),
            next_run_at,
            None,
            Some("Agent session is busy; reminder deferred.".to_string()),
            false,
        )
        .await
    {
        Ok(Some(updated)) => scheduler.upsert_record(updated).await,
        Ok(None) => {
            let _ = scheduler
                .reconcile_user_from_storage(storage, reminder.user_id)
                .await;
        }
        Err(error) => {
            warn!(error = %error, reminder_id = %reminder.reminder_id, "Failed to defer busy reminder")
        }
    }
}

async fn finalize_reminder_execution(
    storage: &Arc<dyn StorageProvider>,
    scheduler: &Arc<ReminderSchedulerHandle>,
    reminder: &ReminderJobRecord,
    result: std::result::Result<&(), &Error>,
) {
    let now = current_timestamp_unix_secs();

    match result {
        Ok(()) if reminder.is_recurring() => {
            finalize_recurring_reminder_success(storage, scheduler, reminder, now).await;
        }
        Ok(()) => finalize_one_shot_reminder_success(storage, scheduler, reminder, now).await,
        Err(error) if reminder.is_recurring() => {
            finalize_recurring_reminder_failure(
                storage,
                scheduler,
                reminder,
                now,
                &error.to_string(),
            )
            .await;
        }
        Err(error) => {
            finalize_one_shot_reminder_failure(
                storage,
                scheduler,
                reminder,
                now,
                &error.to_string(),
            )
            .await;
        }
    }
}

async fn finalize_recurring_reminder_success(
    storage: &Arc<dyn StorageProvider>,
    scheduler: &Arc<ReminderSchedulerHandle>,
    reminder: &ReminderJobRecord,
    now: i64,
) {
    let Some(next_run_at) =
        resolve_recurring_next_run(storage, scheduler, reminder, now, None).await
    else {
        return;
    };
    match storage
        .reschedule_reminder_job(
            reminder.user_id,
            reminder.reminder_id.clone(),
            next_run_at,
            Some(now),
            None,
            true,
        )
        .await
    {
        Ok(Some(updated)) => scheduler.upsert_record(updated).await,
        Ok(None) => {
            let _ = scheduler
                .reconcile_user_from_storage(storage, reminder.user_id)
                .await;
        }
        Err(error) => {
            warn!(error = %error, reminder_id = %reminder.reminder_id, "Failed to reschedule recurring reminder after success")
        }
    }
    let _ = append_reminder_audit_event(
        storage,
        reminder,
        "reminder_job_completed",
        serde_json::json!({
            "next_run_at": next_run_at,
            "recurring": true,
        }),
    )
    .await;
}

async fn finalize_one_shot_reminder_success(
    storage: &Arc<dyn StorageProvider>,
    scheduler: &Arc<ReminderSchedulerHandle>,
    reminder: &ReminderJobRecord,
    now: i64,
) {
    match storage
        .complete_reminder_job(reminder.user_id, reminder.reminder_id.clone(), now)
        .await
    {
        Ok(Some(updated)) => scheduler.upsert_record(updated).await,
        Ok(None) => {
            let _ = scheduler
                .reconcile_user_from_storage(storage, reminder.user_id)
                .await;
        }
        Err(error) => {
            warn!(error = %error, reminder_id = %reminder.reminder_id, "Failed to mark one-shot reminder complete")
        }
    }
    let _ = append_reminder_audit_event(
        storage,
        reminder,
        "reminder_job_completed",
        serde_json::json!({
            "completed_at": now,
            "recurring": false,
        }),
    )
    .await;
    match storage
        .delete_reminder_job(reminder.user_id, reminder.reminder_id.clone())
        .await
    {
        Ok(()) => {
            scheduler
                .delete_record(reminder.user_id, &reminder.reminder_id)
                .await
        }
        Err(error) => {
            warn!(error = %error, reminder_id = %reminder.reminder_id, "Failed to delete completed reminder record")
        }
    }
}

async fn finalize_recurring_reminder_failure(
    storage: &Arc<dyn StorageProvider>,
    scheduler: &Arc<ReminderSchedulerHandle>,
    reminder: &ReminderJobRecord,
    now: i64,
    error_text: &str,
) {
    let Some(next_run_at) = resolve_recurring_next_run(
        storage,
        scheduler,
        reminder,
        now,
        Some(error_text.to_string()),
    )
    .await
    else {
        return;
    };
    match storage
        .reschedule_reminder_job(
            reminder.user_id,
            reminder.reminder_id.clone(),
            next_run_at,
            Some(now),
            Some(error_text.to_string()),
            false,
        )
        .await
    {
        Ok(Some(updated)) => scheduler.upsert_record(updated).await,
        Ok(None) => {
            let _ = scheduler
                .reconcile_user_from_storage(storage, reminder.user_id)
                .await;
        }
        Err(error) => {
            warn!(error = %error, reminder_id = %reminder.reminder_id, "Failed to reschedule recurring reminder after failure")
        }
    }
    let _ = append_reminder_audit_event(
        storage,
        reminder,
        "reminder_job_failed",
        serde_json::json!({
            "error": error_text,
            "next_run_at": next_run_at,
            "recurring": true,
        }),
    )
    .await;
}

async fn finalize_one_shot_reminder_failure(
    storage: &Arc<dyn StorageProvider>,
    scheduler: &Arc<ReminderSchedulerHandle>,
    reminder: &ReminderJobRecord,
    now: i64,
    error_text: &str,
) {
    match storage
        .fail_reminder_job(
            reminder.user_id,
            reminder.reminder_id.clone(),
            now,
            error_text.to_string(),
        )
        .await
    {
        Ok(Some(updated)) => scheduler.upsert_record(updated).await,
        Ok(None) => {
            let _ = scheduler
                .reconcile_user_from_storage(storage, reminder.user_id)
                .await;
        }
        Err(error) => {
            warn!(error = %error, reminder_id = %reminder.reminder_id, "Failed to mark one-shot reminder failed")
        }
    }
    let _ = append_reminder_audit_event(
        storage,
        reminder,
        "reminder_job_failed",
        serde_json::json!({
            "error": error_text,
            "recurring": false,
        }),
    )
    .await;
}

async fn resolve_recurring_next_run(
    storage: &Arc<dyn StorageProvider>,
    scheduler: &Arc<ReminderSchedulerHandle>,
    reminder: &ReminderJobRecord,
    now: i64,
    error_text: Option<String>,
) -> Option<i64> {
    match compute_next_reminder_run_at(reminder, now) {
        Ok(Some(next_run_at)) => Some(next_run_at),
        Ok(None) => {
            match storage
                .complete_reminder_job(reminder.user_id, reminder.reminder_id.clone(), now)
                .await
            {
                Ok(Some(updated)) => scheduler.upsert_record(updated).await,
                Ok(None) => {
                    let _ = scheduler
                        .reconcile_user_from_storage(storage, reminder.user_id)
                        .await;
                }
                Err(error) => {
                    warn!(error = %error, reminder_id = %reminder.reminder_id, "Failed to complete exhausted recurring reminder")
                }
            }
            None
        }
        Err(schedule_error) => {
            let combined_error = match error_text {
                Some(error_text) => format!("{error_text}; reschedule failed: {schedule_error}"),
                None => schedule_error.to_string(),
            };
            match storage
                .fail_reminder_job(
                    reminder.user_id,
                    reminder.reminder_id.clone(),
                    now,
                    combined_error.clone(),
                )
                .await
            {
                Ok(Some(updated)) => scheduler.upsert_record(updated).await,
                Ok(None) => {
                    let _ = scheduler
                        .reconcile_user_from_storage(storage, reminder.user_id)
                        .await;
                }
                Err(error) => {
                    warn!(error = %error, reminder_id = %reminder.reminder_id, "Failed to fail recurring reminder after schedule error")
                }
            }
            let _ = append_reminder_audit_event(
                storage,
                reminder,
                "reminder_job_failed",
                serde_json::json!({
                    "error": combined_error,
                    "recurring": true,
                }),
            )
            .await;
            None
        }
    }
}

async fn append_reminder_audit_event(
    storage: &Arc<dyn StorageProvider>,
    reminder: &ReminderJobRecord,
    action: &str,
    payload: serde_json::Value,
) -> Result<()> {
    storage
        .append_audit_event(oxide_agent_core::storage::AppendAuditEventOptions {
            user_id: reminder.user_id,
            topic_id: Some(reminder.context_key.clone()),
            agent_id: None,
            action: action.to_string(),
            payload: serde_json::json!({
                "reminder_id": reminder.reminder_id.clone(),
                "flow_id": reminder.flow_id.clone(),
                "payload": payload,
            }),
        })
        .await?;
    Ok(())
}

fn scheduled_reminder_task_text(reminder: &ReminderJobRecord) -> String {
    format!(
        "Scheduled wake-up reminder.\nReminder ID: {}\nSchedule: {:?}\nCurrent time (unix): {}\n\nTask:\n{}\n\nExecute the task now and send the user a concise report.",
        reminder.reminder_id,
        reminder.schedule_kind,
        current_timestamp_unix_secs(),
        reminder.task_prompt,
    )
}

fn current_timestamp_unix_secs() -> i64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_secs()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}
