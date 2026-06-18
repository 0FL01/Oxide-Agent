use leptos::prelude::*;
use oxide_agent_web_contracts::{SessionDetail, SessionSummary, TaskDetail, TaskSummary};

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
}
