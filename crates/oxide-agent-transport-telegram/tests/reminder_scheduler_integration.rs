use anyhow::Result;
use oxide_agent_core::agent::provider::ToolProvider;
use oxide_agent_core::agent::providers::{
    ReminderContext, ReminderProvider, ReminderScheduleNotifier,
};
use oxide_agent_core::storage::{
    CreateReminderJobOptions, ReminderJobStatus, ReminderScheduleKind, ReminderThreadKind,
    StorageProvider,
};
use oxide_agent_transport_telegram::reminder_scheduler::ReminderSchedulerHandle;
use oxide_agent_transport_web::in_memory_storage::InMemoryStorage;
use serde_json::json;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

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
) -> ReminderProvider {
    ReminderProvider::new(ReminderContext {
        storage,
        user_id,
        context_key: context_key.to_string(),
        flow_id: "flow-1".to_string(),
        chat_id: user_id,
        thread_id: None,
        thread_kind: ReminderThreadKind::None,
        notifier: Some(scheduler as Arc<dyn ReminderScheduleNotifier>),
    })
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

    provider
        .execute(
            "reminder_schedule",
            &json!({
                "kind": "interval",
                "task": "check deployment",
                "every_minutes": 10
            })
            .to_string(),
            None,
            None,
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

    provider
        .execute(
            "reminder_cancel",
            &json!({
                "reminder_id": scheduled[0].reminder_id
            })
            .to_string(),
            None,
            None,
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

    provider
        .execute(
            "reminder_schedule",
            &json!({
                "kind": "interval",
                "task": "rotate logs",
                "every_minutes": 5
            })
            .to_string(),
            None,
            None,
        )
        .await?;

    let scheduled = storage
        .list_reminder_jobs(99, Some("topic-c".to_string()), None, 10)
        .await?;
    let reminder_id = scheduled[0].reminder_id.clone();

    provider
        .execute(
            "reminder_pause",
            &json!({ "reminder_id": reminder_id }).to_string(),
            None,
            None,
        )
        .await?;

    assert_eq!(scheduler.tracked_count().await, 0);

    provider
        .execute(
            "reminder_resume",
            &json!({
                "reminder_id": reminder_id,
                "delay_secs": 900
            })
            .to_string(),
            None,
            None,
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
