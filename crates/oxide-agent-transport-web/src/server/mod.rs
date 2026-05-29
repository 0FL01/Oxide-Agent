//! HTTP server for the web transport.
//!
//! ## Endpoints
//!
//! - `GET /health`
//! - `GET /api/v1/public-config` — browser-safe web console config
//! - `POST /api/v1/auth/register` — register user when enabled
//! - `POST /api/v1/auth/bootstrap` — create the first admin with a bootstrap token
//! - `POST /api/v1/auth/login` — create browser auth session
//! - `GET /api/v1/me` — current browser user
//! - `POST /api/v1/auth/logout` — revoke browser auth session
//! - `POST /api/v1/auth/change-password` — change current user's password
//! - `GET /api/v1/sessions` — list current user's web sessions
//! - `POST /api/v1/sessions` — create current user's web session
//! - `GET /api/v1/sessions/:session_id` — get current user's web session
//! - `PATCH /api/v1/sessions/:session_id` — rename current user's web session
//! - `DELETE /api/v1/sessions/:session_id` — delete current user's web session

mod auth_helpers;
mod auto_title;
mod converters;
mod sse;
mod static_assets;
mod task_executor;
mod types;
use auth_helpers::*;
use converters::*;
pub use types::*;

use crate::auth::{
    bootstrap_user, change_password, create_auth_session_for_user, current_user_for_token,
    login_user, logout_session, register_user,
};
use crate::persistence::WebUiStore;
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{
        header::{CONTENT_SECURITY_POLICY, SET_COOKIE},
        HeaderMap, HeaderValue, Request, StatusCode,
    },
    middleware::{self, Next},
    response::Response,
    routing::{delete, get, patch, post},
    Json, Router,
};
use oxide_agent_web_contracts::{
    AuthUserResponse, BootstrapRequest, CancelTaskResponse as ApiCancelTaskResponse,
    ChangePasswordRequest, CreateSessionResponse as ApiCreateSessionResponse,
    CreateTaskRequest as ApiCreateTaskRequest, CreateTaskResponse as ApiCreateTaskResponse,
    CurrentUser, CurrentUserResponse, EditTaskInputRequest as ApiEditTaskInputRequest,
    EditTaskInputResponse as ApiEditTaskInputResponse, ErrorCode, ErrorEnvelope,
    GetSessionResponse, GetTaskProgressResponse, GetTaskResponse, ListSessionsResponse,
    ListTasksResponse, LoginRequest, OkResponse, PublicConfigResponse, RegisterRequest,
    ResumeTaskRequest as ApiResumeTaskRequest, ResumeTaskResponse as ApiResumeTaskResponse,
    TaskEventsResponse, TaskStatus as ApiTaskStatus, UpdateSessionRequest, UpdateSessionResponse,
    WebSessionRecord, WebTaskRecord,
};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn api_public_config(State(state): State<AppState>) -> Json<PublicConfigResponse> {
    let registration_enabled = web_bool_env("OXIDE_WEB_REGISTRATION_ENABLED");
    let bootstrap_token_configured = web_non_empty_env("OXIDE_WEB_BOOTSTRAP_TOKEN");
    let users_count = state.web_store.users_count().await.unwrap_or(u64::MAX);

    Json(PublicConfigResponse {
        registration_enabled,
        bootstrap_required: web_bootstrap_required(
            registration_enabled,
            users_count,
            bootstrap_token_configured,
        ),
        build_version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

async fn api_register(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<RegisterRequest>,
) -> Result<(HeaderMap, Json<AuthUserResponse>), (StatusCode, Json<ErrorEnvelope>)> {
    let rate_limit_key = auth_rate_limit_key(&headers, &request.login);
    reject_auth_rate_limited(&state, &rate_limit_key).await?;
    let result = register_user(
        state.web_store.as_ref(),
        request,
        web_bool_env("OXIDE_WEB_REGISTRATION_ENABLED"),
        chrono::Utc::now(),
    )
    .await;
    let user = match result {
        Ok(user) => {
            clear_auth_rate_limit(&state, &rate_limit_key).await;
            user
        }
        Err(error) => {
            record_auth_failure(&state, rate_limit_key).await;
            return Err(auth_error_response(error));
        }
    };
    auth_session_response(state.web_store.as_ref(), user, chrono::Utc::now()).await
}

async fn api_bootstrap(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<BootstrapRequest>,
) -> Result<(HeaderMap, Json<AuthUserResponse>), (StatusCode, Json<ErrorEnvelope>)> {
    let rate_limit_key = auth_rate_limit_key(&headers, &request.login);
    reject_auth_rate_limited(&state, &rate_limit_key).await?;
    let bootstrap_token = web_env_value("OXIDE_WEB_BOOTSTRAP_TOKEN");
    let result = bootstrap_user(
        state.web_store.as_ref(),
        request,
        bootstrap_token.as_deref(),
        chrono::Utc::now(),
    )
    .await;
    let user = match result {
        Ok(user) => {
            clear_auth_rate_limit(&state, &rate_limit_key).await;
            user
        }
        Err(error) => {
            record_auth_failure(&state, rate_limit_key).await;
            return Err(auth_error_response(error));
        }
    };
    auth_session_response(state.web_store.as_ref(), user, chrono::Utc::now()).await
}

async fn api_login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<LoginRequest>,
) -> Result<(HeaderMap, Json<AuthUserResponse>), (StatusCode, Json<ErrorEnvelope>)> {
    let rate_limit_key = auth_rate_limit_key(&headers, &request.login);
    reject_auth_rate_limited(&state, &rate_limit_key).await?;
    let result = login_user(state.web_store.as_ref(), request, chrono::Utc::now()).await;
    let (user, auth_session, raw_session_token) = match result {
        Ok(result) => {
            clear_auth_rate_limit(&state, &rate_limit_key).await;
            result
        }
        Err(error) => {
            record_auth_failure(&state, rate_limit_key).await;
            return Err(auth_error_response(error));
        }
    };
    let mut headers = HeaderMap::new();
    headers.insert(
        SET_COOKIE,
        auth_cookie_header(&raw_session_token, AUTH_SESSION_TTL_SECS)?,
    );
    Ok((
        headers,
        Json(AuthUserResponse {
            user,
            csrf_token: Some(auth_session.csrf_token),
        }),
    ))
}

async fn auth_session_response(
    store: &dyn WebUiStore,
    user: CurrentUser,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(HeaderMap, Json<AuthUserResponse>), (StatusCode, Json<ErrorEnvelope>)> {
    let (auth_session, raw_session_token) = create_auth_session_for_user(store, user.user_id, now)
        .await
        .map_err(auth_error_response)?;
    let mut headers = HeaderMap::new();
    headers.insert(
        SET_COOKIE,
        auth_cookie_header(&raw_session_token, AUTH_SESSION_TTL_SECS)?,
    );
    Ok((
        headers,
        Json(AuthUserResponse {
            user,
            csrf_token: Some(auth_session.csrf_token),
        }),
    ))
}

async fn api_me(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<CurrentUserResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let raw_session_token = auth_cookie_value(&headers).map_err(auth_error_response)?;
    let (user, auth_session) = current_user_for_token(
        state.web_store.as_ref(),
        &raw_session_token,
        chrono::Utc::now(),
    )
    .await
    .map_err(auth_error_response)?;
    Ok(Json(CurrentUserResponse {
        user,
        csrf_token: auth_session.csrf_token,
    }))
}

async fn api_logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<(HeaderMap, Json<OkResponse>), (StatusCode, Json<ErrorEnvelope>)> {
    validate_csrf_request_origin(&headers)?;
    let raw_session_token = auth_cookie_value(&headers).map_err(auth_error_response)?;
    let csrf_token = csrf_header_value(&headers).map_err(auth_error_response)?;
    logout_session(
        state.web_store.as_ref(),
        &raw_session_token,
        &csrf_token,
        chrono::Utc::now(),
    )
    .await
    .map_err(auth_error_response)?;

    let mut response_headers = HeaderMap::new();
    response_headers.insert(SET_COOKIE, expired_auth_cookie_header()?);
    Ok((response_headers, Json(OkResponse::ok())))
}

async fn api_change_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ChangePasswordRequest>,
) -> Result<Json<OkResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    validate_csrf_request_origin(&headers)?;
    let raw_session_token = auth_cookie_value(&headers).map_err(auth_error_response)?;
    let csrf_token = csrf_header_value(&headers).map_err(auth_error_response)?;
    change_password(
        state.web_store.as_ref(),
        &raw_session_token,
        &csrf_token,
        request,
        chrono::Utc::now(),
    )
    .await
    .map_err(auth_error_response)?;
    Ok(Json(OkResponse::ok()))
}

async fn api_list_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ListSessionsResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user(&state, &headers).await?;
    let sessions = state
        .web_store
        .list_sessions(user.user_id)
        .await
        .map_err(store_error_response)?
        .into_iter()
        .map(session_summary_from_record)
        .collect();
    Ok(Json(ListSessionsResponse { sessions }))
}

async fn api_create_session(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ApiCreateSessionResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    let session_id = uuid::Uuid::new_v4().to_string();
    let context_key = format!("web-session-{session_id}");
    let now = chrono::Utc::now();
    state
        .session_manager
        .create_session_with_id(
            user.user_id,
            session_id.clone(),
            context_key.clone(),
            WEB_SESSION_FLOW_ID.to_string(),
        )
        .await;

    let record = WebSessionRecord {
        schema_version: WEB_SESSION_SCHEMA_VERSION,
        session_id,
        user_id: user.user_id,
        title: WEB_SESSION_DEFAULT_TITLE.to_string(),
        context_key,
        agent_flow_id: WEB_SESSION_FLOW_ID.to_string(),
        created_at: now,
        updated_at: now,
        active_task_id: None,
        last_task_status: None,
        last_preview: None,
        manually_renamed: false,
    };
    state
        .web_store
        .save_session(record.clone())
        .await
        .map_err(store_error_response)?;
    Ok(Json(ApiCreateSessionResponse {
        session: session_summary_from_record(record),
    }))
}

async fn api_get_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Result<Json<GetSessionResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user(&state, &headers).await?;
    let record = load_owned_session(&state, user.user_id, &session_id).await?;
    Ok(Json(GetSessionResponse {
        session: session_detail_from_record(record),
    }))
}

async fn api_update_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(request): Json<UpdateSessionRequest>,
) -> Result<Json<UpdateSessionResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    let title = validate_session_title(&request.title)?;
    let mut record = load_owned_session(&state, user.user_id, &session_id).await?;
    record.title = title;
    record.manually_renamed = true;
    record.updated_at = chrono::Utc::now();
    state
        .web_store
        .save_session(record.clone())
        .await
        .map_err(store_error_response)?;
    Ok(Json(UpdateSessionResponse {
        session: session_detail_from_record(record),
    }))
}

async fn api_delete_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Result<Json<OkResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    let _record = load_owned_session(&state, user.user_id, &session_id).await?;
    state.session_manager.delete_session(&session_id).await;
    state
        .web_store
        .delete_session(user.user_id, &session_id)
        .await
        .map_err(store_error_response)?;
    Ok(Json(OkResponse::ok()))
}

async fn api_list_tasks(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Result<Json<ListTasksResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user(&state, &headers).await?;
    let _session = load_owned_session(&state, user.user_id, &session_id).await?;
    let tasks = state
        .web_store
        .list_tasks(user.user_id, &session_id)
        .await
        .map_err(store_error_response)?
        .into_iter()
        .map(task_summary_from_record)
        .collect();
    Ok(Json(ListTasksResponse { tasks }))
}

async fn api_create_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(request): Json<ApiCreateTaskRequest>,
) -> Result<Json<ApiCreateTaskResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    let mut session = load_owned_session(&state, user.user_id, &session_id).await?;
    let input_markdown = validate_task_input(&request.input_markdown)?;
    reject_active_task(&state, user.user_id, &session_id).await?;

    ensure_runtime_session(&state, user.user_id, &session).await;
    let Some(running_task) = state
        .session_manager
        .register_task(&session_id, input_markdown.clone())
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

    let task_record = WebTaskRecord {
        schema_version: WEB_TASK_SCHEMA_VERSION,
        task_id: task_id.clone(),
        session_id: session_id.clone(),
        user_id: user.user_id,
        status: ApiTaskStatus::Running,
        input_markdown: input_markdown.clone(),
        input_edited_at: None,
        final_response_markdown: None,
        error_message: None,
        pending_user_input: None,
        last_progress: None,
        last_event_seq: 0,
        created_at: now,
        started_at: Some(now),
        updated_at: now,
        finished_at: None,
    };
    state
        .web_store
        .save_task(task_record.clone())
        .await
        .map_err(store_error_response)?;

    let preview = markdown_preview(&input_markdown);
    let should_auto_title = is_first_task && !session.manually_renamed;

    session.active_task_id = Some(task_id.clone());
    session.last_task_status = Some(ApiTaskStatus::Running);
    session.last_preview = Some(preview.clone());
    if should_auto_title && session.title == WEB_SESSION_DEFAULT_TITLE {
        session.title = preview.clone();
    }
    session.updated_at = now;
    state
        .web_store
        .save_session(session)
        .await
        .map_err(store_error_response)?;

    if should_auto_title && state.auto_title_enabled {
        auto_title::spawn_auto_title(
            state.clone(),
            auto_title::AutoTitleRequest {
                user_id: user.user_id,
                session_id: session_id.clone(),
                first_user_message: input_markdown.clone(),
                fallback_preview: preview,
            },
        );
    }

    let persistence = task_executor::WebTaskPersistence {
        web_store: state.web_store.clone(),
        user_id: user.user_id,
        session_id: session_id.clone(),
        task_id: task_id.clone(),
    };
    task_executor::spawn_registered_task(
        state.clone(),
        session_id,
        running_task,
        task_executor::TaskRunRequest::Execute(input_markdown),
        Some(persistence),
    )
    .await;

    Ok(Json(ApiCreateTaskResponse {
        task: task_summary_from_record(task_record),
    }))
}

async fn api_get_task(
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

async fn api_get_task_progress(
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

async fn api_get_task_events(
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
    Ok(Json(events))
}

async fn api_edit_task_input(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((session_id, task_id)): Path<(String, String)>,
    Json(request): Json<ApiEditTaskInputRequest>,
) -> Result<Json<ApiEditTaskInputResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    let input_markdown = validate_task_input(&request.input_markdown)?;
    let mut task = load_owned_task(&state, user.user_id, &session_id, &task_id).await?;
    if !task.status.is_terminal() {
        return Err(api_error(
            StatusCode::CONFLICT,
            ErrorCode::TaskActive,
            "Only terminal tasks can be edited.",
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
            "Only the latest task in a session can be edited.",
            false,
        ));
    }

    let now = chrono::Utc::now();
    task.input_markdown = input_markdown.clone();
    task.input_edited_at = Some(task.input_edited_at.unwrap_or(now));
    task.updated_at = now;
    state
        .web_store
        .save_task(task.clone())
        .await
        .map_err(store_error_response)?;

    let mut session = load_owned_session(&state, user.user_id, &session_id).await?;
    let preview = markdown_preview(&input_markdown);
    let old_preview = session.last_preview.clone();
    session.last_preview = Some(preview.clone());
    // Only update title from preview when it is still the default or the
    // previous fallback preview.  An LLM-generated auto-title must not be
    // overwritten by an edit.
    let title_is_still_fallback = tasks.len() == 1
        && !session.manually_renamed
        && (session.title == WEB_SESSION_DEFAULT_TITLE
            || session.title == old_preview.as_deref().unwrap_or(""));
    if title_is_still_fallback {
        session.title = preview;
    }
    session.updated_at = now;
    state
        .web_store
        .save_session(session)
        .await
        .map_err(store_error_response)?;

    Ok(Json(ApiEditTaskInputResponse {
        task: task_summary_from_record(task),
    }))
}

async fn api_resume_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((session_id, task_id)): Path<(String, String)>,
    Json(request): Json<ApiResumeTaskRequest>,
) -> Result<Json<ApiResumeTaskResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    let input_markdown = validate_task_input(&request.input_markdown)?;
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
        .register_existing_task(&session_id, &task_id, input_markdown.clone())
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
    task.status = ApiTaskStatus::Running;
    task.error_message = None;
    task.pending_user_input = None;
    task.updated_at = now;
    task.finished_at = None;
    if task.started_at.is_none() {
        task.started_at = Some(now);
    }
    state
        .web_store
        .save_task(task.clone())
        .await
        .map_err(store_error_response)?;

    let mut session = session;
    session.active_task_id = Some(task_id.clone());
    session.last_task_status = Some(ApiTaskStatus::Running);
    session.last_preview = Some(markdown_preview(&input_markdown));
    session.updated_at = now;
    state
        .web_store
        .save_session(session)
        .await
        .map_err(store_error_response)?;

    let persistence = task_executor::WebTaskPersistence {
        web_store: state.web_store.clone(),
        user_id: user.user_id,
        session_id: session_id.clone(),
        task_id: task_id.clone(),
    };
    task_executor::spawn_registered_task(
        state.clone(),
        session_id,
        running_task,
        task_executor::TaskRunRequest::ResumeUserInput(input_markdown),
        Some(persistence),
    )
    .await;

    Ok(Json(ApiResumeTaskResponse {
        task: task_summary_from_record(task),
    }))
}

async fn api_cancel_task(
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

    let mut session = load_owned_session(&state, user.user_id, &session_id).await?;
    if session.active_task_id.as_deref() == Some(task_id.as_str()) {
        session.active_task_id = None;
    }
    session.last_task_status = Some(ApiTaskStatus::Cancelled);
    session.updated_at = now;
    state
        .web_store
        .save_session(session)
        .await
        .map_err(store_error_response)?;

    state
        .session_manager
        .cancel_task(&task_id, &session_id)
        .await;
    if let Some(handle) = state.task_handles.read().await.get(&task_id).cloned() {
        handle.abort();
    }

    Ok(Json(ApiCancelTaskResponse {
        ok: true,
        status: ApiTaskStatus::Cancelled,
    }))
}

async fn reject_active_task(
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

    state
        .session_manager
        .create_session_with_id(
            user_id,
            session.session_id.clone(),
            session.context_key.clone(),
            session.agent_flow_id.clone(),
        )
        .await;
}

pub(crate) fn api_error(
    status: StatusCode,
    code: ErrorCode,
    message: impl Into<String>,
    retryable: bool,
) -> (StatusCode, Json<ErrorEnvelope>) {
    (status, Json(ErrorEnvelope::new(code, message, retryable)))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------
// Task execution
// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn build_router(state: AppState) -> Router {
    let cors = web_cors_layer();

    Router::new()
        .route("/health", get(health))
        .route("/api/v1/public-config", get(api_public_config))
        .route("/api/v1/me", get(api_me))
        .route("/api/v1/auth/register", post(api_register))
        .route("/api/v1/auth/bootstrap", post(api_bootstrap))
        .route("/api/v1/auth/login", post(api_login))
        .route("/api/v1/auth/logout", post(api_logout))
        .route("/api/v1/auth/change-password", post(api_change_password))
        .route("/api/v1/sessions", get(api_list_sessions))
        .route("/api/v1/sessions", post(api_create_session))
        .route("/api/v1/sessions/:session_id", get(api_get_session))
        .route("/api/v1/sessions/:session_id", patch(api_update_session))
        .route("/api/v1/sessions/:session_id", delete(api_delete_session))
        .route("/api/v1/sessions/:session_id/tasks", get(api_list_tasks))
        .route("/api/v1/sessions/:session_id/tasks", post(api_create_task))
        .route(
            "/api/v1/sessions/:session_id/tasks/:task_id",
            get(api_get_task),
        )
        .route(
            "/api/v1/sessions/:session_id/tasks/:task_id/progress",
            get(api_get_task_progress),
        )
        .route(
            "/api/v1/sessions/:session_id/tasks/:task_id/events",
            get(api_get_task_events),
        )
        .route(
            "/api/v1/sessions/:session_id/tasks/:task_id/stream",
            get(sse::api_sse_task_stream),
        )
        .route(
            "/api/v1/sessions/:session_id/tasks/:task_id/input",
            patch(api_edit_task_input),
        )
        .route(
            "/api/v1/sessions/:session_id/tasks/:task_id/resume",
            post(api_resume_task),
        )
        .route(
            "/api/v1/sessions/:session_id/tasks/:task_id/cancel",
            post(api_cancel_task),
        )
        .fallback(static_assets::static_assets_handler)
        .layer(middleware::from_fn(add_security_headers))
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state)
}

fn web_cors_layer() -> CorsLayer {
    if is_production_run_mode() {
        CorsLayer::new()
    } else {
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
    }
}

async fn add_security_headers(request: Request<Body>, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        "referrer-policy",
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'self'; object-src 'none'",
        ),
    );
    response
}

pub async fn serve(state: AppState, addr: std::net::SocketAddr) {
    state
        .validate_web_store_for_startup()
        .expect("web transport startup validation failed");
    state
        .reconcile_unfinished_tasks_on_startup()
        .await
        .expect("web task startup reconciliation failed");
    let router = build_router(state);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind TCP listener");
    tracing::info!("Web transport listening on {addr}");
    axum::serve(listener, router).await.expect("server error");
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderMap;
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::Instant;

    use oxide_agent_core::agent::progress::{LlmRetryState, ProgressState};
    use oxide_agent_core::agent::{TodoItem, TodoList, TodoStatus};
    use oxide_agent_core::config::{AgentSettings, ModelInfo};
    use oxide_agent_core::llm::LlmClient;
    use oxide_agent_runtime::SessionRegistry;
    #[cfg(feature = "profile-lite")]
    use oxide_agent_web_contracts::{
        CreateTaskRequest as ApiCreateTaskRequest, PendingUserInputView,
        ResumeTaskRequest as ApiResumeTaskRequest, UserInputKind as ApiUserInputKind,
    };
    use oxide_agent_web_contracts::{
        EditTaskInputRequest as ApiEditTaskInputRequest, ErrorCode, LoginRequest,
        PersistedTaskEvent, ProgressSnapshot, RegisterRequest, TaskEventKind,
        TaskStatus as ApiTaskStatus, WebTaskRecord,
    };
    use tokio::sync::mpsc;

    use super::{
        api_cancel_task, api_create_session, api_edit_task_input, api_get_session,
        api_get_task_events, api_get_task_progress, api_list_sessions, auth_cookie_value,
        csrf_header_value, parse_web_bool, AppState, TaskEventsQuery, WebAssetsConfig,
        WebStartupError, AUTH_COOKIE_NAME, WEB_TASK_SCHEMA_VERSION,
    };
    #[cfg(feature = "profile-lite")]
    use super::{api_create_task, api_get_task, api_list_tasks, api_resume_task};
    use crate::auth::{login_user, register_user};
    use crate::scripted_llm::{ScriptedLlmProvider, ScriptedResponse};
    use crate::session::WebSessionManager;

    #[test]
    fn parse_web_bool_accepts_common_enabled_values() {
        for value in ["1", "true", "TRUE", "yes", "on", " on "] {
            assert!(parse_web_bool(value), "{value:?} should be enabled");
        }
    }

    #[test]
    fn parse_web_bool_rejects_disabled_or_unknown_values() {
        for value in ["", "0", "false", "no", "off", "enabled"] {
            assert!(!parse_web_bool(value), "{value:?} should be disabled");
        }
    }

    #[test]
    fn bootstrap_required_depends_on_registration_users_and_token() {
        assert!(super::web_bootstrap_required(false, 0, true));
        assert!(!super::web_bootstrap_required(true, 0, true));
        assert!(!super::web_bootstrap_required(false, 1, true));
        assert!(!super::web_bootstrap_required(false, 0, false));
    }

    #[test]
    fn markdown_preview_strips_common_markdown_title_markup() {
        let preview = super::markdown_preview(
            "# Browser smoke\n\n- item one\n- item two\n\n```rust\nfn main() {}\n```",
        );

        assert_eq!(preview, "Browser smoke item one item two");
    }

    #[test]
    fn auth_cookie_and_csrf_values_are_extracted_from_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            format!("theme=light; {AUTH_COOKIE_NAME}=token-123; other=1")
                .parse()
                .expect("cookie header"),
        );
        headers.insert("x-csrf-token", "csrf-123".parse().expect("csrf header"));

        assert_eq!(
            auth_cookie_value(&headers).expect("auth cookie"),
            "token-123"
        );
        assert_eq!(csrf_header_value(&headers).expect("csrf"), "csrf-123");
    }

    #[test]
    fn csrf_origin_check_accepts_same_origin_and_rejects_cross_origin() {
        let mut same_origin = HeaderMap::new();
        same_origin.insert("x-forwarded-proto", "https".parse().expect("proto"));
        same_origin.insert("x-forwarded-host", "app.example".parse().expect("host"));
        same_origin.insert(
            axum::http::header::ORIGIN,
            "https://app.example".parse().expect("origin"),
        );
        assert!(super::validate_csrf_request_origin(&same_origin).is_ok());

        let mut same_referer = HeaderMap::new();
        same_referer.insert("x-forwarded-proto", "https".parse().expect("proto"));
        same_referer.insert("x-forwarded-host", "app.example".parse().expect("host"));
        same_referer.insert(
            axum::http::header::REFERER,
            "https://app.example/app/session/1"
                .parse()
                .expect("referer"),
        );
        assert!(super::validate_csrf_request_origin(&same_referer).is_ok());

        let mut cross_origin = same_origin;
        cross_origin.insert(
            axum::http::header::ORIGIN,
            "https://evil.example".parse().expect("origin"),
        );
        let (status, axum::Json(error)) =
            super::validate_csrf_request_origin(&cross_origin).expect_err("cross origin");
        assert_eq!(status, axum::http::StatusCode::FORBIDDEN);
        assert_eq!(error.error.code, ErrorCode::CsrfInvalid);
    }

    #[test]
    fn auth_rate_limiter_uses_fixed_window() {
        let mut limiter = super::AuthRateLimiter::new();
        let now = Instant::now();
        let key = "127.0.0.1:alice";

        for _ in 0..super::AUTH_RATE_LIMIT_MAX_FAILURES {
            assert!(!limiter.is_limited(key, now));
            limiter.record_failure(key.to_string(), now);
        }
        assert!(limiter.is_limited(key, now));
        assert!(!limiter.is_limited(key, now + super::AUTH_RATE_LIMIT_WINDOW));
    }

    #[tokio::test]
    async fn api_login_rate_limits_by_ip_and_login_key() {
        let state = test_app_state();
        let now = chrono::Utc::now();
        register_user(
            state.web_store.as_ref(),
            RegisterRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            true,
            now,
        )
        .await
        .expect("register user");

        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "198.51.100.10".parse().expect("ip"));
        for _ in 0..super::AUTH_RATE_LIMIT_MAX_FAILURES {
            let (status, axum::Json(error)) = super::api_login(
                axum::extract::State(state.clone()),
                headers.clone(),
                axum::Json(LoginRequest {
                    login: "alice".to_string(),
                    password: "wrong password".to_string(),
                }),
            )
            .await
            .expect_err("wrong password should fail");
            assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED);
            assert_eq!(error.error.code, ErrorCode::InvalidCredentials);
        }

        let (status, axum::Json(error)) = super::api_login(
            axum::extract::State(state.clone()),
            headers,
            axum::Json(LoginRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            }),
        )
        .await
        .expect_err("same key should be rate limited before password verification");
        assert_eq!(status, axum::http::StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(error.error.code, ErrorCode::RateLimited);

        let mut other_ip_headers = HeaderMap::new();
        other_ip_headers.insert("x-forwarded-for", "198.51.100.20".parse().expect("ip"));
        let (_headers, axum::Json(response)) = super::api_login(
            axum::extract::State(state),
            other_ip_headers,
            axum::Json(LoginRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            }),
        )
        .await
        .expect("different IP/login key should not be rate limited");
        assert_eq!(response.user.login, "alice");
    }

    #[tokio::test]
    async fn api_register_failures_are_rate_limited() {
        let _lock = web_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _guard = EnvGuard::capture(&["OXIDE_WEB_REGISTRATION_ENABLED"]);
        std::env::set_var("OXIDE_WEB_REGISTRATION_ENABLED", "false");

        let state = test_app_state();
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "203.0.113.10".parse().expect("ip"));
        for _ in 0..super::AUTH_RATE_LIMIT_MAX_FAILURES {
            let (status, axum::Json(error)) = super::api_register(
                axum::extract::State(state.clone()),
                headers.clone(),
                axum::Json(RegisterRequest {
                    login: "alice".to_string(),
                    password: "correct horse battery staple".to_string(),
                }),
            )
            .await
            .expect_err("disabled registration should fail");
            assert_eq!(status, axum::http::StatusCode::FORBIDDEN);
            assert_eq!(error.error.code, ErrorCode::RegistrationDisabled);
        }

        let (status, axum::Json(error)) = super::api_register(
            axum::extract::State(state),
            headers,
            axum::Json(RegisterRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            }),
        )
        .await
        .expect_err("disabled registration should become rate limited");
        assert_eq!(status, axum::http::StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(error.error.code, ErrorCode::RateLimited);
    }

    #[tokio::test]
    async fn api_register_starts_browser_auth_session() {
        let _lock = web_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _guard = EnvGuard::capture(&["OXIDE_WEB_REGISTRATION_ENABLED"]);
        std::env::set_var("OXIDE_WEB_REGISTRATION_ENABLED", "true");

        let state = test_app_state();
        let (response_headers, axum::Json(response)) = super::api_register(
            axum::extract::State(state.clone()),
            HeaderMap::new(),
            axum::Json(RegisterRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            }),
        )
        .await
        .expect("register should create auth session");
        assert_eq!(response.user.login, "alice");
        let csrf_token = response.csrf_token.expect("register returns csrf token");
        let raw_cookie = response_headers
            .get(axum::http::header::SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .expect("set-cookie header");
        assert!(raw_cookie.contains("HttpOnly"));
        let raw_token = raw_cookie
            .strip_prefix(&format!("{AUTH_COOKIE_NAME}="))
            .and_then(|value| value.split(';').next())
            .expect("session cookie value")
            .to_string();

        let axum::Json(me) =
            super::api_me(axum::extract::State(state), auth_headers(&raw_token, None))
                .await
                .expect("registered auth session can load current user");
        assert_eq!(me.user.login, "alice");
        assert_eq!(me.csrf_token, csrf_token);
    }

    #[tokio::test]
    async fn mutating_session_api_rejects_cross_origin_csrf_request() {
        let state = test_app_state();
        let now = chrono::Utc::now();
        register_user(
            state.web_store.as_ref(),
            RegisterRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            true,
            now,
        )
        .await
        .expect("register user");
        let (_, auth_session, token) = login_user(
            state.web_store.as_ref(),
            LoginRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            now,
        )
        .await
        .expect("login user");
        let mut headers = auth_headers(&token, Some(&auth_session.csrf_token));
        headers.insert("x-forwarded-proto", "https".parse().expect("proto"));
        headers.insert("x-forwarded-host", "app.example".parse().expect("host"));
        headers.insert(
            axum::http::header::ORIGIN,
            "https://evil.example".parse().expect("origin"),
        );

        let (status, axum::Json(error)) = api_create_session(axum::extract::State(state), headers)
            .await
            .expect_err("cross-origin mutating request should fail");
        assert_eq!(status, axum::http::StatusCode::FORBIDDEN);
        assert_eq!(error.error.code, ErrorCode::CsrfInvalid);
    }

    #[test]
    fn startup_guard_requires_explicit_in_memory_for_web_enabled_mode() {
        let _lock = web_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _guard = EnvGuard::capture(&[
            "RUN_MODE",
            "OXIDE_WEB_ENABLED",
            "OXIDE_WEB_REQUIRE_DURABLE_STORAGE",
            "OXIDE_WEB_ALLOW_IN_MEMORY_STORE",
        ]);
        std::env::remove_var("RUN_MODE");
        std::env::set_var("OXIDE_WEB_ENABLED", "true");
        std::env::remove_var("OXIDE_WEB_REQUIRE_DURABLE_STORAGE");
        std::env::remove_var("OXIDE_WEB_ALLOW_IN_MEMORY_STORE");

        let state = test_app_state();
        assert_eq!(
            state.validate_web_store_for_startup(),
            Err(WebStartupError::InMemoryStoreNotAllowed)
        );

        std::env::set_var("OXIDE_WEB_ALLOW_IN_MEMORY_STORE", "true");
        assert!(state.validate_web_store_for_startup().is_ok());
    }

    #[test]
    fn static_assets_startup_requires_index_when_configured() {
        let asset_dir = unique_test_asset_dir("missing-index");
        std::fs::create_dir_all(&asset_dir).expect("create asset dir");
        let mut state = test_app_state();
        state.web_assets = WebAssetsConfig::required_dir_for_tests(asset_dir.clone());

        let error = state
            .validate_web_store_for_startup()
            .expect_err("missing index should fail startup");
        assert!(matches!(error, WebStartupError::StaticAssetsUnavailable(_)));

        std::fs::write(asset_dir.join("index.html"), "<html>ok</html>").expect("write index");
        assert!(state.validate_web_store_for_startup().is_ok());
        let _ = std::fs::remove_dir_all(asset_dir);
    }

    #[tokio::test]
    async fn router_serves_frontend_assets_and_security_headers() {
        use tower::Service as _;

        let asset_dir = unique_test_asset_dir("static-serving");
        std::fs::create_dir_all(&asset_dir).expect("create asset dir");
        std::fs::write(asset_dir.join("index.html"), "<main id=\"app\"></main>")
            .expect("write index");
        std::fs::write(asset_dir.join("oxide.js"), "console.log('oxide')").expect("write js");
        std::fs::write(asset_dir.join("oxide.wasm"), [0_u8, 97, 115, 109]).expect("write wasm");

        let mut state = test_app_state();
        state.web_assets = WebAssetsConfig {
            dir: Some(asset_dir.clone()),
            required: false,
        };

        let mut app = super::build_router(state.clone());
        let response = app
            .call(
                axum::http::Request::builder()
                    .method(axum::http::Method::GET)
                    .uri("/app/session/session-1")
                    .body(axum::body::Body::empty())
                    .expect("browser route request"),
            )
            .await
            .expect("browser route response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert_eq!(
            response.headers()["x-content-type-options"],
            axum::http::HeaderValue::from_static("nosniff")
        );
        assert_eq!(
            response.headers()["x-frame-options"],
            axum::http::HeaderValue::from_static("DENY")
        );
        let csp = response
            .headers()
            .get("content-security-policy")
            .expect("content security policy");
        assert!(csp
            .to_str()
            .expect("valid csp")
            .contains("script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'"));
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("browser route body");
        assert!(String::from_utf8_lossy(&body).contains("app"));

        let mut app = super::build_router(state);
        let response = app
            .call(
                axum::http::Request::builder()
                    .method(axum::http::Method::GET)
                    .uri("/oxide.wasm")
                    .body(axum::body::Body::empty())
                    .expect("wasm request"),
            )
            .await
            .expect("wasm response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert_eq!(
            response.headers()[axum::http::header::CONTENT_TYPE],
            axum::http::HeaderValue::from_static("application/wasm")
        );

        let mut app = super::build_router(test_app_state());
        let response = app
            .call(
                axum::http::Request::builder()
                    .method(axum::http::Method::GET)
                    .uri("/api/v1/does-not-exist")
                    .body(axum::body::Body::empty())
                    .expect("missing api request"),
            )
            .await
            .expect("missing api response");
        assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);

        let _ = std::fs::remove_dir_all(asset_dir);
    }

    #[cfg(feature = "storage-s3-r2")]
    #[tokio::test]
    async fn r2_backed_app_state_builder_requires_r2_config() {
        let _lock = web_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _guard = EnvGuard::capture(&[
            "OXIDE_R2_ENDPOINT_URL",
            "OXIDE_R2_ENDPOINT",
            "OXIDE_R2_BUCKET_NAME",
            "OXIDE_R2_BUCKET",
            "OXIDE_R2_ACCESS_KEY_ID",
            "OXIDE_R2_SECRET_ACCESS_KEY",
        ]);
        for key in [
            "OXIDE_R2_ENDPOINT_URL",
            "OXIDE_R2_ENDPOINT",
            "OXIDE_R2_BUCKET_NAME",
            "OXIDE_R2_BUCKET",
            "OXIDE_R2_ACCESS_KEY_ID",
            "OXIDE_R2_SECRET_ACCESS_KEY",
        ] {
            std::env::remove_var(key);
        }

        let settings = Arc::new(AgentSettings::default());
        let llm = Arc::new(LlmClient::new(settings.as_ref()));
        let Err(error) =
            super::build_r2_backed_app_state(SessionRegistry::new(), llm, settings).await
        else {
            panic!("missing R2 config should fail before startup");
        };
        assert!(
            error.to_string().contains("OXIDE_R2_ENDPOINT"),
            "unexpected startup error: {error}"
        );
    }

    #[tokio::test]
    async fn router_exposes_api_v1_without_legacy_unversioned_routes() {
        use tower::Service as _;

        let state = test_app_state();
        let mut app = super::build_router(state.clone());
        let public_config = app
            .call(
                axum::http::Request::builder()
                    .method(axum::http::Method::GET)
                    .uri("/api/v1/public-config")
                    .body(axum::body::Body::empty())
                    .expect("public-config request"),
            )
            .await
            .expect("public-config response");
        assert_eq!(public_config.status(), axum::http::StatusCode::OK);

        let legacy_root = format!("{}{}", "/session", "s");
        let debug_logs_path = format!("{}{}", "/debug", "/event_logs");
        for (method, path) in [
            (axum::http::Method::POST, legacy_root.clone()),
            (axum::http::Method::GET, format!("{legacy_root}/session-1")),
            (
                axum::http::Method::DELETE,
                format!("{legacy_root}/session-1"),
            ),
            (
                axum::http::Method::POST,
                format!("{legacy_root}/session-1/tasks"),
            ),
            (
                axum::http::Method::GET,
                format!("{legacy_root}/session-1/tasks/task-1/progress"),
            ),
            (
                axum::http::Method::GET,
                format!("{legacy_root}/session-1/tasks/task-1/events"),
            ),
            (
                axum::http::Method::GET,
                format!("{legacy_root}/session-1/tasks/task-1/stream"),
            ),
            (
                axum::http::Method::GET,
                format!("{legacy_root}/session-1/tasks/task-1/timeline"),
            ),
            (
                axum::http::Method::POST,
                format!("{legacy_root}/session-1/tasks/task-1/cancel"),
            ),
            (axum::http::Method::GET, debug_logs_path),
        ] {
            let response = super::build_router(state.clone())
                .call(
                    axum::http::Request::builder()
                        .method(method)
                        .uri(path.as_str())
                        .body(axum::body::Body::empty())
                        .expect("legacy route request"),
                )
                .await
                .expect("legacy route response");
            assert_eq!(
                response.status(),
                axum::http::StatusCode::NOT_FOUND,
                "legacy route {path} should not be exposed"
            );
        }
    }

    #[test]
    fn sse_start_seq_uses_query_before_last_event_id() {
        let mut headers = HeaderMap::new();
        headers.insert("last-event-id", "41".parse().expect("last-event-id"));

        assert_eq!(
            super::sse::sse_start_seq(
                &headers,
                &TaskEventsQuery {
                    after_seq: None,
                    limit: None,
                },
            ),
            41
        );
        assert_eq!(
            super::sse::sse_start_seq(
                &headers,
                &TaskEventsQuery {
                    after_seq: Some(9),
                    limit: None,
                },
            ),
            9
        );
    }

    #[tokio::test]
    async fn api_sessions_are_auth_scoped_and_use_web_session_context() {
        let state = test_app_state();
        let now = chrono::Utc::now();
        let user_one = register_user(
            state.web_store.as_ref(),
            RegisterRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            true,
            now,
        )
        .await
        .expect("register first user");
        let user_two = register_user(
            state.web_store.as_ref(),
            RegisterRequest {
                login: "bob".to_string(),
                password: "another correct horse battery staple".to_string(),
            },
            true,
            now,
        )
        .await
        .expect("register second user");
        let (_, session_one, token_one) = login_user(
            state.web_store.as_ref(),
            LoginRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            now,
        )
        .await
        .expect("login first user");
        let (_, _, token_two) = login_user(
            state.web_store.as_ref(),
            LoginRequest {
                login: "bob".to_string(),
                password: "another correct horse battery staple".to_string(),
            },
            now,
        )
        .await
        .expect("login second user");

        let axum::Json(created) = api_create_session(
            axum::extract::State(state.clone()),
            auth_headers(&token_one, Some(&session_one.csrf_token)),
        )
        .await
        .expect("create session");
        let session_id = created.session.session_id;
        let record = state
            .web_store
            .load_session(user_one.user_id, &session_id)
            .await
            .expect("load session")
            .expect("session exists");
        assert_eq!(record.context_key, format!("web-session-{session_id}"));
        assert_eq!(record.agent_flow_id, "main");

        let axum::Json(listed) = api_list_sessions(
            axum::extract::State(state.clone()),
            auth_headers(&token_one, None),
        )
        .await
        .expect("list sessions");
        assert_eq!(listed.sessions.len(), 1);

        let axum::Json(foreign_listed) = api_list_sessions(
            axum::extract::State(state.clone()),
            auth_headers(&token_two, None),
        )
        .await
        .expect("list foreign sessions");
        assert!(foreign_listed.sessions.is_empty());

        let foreign_get = api_get_session(
            axum::extract::State(state.clone()),
            auth_headers(&token_two, None),
            axum::extract::Path(session_id.clone()),
        )
        .await;
        assert_eq!(
            foreign_get.expect_err("foreign session should be hidden").0,
            axum::http::StatusCode::NOT_FOUND
        );

        let create_without_csrf =
            api_create_session(axum::extract::State(state), auth_headers(&token_one, None)).await;
        assert_eq!(
            create_without_csrf.expect_err("missing csrf should fail").0,
            axum::http::StatusCode::FORBIDDEN
        );
        assert_ne!(user_one.user_id, user_two.user_id);
    }

    #[tokio::test]
    async fn api_edit_and_cancel_task_are_auth_scoped_and_status_checked() {
        let state = test_app_state();
        let now = chrono::Utc::now();
        let user_one = register_user(
            state.web_store.as_ref(),
            RegisterRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            true,
            now,
        )
        .await
        .expect("register first user");
        register_user(
            state.web_store.as_ref(),
            RegisterRequest {
                login: "bob".to_string(),
                password: "another correct horse battery staple".to_string(),
            },
            true,
            now,
        )
        .await
        .expect("register second user");
        let (_, session_one, token_one) = login_user(
            state.web_store.as_ref(),
            LoginRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            now,
        )
        .await
        .expect("login first user");
        let (_, session_two, token_two) = login_user(
            state.web_store.as_ref(),
            LoginRequest {
                login: "bob".to_string(),
                password: "another correct horse battery staple".to_string(),
            },
            now,
        )
        .await
        .expect("login second user");

        let axum::Json(created_session) = api_create_session(
            axum::extract::State(state.clone()),
            auth_headers(&token_one, Some(&session_one.csrf_token)),
        )
        .await
        .expect("create session");
        let session_id = created_session.session.session_id;

        let completed = task_record(
            user_one.user_id,
            &session_id,
            "task-completed",
            ApiTaskStatus::Completed,
            "Original prompt",
            now,
        );
        state
            .web_store
            .save_task(completed)
            .await
            .expect("save completed task");

        let axum::Json(edited) = api_edit_task_input(
            axum::extract::State(state.clone()),
            auth_headers(&token_one, Some(&session_one.csrf_token)),
            axum::extract::Path((session_id.clone(), "task-completed".to_string())),
            axum::Json(ApiEditTaskInputRequest {
                input_markdown: "Edited prompt".to_string(),
            }),
        )
        .await
        .expect("edit terminal task");
        assert_eq!(edited.task.input_markdown, "Edited prompt");
        assert!(edited.task.input_edited_at.is_some());

        let running = task_record(
            user_one.user_id,
            &session_id,
            "task-running",
            ApiTaskStatus::Running,
            "Running prompt",
            now + chrono::Duration::seconds(1),
        );
        state
            .web_store
            .save_task(running)
            .await
            .expect("save running task");
        let mut session = state
            .web_store
            .load_session(user_one.user_id, &session_id)
            .await
            .expect("load session")
            .expect("session exists");
        session.active_task_id = Some("task-running".to_string());
        session.last_task_status = Some(ApiTaskStatus::Running);
        state
            .web_store
            .save_session(session)
            .await
            .expect("save active session");

        let edit_running = api_edit_task_input(
            axum::extract::State(state.clone()),
            auth_headers(&token_one, Some(&session_one.csrf_token)),
            axum::extract::Path((session_id.clone(), "task-running".to_string())),
            axum::Json(ApiEditTaskInputRequest {
                input_markdown: "Should fail".to_string(),
            }),
        )
        .await;
        let (status, axum::Json(error)) = edit_running.expect_err("running edit should fail");
        assert_eq!(status, axum::http::StatusCode::CONFLICT);
        assert_eq!(error.error.code, ErrorCode::TaskActive);

        let edit_non_latest = api_edit_task_input(
            axum::extract::State(state.clone()),
            auth_headers(&token_one, Some(&session_one.csrf_token)),
            axum::extract::Path((session_id.clone(), "task-completed".to_string())),
            axum::Json(ApiEditTaskInputRequest {
                input_markdown: "Should also fail".to_string(),
            }),
        )
        .await;
        let (status, axum::Json(error)) = edit_non_latest.expect_err("non-latest edit should fail");
        assert_eq!(status, axum::http::StatusCode::CONFLICT);
        assert_eq!(error.error.code, ErrorCode::Conflict);

        let foreign_cancel = api_cancel_task(
            axum::extract::State(state.clone()),
            auth_headers(&token_two, Some(&session_two.csrf_token)),
            axum::extract::Path((session_id.clone(), "task-running".to_string())),
        )
        .await;
        assert_eq!(
            foreign_cancel.expect_err("foreign task should be hidden").0,
            axum::http::StatusCode::NOT_FOUND
        );

        let axum::Json(cancelled) = api_cancel_task(
            axum::extract::State(state.clone()),
            auth_headers(&token_one, Some(&session_one.csrf_token)),
            axum::extract::Path((session_id.clone(), "task-running".to_string())),
        )
        .await
        .expect("cancel active task");
        assert!(cancelled.ok);
        assert_eq!(cancelled.status, ApiTaskStatus::Cancelled);

        let task = state
            .web_store
            .load_task(user_one.user_id, &session_id, "task-running")
            .await
            .expect("load task")
            .expect("task exists");
        assert_eq!(task.status, ApiTaskStatus::Cancelled);
        assert!(task.finished_at.is_some());

        let session = state
            .web_store
            .load_session(user_one.user_id, &session_id)
            .await
            .expect("load session")
            .expect("session exists");
        assert_eq!(session.active_task_id, None);
        assert_eq!(session.last_task_status, Some(ApiTaskStatus::Cancelled));

        let axum::Json(cancelled_again) = api_cancel_task(
            axum::extract::State(state),
            auth_headers(&token_one, Some(&session_one.csrf_token)),
            axum::extract::Path((session_id, "task-running".to_string())),
        )
        .await
        .expect("cancel is idempotent");
        assert!(cancelled_again.ok);
        assert_eq!(cancelled_again.status, ApiTaskStatus::Cancelled);
    }

    #[tokio::test]
    async fn api_task_events_are_auth_scoped_and_replay_after_seq() {
        let state = test_app_state();
        let now = chrono::Utc::now();
        let user_one = register_user(
            state.web_store.as_ref(),
            RegisterRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            true,
            now,
        )
        .await
        .expect("register first user");
        register_user(
            state.web_store.as_ref(),
            RegisterRequest {
                login: "bob".to_string(),
                password: "another correct horse battery staple".to_string(),
            },
            true,
            now,
        )
        .await
        .expect("register second user");
        let (_, session_one, token_one) = login_user(
            state.web_store.as_ref(),
            LoginRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            now,
        )
        .await
        .expect("login first user");
        let (_, _, token_two) = login_user(
            state.web_store.as_ref(),
            LoginRequest {
                login: "bob".to_string(),
                password: "another correct horse battery staple".to_string(),
            },
            now,
        )
        .await
        .expect("login second user");

        let axum::Json(created_session) = api_create_session(
            axum::extract::State(state.clone()),
            auth_headers(&token_one, Some(&session_one.csrf_token)),
        )
        .await
        .expect("create session");
        let session_id = created_session.session.session_id;
        let task = task_record(
            user_one.user_id,
            &session_id,
            "task-events",
            ApiTaskStatus::Completed,
            "Prompt",
            now,
        );
        state.web_store.save_task(task).await.expect("save task");
        state
            .web_store
            .append_task_events(
                user_one.user_id,
                &session_id,
                "task-events",
                vec![
                    persisted_event(
                        user_one.user_id,
                        &session_id,
                        "task-events",
                        1,
                        TaskEventKind::Thinking,
                    ),
                    persisted_event(
                        user_one.user_id,
                        &session_id,
                        "task-events",
                        2,
                        TaskEventKind::ToolResult,
                    ),
                ],
            )
            .await
            .expect("append events");

        let axum::Json(response) = api_get_task_events(
            axum::extract::State(state.clone()),
            auth_headers(&token_one, None),
            axum::extract::Path((session_id.clone(), "task-events".to_string())),
            axum::extract::Query(TaskEventsQuery {
                after_seq: Some(1),
                limit: Some(1),
            }),
        )
        .await
        .expect("get task events");
        assert_eq!(response.events.len(), 1);
        assert_eq!(response.events[0].seq, 2);
        assert_eq!(response.events[0].kind, TaskEventKind::ToolResult);
        assert_eq!(response.last_seq, 2);
        assert!(!response.has_more);

        let foreign = api_get_task_events(
            axum::extract::State(state),
            auth_headers(&token_two, None),
            axum::extract::Path((session_id, "task-events".to_string())),
            axum::extract::Query(TaskEventsQuery {
                after_seq: Some(0),
                limit: Some(200),
            }),
        )
        .await;
        assert_eq!(
            foreign.expect_err("foreign events should be hidden").0,
            axum::http::StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn api_task_progress_is_auth_scoped_and_reads_persisted_snapshot() {
        let state = test_app_state();
        let now = chrono::Utc::now();
        let user_one = register_user(
            state.web_store.as_ref(),
            RegisterRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            true,
            now,
        )
        .await
        .expect("register first user");
        register_user(
            state.web_store.as_ref(),
            RegisterRequest {
                login: "bob".to_string(),
                password: "another correct horse battery staple".to_string(),
            },
            true,
            now,
        )
        .await
        .expect("register second user");
        let (_, session_one, token_one) = login_user(
            state.web_store.as_ref(),
            LoginRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            now,
        )
        .await
        .expect("login first user");
        let (_, _, token_two) = login_user(
            state.web_store.as_ref(),
            LoginRequest {
                login: "bob".to_string(),
                password: "another correct horse battery staple".to_string(),
            },
            now,
        )
        .await
        .expect("login second user");

        let axum::Json(created_session) = api_create_session(
            axum::extract::State(state.clone()),
            auth_headers(&token_one, Some(&session_one.csrf_token)),
        )
        .await
        .expect("create session");
        let session_id = created_session.session.session_id;
        let mut task = task_record(
            user_one.user_id,
            &session_id,
            "task-progress",
            ApiTaskStatus::Running,
            "Prompt",
            now,
        );
        task.last_event_seq = 7;
        task.last_progress = Some(ProgressSnapshot {
            current_iteration: 3,
            max_iterations: 100,
            is_finished: false,
            error: None,
            current_thought: Some("Collecting evidence".to_string()),
            current_todos: Some(serde_json::json!({ "items": [] })),
            last_compaction_status: Some("Compaction: compacted history".to_string()),
            repeated_compaction_warning: None,
            last_history_repair_status: Some("History repaired".to_string()),
            latest_token_snapshot: None,
            llm_retry: Some(serde_json::json!({ "attempt": 2 })),
            provider_failover_notice: Some("Failover: primary -> backup".to_string()),
        });
        state.web_store.save_task(task).await.expect("save task");

        let axum::Json(response) = api_get_task_progress(
            axum::extract::State(state.clone()),
            auth_headers(&token_one, None),
            axum::extract::Path((session_id.clone(), "task-progress".to_string())),
        )
        .await
        .expect("get persisted task progress");
        let progress = response.progress.expect("progress snapshot");
        assert_eq!(response.status, ApiTaskStatus::Running);
        assert_eq!(response.last_event_seq, 7);
        assert_eq!(progress.current_iteration, 3);
        assert_eq!(
            progress.current_todos.expect("todos snapshot")["items"],
            serde_json::json!([])
        );
        assert_eq!(progress.llm_retry.expect("retry snapshot")["attempt"], 2);
        assert_eq!(
            progress.provider_failover_notice.as_deref(),
            Some("Failover: primary -> backup")
        );

        let foreign = api_get_task_progress(
            axum::extract::State(state),
            auth_headers(&token_two, None),
            axum::extract::Path((session_id, "task-progress".to_string())),
        )
        .await;
        assert_eq!(
            foreign.expect_err("foreign progress should be hidden").0,
            axum::http::StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn live_progress_persister_updates_running_task_record() {
        let state = test_app_state();
        let now = chrono::Utc::now();
        let user_id = 77;
        let session_id = "session-live-progress";
        let task_id = "task-live-progress";
        state
            .web_store
            .save_task(task_record(
                user_id,
                session_id,
                task_id,
                ApiTaskStatus::Running,
                "Prompt",
                now,
            ))
            .await
            .expect("save running task");

        let web_task = super::task_executor::WebTaskPersistence {
            web_store: state.web_store.clone(),
            user_id,
            session_id: session_id.to_string(),
            task_id: task_id.to_string(),
        };
        let (tx, rx) = mpsc::unbounded_channel();
        let handle = super::task_executor::spawn_live_progress_persister(web_task, rx);

        let mut progress = ProgressState::new(100);
        progress.current_iteration = 4;
        progress.current_thought = Some("Persisting progress".to_string());
        progress.current_todos = Some(TodoList {
            items: vec![TodoItem {
                description: "Persist progress".to_string(),
                status: TodoStatus::InProgress,
            }],
            updated_at: Some(now),
        });
        progress.llm_retry = Some(LlmRetryState {
            attempt: 2,
            max_attempts: 5,
            unbounded: false,
            wait_secs: Some(3),
            provider: "mock".to_string(),
            error_class: Some("rate_limit".to_string()),
        });
        progress.provider_failover_notice = Some("Failover: mock:a -> mock:b".to_string());
        tx.send(progress).expect("send live progress");

        let snapshot = wait_for_persisted_progress(&state, user_id, session_id, task_id).await;
        assert_eq!(snapshot.current_iteration, 4);
        assert_eq!(
            snapshot.current_thought.as_deref(),
            Some("Persisting progress")
        );
        assert_eq!(
            snapshot.current_todos.expect("todos persisted")["items"][0]["description"],
            "Persist progress"
        );
        assert_eq!(snapshot.llm_retry.expect("retry persisted")["attempt"], 2);
        assert_eq!(
            snapshot.provider_failover_notice.as_deref(),
            Some("Failover: mock:a -> mock:b")
        );

        drop(tx);
        handle.await.expect("live progress persister joins");
    }

    #[tokio::test]
    async fn api_task_stream_replays_persisted_events_after_seq() {
        use tower::Service as _;

        let state = test_app_state();
        let now = chrono::Utc::now();
        let user_one = register_user(
            state.web_store.as_ref(),
            RegisterRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            true,
            now,
        )
        .await
        .expect("register first user");
        register_user(
            state.web_store.as_ref(),
            RegisterRequest {
                login: "bob".to_string(),
                password: "another correct horse battery staple".to_string(),
            },
            true,
            now,
        )
        .await
        .expect("register second user");
        let (_, session_one, token_one) = login_user(
            state.web_store.as_ref(),
            LoginRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            now,
        )
        .await
        .expect("login first user");
        let (_, _, token_two) = login_user(
            state.web_store.as_ref(),
            LoginRequest {
                login: "bob".to_string(),
                password: "another correct horse battery staple".to_string(),
            },
            now,
        )
        .await
        .expect("login second user");

        let axum::Json(created_session) = api_create_session(
            axum::extract::State(state.clone()),
            auth_headers(&token_one, Some(&session_one.csrf_token)),
        )
        .await
        .expect("create session");
        let session_id = created_session.session.session_id;
        let mut task = task_record(
            user_one.user_id,
            &session_id,
            "task-events",
            ApiTaskStatus::Completed,
            "Prompt",
            now,
        );
        task.last_event_seq = 2;
        state.web_store.save_task(task).await.expect("save task");
        state
            .web_store
            .append_task_events(
                user_one.user_id,
                &session_id,
                "task-events",
                vec![
                    persisted_event(
                        user_one.user_id,
                        &session_id,
                        "task-events",
                        1,
                        TaskEventKind::Thinking,
                    ),
                    persisted_event(
                        user_one.user_id,
                        &session_id,
                        "task-events",
                        2,
                        TaskEventKind::ToolResult,
                    ),
                ],
            )
            .await
            .expect("append events");

        let mut app = super::build_router(state.clone());
        let response = app
            .call(sse_request(&session_id, "task-events", &token_one, Some(1)))
            .await
            .expect("sse response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("sse body");
        let body = String::from_utf8(body.to_vec()).expect("sse body utf8");
        assert!(body.contains("event: snapshot"));
        assert!(body.contains("event: task_event"));
        assert!(body.contains("id: 2"));
        assert!(!body.contains("\"seq\":1"));
        assert!(body.contains("event: task_status"));
        assert!(body.contains("\"status\":\"completed\""));

        let mut app = super::build_router(state);
        let response = app
            .call(sse_request(&session_id, "task-events", &token_two, Some(0)))
            .await
            .expect("foreign sse response");
        assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[cfg(feature = "profile-lite")]
    #[tokio::test]
    async fn api_tasks_are_auth_scoped_and_persist_final_response() {
        let state = test_app_state();
        let now = chrono::Utc::now();
        let user_one = register_user(
            state.web_store.as_ref(),
            RegisterRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            true,
            now,
        )
        .await
        .expect("register first user");
        let _user_two = register_user(
            state.web_store.as_ref(),
            RegisterRequest {
                login: "bob".to_string(),
                password: "another correct horse battery staple".to_string(),
            },
            true,
            now,
        )
        .await
        .expect("register second user");
        let (_, session_one, token_one) = login_user(
            state.web_store.as_ref(),
            LoginRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            now,
        )
        .await
        .expect("login first user");
        let (_, _, token_two) = login_user(
            state.web_store.as_ref(),
            LoginRequest {
                login: "bob".to_string(),
                password: "another correct horse battery staple".to_string(),
            },
            now,
        )
        .await
        .expect("login second user");

        let axum::Json(created_session) = api_create_session(
            axum::extract::State(state.clone()),
            auth_headers(&token_one, Some(&session_one.csrf_token)),
        )
        .await
        .expect("create session");
        let session_id = created_session.session.session_id;

        let axum::Json(created_task) = api_create_task(
            axum::extract::State(state.clone()),
            auth_headers(&token_one, Some(&session_one.csrf_token)),
            axum::extract::Path(session_id.clone()),
            axum::Json(ApiCreateTaskRequest {
                input_markdown: "Summarize this".to_string(),
            }),
        )
        .await
        .expect("create task");
        let task_id = created_task.task.task_id;

        let completed = wait_for_task_status(
            &state,
            user_one.user_id,
            &session_id,
            &task_id,
            ApiTaskStatus::Completed,
        )
        .await;
        assert_eq!(completed.final_response_markdown.as_deref(), Some("ok"));
        assert!(completed.finished_at.is_some());
        assert!(completed.last_progress.is_some());
        assert!(completed.last_event_seq > 0);

        let axum::Json(task_events) = api_get_task_events(
            axum::extract::State(state.clone()),
            auth_headers(&token_one, None),
            axum::extract::Path((session_id.clone(), task_id.clone())),
            axum::extract::Query(TaskEventsQuery {
                after_seq: Some(0),
                limit: Some(200),
            }),
        )
        .await
        .expect("get persisted task events");
        assert!(!task_events.events.is_empty());
        assert_eq!(task_events.last_seq, completed.last_event_seq);

        let session_record = state
            .web_store
            .load_session(user_one.user_id, &session_id)
            .await
            .expect("load session")
            .expect("session exists");
        assert_eq!(session_record.active_task_id, None);
        assert_eq!(
            session_record.last_task_status,
            Some(ApiTaskStatus::Completed)
        );

        let axum::Json(listed_tasks) = api_list_tasks(
            axum::extract::State(state.clone()),
            auth_headers(&token_one, None),
            axum::extract::Path(session_id.clone()),
        )
        .await
        .expect("list tasks");
        assert_eq!(listed_tasks.tasks.len(), 1);
        assert_eq!(listed_tasks.tasks[0].task_id, task_id);

        let axum::Json(task_detail) = api_get_task(
            axum::extract::State(state.clone()),
            auth_headers(&token_one, None),
            axum::extract::Path((session_id.clone(), task_id.clone())),
        )
        .await
        .expect("get task");
        assert_eq!(
            task_detail.task.final_response_markdown.as_deref(),
            Some("ok")
        );

        let foreign_get = api_get_task(
            axum::extract::State(state.clone()),
            auth_headers(&token_two, None),
            axum::extract::Path((session_id.clone(), task_id.clone())),
        )
        .await;
        assert_eq!(
            foreign_get.expect_err("foreign task should be hidden").0,
            axum::http::StatusCode::NOT_FOUND
        );

        save_active_task(&state, &completed, ApiTaskStatus::Running, None).await;
        let busy_create = api_create_task(
            axum::extract::State(state.clone()),
            auth_headers(&token_one, Some(&session_one.csrf_token)),
            axum::extract::Path(session_id.clone()),
            axum::Json(ApiCreateTaskRequest {
                input_markdown: "Second task".to_string(),
            }),
        )
        .await;
        let (status, axum::Json(error)) = busy_create.expect_err("active task should fail");
        assert_eq!(status, axum::http::StatusCode::CONFLICT);
        assert_eq!(error.error.code, ErrorCode::SessionBusy);

        save_active_task(
            &state,
            &completed,
            ApiTaskStatus::WaitingForUserInput,
            Some(PendingUserInputView {
                kind: ApiUserInputKind::Text,
                prompt: "Need more input".to_string(),
            }),
        )
        .await;
        let waiting_create = api_create_task(
            axum::extract::State(state),
            auth_headers(&token_one, Some(&session_one.csrf_token)),
            axum::extract::Path(session_id),
            axum::Json(ApiCreateTaskRequest {
                input_markdown: "Third task".to_string(),
            }),
        )
        .await;
        let (status, axum::Json(error)) =
            waiting_create.expect_err("waiting task should fail distinctly");
        assert_eq!(status, axum::http::StatusCode::CONFLICT);
        assert_eq!(error.error.code, ErrorCode::TaskWaitingForUserInput);
        assert_eq!(
            error
                .error
                .details
                .as_ref()
                .and_then(|details| details.get("task_id").and_then(serde_json::Value::as_str)),
            Some("active-waiting")
        );
    }

    #[cfg(feature = "profile-lite")]
    #[tokio::test]
    async fn api_resume_waiting_task_reuses_task_id_and_persists_completion() {
        let state = test_app_state_with_responses(vec![
            ScriptedResponse::ToolCalls {
                tool_calls: Vec::new(),
                final_text: Some(
                    r#"{"thought":"need details","tool_call":null,"final_answer":null,"awaiting_user_input":{"kind":"text","prompt":"Send scope"}}"#
                        .to_string(),
                ),
            },
            ScriptedResponse::Text("resumed ok".to_string()),
        ]);
        let now = chrono::Utc::now();
        let user = register_user(
            state.web_store.as_ref(),
            RegisterRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            true,
            now,
        )
        .await
        .expect("register user");
        let (_, auth_session, token) = login_user(
            state.web_store.as_ref(),
            LoginRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            now,
        )
        .await
        .expect("login user");

        let axum::Json(created_session) = api_create_session(
            axum::extract::State(state.clone()),
            auth_headers(&token, Some(&auth_session.csrf_token)),
        )
        .await
        .expect("create session");
        let session_id = created_session.session.session_id;

        let axum::Json(created_task) = api_create_task(
            axum::extract::State(state.clone()),
            auth_headers(&token, Some(&auth_session.csrf_token)),
            axum::extract::Path(session_id.clone()),
            axum::Json(ApiCreateTaskRequest {
                input_markdown: "Investigate Codex limits".to_string(),
            }),
        )
        .await
        .expect("create task");
        let task_id = created_task.task.task_id;

        let waiting = wait_for_task_status(
            &state,
            user.user_id,
            &session_id,
            &task_id,
            ApiTaskStatus::WaitingForUserInput,
        )
        .await;
        assert_eq!(
            waiting
                .pending_user_input
                .as_ref()
                .map(|input| input.prompt.as_str()),
            Some("Send scope")
        );

        let axum::Json(resumed) = api_resume_task(
            axum::extract::State(state.clone()),
            auth_headers(&token, Some(&auth_session.csrf_token)),
            axum::extract::Path((session_id.clone(), task_id.clone())),
            axum::Json(ApiResumeTaskRequest {
                input_markdown: "Scope is GPT-5.4-mini".to_string(),
            }),
        )
        .await
        .expect("resume waiting task");
        assert_eq!(resumed.task.task_id, task_id);
        assert_eq!(resumed.task.status, ApiTaskStatus::Running);

        let completed = wait_for_task_status(
            &state,
            user.user_id,
            &session_id,
            &task_id,
            ApiTaskStatus::Completed,
        )
        .await;
        assert_eq!(
            completed.final_response_markdown.as_deref(),
            Some("resumed ok")
        );

        let session = state
            .web_store
            .load_session(user.user_id, &session_id)
            .await
            .expect("load session")
            .expect("session exists");
        assert_eq!(session.active_task_id, None);
        assert_eq!(session.last_task_status, Some(ApiTaskStatus::Completed));
    }

    fn test_app_state() -> AppState {
        test_app_state_with_responses(vec![ScriptedResponse::Text("ok".to_string())])
    }

    fn test_app_state_with_responses(responses: Vec<ScriptedResponse>) -> AppState {
        let scripted = Arc::new(ScriptedLlmProvider::new(responses));
        let settings = Arc::new(AgentSettings {
            agent_model_id: Some("opencode-go/deepseek-v4-flash".to_string()),
            agent_model_provider: Some("opencode_go".to_string()),
            agent_model_routes: Some(vec![ModelInfo {
                id: "opencode-go/deepseek-v4-flash".to_string(),
                provider: "opencode_go".to_string(),
                max_output_tokens: 32_000,
                context_window_tokens: 200_000,
                weight: 1,
            }]),
            ..AgentSettings::default()
        });
        let mut llm = LlmClient::new(&settings);
        llm.register_provider("opencode_go".to_string(), scripted);
        let session_manager =
            WebSessionManager::new(SessionRegistry::new(), Arc::new(llm), settings);
        let mut state = AppState::new(Arc::new(session_manager));
        state.auto_title_enabled = false;
        state
    }

    fn unique_test_asset_dir(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("oxide-web-assets-{label}-{}", uuid::Uuid::new_v4()))
    }

    fn auth_headers(raw_token: &str, csrf_token: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            format!("{AUTH_COOKIE_NAME}={raw_token}")
                .parse()
                .expect("cookie header"),
        );
        if let Some(csrf_token) = csrf_token {
            headers.insert("x-csrf-token", csrf_token.parse().expect("csrf header"));
        }
        headers
    }

    fn sse_request(
        session_id: &str,
        task_id: &str,
        raw_token: &str,
        after_seq: Option<u64>,
    ) -> axum::http::Request<axum::body::Body> {
        let mut uri = format!("/api/v1/sessions/{session_id}/tasks/{task_id}/stream");
        if let Some(after_seq) = after_seq {
            uri.push_str(&format!("?after_seq={after_seq}"));
        }

        axum::http::Request::builder()
            .uri(uri)
            .header(
                axum::http::header::COOKIE,
                format!("{AUTH_COOKIE_NAME}={raw_token}"),
            )
            .body(axum::body::Body::empty())
            .expect("sse request")
    }

    fn web_env_mutex() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct EnvGuard {
        values: Vec<(&'static str, Option<String>)>,
    }

    impl EnvGuard {
        fn capture(keys: &[&'static str]) -> Self {
            Self {
                values: keys
                    .iter()
                    .map(|key| (*key, std::env::var(key).ok()))
                    .collect(),
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.values {
                if let Some(value) = value {
                    std::env::set_var(key, value);
                } else {
                    std::env::remove_var(key);
                }
            }
        }
    }

    fn task_record(
        user_id: i64,
        session_id: &str,
        task_id: &str,
        status: ApiTaskStatus,
        input_markdown: &str,
        created_at: chrono::DateTime<chrono::Utc>,
    ) -> WebTaskRecord {
        WebTaskRecord {
            schema_version: WEB_TASK_SCHEMA_VERSION,
            task_id: task_id.to_string(),
            session_id: session_id.to_string(),
            user_id,
            status,
            input_markdown: input_markdown.to_string(),
            input_edited_at: None,
            final_response_markdown: status
                .is_terminal()
                .then(|| "terminal response".to_string()),
            error_message: None,
            pending_user_input: None,
            last_progress: None,
            last_event_seq: 0,
            created_at,
            started_at: Some(created_at),
            updated_at: created_at,
            finished_at: status.is_terminal().then_some(created_at),
        }
    }

    fn persisted_event(
        user_id: i64,
        session_id: &str,
        task_id: &str,
        seq: u64,
        kind: TaskEventKind,
    ) -> PersistedTaskEvent {
        PersistedTaskEvent {
            schema_version: 1,
            task_id: task_id.to_string(),
            session_id: session_id.to_string(),
            user_id,
            seq,
            created_at: chrono::Utc::now(),
            kind,
            summary: format!("event-{seq}"),
            payload: serde_json::json!({ "seq": seq }),
            redacted: false,
            truncated: false,
        }
    }

    #[cfg(feature = "profile-lite")]
    async fn wait_for_task_status(
        state: &AppState,
        user_id: i64,
        session_id: &str,
        task_id: &str,
        status: ApiTaskStatus,
    ) -> WebTaskRecord {
        let mut last_task = None;
        for _ in 0..200 {
            let task = state
                .web_store
                .load_task(user_id, session_id, task_id)
                .await
                .expect("load task")
                .expect("task exists");
            if task.status == status {
                return task;
            }
            last_task = Some(task);
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        panic!("task {task_id} did not reach {status:?}; last state: {last_task:?}");
    }

    async fn wait_for_persisted_progress(
        state: &AppState,
        user_id: i64,
        session_id: &str,
        task_id: &str,
    ) -> ProgressSnapshot {
        for _ in 0..40 {
            let task = state
                .web_store
                .load_task(user_id, session_id, task_id)
                .await
                .expect("load task")
                .expect("task exists");
            if let Some(progress) = task.last_progress {
                return progress;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("task {task_id} did not receive persisted progress");
    }

    #[cfg(feature = "profile-lite")]
    async fn save_active_task(
        state: &AppState,
        base_task: &WebTaskRecord,
        status: ApiTaskStatus,
        pending_user_input: Option<PendingUserInputView>,
    ) {
        let now = chrono::Utc::now();
        let mut task = base_task.clone();
        task.task_id = format!("active-{}", status_string(status));
        task.status = status;
        task.final_response_markdown = None;
        task.error_message = None;
        task.pending_user_input = pending_user_input;
        task.updated_at = now;
        task.finished_at = None;
        task.schema_version = WEB_TASK_SCHEMA_VERSION;
        state
            .web_store
            .save_task(task.clone())
            .await
            .expect("save active task");

        let mut session = state
            .web_store
            .load_session(task.user_id, &task.session_id)
            .await
            .expect("load session")
            .expect("session exists");
        session.active_task_id = Some(task.task_id);
        session.last_task_status = Some(status);
        session.updated_at = now;
        state
            .web_store
            .save_session(session)
            .await
            .expect("save active session");
    }

    #[cfg(feature = "profile-lite")]
    fn status_string(status: ApiTaskStatus) -> &'static str {
        match status {
            ApiTaskStatus::Queued => "queued",
            ApiTaskStatus::Running => "running",
            ApiTaskStatus::WaitingForUserInput => "waiting",
            ApiTaskStatus::Completed => "completed",
            ApiTaskStatus::Failed => "failed",
            ApiTaskStatus::Cancelled => "cancelled",
            ApiTaskStatus::Interrupted => "interrupted",
        }
    }
}
