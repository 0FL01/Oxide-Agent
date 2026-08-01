use leptos::prelude::*;
use oxide_agent_web_contracts::{
    PersistedTaskEvent, ProgressSnapshot, SessionSummary, TaskDetail, TaskEventKind, TaskStatus,
    TaskSummary,
};
use serde_json::Value;
use std::collections::HashMap;

const SEARCH_PROBE_REASONING_PREFIX: &str = "Search Probe #";
const SEARCH_PROBE_START_UPDATE: &str = "Starting web research before the main answer.";
const MAX_INLINE_PROGRESS_BLOCKS: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct InlineProgressBlock {
    pub(super) seq: u64,
    pub(super) headline: String,
    pub(super) detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ActivityLoadPhase {
    Loading,
    Ready,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TaskActivityState {
    pub(super) phase: ActivityLoadPhase,
    pub(super) progress: Option<ProgressSnapshot>,
    pub(super) run_floor_seq: u64,
    pub(super) before_seq: u64,
    pub(super) has_more: bool,
    pub(super) loading_older: bool,
}

impl TaskActivityState {
    pub(super) const fn loading() -> Self {
        Self {
            phase: ActivityLoadPhase::Loading,
            progress: None,
            run_floor_seq: 0,
            before_seq: 0,
            has_more: false,
            loading_older: false,
        }
    }

    pub(super) const fn live() -> Self {
        Self {
            phase: ActivityLoadPhase::Ready,
            progress: None,
            run_floor_seq: 0,
            before_seq: 0,
            has_more: false,
            loading_older: false,
        }
    }
}

pub(super) fn begin_activity_load(
    states: &mut HashMap<String, TaskActivityState>,
    task_id: &str,
) -> bool {
    if states.contains_key(task_id) {
        return false;
    }
    states.insert(task_id.to_string(), TaskActivityState::loading());
    true
}

pub(super) fn complete_activity_load(
    states: &mut HashMap<String, TaskActivityState>,
    task_id: &str,
    before_seq: u64,
    has_more: bool,
    progress: Option<ProgressSnapshot>,
) -> bool {
    let Some(state) = states.get_mut(task_id) else {
        return false;
    };
    if !matches!(state.phase, ActivityLoadPhase::Loading) {
        return false;
    }
    state.phase = ActivityLoadPhase::Ready;
    state.progress = progress;
    state.before_seq = before_seq;
    state.has_more = has_more;
    state.loading_older = false;
    true
}

pub(super) fn fail_activity_load(
    states: &mut HashMap<String, TaskActivityState>,
    task_id: &str,
    error: String,
) {
    if let Some(state) = states.get_mut(task_id)
        && matches!(state.phase, ActivityLoadPhase::Loading)
    {
        state.phase = ActivityLoadPhase::Failed(error);
    }
}

pub(super) fn update_activity_progress(
    states: &mut HashMap<String, TaskActivityState>,
    task_id: String,
    progress: Option<ProgressSnapshot>,
) {
    states
        .entry(task_id)
        .or_insert_with(TaskActivityState::live)
        .progress = progress;
}

pub(super) fn begin_activity_run(
    states: &mut HashMap<String, TaskActivityState>,
    task_id: String,
    run_floor_seq: u64,
) {
    let state = states
        .entry(task_id)
        .or_insert_with(TaskActivityState::live);
    state.phase = ActivityLoadPhase::Ready;
    state.progress = None;
    state.run_floor_seq = run_floor_seq;
}

pub(crate) fn stream_owner_matches(
    owner: Option<&(String, u64)>,
    task_id: &str,
    generation: u64,
) -> bool {
    owner.is_some_and(|(owner_task_id, owner_generation)| {
        owner_task_id == task_id && *owner_generation == generation
    })
}

pub(super) fn inline_progress_blocks(
    task_id: &str,
    status: TaskStatus,
    events: &[PersistedTaskEvent],
    activity: Option<&TaskActivityState>,
) -> Vec<InlineProgressBlock> {
    if !matches!(status, TaskStatus::Queued | TaskStatus::Running) {
        return Vec::new();
    }

    let current_run_start = events
        .iter()
        .filter(|event| event.task_id == task_id && event.kind == TaskEventKind::UserMessage)
        .map(|event| event.seq)
        .max()
        .unwrap_or(0)
        .max(activity.map_or(0, |state| state.run_floor_seq));
    let mut blocks: Vec<_> = events
        .iter()
        .filter(|event| event.task_id == task_id && event.seq > current_run_start)
        .filter_map(inline_progress_block)
        .collect();
    blocks.sort_by_key(|block| block.seq);
    blocks.dedup_by(|next, previous| {
        next.headline == previous.headline && next.detail == previous.detail
    });
    if blocks.len() > MAX_INLINE_PROGRESS_BLOCKS {
        blocks.drain(..blocks.len() - MAX_INLINE_PROGRESS_BLOCKS);
    }
    if blocks.is_empty() {
        blocks.push(InlineProgressBlock {
            seq: 0,
            headline: match status {
                TaskStatus::Queued => "Queued...".to_string(),
                TaskStatus::Running => "Thinking...".to_string(),
                _ => unreachable!("non-live task status was rejected above"),
            },
            detail: None,
        });
    }
    blocks
}

fn inline_progress_block(event: &PersistedTaskEvent) -> Option<InlineProgressBlock> {
    let (headline, detail) = match event.kind {
        TaskEventKind::Reasoning => (reasoning_event_summary(event)?, None),
        TaskEventKind::ToolCall => tool_call_progress(event)?,
        TaskEventKind::BrowserLive => (
            normalize_inline_detail(&event.summary)?,
            payload_str(event, "blocked_reason").and_then(normalize_inline_detail),
        ),
        TaskEventKind::Continuation => (
            "Continuing the task".to_string(),
            payload_str(event, "reason").and_then(normalize_inline_detail),
        ),
        TaskEventKind::RuntimeCompactionStarted | TaskEventKind::HistoryRepairApplied => {
            (normalize_inline_detail(&event.summary)?, None)
        }
        _ => return None,
    };
    Some(InlineProgressBlock {
        seq: event.seq,
        headline: compact_reasoning_preview(&headline, 280),
        detail,
    })
}

fn tool_call_progress(event: &PersistedTaskEvent) -> Option<(String, Option<String>)> {
    let name = payload_str(event, "name")?;
    if name == "write_todos" {
        return None;
    }
    let headline = match name {
        "web_search" => "Searching the web".to_string(),
        "web_crawler" | "webfetch_md" => "Reading a web page".to_string(),
        "read_file" => "Reading a file".to_string(),
        "execute_command" => "Executing a command".to_string(),
        "spawn_sub_agents" => "Delegating work".to_string(),
        "wait_sub_agents" => "Waiting for delegated work".to_string(),
        name if name.starts_with("browser_") => "Working in the browser".to_string(),
        name => format!("Using {}", humanize_identifier(name)),
    };
    let detail = if event.redacted {
        None
    } else {
        payload_str(event, "command_preview")
            .and_then(normalize_inline_detail)
            .or_else(|| tool_input_detail(event))
    };
    Some((headline, detail))
}

fn tool_input_detail(event: &PersistedTaskEvent) -> Option<String> {
    let input = payload_str(event, "input_preview")?;
    let value = serde_json::from_str::<Value>(input).ok()?;
    ["query", "url", "path", "directory"]
        .iter()
        .find_map(|key| value.get(key).and_then(Value::as_str))
        .and_then(normalize_inline_detail)
}

fn humanize_identifier(value: &str) -> String {
    let mut value = value.replace(['_', '-'], " ");
    if let Some(first) = value.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    value
}

fn normalize_inline_detail(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(compact_reasoning_preview(value, 180))
    }
}

pub(crate) fn reasoning_event_summary(event: &PersistedTaskEvent) -> Option<String> {
    payload_str(event, "summary")
        .and_then(normalize_reasoning_text)
        .or_else(|| normalize_reasoning_text(&event.summary))
}

pub(crate) fn normalize_reasoning_text(summary: &str) -> Option<String> {
    let summary = summary.trim();
    if summary.is_empty() || summary == "Reasoning" {
        return None;
    }
    let Some(rest) = summary.strip_prefix(SEARCH_PROBE_REASONING_PREFIX) else {
        return Some(summary.to_string());
    };
    let (generation, body) = rest.split_once(':')?;
    if generation.trim().is_empty() || !generation.trim().chars().all(|ch| ch.is_ascii_digit()) {
        return Some(summary.to_string());
    }
    let body = body.trim();
    if body.is_empty() || body == SEARCH_PROBE_START_UPDATE {
        return None;
    }
    Some(body.to_string())
}

pub(crate) fn compact_reasoning_preview(summary: &str, max_chars: usize) -> String {
    let compact = summary.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = compact.chars();
    let preview = chars.by_ref().take(max_chars).collect::<String>();

    if chars.next().is_some() {
        format!("{preview}...")
    } else {
        preview
    }
}

fn payload_str<'a>(event: &'a PersistedTaskEvent, key: &str) -> Option<&'a str> {
    event.payload.get(key).and_then(Value::as_str)
}

pub(super) fn artifact_image_url(
    session_id: Option<&str>,
    task_id: Option<&str>,
    artifact_uri: &str,
) -> Option<String> {
    let browser_path = artifact_uri.strip_prefix("artifact://browser/");
    let path = artifact_uri
        .strip_prefix("artifact://")
        .unwrap_or(artifact_uri);
    match (
        session_id.filter(|value| !value.is_empty()),
        task_id.filter(|value| !value.is_empty()),
    ) {
        (Some(session_id), Some(task_id)) => Some(format!(
            "/api/v1/sessions/{session_id}/tasks/{task_id}/artifacts/{path}"
        )),
        _ => browser_path.map(|path| format!("/api/v1/browser-artifacts/{path}")),
    }
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

pub(crate) fn task_summary_is_fresh(incoming: &TaskSummary, existing: &TaskSummary) -> bool {
    match incoming.updated_at.cmp(&existing.updated_at) {
        std::cmp::Ordering::Greater => return true,
        std::cmp::Ordering::Less => return false,
        std::cmp::Ordering::Equal => {}
    }
    match incoming.last_event_seq.cmp(&existing.last_event_seq) {
        std::cmp::Ordering::Greater => return true,
        std::cmp::Ordering::Less => return false,
        std::cmp::Ordering::Equal => {}
    }

    incoming.status == existing.status
        || (task_status_is_closed(incoming.status) && !task_status_is_closed(existing.status))
}

const fn task_status_is_closed(status: TaskStatus) -> bool {
    matches!(status, TaskStatus::WaitingForUserInput) || status.is_terminal()
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
pub(crate) fn latest_pinned_todos(task_events: &[PersistedTaskEvent]) -> Option<Value> {
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
    fn artifact_image_url_uses_direct_browser_artifact_route_without_task_identity() {
        assert_eq!(
            artifact_image_url(
                None,
                None,
                "artifact://browser/owner/br/step-0001-milestone.jpg"
            ),
            Some("/api/v1/browser-artifacts/owner/br/step-0001-milestone.jpg".to_string())
        );
    }

    #[test]
    fn artifact_image_url_keeps_browser_artifacts_task_scoped_with_task_identity() {
        assert_eq!(
            artifact_image_url(
                Some("sess-1"),
                Some("task-1"),
                "artifact://browser/owner/br/step-0001-milestone.jpg"
            ),
            Some(
                "/api/v1/sessions/sess-1/tasks/task-1/artifacts/browser/owner/br/step-0001-milestone.jpg"
                    .to_string()
            )
        );
    }

    #[test]
    fn artifact_image_url_does_not_use_empty_life_task_identity_for_browser_artifacts() {
        assert_eq!(
            artifact_image_url(
                Some(""),
                Some(""),
                "artifact://browser/life-task/br-1/step-0001-milestone.jpg"
            ),
            Some("/api/v1/browser-artifacts/life-task/br-1/step-0001-milestone.jpg".to_string())
        );
    }

    #[test]
    fn artifact_image_url_keeps_legacy_task_artifacts_task_scoped() {
        assert_eq!(
            artifact_image_url(Some("sess-1"), Some("task-1"), "sandbox/output.txt"),
            Some("/api/v1/sessions/sess-1/tasks/task-1/artifacts/sandbox/output.txt".to_string())
        );
    }

    #[test]
    fn artifact_image_url_rejects_task_scoped_artifacts_without_task_identity() {
        assert_eq!(
            artifact_image_url(None, Some("task-1"), "sandbox/output.txt"),
            None
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

    fn progress(iteration: usize) -> ProgressSnapshot {
        ProgressSnapshot {
            current_iteration: iteration,
            max_iterations: 100,
            is_finished: false,
            error: None,
            current_thought: None,
            current_todos: None,
            last_compaction_status: None,
            repeated_compaction_warning: None,
            last_history_repair_status: None,
            latest_token_snapshot: None,
            llm_retry: None,
        }
    }

    #[test]
    fn activity_load_completion_only_updates_requested_task() {
        let mut states = HashMap::new();
        assert!(begin_activity_load(&mut states, "task-a"));
        assert!(begin_activity_load(&mut states, "task-b"));

        assert!(complete_activity_load(
            &mut states,
            "task-a",
            10,
            true,
            Some(progress(4)),
        ));

        assert!(matches!(states["task-a"].phase, ActivityLoadPhase::Ready));
        assert_eq!(states["task-a"].progress, Some(progress(4)));
        assert!(matches!(states["task-b"].phase, ActivityLoadPhase::Loading));
        assert_eq!(states["task-b"].progress, None);
    }

    #[test]
    fn live_progress_update_keeps_task_ownership() {
        let mut states = HashMap::from([
            ("task-a".to_string(), TaskActivityState::live()),
            ("task-b".to_string(), TaskActivityState::live()),
        ]);

        update_activity_progress(&mut states, "task-b".to_string(), Some(progress(7)));

        assert_eq!(states["task-a"].progress, None);
        assert_eq!(states["task-b"].progress, Some(progress(7)));
    }

    #[test]
    fn beginning_resumed_run_clears_progress_and_records_floor() {
        let mut state = TaskActivityState::live();
        state.progress = Some(progress(3));
        let mut states = HashMap::from([("task-a".to_string(), state)]);

        begin_activity_run(&mut states, "task-a".to_string(), 17);

        assert_eq!(states["task-a"].run_floor_seq, 17);
        assert_eq!(states["task-a"].progress, None);
    }

    #[test]
    fn stream_owner_requires_task_and_generation() {
        let owner = ("task-a".to_string(), 4);
        assert!(stream_owner_matches(Some(&owner), "task-a", 4));
        assert!(!stream_owner_matches(Some(&owner), "task-a", 3));
        assert!(!stream_owner_matches(Some(&owner), "task-b", 4));
    }

    #[test]
    fn inline_progress_orders_task_events_and_adjacent_deduplicates() {
        let mut other_task = task_event(
            99,
            TaskEventKind::Reasoning,
            serde_json::json!({ "summary": "other task" }),
        );
        other_task.task_id = "task-2".to_string();
        let events = vec![
            task_event(
                5,
                TaskEventKind::Reasoning,
                serde_json::json!({ "summary": "First status" }),
            ),
            task_event(
                2,
                TaskEventKind::Reasoning,
                serde_json::json!({ "summary": "First status" }),
            ),
            task_event(
                4,
                TaskEventKind::Reasoning,
                serde_json::json!({ "summary": "Second status" }),
            ),
            task_event(
                3,
                TaskEventKind::Reasoning,
                serde_json::json!({ "summary": "First status" }),
            ),
            other_task,
        ];
        let blocks = inline_progress_blocks("task-1", TaskStatus::Running, &events, None);

        assert_eq!(
            blocks
                .iter()
                .map(|block| (block.seq, block.headline.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (2, "First status"),
                (4, "Second status"),
                (5, "First status")
            ]
        );
    }

    #[test]
    fn inline_progress_maps_tool_calls_without_tool_results() {
        let events = vec![
            task_event(
                2,
                TaskEventKind::ToolCall,
                serde_json::json!({
                    "name": "web_search",
                    "input_preview": "{\"query\":\"oxide agent\"}",
                    "command_preview": null
                }),
            ),
            task_event(
                3,
                TaskEventKind::ToolResult,
                serde_json::json!({ "name": "web_search" }),
            ),
        ];

        assert_eq!(
            inline_progress_blocks("task-1", TaskStatus::Running, &events, None),
            vec![InlineProgressBlock {
                seq: 2,
                headline: "Searching the web".to_string(),
                detail: Some("oxide agent".to_string()),
            }]
        );
    }

    #[test]
    fn inline_progress_normalizes_probe_and_caps_newest_eight() {
        let mut events = vec![task_event(
            1,
            TaskEventKind::Reasoning,
            serde_json::json!({
                "summary": "Search Probe #2: Starting web research before the main answer."
            }),
        )];
        events.extend((2..=11).map(|seq| {
            task_event(
                seq,
                TaskEventKind::Reasoning,
                serde_json::json!({ "summary": format!("Search Probe #2: status {seq}") }),
            )
        }));

        let blocks = inline_progress_blocks("task-1", TaskStatus::Running, &events, None);
        assert_eq!(blocks.len(), 8);
        assert_eq!(blocks.first().map(|block| block.seq), Some(4));
        assert_eq!(
            blocks.last().map(|block| block.headline.as_str()),
            Some("status 11")
        );
    }

    #[test]
    fn inline_progress_is_live_only_and_uses_deterministic_placeholder() {
        assert_eq!(
            inline_progress_blocks("task-1", TaskStatus::Queued, &[], None),
            vec![InlineProgressBlock {
                seq: 0,
                headline: "Queued...".to_string(),
                detail: None,
            }]
        );
        assert_eq!(
            inline_progress_blocks("task-1", TaskStatus::Running, &[], None),
            vec![InlineProgressBlock {
                seq: 0,
                headline: "Thinking...".to_string(),
                detail: None,
            }]
        );
        assert!(
            inline_progress_blocks("task-1", TaskStatus::WaitingForUserInput, &[], None).is_empty()
        );
        assert!(inline_progress_blocks("task-1", TaskStatus::Completed, &[], None).is_empty());
    }

    #[test]
    fn inline_progress_run_floor_prevents_pre_resume_flash() {
        let events = vec![
            task_event(
                7,
                TaskEventKind::Reasoning,
                serde_json::json!({ "summary": "previous execution" }),
            ),
            task_event(
                9,
                TaskEventKind::Reasoning,
                serde_json::json!({ "summary": "resumed execution" }),
            ),
        ];
        let mut activity = TaskActivityState::live();
        activity.run_floor_seq = 8;

        assert_eq!(
            inline_progress_blocks("task-1", TaskStatus::Running, &events, Some(&activity)),
            vec![InlineProgressBlock {
                seq: 9,
                headline: "resumed execution".to_string(),
                detail: None,
            }]
        );
    }

    #[test]
    fn task_summary_freshness_prevents_terminal_resurrection() {
        let mut terminal = task(TaskStatus::Completed, Some("2026-06-11T00:00:05Z"));
        terminal.last_event_seq = 10;
        let mut stale_running = terminal.clone();
        stale_running.status = TaskStatus::Running;
        stale_running.last_event_seq = 9;

        assert!(!task_summary_is_fresh(&stale_running, &terminal));

        stale_running.last_event_seq = 10;
        assert!(!task_summary_is_fresh(&stale_running, &terminal));

        stale_running.updated_at = chrono::DateTime::parse_from_rfc3339("2026-06-11T00:00:06Z")
            .expect("timestamp")
            .with_timezone(&chrono::Utc);
        assert!(task_summary_is_fresh(&stale_running, &terminal));

        let mut waiting = terminal.clone();
        waiting.status = TaskStatus::WaitingForUserInput;
        assert!(!task_summary_is_fresh(&waiting, &terminal));
    }
}
