use leptos::prelude::*;
use oxide_agent_web_contracts::{
    BrowserLiveEventPayload, BrowserLiveEventType, BrowserLiveScreenshotRef, PersistedTaskEvent,
    SessionDetail, SessionSummary, TaskDetail, TaskEventKind, TaskSummary,
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

#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct BrowserLiveState {
    pub session_id: Option<String>,
    pub action_seq: Option<u64>,
    pub url: Option<String>,
    pub title: Option<String>,
    pub latest_action: Option<String>,
    pub confidence: Option<f64>,
    pub verification_status: Option<String>,
    pub recovery_status: Option<String>,
    pub blocked_reason: Option<String>,
    pub screenshot: Option<BrowserLiveScreenshotRef>,
    pub network_failed_count: u32,
    pub console_error_count: u32,
    pub console_warning_count: u32,
    pub artifact_refs: Vec<String>,
}

impl BrowserLiveState {
    #[must_use]
    pub(super) fn is_blocked(&self) -> bool {
        self.blocked_reason.is_some()
            || matches!(
                self.verification_status.as_deref(),
                Some("needs_user" | "needsuser" | "blocked" | "timeout")
            )
    }
}

pub(super) fn browser_live_state_for_task(
    events: &[PersistedTaskEvent],
    task_id: &str,
) -> Option<BrowserLiveState> {
    let mut state = BrowserLiveState::default();
    let mut seen = false;
    for event in events.iter().filter(|event| event.task_id == task_id) {
        match event.kind {
            TaskEventKind::BrowserLive => {
                if let Ok(payload) =
                    serde_json::from_value::<BrowserLiveEventPayload>(event.payload.clone())
                {
                    apply_browser_live_payload(&mut state, payload);
                    seen = true;
                }
            }
            TaskEventKind::ToolResult => {
                if apply_browser_tool_result_event(&mut state, event) {
                    seen = true;
                }
            }
            _ => {}
        }
    }
    seen.then_some(state)
}

fn apply_browser_live_payload(state: &mut BrowserLiveState, payload: BrowserLiveEventPayload) {
    state.session_id = Some(payload.session_id);
    state.action_seq = payload.action_seq.or(state.action_seq);
    state.url = payload.url.or(state.url.take());
    state.title = payload.title.or(state.title.take());
    state.latest_action = payload.action.or(state.latest_action.take());
    state.confidence = payload.confidence.or(state.confidence);
    if let Some(status) = payload.status {
        match payload.event_type {
            BrowserLiveEventType::Recovery => state.recovery_status = Some(status),
            BrowserLiveEventType::Verification => state.verification_status = Some(status),
            _ => state.verification_status = Some(status),
        }
    }
    state.blocked_reason = payload.blocked_reason.or(state.blocked_reason.take());
    state.screenshot = payload.screenshot.or(state.screenshot.take());
    if let Some(debug) = payload.debug {
        state.network_failed_count = debug.network_failed_count;
        state.console_error_count = debug.console_error_count;
        state.console_warning_count = debug.console_warning_count;
    }
    if let Some(mut refs) = payload.artifact_refs {
        state.artifact_refs.append(&mut refs);
        state.artifact_refs.sort();
        state.artifact_refs.dedup();
    }
}

fn apply_browser_tool_result_event(
    state: &mut BrowserLiveState,
    event: &PersistedTaskEvent,
) -> bool {
    let Some(name) = event.payload.get("name").and_then(Value::as_str) else {
        return false;
    };
    if !name.starts_with("browser_") {
        return false;
    }
    let Some(output) = event
        .payload
        .get("output_preview")
        .and_then(Value::as_str)
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
    else {
        return true;
    };
    let payload = output
        .get("structured_payload")
        .filter(|value| value.is_object())
        .unwrap_or(&output);
    apply_browser_structured_payload(state, payload);
    true
}

fn apply_browser_structured_payload(state: &mut BrowserLiveState, payload: &Value) {
    update_string(&mut state.session_id, payload.get("session_id"));
    state.action_seq = payload
        .get("action_seq")
        .and_then(Value::as_u64)
        .or(state.action_seq);
    update_string(&mut state.verification_status, payload.get("status"));
    update_string(&mut state.blocked_reason, payload.get("question"));
    update_decision(state, payload.get("decision"));

    let observation = payload
        .get("after")
        .or_else(|| payload.get("observation"))
        .or_else(|| payload.get("before"))
        .unwrap_or(payload);
    update_observation(state, observation);

    if let Some(recovery) = payload.get("recovery").filter(|value| value.is_object()) {
        update_string(&mut state.recovery_status, recovery.get("status"));
        if state.blocked_reason.is_none() {
            update_string(&mut state.blocked_reason, recovery.get("safe_stop_reason"));
        }
    }
    if let Some(network) = payload.get("network") {
        state.network_failed_count = network
            .get("failed_count")
            .and_then(Value::as_u64)
            .unwrap_or(state.network_failed_count as u64)
            as u32;
    }
    if let Some(console) = payload.get("console") {
        state.console_error_count = console
            .get("error_count")
            .and_then(Value::as_u64)
            .unwrap_or(state.console_error_count as u64) as u32;
        state.console_warning_count = console
            .get("warning_count")
            .and_then(Value::as_u64)
            .unwrap_or(state.console_warning_count as u64)
            as u32;
    }
    if let Some(refs) = payload
        .get("retained_artifact_refs")
        .and_then(Value::as_array)
    {
        for value in refs.iter().filter_map(Value::as_str) {
            if !state.artifact_refs.iter().any(|existing| existing == value) {
                state.artifact_refs.push(value.to_string());
            }
        }
    }
}

fn update_observation(state: &mut BrowserLiveState, observation: &Value) {
    update_string(&mut state.url, observation.get("url"));
    update_string(&mut state.title, observation.get("title"));
    if let Some(action_seq) = observation.get("action_seq").and_then(Value::as_u64) {
        state.action_seq = Some(action_seq);
    }
    if let Some(screenshot) = observation.get("screenshot") {
        state.screenshot = screenshot_ref_from_value(screenshot).or(state.screenshot.take());
    }
    if let Some(network) = observation.get("network_summary") {
        state.network_failed_count = network
            .get("failed_count")
            .and_then(Value::as_u64)
            .unwrap_or(state.network_failed_count as u64)
            as u32;
    }
    if let Some(console) = observation.get("console_summary") {
        state.console_error_count = console
            .get("error_count")
            .and_then(Value::as_u64)
            .unwrap_or(state.console_error_count as u64) as u32;
        state.console_warning_count = console
            .get("warning_count")
            .and_then(Value::as_u64)
            .unwrap_or(state.console_warning_count as u64)
            as u32;
    }
}

fn update_decision(state: &mut BrowserLiveState, decision: Option<&Value>) {
    let Some(decision) = decision else {
        return;
    };
    state.confidence = decision
        .get("confidence")
        .and_then(Value::as_f64)
        .or(state.confidence);
    if let Some(action) = decision.get("action") {
        update_string(&mut state.latest_action, action.get("kind"));
        update_string(&mut state.blocked_reason, action.get("question"));
    }
}

fn update_string(target: &mut Option<String>, value: Option<&Value>) {
    if let Some(value) = value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        *target = Some(value.to_string());
    }
}

fn screenshot_ref_from_value(value: &Value) -> Option<BrowserLiveScreenshotRef> {
    let artifact_uri = value.get("artifact_uri")?.as_str()?.to_string();
    if artifact_uri.contains("base64") || artifact_uri.starts_with("data:") {
        return None;
    }
    Some(BrowserLiveScreenshotRef {
        artifact_uri,
        screenshot_id: value
            .get("screenshot_id")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        mime_type: value
            .get("mime_type")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        width: value
            .get("width")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok()),
        height: value
            .get("height")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok()),
        redacted: value
            .get("redacted")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(task_id: &str, seq: u64, kind: TaskEventKind, payload: Value) -> PersistedTaskEvent {
        serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "task_id": task_id,
            "session_id": "session",
            "user_id": 1,
            "seq": seq,
            "created_at": "2026-06-16T00:00:00Z",
            "kind": kind,
            "summary": "event",
            "payload": payload,
            "redacted": false,
            "truncated": false,
        }))
        .expect("event JSON is valid")
    }

    fn browser_tool_result(
        task_id: &str,
        seq: u64,
        name: &str,
        structured: Value,
    ) -> PersistedTaskEvent {
        let output = serde_json::json!({
            "status": "success",
            "structured_payload": structured,
        });
        event(
            task_id,
            seq,
            TaskEventKind::ToolResult,
            serde_json::json!({
                "name": name,
                "success": true,
                "output_preview": output.to_string(),
            }),
        )
    }

    #[test]
    fn browser_live_state_uses_latest_artifact_ref_without_base64() {
        let events = vec![
            browser_tool_result(
                "task-1",
                1,
                "browser_observe",
                serde_json::json!({
                    "status": "observed",
                    "session_id": "browser-1",
                    "action_seq": 1,
                    "url": "https://old.test",
                    "title": "Old",
                    "network_summary": { "failed_count": 0 },
                    "console_summary": { "error_count": 0, "warning_count": 0 },
                    "screenshot": {
                        "artifact_uri": "artifact://browser/task/browser/step-0001-live.jpg",
                        "screenshot_id": "shot-1",
                        "mime_type": "image/jpeg",
                        "width": 1365,
                        "height": 768,
                        "redacted": false
                    }
                }),
            ),
            browser_tool_result(
                "task-1",
                2,
                "browser_step",
                serde_json::json!({
                    "status": "action_verified",
                    "session_id": "browser-1",
                    "action_seq": 2,
                    "decision": {
                        "confidence": 0.84,
                        "action": { "kind": "click_xy" }
                    },
                    "after": {
                        "url": "https://new.test",
                        "title": "New",
                        "action_seq": 2,
                        "network_summary": { "failed_count": 1 },
                        "console_summary": { "error_count": 2, "warning_count": 3 },
                        "screenshot": {
                            "artifact_uri": "artifact://browser/task/browser/step-0002-milestone.jpg",
                            "screenshot_id": "shot-2",
                            "mime_type": "image/jpeg",
                            "width": 1365,
                            "height": 768,
                            "redacted": false
                        }
                    }
                }),
            ),
        ];

        let state = browser_live_state_for_task(&events, "task-1").expect("browser state");

        assert_eq!(state.session_id.as_deref(), Some("browser-1"));
        assert_eq!(state.action_seq, Some(2));
        assert_eq!(state.url.as_deref(), Some("https://new.test"));
        assert_eq!(state.title.as_deref(), Some("New"));
        assert_eq!(state.latest_action.as_deref(), Some("click_xy"));
        assert_eq!(
            state.verification_status.as_deref(),
            Some("action_verified")
        );
        assert_eq!(state.network_failed_count, 1);
        assert_eq!(state.console_error_count, 2);
        assert_eq!(state.console_warning_count, 3);
        let screenshot = state.screenshot.expect("latest screenshot");
        assert_eq!(
            screenshot.artifact_uri,
            "artifact://browser/task/browser/step-0002-milestone.jpg"
        );
        assert!(!screenshot.artifact_uri.contains("base64"));
    }

    #[test]
    fn browser_live_state_coalesces_preview_frames_to_latest() {
        let events = (1..=20)
            .map(|seq| {
                browser_tool_result(
                    "task-1",
                    seq,
                    "browser_observe",
                    serde_json::json!({
                        "status": "observed",
                        "session_id": "browser-1",
                        "action_seq": seq,
                        "screenshot": {
                            "artifact_uri": format!("artifact://browser/task/browser/step-{seq:04}-live.jpg"),
                            "width": 1365,
                            "height": 768,
                            "redacted": false
                        }
                    }),
                )
            })
            .collect::<Vec<_>>();

        let state = browser_live_state_for_task(&events, "task-1").expect("browser state");

        assert_eq!(state.action_seq, Some(20));
        assert_eq!(
            state.screenshot.expect("latest screenshot").artifact_uri,
            "artifact://browser/task/browser/step-0020-live.jpg"
        );
    }

    #[test]
    fn browser_live_state_rejects_data_url_screenshot_refs() {
        let events = vec![browser_tool_result(
            "task-1",
            1,
            "browser_observe",
            serde_json::json!({
                "status": "observed",
                "session_id": "browser-1",
                "screenshot": { "artifact_uri": "data:image/png;base64,AAAA" }
            }),
        )];

        let state = browser_live_state_for_task(&events, "task-1").expect("browser state");

        assert!(state.screenshot.is_none());
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
}
