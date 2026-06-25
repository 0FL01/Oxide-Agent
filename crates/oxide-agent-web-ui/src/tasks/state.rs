use leptos::prelude::*;
use oxide_agent_web_contracts::{
    PersistedTaskEvent, SessionDetail, SessionSummary, TaskDetail, TaskEventKind, TaskStatus,
    TaskSummary,
};
use serde_json::Value;

pub(super) fn artifact_image_url(session_id: &str, task_id: &str, artifact_uri: &str) -> String {
    let path = artifact_uri
        .strip_prefix("artifact://")
        .unwrap_or(artifact_uri);
    format!("/api/v1/sessions/{session_id}/tasks/{task_id}/artifacts/{path}")
}

pub(super) fn artifact_filename(artifact_uri: &str) -> String {
    artifact_uri
        .rsplit('/')
        .next()
        .unwrap_or(artifact_uri)
        .to_string()
}

pub(super) fn summary_to_detail(session_id: &str, task: &TaskSummary) -> TaskDetail {
    TaskDetail {
        task_id: task.task_id.clone(),
        session_id: session_id.to_string(),
        version_group_id: task.effective_version_group_id().to_string(),
        version_index: task.effective_version_index(),
        parent_task_id: task.parent_task_id.clone(),
        status: task.status,
        input_markdown: task.input_markdown.clone(),
        attachments: task.attachments.clone(),
        input_edited_at: task.input_edited_at,
        final_response_markdown: task.final_response_markdown.clone(),
        error_message: task.error_message.clone(),
        pending_user_input: task.pending_user_input.clone(),
        last_progress: None,
        last_event_seq: task.last_event_seq,
        created_at: task.created_at,
        started_at: task.started_at,
        updated_at: task.updated_at,
        finished_at: task.finished_at,
    }
}

pub(super) fn upsert_session_summary(
    set_sessions: WriteSignal<Vec<SessionSummary>>,
    summary: SessionSummary,
) {
    set_sessions.update(|items| {
        if let Some(existing) = items
            .iter_mut()
            .find(|item| item.session_id == summary.session_id)
        {
            *existing = summary;
        } else {
            items.push(summary);
        }
        items.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| b.created_at.cmp(&a.created_at))
                .then_with(|| b.session_id.cmp(&a.session_id))
        });
    });
}

pub(super) fn remove_session_summary(
    set_sessions: WriteSignal<Vec<SessionSummary>>,
    session_id: &str,
) {
    set_sessions.update(|items| items.retain(|item| item.session_id != session_id));
}

pub(super) fn session_detail_to_summary(session: SessionDetail) -> SessionSummary {
    SessionSummary {
        session_id: session.session_id,
        title: session.title,
        model_selection: session.model_selection,
        agent_profile_id: session.agent_profile_id,
        last_preview: session.last_preview,
        active_task_id: session.active_task_id,
        last_task_status: session.last_task_status,
        created_at: session.created_at,
        updated_at: session.updated_at,
    }
}

pub(super) fn latest_task(tasks: &[TaskSummary]) -> Option<TaskSummary> {
    tasks.iter().max_by_key(|task| task.updated_at).cloned()
}

pub(super) fn latest_editable_task_id(tasks: &[TaskSummary]) -> Option<String> {
    tasks
        .iter()
        .max_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.task_id.cmp(&b.task_id))
        })
        .and_then(|task| task.status.is_terminal().then(|| task.task_id.clone()))
}

pub(super) fn activity_button_label(task: &TaskSummary, now_millis: i64) -> String {
    match task.status {
        TaskStatus::Queued | TaskStatus::Running => {
            format!(
                "Thinking for {}",
                format_duration(activity_elapsed_seconds(
                    ActivityTiming::from(task),
                    now_millis
                ))
            )
        }
        TaskStatus::WaitingForUserInput => "Waiting for your input".to_string(),
        TaskStatus::Completed
        | TaskStatus::Failed
        | TaskStatus::Cancelled
        | TaskStatus::Interrupted => {
            format!(
                "Thought for {}",
                format_duration(activity_elapsed_seconds(
                    ActivityTiming::from(task),
                    now_millis
                ))
            )
        }
    }
}

pub(super) fn should_render_global_activity_chip(
    task_id: Option<&str>,
    visible_task_ids: &[String],
) -> bool {
    task_id.is_some_and(|task_id| !visible_task_ids.iter().any(|visible| visible == task_id))
}

pub(super) fn format_duration(total_seconds: i64) -> String {
    let seconds = total_seconds.max(0);
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        return format!("{hours}h {minutes}m {seconds}s");
    }
    if minutes > 0 {
        return format!("{minutes}m {seconds}s");
    }
    format!("{seconds}s")
}

#[derive(Clone, Copy)]
pub(super) struct ActivityTiming {
    pub(super) status: TaskStatus,
    pub(super) created_at_ms: i64,
    pub(super) started_at_ms: Option<i64>,
    pub(super) updated_at_ms: i64,
    pub(super) finished_at_ms: Option<i64>,
}

impl From<&TaskSummary> for ActivityTiming {
    fn from(task: &TaskSummary) -> Self {
        Self {
            status: task.status,
            created_at_ms: task.created_at.timestamp_millis(),
            started_at_ms: task.started_at.map(|value| value.timestamp_millis()),
            updated_at_ms: task.updated_at.timestamp_millis(),
            finished_at_ms: task.finished_at.map(|value| value.timestamp_millis()),
        }
    }
}

impl From<&TaskDetail> for ActivityTiming {
    fn from(task: &TaskDetail) -> Self {
        Self {
            status: task.status,
            created_at_ms: task.created_at.timestamp_millis(),
            started_at_ms: task.started_at.map(|value| value.timestamp_millis()),
            updated_at_ms: task.updated_at.timestamp_millis(),
            finished_at_ms: task.finished_at.map(|value| value.timestamp_millis()),
        }
    }
}

/// Elapsed seconds for a task. Active (non-terminal) tasks use the live
/// browser clock `now_millis` as the end, clamped to at least the last
/// persisted `updated_at` so the timer never runs backwards between ticks.
/// Terminal tasks freeze at `finished_at` (falling back to `updated_at`).
pub(super) fn activity_elapsed_seconds(timing: ActivityTiming, now_millis: i64) -> i64 {
    let start_ms = timing.started_at_ms.unwrap_or(timing.created_at_ms);
    let end_ms = if timing.status.is_terminal() {
        timing.finished_at_ms.unwrap_or(timing.updated_at_ms)
    } else {
        now_millis.max(timing.updated_at_ms)
    };
    end_ms.saturating_sub(start_ms) / 1_000
}

/// Current wall-clock time in milliseconds from the browser performance API.
/// Single source for the shared 1s elapsed clock owned by `SessionWorkspace`.
pub(super) fn browser_now_millis() -> Option<i64> {
    let performance = web_sys::window()?.performance()?;
    let millis = performance.time_origin() + performance.now();
    millis.is_finite().then_some(millis.round() as i64)
}

pub(super) fn upsert_task_summary(items: &mut Vec<TaskSummary>, task: TaskSummary) {
    if let Some(existing) = items.iter_mut().find(|item| item.task_id == task.task_id) {
        *existing = task;
    } else {
        items.push(task);
    }
    items.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.task_id.cmp(&b.task_id))
    });
}

/// Derive the pinned Activity todos snapshot from per-task persisted events.
///
/// Todos are a durable per-task artifact: `TodosUpdated` events are persisted
/// for every web task and survive page reloads. Building the pinned todos card
/// from events (instead of the single shared global `progress` signal) keeps it
/// correct for any selected task — including terminal tasks after reload,
/// where `active_task` is `None` and the previous `live_owner` gate hid the
/// card — and avoids cross-task contamination from `progress`.
///
/// `task_events` are expected to already be filtered to a single task and
/// sorted chronologically (the caller in `ActivityDrawer` does this).
///
/// Falls back to the last `write_todos` tool-call input when no `TodosUpdated`
/// event is present (e.g. the task ended before the first update was emitted).
pub(super) fn latest_pinned_todos(task_events: &[PersistedTaskEvent]) -> Option<Value> {
    // Primary: last TodosUpdated event carries the structured TodoList.
    let from_todos_updated = task_events
        .iter()
        .rev()
        .find(|event| event.kind == TaskEventKind::TodosUpdated)
        .and_then(|event| event.payload.get("todos").cloned());
    if from_todos_updated.is_some() {
        return from_todos_updated;
    }
    // Fallback: last write_todos tool-call input_preview (JSON string).
    task_events
        .iter()
        .rev()
        .find(|event| {
            event.kind == TaskEventKind::ToolCall
                && event
                    .payload
                    .get("name")
                    .and_then(|v| v.as_str())
                    .is_some_and(|name| name == "write_todos")
        })
        .and_then(|event| {
            event
                .payload
                .get("input_preview")
                .and_then(|v| v.as_str())
                .and_then(|input| serde_json::from_str::<Value>(input).ok())
        })
        .and_then(|input| input.get("todos").cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(status: TaskStatus, finished_at: Option<&str>) -> TaskSummary {
        serde_json::from_value(serde_json::json!({
            "task_id": "task-1",
            "version_group_id": "group-1",
            "version_index": 0,
            "parent_task_id": null,
            "status": status,
            "input_markdown": "input",
            "attachments": [],
            "input_edited_at": null,
            "final_response_markdown": null,
            "error_message": null,
            "pending_user_input": null,
            "last_event_seq": 0,
            "created_at": "2026-06-11T00:00:00Z",
            "started_at": "2026-06-11T00:00:00Z",
            "updated_at": "2026-06-11T00:00:05Z",
            "finished_at": finished_at,
        }))
        .expect("task summary is valid")
    }

    #[test]
    fn artifact_image_url_strips_artifact_scheme() {
        assert_eq!(
            artifact_image_url(
                "sess-1",
                "task-1",
                "artifact://browser/owner/br/step-0001-milestone.jpg"
            ),
            "/api/v1/sessions/sess-1/tasks/task-1/artifacts/browser/owner/br/step-0001-milestone.jpg"
        );
    }

    #[test]
    fn artifact_image_url_leaves_non_artifact_uris_unchanged() {
        assert_eq!(
            artifact_image_url("sess-1", "task-1", "browser/owner/br/step-0001.jpg"),
            "/api/v1/sessions/sess-1/tasks/task-1/artifacts/browser/owner/br/step-0001.jpg"
        );
    }

    #[test]
    fn artifact_filename_extracts_last_segment() {
        assert_eq!(
            artifact_filename("artifact://browser/owner/br/step-0001-milestone.jpg"),
            "step-0001-milestone.jpg"
        );
    }

    #[test]
    fn activity_button_label_is_status_aware() {
        // now_millis=0 falls back to updated_at (5s after start) for running tasks.
        assert_eq!(
            activity_button_label(&task(TaskStatus::Running, None), 0),
            "Thinking for 5s"
        );
        assert_eq!(
            activity_button_label(&task(TaskStatus::WaitingForUserInput, None), 0),
            "Waiting for your input"
        );
        assert_eq!(
            activity_button_label(
                &task(TaskStatus::Completed, Some("2026-06-11T00:00:05Z")),
                0
            ),
            "Thought for 5s"
        );
    }

    #[test]
    fn activity_button_label_running_advances_with_clock() {
        let t = task(TaskStatus::Running, None);
        let start_ms = t.created_at.timestamp_millis();
        // Clock behind updated_at falls back to updated_at (5s).
        assert_eq!(activity_button_label(&t, 0), "Thinking for 5s");
        // Clock ahead of updated_at drives the timer forward independently of
        // any persisted update — the original "stuck timer" regression.
        assert_eq!(
            activity_button_label(&t, start_ms + 12_000),
            "Thinking for 12s"
        );
    }

    #[test]
    fn activity_button_label_terminal_freezes_with_clock() {
        let t = task(TaskStatus::Completed, Some("2026-06-11T00:00:05Z"));
        let start_ms = t.created_at.timestamp_millis();
        // Terminal tasks must not advance with the live clock.
        assert_eq!(
            activity_button_label(&t, start_ms + 999_000),
            "Thought for 5s"
        );
    }

    #[test]
    fn global_activity_chip_only_renders_for_non_visible_task() {
        let visible = vec!["task-1".to_string(), "task-2".to_string()];

        assert!(!should_render_global_activity_chip(None, &visible));
        assert!(!should_render_global_activity_chip(
            Some("task-1"),
            &visible
        ));
        assert!(should_render_global_activity_chip(Some("task-3"), &visible));
    }

    fn task_event(seq: u64, kind: TaskEventKind, payload: Value) -> PersistedTaskEvent {
        serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "task_id": "task-1",
            "session_id": "session-1",
            "user_id": 1,
            "seq": seq,
            "created_at": "2026-06-21T00:00:00Z",
            "kind": kind,
            "summary": "test",
            "payload": payload,
            "redacted": false,
            "truncated": false,
        }))
        .expect("event JSON is valid")
    }

    fn todos_updated_event(seq: u64, items: &[(&str, &str)]) -> PersistedTaskEvent {
        let items: Vec<Value> = items
            .iter()
            .map(|(desc, status)| serde_json::json!({ "description": desc, "status": status }))
            .collect();
        task_event(
            seq,
            TaskEventKind::TodosUpdated,
            serde_json::json!({ "source": "root", "todos": { "items": items } }),
        )
    }

    fn write_todos_call_event(seq: u64, items: &[(&str, &str)]) -> PersistedTaskEvent {
        let items: Vec<Value> = items
            .iter()
            .map(|(desc, status)| serde_json::json!({ "description": desc, "status": status }))
            .collect();
        let input_preview = serde_json::to_string(&serde_json::json!({ "todos": items }))
            .expect("input_preview serializes");
        task_event(
            seq,
            TaskEventKind::ToolCall,
            serde_json::json!({
                "id": format!("call_{seq}"),
                "source": "root",
                "name": "write_todos",
                "input_preview": input_preview,
                "command_preview": null,
            }),
        )
    }

    #[test]
    fn latest_pinned_todos_returns_last_todos_updated() {
        let events = vec![
            todos_updated_event(1, &[("First", "completed"), ("Second", "in_progress")]),
            todos_updated_event(2, &[("First", "completed"), ("Second", "completed")]),
        ];
        let todos = latest_pinned_todos(&events).expect("todos present");
        let items = todos
            .get("items")
            .and_then(Value::as_array)
            .expect("items array");
        assert_eq!(items.len(), 2);
        assert_eq!(items[1]["status"], "completed");
    }

    #[test]
    fn latest_pinned_todos_falls_back_to_write_todos_call_input() {
        let events = vec![
            task_event(
                1,
                TaskEventKind::Reasoning,
                serde_json::json!({ "summary": "thinking" }),
            ),
            write_todos_call_event(2, &[("Research", "in_progress")]),
        ];
        let todos = latest_pinned_todos(&events).expect("fallback todos present");
        let items = todos.as_array().expect("todos is array from input_preview");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["description"], "Research");
    }

    #[test]
    fn latest_pinned_todos_returns_none_when_no_todo_events() {
        let events = vec![
            task_event(
                1,
                TaskEventKind::Reasoning,
                serde_json::json!({ "summary": "thinking" }),
            ),
            task_event(
                2,
                TaskEventKind::ToolCall,
                serde_json::json!({ "name": "execute_command", "input_preview": "ls" }),
            ),
        ];
        assert!(latest_pinned_todos(&events).is_none());
    }

    #[test]
    fn latest_pinned_todos_returns_empty_todolist_when_last_update_has_no_items() {
        let events = vec![todos_updated_event(1, &[])];
        let todos = latest_pinned_todos(&events).expect("todos present");
        let items = todos
            .get("items")
            .and_then(Value::as_array)
            .expect("items array");
        assert!(items.is_empty());
    }

    #[test]
    fn latest_pinned_todos_ignores_non_write_todos_tool_calls_in_fallback() {
        let events = vec![
            task_event(
                1,
                TaskEventKind::ToolCall,
                serde_json::json!({ "name": "execute_command", "input_preview": "{\"todos\":[]}" }),
            ),
            task_event(
                2,
                TaskEventKind::ToolResult,
                serde_json::json!({ "name": "execute_command" }),
            ),
        ];
        assert!(latest_pinned_todos(&events).is_none());
    }
}
