use leptos::prelude::*;
use oxide_agent_web_contracts::{
    SessionDetail, SessionSummary, TaskDetail, TaskStatus, TaskSummary,
};

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
}
