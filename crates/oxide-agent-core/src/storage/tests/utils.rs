use super::*;

#[test]
fn audit_page_cursor_returns_descending_window() {
    let events = vec![
        AuditEventRecord {
            schema_version: 1,
            version: 1,
            event_id: "evt-1".to_string(),
            user_id: 9,
            topic_id: None,
            agent_id: None,
            action: "a".to_string(),
            payload: json!({}),
            created_at: 1,
        },
        AuditEventRecord {
            schema_version: 1,
            version: 2,
            event_id: "evt-2".to_string(),
            user_id: 9,
            topic_id: None,
            agent_id: None,
            action: "b".to_string(),
            payload: json!({}),
            created_at: 2,
        },
        AuditEventRecord {
            schema_version: 1,
            version: 3,
            event_id: "evt-3".to_string(),
            user_id: 9,
            topic_id: None,
            agent_id: None,
            action: "c".to_string(),
            payload: json!({}),
            created_at: 3,
        },
    ];

    let first_page: Vec<u64> = select_audit_events_page(events.clone(), None, 2)
        .iter()
        .map(|event| event.version)
        .collect();
    let second_page: Vec<u64> = select_audit_events_page(events, Some(2), 2)
        .iter()
        .map(|event| event.version)
        .collect();

    assert_eq!(first_page, vec![3, 2]);
    assert_eq!(second_page, vec![1]);
}

#[test]
fn control_plane_retry_policy_stops_at_max_attempt() {
    assert!(should_retry_control_plane_rmw(1));
    assert!(should_retry_control_plane_rmw(4));
    assert!(!should_retry_control_plane_rmw(5));
    assert!(!should_retry_control_plane_rmw(6));
}

#[tokio::test]
async fn control_plane_lock_serializes_same_key_updates() {
    let locks = Arc::new(ControlPlaneLocks::new());
    let first_guard = locks
        .acquire("users/7/control_plane/topic_bindings/topic-a.json".to_string())
        .await;

    let locks_for_task = Arc::clone(&locks);
    let (tx, rx) = oneshot::channel();
    let join = tokio::spawn(async move {
        let _second_guard = locks_for_task
            .acquire("users/7/control_plane/topic_bindings/topic-a.json".to_string())
            .await;
        let _ = tx.send(());
    });

    let blocked_result = timeout(Duration::from_millis(50), rx).await;
    assert!(blocked_result.is_err());

    drop(first_guard);

    let join_result = timeout(Duration::from_secs(1), join).await;
    assert!(join_result.is_ok());
}
