use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{
        header::{CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE},
        HeaderMap, HeaderValue, StatusCode,
    },
    response::Response,
    Json,
};
use oxide_agent_core::agent::providers::sandbox::SandboxRuntime;
use oxide_agent_core::agent::{AgentMessageAttachment, AgentUserInput};
use oxide_agent_core::sandbox::SandboxFileOps;
use oxide_agent_web_contracts::{
    CancelTaskResponse as ApiCancelTaskResponse, CreateTaskRequest as ApiCreateTaskRequest,
    CreateTaskResponse as ApiCreateTaskResponse,
    CreateTaskVersionRequest as ApiCreateTaskVersionRequest,
    CreateTaskVersionResponse as ApiCreateTaskVersionResponse, ErrorCode, ErrorEnvelope,
    GetTaskProgressResponse, GetTaskResponse, ListTasksResponse, PersistedTaskEvent,
    ResumeTaskRequest as ApiResumeTaskRequest, ResumeTaskResponse as ApiResumeTaskResponse,
    TaskAttachment, TaskEventKind, TaskEventsResponse, TaskStatus as ApiTaskStatus,
    UserMessageEventPayload, WebSessionRecord, WebTaskRecord,
};
use serde::Deserialize;

use crate::session::{RunningTask, WebSessionRuntimeOptions};

use super::task_executor::{self, TaskRunRequest, WebTaskPersistence};
use super::{
    api_error, authenticated_user, authenticated_user_with_csrf, auto_title,
    backend_unavailable_response, default_session_model_selection,
    load_execution_profile_for_agent_profile_id, load_owned_session, load_owned_task,
    markdown_preview, not_found_response, store_error_response, task_detail_from_record,
    task_summary_from_record, validate_task_input_with_attachments, AppState, TaskEventsQuery,
    DEFAULT_TASK_EVENTS_LIMIT, MAX_TASK_EVENTS_LIMIT, WEB_SESSION_DEFAULT_TITLE,
    WEB_TASK_SCHEMA_VERSION,
};

pub(crate) async fn abort_task_handle(state: &AppState, task_id: &str) {
    let handle = {
        let mut handles = state.task_handles.write().await;
        handles.remove(task_id)
    };
    if let Some(handle) = handle {
        handle.abort();
    }
}

fn format_attachment_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;

    if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

pub(crate) fn task_preview_source(input_markdown: &str, attachments: &[TaskAttachment]) -> String {
    let trimmed = input_markdown.trim();
    if !trimmed.is_empty() {
        return trimmed.to_string();
    }

    match attachments {
        [] => String::new(),
        [attachment] => format!("Attachment: {}", attachment.file_name),
        attachments => format!(
            "Attachments: {}",
            attachments
                .iter()
                .map(|attachment| attachment.file_name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

pub(crate) fn build_task_execution_input(
    input_markdown: &str,
    attachments: &[TaskAttachment],
) -> String {
    let trimmed = input_markdown.trim();
    if attachments.is_empty() {
        return trimmed.to_string();
    }

    let mut sections = Vec::new();
    if !trimmed.is_empty() {
        sections.push(trimmed.to_string());
    }

    let mut attachment_lines =
        vec!["📎 User attached files that are already staged in the sandbox:".to_string()];
    for attachment in attachments {
        let mut line = format!(
            "- `{}` ({}) at `{}`",
            attachment.file_name,
            format_attachment_size(attachment.size_bytes),
            attachment.sandbox_path,
        );
        if let Some(mime_type) = &attachment.mime_type {
            line.push_str(&format!(" [{mime_type}]"));
        }
        attachment_lines.push(line);
    }
    attachment_lines.push(
        "These uploaded files are sandbox-local and will be lost if this sandbox is destroyed or recreated."
            .to_string(),
    );
    sections.push(attachment_lines.join("\n"));

    sections.join("\n\n")
}

pub(crate) fn build_task_agent_user_input(
    input_markdown: &str,
    attachments: &[TaskAttachment],
) -> AgentUserInput {
    AgentUserInput::new(build_task_execution_input(input_markdown, attachments))
        .with_attachments(web_image_attachment_refs(attachments))
}

fn web_image_attachment_refs(attachments: &[TaskAttachment]) -> Vec<AgentMessageAttachment> {
    attachments
        .iter()
        .filter(|attachment| is_image_attachment(attachment))
        .map(|attachment| {
            AgentMessageAttachment::image(
                attachment.file_name.clone(),
                attachment.mime_type.clone(),
                attachment.size_bytes,
                attachment.sandbox_path.clone(),
            )
        })
        .collect()
}

fn is_image_attachment(attachment: &TaskAttachment) -> bool {
    attachment
        .mime_type
        .as_deref()
        .is_some_and(|mime_type| mime_type.trim().to_ascii_lowercase().starts_with("image/"))
}

async fn best_effort_copy_attachments_between_web_sandboxes(
    user_id: i64,
    source_context_key: &str,
    target_context_key: &str,
    attachments: &[TaskAttachment],
) {
    if attachments.is_empty() || source_context_key == target_context_key {
        return;
    }

    let source = SandboxRuntime::new(crate::session::web_session_sandbox_scope(
        user_id,
        source_context_key,
    ));
    let target = SandboxRuntime::new(crate::session::web_session_sandbox_scope(
        user_id,
        target_context_key,
    ));

    for attachment in attachments {
        let bytes = match source.read_file(&attachment.sandbox_path).await {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(
                    source_context_key,
                    target_context_key,
                    path = attachment.sandbox_path.as_str(),
                    error = %error,
                    "Could not copy task-version attachment from source sandbox; continuing with text fallback"
                );
                continue;
            }
        };

        if let Err(error) = target.write_file(&attachment.sandbox_path, &bytes).await {
            tracing::warn!(
                source_context_key,
                target_context_key,
                path = attachment.sandbox_path.as_str(),
                error = %error,
                "Could not copy task-version attachment into branch sandbox; continuing with text fallback"
            );
        }
    }
}

fn persisted_user_message_event(
    task: &WebTaskRecord,
    seq: u64,
    created_at: chrono::DateTime<chrono::Utc>,
    input_markdown: &str,
    attachments: &[TaskAttachment],
) -> PersistedTaskEvent {
    PersistedTaskEvent {
        schema_version: 1,
        task_id: task.task_id.clone(),
        session_id: task.session_id.clone(),
        user_id: task.user_id,
        seq,
        created_at,
        kind: TaskEventKind::UserMessage,
        summary: markdown_preview(&task_preview_source(input_markdown, attachments)),
        payload: serde_json::to_value(UserMessageEventPayload {
            input_markdown: input_markdown.to_string(),
            attachments: attachments.to_vec(),
        })
        .expect("user message payload serializes"),
        redacted: false,
        truncated: false,
    }
}

fn validate_task_attachments(
    attachments: &[TaskAttachment],
) -> Result<Vec<TaskAttachment>, (StatusCode, Json<ErrorEnvelope>)> {
    if attachments
        .iter()
        .any(|attachment| attachment.file_name.trim().is_empty())
    {
        return Err(api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationError,
            "Attachment file_name must not be empty.",
            false,
        ));
    }
    if attachments
        .iter()
        .any(|attachment| !attachment.sandbox_path.starts_with("/workspace/uploads/"))
    {
        return Err(api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationError,
            "Attachment sandbox_path must point to /workspace/uploads/.",
            false,
        ));
    }

    Ok(attachments.to_vec())
}

struct RunningWebTaskRecordInput {
    task_id: String,
    session_id: String,
    user_id: i64,
    input_markdown: String,
    attachments: Vec<TaskAttachment>,
    version_group_id: String,
    version_index: u32,
    parent_task_id: Option<String>,
    input_edited_at: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
}

fn new_running_web_task_record(input: RunningWebTaskRecordInput) -> WebTaskRecord {
    WebTaskRecord {
        schema_version: WEB_TASK_SCHEMA_VERSION,
        task_id: input.task_id,
        session_id: input.session_id,
        user_id: input.user_id,
        version_group_id: input.version_group_id,
        version_index: input.version_index,
        parent_task_id: input.parent_task_id,
        status: ApiTaskStatus::Running,
        input_markdown: input.input_markdown,
        attachments: input.attachments,
        input_edited_at: input.input_edited_at,
        final_response_markdown: None,
        error_message: None,
        pending_user_input: None,
        last_progress: None,
        last_event_seq: 0,
        created_at: input.now,
        started_at: Some(input.now),
        updated_at: input.now,
        finished_at: None,
    }
}

async fn save_session_record(
    state: &AppState,
    session: WebSessionRecord,
) -> Result<(), (StatusCode, Json<ErrorEnvelope>)> {
    state
        .web_store
        .save_session(session)
        .await
        .map_err(store_error_response)
}

async fn save_session_task_update(
    state: &AppState,
    mut session: WebSessionRecord,
    now: chrono::DateTime<chrono::Utc>,
    active_task_id: String,
    status: ApiTaskStatus,
    preview: String,
) -> Result<(), (StatusCode, Json<ErrorEnvelope>)> {
    session.active_task_id = Some(active_task_id);
    session.last_task_status = Some(status);
    session.last_preview = Some(preview);
    session.updated_at = now;
    save_session_record(state, session).await
}

async fn save_cancelled_session_task_status(
    state: &AppState,
    mut session: WebSessionRecord,
    task_id: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), (StatusCode, Json<ErrorEnvelope>)> {
    if session.active_task_id.as_deref() == Some(task_id) {
        session.active_task_id = None;
    }
    session.last_task_status = Some(ApiTaskStatus::Cancelled);
    session.updated_at = now;
    save_session_record(state, session).await
}

async fn spawn_persisted_registered_task(
    state: &AppState,
    user_id: i64,
    session_id: String,
    task_id: String,
    running_task: RunningTask,
    run_request: TaskRunRequest,
) {
    let persistence = WebTaskPersistence {
        web_store: state.web_store.clone(),
        user_id,
        session_id: session_id.clone(),
        task_id,
    };
    task_executor::spawn_registered_task(
        state.clone(),
        session_id,
        running_task,
        run_request,
        Some(persistence),
    )
    .await;
}

pub(crate) async fn api_list_tasks(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Result<Json<ListTasksResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user(&state, &headers).await?;
    let _session = load_owned_session(&state, user.user_id, &session_id).await?;
    let task_records = state
        .web_store
        .list_tasks(user.user_id, &session_id)
        .await
        .map_err(store_error_response)?;
    let tasks_count = task_records.len();
    let total_last_event_seq: u64 = task_records.iter().map(|task| task.last_event_seq).sum();
    let tasks = task_records
        .into_iter()
        .map(task_summary_from_record)
        .collect();
    tracing::debug!(
        target: "oxide_agent_transport_web::web_perf",
        user_id = user.user_id,
        session_id = %session_id,
        tasks_count,
        total_last_event_seq,
        "web tasks listed"
    );
    Ok(Json(ListTasksResponse { tasks }))
}

pub(crate) async fn api_create_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(request): Json<ApiCreateTaskRequest>,
) -> Result<Json<ApiCreateTaskResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    let mut session = load_owned_session(&state, user.user_id, &session_id).await?;
    let attachments = validate_task_attachments(&request.attachments)?;
    let input_markdown =
        validate_task_input_with_attachments(&request.input_markdown, !attachments.is_empty())?;
    let execution_input = build_task_agent_user_input(&input_markdown, &attachments);
    reject_active_task(&state, user.user_id, &session_id).await?;

    ensure_runtime_session(&state, user.user_id, &session).await;
    let Some(running_task) = state
        .session_manager
        .register_task(&session_id, execution_input.text_projection().to_string())
        .await
    else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::BackendUnavailable,
            "Failed to register runtime task.",
            true,
        ));
    };

    let now = chrono::Utc::now();
    let task_id = running_task.task_id.clone();

    // Check whether this is the first task BEFORE saving the new one,
    // otherwise list_tasks would already include it and is_empty()
    // would always return false.
    let is_first_task = state
        .web_store
        .list_tasks(user.user_id, &session_id)
        .await
        .map_err(store_error_response)?
        .is_empty();

    let task_record = new_running_web_task_record(RunningWebTaskRecordInput {
        task_id: task_id.clone(),
        session_id: session_id.clone(),
        user_id: user.user_id,
        input_markdown: input_markdown.clone(),
        attachments: attachments.clone(),
        version_group_id: task_id.clone(),
        version_index: 1,
        parent_task_id: None,
        input_edited_at: None,
        now,
    });
    state
        .web_store
        .save_task(task_record.clone())
        .await
        .map_err(store_error_response)?;

    let preview_source = task_preview_source(&input_markdown, &attachments);
    let preview = markdown_preview(&preview_source);
    let should_auto_title = is_first_task && !session.manually_renamed;

    if should_auto_title && state.auto_title_enabled {
        auto_title::prepare_session_auto_title(
            &mut session,
            preview_source.clone(),
            preview.clone(),
            now,
        );
    }
    save_session_task_update(
        &state,
        session,
        now,
        task_id.clone(),
        ApiTaskStatus::Running,
        preview.clone(),
    )
    .await?;

    spawn_persisted_registered_task(
        &state,
        user.user_id,
        session_id.clone(),
        task_id.clone(),
        running_task,
        TaskRunRequest::Execute {
            input: execution_input,
            effort: request.effort,
        },
    )
    .await;

    if should_auto_title && state.auto_title_enabled {
        auto_title::spawn_background_auto_title(state.clone(), user.user_id, session_id);
    }

    Ok(Json(ApiCreateTaskResponse {
        task: task_summary_from_record(task_record),
    }))
}

pub(crate) async fn api_get_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((session_id, task_id)): Path<(String, String)>,
) -> Result<Json<GetTaskResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user(&state, &headers).await?;
    let task = load_owned_task(&state, user.user_id, &session_id, &task_id).await?;
    Ok(Json(GetTaskResponse {
        task: task_detail_from_record(task),
    }))
}

pub(crate) async fn api_get_task_progress(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((session_id, task_id)): Path<(String, String)>,
) -> Result<Json<GetTaskProgressResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user(&state, &headers).await?;
    let task = load_owned_task(&state, user.user_id, &session_id, &task_id).await?;
    Ok(Json(GetTaskProgressResponse {
        task_id: task.task_id,
        status: task.status,
        progress: task.last_progress,
        last_event_seq: task.last_event_seq,
        updated_at: task.updated_at,
    }))
}

pub(crate) async fn api_get_task_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((session_id, task_id)): Path<(String, String)>,
    Query(query): Query<TaskEventsQuery>,
) -> Result<Json<TaskEventsResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user(&state, &headers).await?;
    let _task = load_owned_task(&state, user.user_id, &session_id, &task_id).await?;
    let after_seq = query.after_seq.unwrap_or_default();
    let limit = query
        .limit
        .unwrap_or(DEFAULT_TASK_EVENTS_LIMIT)
        .clamp(1, MAX_TASK_EVENTS_LIMIT);

    let events = state
        .web_store
        .list_task_events(user.user_id, &session_id, &task_id, after_seq, limit)
        .await
        .map_err(store_error_response)?;
    tracing::debug!(
        target: "oxide_agent_transport_web::web_perf",
        user_id = user.user_id,
        session_id = %session_id,
        task_id = %task_id,
        after_seq,
        limit,
        events_count = events.events.len(),
        last_seq = events.last_seq,
        has_more = events.has_more,
        "web task events listed"
    );
    Ok(Json(events))
}

pub(crate) async fn api_download_task_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((session_id, task_id, file_id)): Path<(String, String, String)>,
    Query(query): Query<TaskFileDownloadQuery>,
) -> Result<Response, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user(&state, &headers).await?;
    let _task = load_owned_task(&state, user.user_id, &session_id, &task_id).await?;
    let Some(file) = state
        .web_store
        .load_task_file(user.user_id, &session_id, &task_id, &file_id)
        .await
        .map_err(store_error_response)?
    else {
        return Err(not_found_response());
    };

    let mut response = Response::new(Body::from(file.content));
    let headers = response.headers_mut();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_str(&file.record.content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    if let Ok(length) = HeaderValue::from_str(&file.record.size_bytes.to_string()) {
        headers.insert(CONTENT_LENGTH, length);
    }
    headers.insert(
        CONTENT_DISPOSITION,
        content_disposition_header(&file.record.file_name, query.inline()),
    );
    Ok(response)
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct TaskFileDownloadQuery {
    disposition: Option<String>,
}

impl TaskFileDownloadQuery {
    fn inline(&self) -> bool {
        self.disposition
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("inline"))
    }
}

fn content_disposition_header(file_name: &str, inline: bool) -> HeaderValue {
    let sanitized = file_name
        .chars()
        .map(|ch| match ch {
            '"' | '\\' | '\r' | '\n' => '_',
            _ => ch,
        })
        .collect::<String>();
    let disposition = if inline { "inline" } else { "attachment" };
    HeaderValue::from_str(&format!("{disposition}; filename=\"{sanitized}\""))
        .unwrap_or_else(|_| HeaderValue::from_static("attachment"))
}

pub(crate) async fn api_create_task_version(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((session_id, task_id)): Path<(String, String)>,
    Json(request): Json<ApiCreateTaskVersionRequest>,
) -> Result<Json<ApiCreateTaskVersionResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    let mut session = load_owned_session(&state, user.user_id, &session_id).await?;
    let attachments = validate_task_attachments(&request.attachments)?;
    let input_markdown =
        validate_task_input_with_attachments(&request.input_markdown, !attachments.is_empty())?;
    let execution_input = build_task_agent_user_input(&input_markdown, &attachments);
    let parent_task = load_owned_task(&state, user.user_id, &session_id, &task_id).await?;
    if !parent_task.status.is_terminal() {
        return Err(api_error(
            StatusCode::CONFLICT,
            ErrorCode::TaskActive,
            "Only terminal tasks can be versioned.",
            false,
        ));
    }

    let tasks = state
        .web_store
        .list_tasks(user.user_id, &session_id)
        .await
        .map_err(store_error_response)?;
    let latest_task_id = tasks
        .iter()
        .max_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.task_id.cmp(&b.task_id))
        })
        .map(|task| task.task_id.as_str());
    if latest_task_id != Some(task_id.as_str()) {
        return Err(api_error(
            StatusCode::CONFLICT,
            ErrorCode::Conflict,
            "Only the latest task in a session can be versioned.",
            false,
        ));
    }

    let now = chrono::Utc::now();
    let source_context_key = session.context_key.clone();
    let branch_context_key = web_task_version_context_key(&session_id);
    if session.context_keys.is_empty() {
        session.context_keys.push(session.context_key.clone());
    }
    if !session.context_keys.contains(&branch_context_key) {
        session.context_keys.push(branch_context_key.clone());
    }
    session.context_key = branch_context_key;
    session.updated_at = now;
    state
        .sandbox_control()
        .ensure_scope_sandbox(crate::session::web_session_sandbox_scope(
            user.user_id,
            &session.context_key,
        ))
        .await
        .map_err(|error| backend_unavailable_response(error.to_string()))?;
    best_effort_copy_attachments_between_web_sandboxes(
        user.user_id,
        &source_context_key,
        &session.context_key,
        &attachments,
    )
    .await;
    save_session_record(&state, session.clone()).await?;

    recreate_runtime_session(&state, user.user_id, &session).await;
    let Some(running_task) = state
        .session_manager
        .register_task(&session_id, execution_input.text_projection().to_string())
        .await
    else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::BackendUnavailable,
            "Failed to register runtime task.",
            true,
        ));
    };

    let version_group_id = parent_task.effective_version_group_id().to_string();
    let version_index = tasks
        .iter()
        .filter(|task| task.effective_version_group_id() == version_group_id.as_str())
        .map(WebTaskRecord::effective_version_index)
        .max()
        .unwrap_or(parent_task.effective_version_index())
        + 1;
    let version_task_id = running_task.task_id.clone();
    let task = new_running_web_task_record(RunningWebTaskRecordInput {
        task_id: version_task_id.clone(),
        session_id: session_id.clone(),
        user_id: user.user_id,
        input_markdown: input_markdown.clone(),
        attachments: attachments.clone(),
        version_group_id,
        version_index,
        parent_task_id: Some(parent_task.task_id.clone()),
        input_edited_at: Some(now),
        now,
    });
    state
        .web_store
        .save_task(task.clone())
        .await
        .map_err(store_error_response)?;

    let preview_source = task_preview_source(&input_markdown, &attachments);
    let preview = markdown_preview(&preview_source);
    let old_preview = session.last_preview.clone();
    // Only update title from preview when it is still the default or the
    // previous fallback preview.  An LLM-generated auto-title must not be
    // overwritten by an edit.
    let title_is_still_fallback = tasks.len() == 1
        && !session.manually_renamed
        && (session.title == WEB_SESSION_DEFAULT_TITLE
            || session.title == old_preview.as_deref().unwrap_or(""));
    if title_is_still_fallback {
        session.title = preview.clone();
    }
    save_session_task_update(
        &state,
        session,
        now,
        version_task_id.clone(),
        ApiTaskStatus::Running,
        preview,
    )
    .await?;

    spawn_persisted_registered_task(
        &state,
        user.user_id,
        session_id,
        version_task_id,
        running_task,
        TaskRunRequest::Execute {
            input: execution_input,
            effort: request.effort,
        },
    )
    .await;

    Ok(Json(ApiCreateTaskVersionResponse {
        task: task_summary_from_record(task),
    }))
}

pub(crate) async fn api_resume_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((session_id, task_id)): Path<(String, String)>,
    Json(request): Json<ApiResumeTaskRequest>,
) -> Result<Json<ApiResumeTaskResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    let attachments = validate_task_attachments(&request.attachments)?;
    let input_markdown =
        validate_task_input_with_attachments(&request.input_markdown, !attachments.is_empty())?;
    let execution_input = build_task_agent_user_input(&input_markdown, &attachments);
    let session = load_owned_session(&state, user.user_id, &session_id).await?;
    let mut task = load_owned_task(&state, user.user_id, &session_id, &task_id).await?;
    if task.status != ApiTaskStatus::WaitingForUserInput {
        return Err(api_error(
            StatusCode::CONFLICT,
            ErrorCode::TaskNotRunning,
            "Task is not waiting for user input.",
            false,
        ));
    }
    if session.active_task_id.as_deref() != Some(task_id.as_str()) {
        return Err(api_error(
            StatusCode::CONFLICT,
            ErrorCode::Conflict,
            "Session active task does not match the task being resumed.",
            false,
        ));
    }

    ensure_runtime_session(&state, user.user_id, &session).await;
    let Some(running_task) = state
        .session_manager
        .register_existing_task(
            &session_id,
            &task_id,
            execution_input.text_projection().to_string(),
        )
        .await
    else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::BackendUnavailable,
            "Failed to register runtime task resume.",
            true,
        ));
    };

    let now = chrono::Utc::now();
    let resume_event = persisted_user_message_event(
        &task,
        task.last_event_seq.saturating_add(1),
        now,
        &input_markdown,
        &attachments,
    );
    task.status = ApiTaskStatus::Running;
    task.error_message = None;
    task.pending_user_input = None;
    task.last_event_seq = resume_event.seq;
    task.updated_at = now;
    task.finished_at = None;
    if task.started_at.is_none() {
        task.started_at = Some(now);
    }
    state
        .web_store
        .append_task_events(user.user_id, &session_id, &task_id, vec![resume_event])
        .await
        .map_err(store_error_response)?;
    state
        .web_store
        .save_task(task.clone())
        .await
        .map_err(store_error_response)?;

    let preview = markdown_preview(&task_preview_source(&input_markdown, &attachments));
    save_session_task_update(
        &state,
        session,
        now,
        task_id.clone(),
        ApiTaskStatus::Running,
        preview,
    )
    .await?;

    spawn_persisted_registered_task(
        &state,
        user.user_id,
        session_id,
        task_id.clone(),
        running_task,
        TaskRunRequest::ResumeUserInput {
            input: execution_input,
            effort: request.effort,
        },
    )
    .await;

    Ok(Json(ApiResumeTaskResponse {
        task: task_summary_from_record(task),
    }))
}

pub(crate) async fn api_cancel_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((session_id, task_id)): Path<(String, String)>,
) -> Result<Json<ApiCancelTaskResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    let mut task = load_owned_task(&state, user.user_id, &session_id, &task_id).await?;
    if task.status.is_terminal() {
        return Ok(Json(ApiCancelTaskResponse {
            ok: task.status == ApiTaskStatus::Cancelled,
            status: task.status,
        }));
    }

    let now = chrono::Utc::now();
    task.status = ApiTaskStatus::Cancelled;
    task.error_message = None;
    task.pending_user_input = None;
    task.updated_at = now;
    task.finished_at = Some(now);
    state
        .web_store
        .save_task(task)
        .await
        .map_err(store_error_response)?;

    let session = load_owned_session(&state, user.user_id, &session_id).await?;
    save_cancelled_session_task_status(&state, session, &task_id, now).await?;

    state
        .session_manager
        .cancel_task(&task_id, &session_id)
        .await;
    abort_task_handle(&state, &task_id).await;

    Ok(Json(ApiCancelTaskResponse {
        ok: true,
        status: ApiTaskStatus::Cancelled,
    }))
}

pub(crate) async fn reject_active_task(
    state: &AppState,
    user_id: i64,
    session_id: &str,
) -> Result<(), (StatusCode, Json<ErrorEnvelope>)> {
    let session = load_owned_session(state, user_id, session_id).await?;
    let Some(active_task_id) = session.active_task_id else {
        return Ok(());
    };

    let Some(task) = state
        .web_store
        .load_task(user_id, session_id, &active_task_id)
        .await
        .map_err(store_error_response)?
    else {
        return Ok(());
    };

    if task.status == ApiTaskStatus::WaitingForUserInput {
        return Err((
            StatusCode::CONFLICT,
            Json(
                ErrorEnvelope::new(
                    ErrorCode::TaskWaitingForUserInput,
                    "The current task is waiting for user input.",
                    false,
                )
                .with_details(serde_json::json!({ "task_id": active_task_id })),
            ),
        ));
    }

    if task.status.is_active() {
        return Err(api_error(
            StatusCode::CONFLICT,
            ErrorCode::SessionBusy,
            "The session already has an active task.",
            false,
        ));
    }

    Ok(())
}

async fn ensure_runtime_session(state: &AppState, user_id: i64, session: &WebSessionRecord) {
    if state
        .session_manager
        .get_session(&session.session_id)
        .await
        .is_some()
    {
        return;
    }

    materialize_runtime_session(state, user_id, session).await;
}

async fn recreate_runtime_session(state: &AppState, user_id: i64, session: &WebSessionRecord) {
    materialize_runtime_session(state, user_id, session).await;
}

async fn materialize_runtime_session(state: &AppState, user_id: i64, session: &WebSessionRecord) {
    let execution_profile = match load_execution_profile_for_agent_profile_id(
        state,
        user_id,
        session.agent_profile_id.as_deref(),
    )
    .await
    {
        Ok(execution_profile) => execution_profile,
        Err((_, Json(error))) => {
            tracing::warn!(
                user_id,
                session_id = %session.session_id,
                message = %error.error.message,
                "Failed to load web agent profile for runtime session restore"
            );
            None
        }
    };

    state
        .session_manager
        .create_session_with_model_selection(
            user_id,
            session.session_id.clone(),
            session.context_key.clone(),
            session.agent_flow_id.clone(),
            WebSessionRuntimeOptions {
                model_selection: session
                    .model_selection
                    .clone()
                    .or_else(|| Some(default_session_model_selection(state))),
                agent_profile_id: session.agent_profile_id.clone(),
                execution_profile,
            },
        )
        .await;
}

fn web_task_version_context_key(session_id: &str) -> String {
    format!("web-session-{session_id}-branch-{}", uuid::Uuid::new_v4())
}
