//! Record mappers, validators, and markdown preview helpers.

use super::{
    api_error, SerializableProgress, MAX_SESSION_TITLE_CHARS, MAX_TASK_INPUT_CHARS,
    TASK_PREVIEW_CHARS, WEB_SESSION_DEFAULT_TITLE,
};
use axum::{http::StatusCode, Json};
use oxide_agent_core::agent::PendingUserInput;
use oxide_agent_web_contracts::{
    ErrorCode, ErrorEnvelope, PendingUserInputView, ProgressSnapshot, SessionDetail,
    SessionSummary, TaskDetail, TaskSummary, UserInputKind as ApiUserInputKind, WebSessionRecord,
    WebTaskRecord,
};

// ---------------------------------------------------------------------------
// Validators
// ---------------------------------------------------------------------------

pub(crate) fn validate_session_title(
    title: &str,
) -> Result<String, (StatusCode, Json<ErrorEnvelope>)> {
    let title = title.trim();
    if title.is_empty() {
        return Err(api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationError,
            "Session title must not be empty.",
            false,
        ));
    }
    if title.chars().count() > MAX_SESSION_TITLE_CHARS {
        return Err(api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationError,
            format!("Session title must be at most {MAX_SESSION_TITLE_CHARS} characters."),
            false,
        ));
    }
    Ok(title.to_string())
}

pub(crate) fn validate_task_input(
    input: &str,
) -> Result<String, (StatusCode, Json<ErrorEnvelope>)> {
    let input = input.trim();
    if input.is_empty() {
        return Err(api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationError,
            "Task input must not be empty.",
            false,
        ));
    }
    if input.chars().count() > MAX_TASK_INPUT_CHARS {
        return Err(api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationError,
            format!("Task input must be at most {MAX_TASK_INPUT_CHARS} characters."),
            false,
        ));
    }
    Ok(input.to_string())
}

// ---------------------------------------------------------------------------
// Record mappers
// ---------------------------------------------------------------------------

pub(crate) fn session_summary_from_record(record: WebSessionRecord) -> SessionSummary {
    SessionSummary {
        session_id: record.session_id,
        title: record.title,
        last_preview: record.last_preview,
        active_task_id: record.active_task_id,
        last_task_status: record.last_task_status,
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}

pub(crate) fn session_detail_from_record(record: WebSessionRecord) -> SessionDetail {
    SessionDetail {
        session_id: record.session_id,
        title: record.title,
        last_preview: record.last_preview,
        active_task_id: record.active_task_id,
        last_task_status: record.last_task_status,
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}

pub(crate) fn task_summary_from_record(record: WebTaskRecord) -> TaskSummary {
    let version_group_id = record.effective_version_group_id().to_string();
    let version_index = record.effective_version_index();
    TaskSummary {
        task_id: record.task_id,
        version_group_id,
        version_index,
        parent_task_id: record.parent_task_id,
        status: record.status,
        input_markdown: record.input_markdown,
        input_edited_at: record.input_edited_at,
        final_response_markdown: record.final_response_markdown,
        error_message: record.error_message,
        pending_user_input: record.pending_user_input,
        last_event_seq: record.last_event_seq,
        created_at: record.created_at,
        started_at: record.started_at,
        updated_at: record.updated_at,
        finished_at: record.finished_at,
    }
}

pub(crate) fn task_detail_from_record(record: WebTaskRecord) -> TaskDetail {
    let version_group_id = record.effective_version_group_id().to_string();
    let version_index = record.effective_version_index();
    TaskDetail {
        task_id: record.task_id,
        session_id: record.session_id,
        version_group_id,
        version_index,
        parent_task_id: record.parent_task_id,
        status: record.status,
        input_markdown: record.input_markdown,
        input_edited_at: record.input_edited_at,
        final_response_markdown: record.final_response_markdown,
        error_message: record.error_message,
        pending_user_input: record.pending_user_input,
        last_progress: record.last_progress,
        last_event_seq: record.last_event_seq,
        created_at: record.created_at,
        started_at: record.started_at,
        updated_at: record.updated_at,
        finished_at: record.finished_at,
    }
}

// ---------------------------------------------------------------------------
// Markdown preview
// ---------------------------------------------------------------------------

pub(crate) fn markdown_preview(markdown: &str) -> String {
    let mut in_fenced_code = false;
    let normalized = markdown
        .lines()
        .filter_map(|line| markdown_preview_line(line, &mut in_fenced_code))
        .flat_map(|line| {
            line.split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>()
        .join(" ");
    let mut preview = normalized
        .chars()
        .take(TASK_PREVIEW_CHARS)
        .collect::<String>();
    if normalized.chars().count() > TASK_PREVIEW_CHARS {
        preview.push_str("...");
    }
    if preview.is_empty() {
        WEB_SESSION_DEFAULT_TITLE.to_string()
    } else {
        preview
    }
}

fn markdown_preview_line(line: &str, in_fenced_code: &mut bool) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
        *in_fenced_code = !*in_fenced_code;
        return None;
    }
    if *in_fenced_code {
        return None;
    }

    let stripped = strip_markdown_prefix(trimmed).trim();
    if stripped.is_empty() {
        None
    } else {
        Some(
            stripped
                .trim_matches(|ch| matches!(ch, '*' | '_' | '`'))
                .to_string(),
        )
    }
}

fn strip_markdown_prefix(mut value: &str) -> &str {
    loop {
        let trimmed = value.trim_start();
        if let Some(stripped) = trimmed.strip_prefix('#') {
            value = stripped;
            continue;
        }
        if let Some(stripped) = trimmed.strip_prefix('>') {
            value = stripped;
            continue;
        }
        if let Some(stripped) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "))
        {
            value = stripped;
            continue;
        }
        if let Some(stripped) = strip_ordered_list_prefix(trimmed) {
            value = stripped;
            continue;
        }
        return trimmed;
    }
}

fn strip_ordered_list_prefix(value: &str) -> Option<&str> {
    let (number, rest) = value.split_once(". ")?;
    if number.chars().all(|ch| ch.is_ascii_digit()) {
        Some(rest)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Pending user input & progress conversion
// ---------------------------------------------------------------------------

pub(crate) fn pending_user_input_view(pending: PendingUserInput) -> PendingUserInputView {
    let kind = match pending.kind {
        oxide_agent_core::agent::session::UserInputKind::Text => ApiUserInputKind::Text,
        oxide_agent_core::agent::session::UserInputKind::Url => ApiUserInputKind::Url,
        oxide_agent_core::agent::session::UserInputKind::File => ApiUserInputKind::File,
        oxide_agent_core::agent::session::UserInputKind::UrlOrFile => ApiUserInputKind::UrlOrFile,
    };
    PendingUserInputView {
        kind,
        prompt: pending.prompt,
    }
}

pub(crate) fn progress_snapshot_from_serializable(
    progress: SerializableProgress,
) -> ProgressSnapshot {
    ProgressSnapshot {
        current_iteration: progress.current_iteration,
        max_iterations: progress.max_iterations,
        is_finished: progress.is_finished,
        error: progress.error,
        current_thought: progress.current_thought,
        current_todos: progress.current_todos,
        last_compaction_status: progress.last_compaction_status,
        repeated_compaction_warning: progress.repeated_compaction_warning,
        last_history_repair_status: progress.last_history_repair_status,
        latest_token_snapshot: progress
            .latest_token_snapshot
            .and_then(|snapshot| serde_json::to_value(snapshot).ok()),
        llm_retry: progress.llm_retry,
        provider_failover_notice: progress.provider_failover_notice,
    }
}
