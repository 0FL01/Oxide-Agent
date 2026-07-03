use chrono::DateTime;
use leptos::prelude::*;
use oxide_agent_web_contracts::{
    ApiLifeEventResponse, ApiLifeTurnResponse, PersistedTaskEvent, TaskEventKind,
};
use serde_json::Value;

/// Convert a life event DTO to a `PersistedTaskEvent` so the existing
/// activity rendering (tool cards, event grouping, filtering) can be
/// reused without duplication.
///
/// The life event `kind` is the snake_case `TaskEventKind` variant string
/// and the `payload` is a wrapper `{summary, payload, redacted, truncated}`
/// (established by the C4 `agent_event_to_life_parts` bridge).
pub(crate) fn life_event_to_persisted(event: &ApiLifeEventResponse) -> Option<PersistedTaskEvent> {
    let kind: TaskEventKind = serde_json::from_str(&format!("\"{}\"", event.kind)).ok()?;

    let summary = event
        .payload
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let payload = event.payload.get("payload").cloned().unwrap_or(Value::Null);
    let redacted = event
        .payload
        .get("redacted")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let truncated = event
        .payload
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let created_at = DateTime::from_timestamp_millis(event.created_at)?;

    Some(PersistedTaskEvent {
        schema_version: 1,
        task_id: String::new(),
        session_id: String::new(),
        user_id: 0,
        seq: event.seq.max(0) as u64,
        created_at,
        kind,
        summary,
        payload,
        redacted,
        truncated,
    })
}

/// Merge new turns into the existing list, deduplicating by `turn_id`.
/// Turns are kept in ascending `created_at` order (oldest first, newest last)
/// so the transcript renders chronologically top-to-bottom.
pub(crate) fn merge_turns(
    set_turns: WriteSignal<Vec<ApiLifeTurnResponse>>,
    new_turns: Vec<ApiLifeTurnResponse>,
    prepend: bool,
) {
    set_turns.update(|items| {
        for turn in new_turns {
            if !items
                .iter()
                .any(|existing| existing.turn_id == turn.turn_id)
            {
                if prepend {
                    items.insert(0, turn);
                } else {
                    items.push(turn);
                }
            }
        }
        items.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    });
}

/// Merge new events into the existing list, deduplicating by `(run_id, seq)`.
/// Events are kept in ascending `(created_at, seq)` order.
pub(crate) fn merge_events(
    set_events: WriteSignal<Vec<ApiLifeEventResponse>>,
    new_events: Vec<ApiLifeEventResponse>,
    prepend: bool,
) {
    set_events.update(|items| {
        for event in new_events {
            let exists = items
                .iter()
                .any(|existing| existing.run_id == event.run_id && existing.seq == event.seq);
            if !exists {
                if prepend {
                    items.insert(0, event);
                } else {
                    items.push(event);
                }
            }
        }
        items.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.seq.cmp(&b.seq))
        });
    });
}

/// Convert only events for the selected run into persisted task events.
///
/// Life activity is anchored to an explicit run selected by the user-visible
/// transcript button. With no selected run there is intentionally no implicit
/// "all runs" fallback, because that mixes permanent-chat activity from
/// unrelated answers.
pub(crate) fn life_persisted_events_for_run(
    events: &[ApiLifeEventResponse],
    selected_run_id: Option<&str>,
) -> Vec<PersistedTaskEvent> {
    let Some(run_id) = selected_run_id else {
        return Vec::new();
    };

    events
        .iter()
        .filter(|event| event.run_id == run_id)
        .filter_map(life_event_to_persisted)
        .collect()
}

/// User-visible Life activity anchors belong to final assistant turns.
///
/// The run id is used only as an internal selection key by the UI; it is not
/// rendered in the transcript or drawer header.
pub(crate) fn life_turn_activity_run_id(turn: &ApiLifeTurnResponse) -> Option<&str> {
    if turn.role == "assistant" {
        turn.run_id.as_deref()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_agent_web_contracts::{ApiLifeEventResponse, ApiLifeTurnResponse};

    fn life_event(seq: i64, kind: &str, summary: &str) -> ApiLifeEventResponse {
        life_event_for_run("run-1", seq, kind, summary)
    }

    fn life_event_for_run(
        run_id: &str,
        seq: i64,
        kind: &str,
        summary: &str,
    ) -> ApiLifeEventResponse {
        ApiLifeEventResponse {
            event_id: format!("evt-{seq}"),
            run_id: run_id.to_string(),
            seq,
            kind: kind.to_string(),
            payload: serde_json::json!({
                "summary": summary,
                "payload": {"name": "test_tool"},
                "redacted": false,
                "truncated": false,
            }),
            created_at: 1_700_000_000_000 + seq * 1000,
        }
    }

    fn life_turn(role: &str, run_id: Option<&str>) -> ApiLifeTurnResponse {
        ApiLifeTurnResponse {
            turn_id: "turn-1".to_owned(),
            run_id: run_id.map(ToOwned::to_owned),
            role: role.to_owned(),
            source_transport: "web".to_owned(),
            source_ref: None,
            content: "content".to_owned(),
            attachments: serde_json::json!([]),
            transport_metadata: serde_json::json!({}),
            redaction_state: "clean".to_owned(),
            created_at: 1_700_000_000_000,
        }
    }

    #[test]
    fn converts_tool_call_event() {
        let event = life_event(1, "tool_call", "execute_command");
        let persisted = life_event_to_persisted(&event).expect("conversion succeeds");
        assert_eq!(persisted.kind, TaskEventKind::ToolCall);
        assert_eq!(persisted.summary, "execute_command");
        assert_eq!(persisted.seq, 1);
        assert!(!persisted.redacted);
        assert!(!persisted.truncated);
    }

    #[test]
    fn converts_reasoning_event() {
        let event = life_event(2, "reasoning", "thinking about it");
        let persisted = life_event_to_persisted(&event).expect("conversion succeeds");
        assert_eq!(persisted.kind, TaskEventKind::Reasoning);
        assert_eq!(persisted.summary, "thinking about it");
    }

    #[test]
    fn converts_finished_event() {
        let event = life_event(3, "finished", "done");
        let persisted = life_event_to_persisted(&event).expect("conversion succeeds");
        assert_eq!(persisted.kind, TaskEventKind::Finished);
    }

    #[test]
    fn returns_none_for_unknown_kind() {
        let event = life_event(1, "nonexistent_kind", "test");
        assert!(life_event_to_persisted(&event).is_none());
    }

    #[test]
    fn handles_redacted_and_truncated_flags() {
        let event = ApiLifeEventResponse {
            event_id: "evt-1".to_string(),
            run_id: "run-1".to_string(),
            seq: 1,
            kind: "tool_result".to_string(),
            payload: serde_json::json!({
                "summary": "secret",
                "payload": {},
                "redacted": true,
                "truncated": true,
            }),
            created_at: 1_700_000_000_000,
        };
        let persisted = life_event_to_persisted(&event).expect("conversion succeeds");
        assert!(persisted.redacted);
        assert!(persisted.truncated);
    }

    #[test]
    fn persisted_events_require_selected_run() {
        let events = vec![life_event(1, "tool_call", "execute_command")];

        assert!(life_persisted_events_for_run(&events, None).is_empty());
    }

    #[test]
    fn persisted_events_are_scoped_to_selected_run() {
        let events = vec![
            life_event_for_run("run-1", 1, "tool_call", "run one"),
            life_event_for_run("run-2", 1, "tool_call", "run two"),
        ];

        let visible = life_persisted_events_for_run(&events, Some("run-2"));

        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].summary, "run two");
    }

    #[test]
    fn assistant_turn_with_run_gets_activity_anchor() {
        let turn = life_turn("assistant", Some("run-1"));

        assert_eq!(life_turn_activity_run_id(&turn), Some("run-1"));
    }

    #[test]
    fn user_turn_does_not_get_activity_anchor() {
        let turn = life_turn("user", Some("run-1"));

        assert_eq!(life_turn_activity_run_id(&turn), None);
    }

    #[test]
    fn assistant_turn_without_run_does_not_get_activity_anchor() {
        let turn = life_turn("assistant", None);

        assert_eq!(life_turn_activity_run_id(&turn), None);
    }
}
