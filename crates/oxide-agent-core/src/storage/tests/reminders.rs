use super::*;

#[test]
fn parse_reminder_timezone_defaults_to_utc() {
    let timezone = parse_reminder_timezone(None).expect("timezone should parse");
    assert_eq!(timezone.name(), "UTC");
}

#[test]
fn parse_reminder_timezone_accepts_utc_offsets() {
    let timezone = parse_reminder_timezone(Some("UTC+3")).expect("timezone should parse");
    assert_eq!(timezone.name(), "UTC+3");
}

#[test]
fn compute_cron_next_run_at_uses_timezone() {
    let after = chrono::Utc
        .with_ymd_and_hms(2026, 6, 1, 6, 0, 0)
        .single()
        .expect("valid datetime")
        .timestamp();
    let next = compute_cron_next_run_at("0 0 9 * * * *", Some("Europe/Berlin"), after)
        .expect("cron should resolve");
    let expected = chrono::Utc
        .with_ymd_and_hms(2026, 6, 1, 7, 0, 0)
        .single()
        .expect("valid datetime")
        .timestamp();
    assert_eq!(next, expected);
}

#[test]
fn compute_next_reminder_run_at_supports_cron_records() {
    let record = ReminderJobRecord {
        schema_version: 2,
        version: 1,
        reminder_id: "rem-1".to_string(),
        user_id: 1,
        context_key: "ctx".to_string(),
        flow_id: "flow".to_string(),
        chat_id: 1,
        thread_id: None,
        thread_kind: ReminderThreadKind::Dm,
        task_prompt: "Ping".to_string(),
        schedule_kind: ReminderScheduleKind::Cron,
        status: ReminderJobStatus::Scheduled,
        next_run_at: 0,
        interval_secs: None,
        cron_expression: Some("0 0 9 * * * *".to_string()),
        timezone: Some("UTC".to_string()),
        lease_until: None,
        last_run_at: None,
        last_error: None,
        run_count: 0,
        created_at: 0,
        updated_at: 0,
    };
    let after = chrono::Utc
        .with_ymd_and_hms(2026, 6, 1, 8, 0, 0)
        .single()
        .expect("valid datetime")
        .timestamp();

    let next = compute_next_reminder_run_at(&record, after).expect("next run should compute");
    let expected = chrono::Utc
        .with_ymd_and_hms(2026, 6, 1, 9, 0, 0)
        .single()
        .expect("valid datetime")
        .timestamp();
    assert_eq!(next, Some(expected));
}

#[test]
fn compute_cron_next_run_at_supports_fixed_utc_offset_timezones() {
    let after = chrono::Utc
        .with_ymd_and_hms(2026, 3, 23, 5, 0, 0)
        .single()
        .expect("valid datetime")
        .timestamp();
    let next = compute_cron_next_run_at("0 0 9 * * * *", Some("UTC+3"), after)
        .expect("cron should resolve");
    let expected = chrono::Utc
        .with_ymd_and_hms(2026, 3, 23, 6, 0, 0)
        .single()
        .expect("valid datetime")
        .timestamp();
    assert_eq!(next, expected);
}

#[test]
fn resolve_local_datetime_uses_offset_timezone() {
    let unix = resolve_reminder_local_datetime("2026-03-24", "09:00", Some("UTC+3"))
        .expect("local datetime should resolve");
    let expected = chrono::Utc
        .with_ymd_and_hms(2026, 3, 24, 6, 0, 0)
        .single()
        .expect("valid datetime")
        .timestamp();
    assert_eq!(unix, expected);
}
