#![cfg(feature = "storage-sqlx")]

use anyhow::Result;
use oxide_agent_core::agent::identity::SessionId;
use oxide_agent_core::agent::providers::{
    ReminderContext, ReminderProvider, ReminderScheduleNotifier,
};
use oxide_agent_core::agent::tool_runtime::{
    ModelMetadata, ProviderMetadata, ToolBatchId, ToolCallId, ToolExecutionContext, ToolInvocation,
    ToolName, ToolOutputStatus, ToolTimeoutConfig, TurnId,
};
use oxide_agent_core::llm::InvocationId;
use oxide_agent_core::storage::{
    CreateReminderJobOptions, ReminderJobStatus, ReminderScheduleKind, ReminderThreadKind,
    StorageProvider,
};
use oxide_agent_transport_telegram::reminder_scheduler::ReminderSchedulerHandle;
use oxide_agent_transport_web::in_memory_storage::InMemoryStorage;
use serde_json::json;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio_util::sync::CancellationToken;

fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

fn reminder_provider(
    storage: Arc<dyn StorageProvider>,
    scheduler: Arc<ReminderSchedulerHandle>,
    user_id: i64,
    context_key: &str,
) -> Arc<ReminderProvider> {
    Arc::new(ReminderProvider::new(ReminderContext {
        storage,
        user_id,
        context_key: context_key.to_string(),
        flow_id: "flow-1".to_string(),
        chat_id: user_id,
        thread_id: None,
        thread_kind: ReminderThreadKind::None,
        notifier: Some(scheduler as Arc<dyn ReminderScheduleNotifier>),
    }))
}

fn reminder_invocation(tool_name: &str, raw_arguments: String) -> ToolInvocation {
    let now = chrono::Utc::now();
    ToolInvocation {
        session_id: SessionId::from(77),
        turn_id: TurnId::from("turn-reminder-integration"),
        batch_id: ToolBatchId::from("batch-reminder-integration"),
        batch_index: 0,
        invocation_id: InvocationId::from(format!("invoke-{tool_name}")),
        tool_call_id: ToolCallId::from(format!("call-{tool_name}")),
        provider_tool_call_id: None,
        tool_name: ToolName::from(tool_name),
        raw_provider_payload: json!({}),
        raw_arguments,
        normalized_arguments: serde_json::Value::Null,
        cancellation_token: CancellationToken::new(),
        timeout: ToolTimeoutConfig::default(),
        execution_context: ToolExecutionContext::new(std::env::temp_dir()),
        provider_metadata: ProviderMetadata {
            provider: "test".to_string(),
            protocol: "chat_like".to_string(),
        },
        model_metadata: ModelMetadata {
            model: "test-model".to_string(),
        },
        working_directory: None,
        environment_metadata: None,
        created_at: now,
        started_at: Some(now),
    }
}

async fn execute_reminder(
    provider: &Arc<ReminderProvider>,
    tool_name: &str,
    arguments: String,
) -> Result<()> {
    let executor = provider
        .tool_runtime_executors()
        .into_iter()
        .find(|executor| executor.name().as_str() == tool_name)
        .expect("typed reminder executor registered");
    let output = executor
        .execute(reminder_invocation(tool_name, arguments))
        .await?;
    assert_eq!(output.status, ToolOutputStatus::Success);
    Ok(())
}

#[tokio::test]
async fn bootstrap_loads_due_reminders_from_storage() -> Result<()> {
    let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorage::new());
    let scheduler = Arc::new(ReminderSchedulerHandle::new([77]));
    let now = now_unix_secs();
    let created = storage
        .create_reminder_job(CreateReminderJobOptions {
            user_id: 77,
            context_key: "topic-a".to_string(),
            flow_id: "flow-a".to_string(),
            chat_id: 77,
            thread_id: None,
            thread_kind: ReminderThreadKind::None,
            task_prompt: "wake up".to_string(),
            schedule_kind: ReminderScheduleKind::Once,
            next_run_at: now - 5,
            interval_secs: None,
            cron_expression: None,
            timezone: None,
        })
        .await?;

    let loaded = scheduler.bootstrap_from_storage(&storage).await?;
    let due = scheduler.take_due_batch(now, 8).await;

    assert_eq!(loaded, 1);
    assert_eq!(scheduler.tracked_count().await, 1);
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].reminder_id, created.reminder_id);
    Ok(())
}

#[tokio::test]
async fn provider_schedule_and_cancel_update_in_memory_queue() -> Result<()> {
    let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorage::new());
    let scheduler = Arc::new(ReminderSchedulerHandle::new([88]));
    let provider = reminder_provider(storage.clone(), scheduler.clone(), 88, "topic-b");

    execute_reminder(
        &provider,
        "reminder_schedule",
        json!({
            "kind": "interval",
            "task": "check deployment",
            "every_minutes": 10
        })
        .to_string(),
    )
    .await?;

    let scheduled = storage
        .list_reminder_jobs(88, Some("topic-b".to_string()), None, 10)
        .await?;
    assert_eq!(scheduled.len(), 1);
    assert_eq!(scheduler.tracked_count().await, 1);
    assert_eq!(
        scheduler.next_due_at().await,
        Some(scheduled[0].next_run_at)
    );

    execute_reminder(
        &provider,
        "reminder_cancel",
        json!({
            "reminder_id": scheduled[0].reminder_id
        })
        .to_string(),
    )
    .await?;

    let cancelled = storage
        .get_reminder_job(88, scheduled[0].reminder_id.clone())
        .await?
        .expect("cancelled reminder should remain stored");
    assert_eq!(cancelled.status, ReminderJobStatus::Cancelled);
    assert_eq!(scheduler.tracked_count().await, 0);
    assert_eq!(scheduler.next_due_at().await, None);
    Ok(())
}

#[tokio::test]
async fn provider_pause_and_resume_refresh_in_memory_queue() -> Result<()> {
    let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorage::new());
    let scheduler = Arc::new(ReminderSchedulerHandle::new([99]));
    let provider = reminder_provider(storage.clone(), scheduler.clone(), 99, "topic-c");

    execute_reminder(
        &provider,
        "reminder_schedule",
        json!({
            "kind": "interval",
            "task": "rotate logs",
            "every_minutes": 5
        })
        .to_string(),
    )
    .await?;

    let scheduled = storage
        .list_reminder_jobs(99, Some("topic-c".to_string()), None, 10)
        .await?;
    let reminder_id = scheduled[0].reminder_id.clone();

    execute_reminder(
        &provider,
        "reminder_pause",
        json!({ "reminder_id": reminder_id }).to_string(),
    )
    .await?;

    assert_eq!(scheduler.tracked_count().await, 0);

    execute_reminder(
        &provider,
        "reminder_resume",
        json!({
            "reminder_id": reminder_id,
            "delay_secs": 900
        })
        .to_string(),
    )
    .await?;

    let resumed = storage
        .get_reminder_job(99, scheduled[0].reminder_id.clone())
        .await?
        .expect("resumed reminder should exist");
    assert_eq!(resumed.status, ReminderJobStatus::Scheduled);
    assert_eq!(scheduler.tracked_count().await, 1);
    assert_eq!(scheduler.next_due_at().await, Some(resumed.next_run_at));
    Ok(())
}

#[tokio::test]
async fn reconcile_preserves_local_busy_snooze_without_rephasing_storage() -> Result<()> {
    let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorage::new());
    let scheduler = Arc::new(ReminderSchedulerHandle::new([111]));
    let now = now_unix_secs();
    let original_next_run_at = now - 5;
    let created = storage
        .create_reminder_job(CreateReminderJobOptions {
            user_id: 111,
            context_key: "topic-d".to_string(),
            flow_id: "flow-d".to_string(),
            chat_id: 111,
            thread_id: None,
            thread_kind: ReminderThreadKind::None,
            task_prompt: "wake up".to_string(),
            schedule_kind: ReminderScheduleKind::Interval,
            next_run_at: original_next_run_at,
            interval_secs: Some(86_400),
            cron_expression: None,
            timezone: None,
        })
        .await?;
    scheduler.bootstrap_from_storage(&storage).await?;

    let storage_record = storage
        .reschedule_reminder_job(
            111,
            created.reminder_id.clone(),
            original_next_run_at,
            None,
            Some("Agent session is busy; reminder deferred.".to_string()),
            false,
        )
        .await?
        .expect("reminder should reschedule");
    let retry_at = now + 30;
    let mut snoozed_record = storage_record.clone();
    snoozed_record.next_run_at = retry_at;
    scheduler.upsert_record(snoozed_record).await;

    scheduler.reconcile_user_from_storage(&storage, 111).await?;

    let persisted = storage
        .get_reminder_job(111, created.reminder_id.clone())
        .await?
        .expect("reminder should exist");
    assert_eq!(persisted.next_run_at, original_next_run_at);
    assert_eq!(scheduler.next_due_at().await, Some(retry_at));
    assert!(scheduler.take_due_batch(now, 8).await.is_empty());
    assert_eq!(scheduler.take_due_batch(retry_at, 8).await.len(), 1);
    Ok(())
}
