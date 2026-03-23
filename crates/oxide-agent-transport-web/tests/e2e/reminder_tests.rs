use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, Duration as ChronoDuration, Local};
use oxide_agent_core::storage::{compute_cron_next_run_at, resolve_reminder_local_datetime};

use super::helpers::{
    create_session_http, create_task_http_with_body, fetch_task_events, spawn_test_server,
    tool_call_response, unstructured_text_response, wait_for_task_status, wait_for_zai_calls,
};
use super::providers::{ControlledNarratorProvider, SequencedZaiProvider};
use super::setup::setup_web_test_with_custom_providers;

#[tokio::test]
async fn e2e_reminder_schedule_supports_tomorrow_local_time_without_unix_math() {
    let tomorrow = Local::now() + ChronoDuration::days(1);
    let tomorrow_date = format!(
        "{:04}-{:02}-{:02}",
        tomorrow.year(),
        tomorrow.month(),
        tomorrow.day()
    );

    let zai_provider = Arc::new(SequencedZaiProvider::new(vec![
        tool_call_response(
            "reminder_schedule",
            serde_json::json!({
                "kind": "once",
                "task": "Проверить отчет",
                "date": tomorrow_date,
                "time": "09:00"
            }),
        ),
        unstructured_text_response("Готово, напоминание поставлено."),
    ]));
    let narrator_provider = Arc::new(ControlledNarratorProvider::new(None));
    let app_state =
        setup_web_test_with_custom_providers(zai_provider.clone(), narrator_provider.clone());
    let session_manager = app_state.session_manager();
    let storage = session_manager.storage();
    let (server, base_url) = spawn_test_server(app_state).await;
    let client = reqwest::Client::new();

    let session_id = create_session_http(&client, &base_url).await;
    let task_id = create_task_http_with_body(
        &client,
        &base_url,
        &session_id,
        "Напомни завтра в 09:00 проверить отчет",
    )
    .await;

    wait_for_task_status(
        session_manager.as_ref(),
        &task_id,
        oxide_agent_transport_web::session::TaskStatus::Completed,
        Duration::from_secs(2),
    )
    .await;
    wait_for_zai_calls(&zai_provider, 2, Duration::from_secs(2)).await;

    let reminders = storage
        .list_reminder_jobs(1, Some("default".to_string()), None, 10)
        .await
        .expect("reminders should list");
    assert_eq!(reminders.len(), 1);
    let reminder = &reminders[0];
    let expected_next_run = resolve_reminder_local_datetime(
        &format!(
            "{:04}-{:02}-{:02}",
            tomorrow.year(),
            tomorrow.month(),
            tomorrow.day()
        ),
        "09:00",
        Some(&local_offset_timezone()),
    )
    .expect("local datetime should resolve");
    assert_eq!(reminder.next_run_at, expected_next_run);
    assert_eq!(reminder.cron_expression, None);
    assert_eq!(reminder.interval_secs, None);

    let events = fetch_task_events(&client, &base_url, &session_id, &task_id).await;
    let event_names = events
        .iter()
        .filter_map(|event| event["event_name"].as_str())
        .collect::<Vec<_>>();
    assert!(event_names.contains(&"tool_call:reminder_schedule"));
    assert!(event_names.contains(&"tool_result:reminder_schedule"));
    assert!(event_names.contains(&"finished"));

    server.abort();
}

#[tokio::test]
async fn e2e_reminder_schedule_supports_weekday_wall_clock_recurring_jobs() {
    let timezone = "UTC+3";

    let zai_provider = Arc::new(SequencedZaiProvider::new(vec![
        tool_call_response(
            "reminder_schedule",
            serde_json::json!({
                "kind": "cron",
                "task": "Закрыть день",
                "time": "18:30",
                "weekdays": ["mon", "tue", "wed", "thu", "fri"],
                "timezone": timezone
            }),
        ),
        unstructured_text_response("Готово, поставил напоминание по будням."),
    ]));
    let narrator_provider = Arc::new(ControlledNarratorProvider::new(None));
    let app_state =
        setup_web_test_with_custom_providers(zai_provider.clone(), narrator_provider.clone());
    let session_manager = app_state.session_manager();
    let storage = session_manager.storage();
    let (server, base_url) = spawn_test_server(app_state).await;
    let client = reqwest::Client::new();

    let session_id = create_session_http(&client, &base_url).await;
    let task_id = create_task_http_with_body(
        &client,
        &base_url,
        &session_id,
        "Напоминай по будням в 18:30 закрывать день",
    )
    .await;

    wait_for_task_status(
        session_manager.as_ref(),
        &task_id,
        oxide_agent_transport_web::session::TaskStatus::Completed,
        Duration::from_secs(2),
    )
    .await;
    wait_for_zai_calls(&zai_provider, 2, Duration::from_secs(2)).await;

    let reminders = storage
        .list_reminder_jobs(1, Some("default".to_string()), None, 10)
        .await
        .expect("reminders should list");
    assert_eq!(reminders.len(), 1);
    let reminder = &reminders[0];
    assert_eq!(
        reminder.cron_expression.as_deref(),
        Some("0 30 18 * * Mon,Tue,Wed,Thu,Fri *")
    );
    assert_eq!(reminder.timezone.as_deref(), Some(timezone));
    let expected_next_run = compute_cron_next_run_at(
        "0 30 18 * * Mon,Tue,Wed,Thu,Fri *",
        Some(timezone),
        reminder.created_at,
    )
    .expect("cron next run should resolve");
    assert_eq!(reminder.next_run_at, expected_next_run);

    let events = fetch_task_events(&client, &base_url, &session_id, &task_id).await;
    let event_names = events
        .iter()
        .filter_map(|event| event["event_name"].as_str())
        .collect::<Vec<_>>();
    assert!(event_names.contains(&"tool_call:reminder_schedule"));
    assert!(event_names.contains(&"tool_result:reminder_schedule"));
    assert!(event_names.contains(&"finished"));

    server.abort();
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
