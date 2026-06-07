use axum::{
    extract::{Multipart, Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use oxide_agent_core::agent::preprocessor::Preprocessor;
use oxide_agent_web_contracts::{
    CreateSessionRequest as ApiCreateSessionRequest,
    CreateSessionResponse as ApiCreateSessionResponse, ErrorCode, ErrorEnvelope,
    GetSessionResponse, ListSessionsResponse, OkResponse, TaskAttachment,
    UpdateSessionProfileRequest, UpdateSessionRequest, UpdateSessionResponse,
    UploadTaskAttachmentsResponse, WebSessionRecord,
};
use std::collections::HashSet;
use std::sync::Arc;

use crate::persistence::WebSessionContextKeys;
use crate::session::{web_session_sandbox_scope, WebSessionRuntimeOptions};

use super::task_routes::{abort_task_handle, reject_active_task};
use super::{
    api_error, authenticated_user, authenticated_user_with_csrf, auto_title,
    backend_unavailable_response, canonical_model_selection, default_session_model_selection,
    load_current_user_record, load_execution_profile_for_agent_profile_id, load_owned_session,
    resolve_session_agent_profile_id, session_detail_from_record, session_summary_from_record,
    store_error_response, validate_optional_agent_profile_id, validate_session_title,
    web_chat_upload_limit_mb, AppState, WEB_SESSION_DEFAULT_TITLE, WEB_SESSION_FLOW_ID,
    WEB_SESSION_SCHEMA_VERSION,
};

async fn reconcile_web_sandbox_orphans_with_sessions(
    state: &AppState,
    user_id: i64,
    sessions: &[WebSessionContextKeys],
) -> Result<u64, String> {
    let live_contexts = sessions
        .iter()
        .flat_map(WebSessionContextKeys::tracked_context_keys)
        .collect::<HashSet<_>>();
    let sandbox_control = state.sandbox_control();
    let sandboxes = sandbox_control
        .list_user_sandboxes(user_id)
        .await
        .map_err(|error| error.to_string())?;
    let mut deleted = 0u64;

    for sandbox in sandboxes {
        let Some(scope) = sandbox.scope.as_deref() else {
            continue;
        };
        if !is_web_session_sandbox_scope(scope) {
            continue;
        }
        if scope != "web" && live_contexts.contains(scope) {
            continue;
        }

        match sandbox_control
            .delete_sandbox_by_name(user_id, &sandbox.container_name)
            .await
        {
            Ok(true) => deleted = deleted.saturating_add(1),
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(
                    user_id,
                    scope,
                    container_name = %sandbox.container_name,
                    error = %error,
                    "Failed to prune orphan web sandbox"
                );
            }
        }
    }

    Ok(deleted)
}

async fn reconcile_web_sandbox_orphans(state: &AppState, user_id: i64) -> Result<u64, String> {
    let sessions = state
        .web_store
        .list_session_context_keys(user_id)
        .await
        .map_err(|error| error.to_string())?;
    reconcile_web_sandbox_orphans_with_sessions(state, user_id, &sessions).await
}

#[derive(Debug)]
pub(crate) struct SessionSummariesCacheLoadError(String);

fn session_summaries_cache_error_response(
    error: Arc<SessionSummariesCacheLoadError>,
) -> (StatusCode, Json<ErrorEnvelope>) {
    api_error(
        StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::BackendUnavailable,
        format!("Web sessions are unavailable: {}", error.0),
        true,
    )
}

pub(crate) async fn prewarm_session_summaries_cache(state: AppState, user_id: i64) {
    if let Err(error) = cached_session_summaries(&state, user_id).await {
        tracing::debug!(
            target: "oxide_agent_transport_web::web_perf",
            user_id,
            error = ?error,
            "web sessions cache prewarm failed"
        );
    }
}

async fn cached_session_summaries(
    state: &AppState,
    user_id: i64,
) -> Result<ListSessionsResponse, Arc<SessionSummariesCacheLoadError>> {
    let state = state.clone();
    let cache = state.session_summaries_cache.clone();
    cache
        .try_get_with(user_id, async move {
            let sessions = state
                .web_store
                .list_session_summaries(user_id)
                .await
                .map_err(|error| SessionSummariesCacheLoadError(error.to_string()))?;
            Ok(ListSessionsResponse { sessions })
        })
        .await
}

pub(crate) async fn invalidate_session_summaries_cache(state: &AppState, user_id: i64) {
    state.session_summaries_cache.invalidate(&user_id).await;
}

fn is_web_session_sandbox_scope(scope: &str) -> bool {
    scope == "web" || scope.starts_with("web-session-")
}

async fn stage_task_attachments(
    state: &AppState,
    user_id: i64,
    session: &WebSessionRecord,
    mut multipart: Multipart,
) -> Result<Vec<TaskAttachment>, (StatusCode, Json<ErrorEnvelope>)> {
    let limit_mb = web_chat_upload_limit_mb();
    let max_bytes = limit_mb.saturating_mul(1024 * 1024);
    let sandbox_scope = web_session_sandbox_scope(user_id, &session.context_key);
    let preprocessor = Preprocessor::new(state.session_manager.llm_client(), sandbox_scope);
    let mut total_bytes = 0_u64;
    let mut attachments = Vec::new();

    while let Some(field) = multipart.next_field().await.map_err(|error| {
        api_error(
            StatusCode::BAD_REQUEST,
            ErrorCode::ValidationError,
            format!("Invalid multipart upload payload: {error}"),
            false,
        )
    })? {
        let Some(file_name) = field.file_name().map(ToString::to_string) else {
            continue;
        };
        let mime_type = field.content_type().map(ToString::to_string);
        let bytes = field.bytes().await.map_err(|error| {
            api_error(
                StatusCode::BAD_REQUEST,
                ErrorCode::ValidationError,
                format!("Failed to read uploaded file bytes: {error}"),
                false,
            )
        })?;
        total_bytes = total_bytes.saturating_add(bytes.len() as u64);
        if total_bytes > max_bytes {
            return Err(api_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorCode::ValidationError,
                format!("Total attachment upload size must be at most {limit_mb} MB per request."),
                false,
            ));
        }

        let staged = preprocessor
            .stage_document_upload(bytes.to_vec(), file_name.clone(), mime_type.clone(), None)
            .await
            .map_err(|error| {
                backend_unavailable_response(format!("Failed to stage uploaded file: {error}"))
            })?;
        attachments.push(TaskAttachment {
            file_name,
            mime_type,
            size_bytes: staged.size_bytes,
            sandbox_path: staged.sandbox_path,
        });
    }

    if attachments.is_empty() {
        return Err(api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationError,
            "At least one attachment file must be provided.",
            false,
        ));
    }

    Ok(attachments)
}

pub(crate) async fn api_list_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ListSessionsResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user(&state, &headers).await?;
    if let Some(response) = state.session_summaries_cache.get(&user.user_id).await {
        tracing::debug!(
            target: "oxide_agent_transport_web::web_perf",
            user_id = user.user_id,
            sessions_count = response.sessions.len(),
            sessions_cache_hit = true,
            "web sessions listed"
        );
        return Ok(Json(response));
    }

    let session_contexts = state
        .web_store
        .list_session_context_keys(user.user_id)
        .await
        .map_err(store_error_response)?;
    if let Err(error) =
        reconcile_web_sandbox_orphans_with_sessions(&state, user.user_id, &session_contexts).await
    {
        tracing::warn!(
            user_id = user.user_id,
            error = %error,
            "Web sandbox reconcile during list_sessions failed"
        );
    }
    let response = cached_session_summaries(&state, user.user_id)
        .await
        .map_err(session_summaries_cache_error_response)?;
    let sessions_count = response.sessions.len();
    tracing::debug!(
        target: "oxide_agent_transport_web::web_perf",
        user_id = user.user_id,
        sessions_count,
        sessions_cache_hit = false,
        "web sessions listed"
    );
    Ok(Json(response))
}

#[cfg(test)]
pub(crate) async fn api_create_session(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ApiCreateSessionResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    create_session_for_request(state, headers, ApiCreateSessionRequest::default()).await
}

pub(crate) async fn api_create_session_with_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ApiCreateSessionRequest>,
) -> Result<Json<ApiCreateSessionResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    create_session_for_request(state, headers, request).await
}

async fn create_session_for_request(
    state: AppState,
    headers: HeaderMap,
    request: ApiCreateSessionRequest,
) -> Result<Json<ApiCreateSessionResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    let user_record = load_current_user_record(&state, user.user_id).await?;
    let requested_model_selection = request
        .model_selection
        .map(canonical_model_selection)
        .transpose()?;
    let user_default_model_selection = if requested_model_selection.is_none() {
        user_record
            .default_model_selection
            .clone()
            .and_then(|selection| canonical_model_selection(selection).ok())
    } else {
        None
    };
    let model_selection = requested_model_selection
        .or(user_default_model_selection)
        .or_else(|| Some(default_session_model_selection(&state)));
    let agent_profile_id = resolve_session_agent_profile_id(
        &state,
        user.user_id,
        &user_record,
        request.agent_profile_selection,
    )
    .await?;
    let execution_profile = load_execution_profile_for_agent_profile_id(
        &state,
        user.user_id,
        agent_profile_id.as_deref(),
    )
    .await?;
    let session_id = uuid::Uuid::new_v4().to_string();
    let context_key = format!("web-session-{session_id}");
    let now = chrono::Utc::now();
    let sandbox_scope = web_session_sandbox_scope(user.user_id, &context_key);
    state
        .sandbox_control()
        .ensure_scope_sandbox(sandbox_scope.clone())
        .await
        .map_err(|error| backend_unavailable_response(error.to_string()))?;
    state
        .session_manager
        .create_session_with_model_selection(
            user.user_id,
            session_id.clone(),
            context_key.clone(),
            WEB_SESSION_FLOW_ID.to_string(),
            WebSessionRuntimeOptions {
                model_selection: model_selection.clone(),
                agent_profile_id: agent_profile_id.clone(),
                execution_profile,
            },
        )
        .await;

    let record = WebSessionRecord {
        schema_version: WEB_SESSION_SCHEMA_VERSION,
        session_id,
        user_id: user.user_id,
        title: WEB_SESSION_DEFAULT_TITLE.to_string(),
        context_key: context_key.clone(),
        context_keys: vec![context_key],
        agent_flow_id: WEB_SESSION_FLOW_ID.to_string(),
        model_selection,
        agent_profile_id,
        created_at: now,
        updated_at: now,
        active_task_id: None,
        last_task_status: None,
        last_preview: None,
        manually_renamed: false,
        auto_title_source_message: None,
        auto_title_replaceable_title: None,
        auto_title_attempts: 0,
        auto_title_next_attempt_at: None,
        auto_title_last_error: None,
    };
    if let Err(error) = state.web_store.save_session(record.clone()).await {
        state
            .session_manager
            .delete_session(&record.session_id)
            .await;
        if let Err(cleanup_error) = state.sandbox_control().destroy_scope(sandbox_scope).await {
            tracing::warn!(
                error = %cleanup_error,
                "Failed to rollback web sandbox after session save failure"
            );
        }
        return Err(store_error_response(error));
    }
    if let Err(error) = reconcile_web_sandbox_orphans(&state, user.user_id).await {
        tracing::warn!(
            user_id = user.user_id,
            error = %error,
            "Web sandbox reconcile after create_session failed"
        );
    }
    invalidate_session_summaries_cache(&state, user.user_id).await;
    Ok(Json(ApiCreateSessionResponse {
        session: session_summary_from_record(record),
    }))
}

pub(crate) async fn api_upload_task_attachments(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    multipart: Multipart,
) -> Result<Json<UploadTaskAttachmentsResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    let session = load_owned_session(&state, user.user_id, &session_id).await?;
    let attachments = stage_task_attachments(&state, user.user_id, &session, multipart).await?;
    Ok(Json(UploadTaskAttachmentsResponse { attachments }))
}

pub(crate) async fn api_get_session(
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

pub(crate) async fn api_update_session(
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
    auto_title::clear_session_auto_title(&mut record);
    record.updated_at = chrono::Utc::now();
    state
        .web_store
        .save_session(record.clone())
        .await
        .map_err(store_error_response)?;
    invalidate_session_summaries_cache(&state, user.user_id).await;
    Ok(Json(UpdateSessionResponse {
        session: session_detail_from_record(record),
    }))
}

pub(crate) async fn api_update_session_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(request): Json<UpdateSessionProfileRequest>,
) -> Result<Json<UpdateSessionResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    let mut record = load_owned_session(&state, user.user_id, &session_id).await?;
    reject_active_task(&state, user.user_id, &record).await?;
    let agent_profile_id =
        validate_optional_agent_profile_id(&state, user.user_id, request.agent_profile_id, true)
            .await?;
    let execution_profile = load_execution_profile_for_agent_profile_id(
        &state,
        user.user_id,
        agent_profile_id.as_deref(),
    )
    .await?;
    record.agent_profile_id = agent_profile_id.clone();
    record.updated_at = chrono::Utc::now();
    state
        .web_store
        .save_session(record.clone())
        .await
        .map_err(store_error_response)?;
    invalidate_session_summaries_cache(&state, user.user_id).await;
    state
        .session_manager
        .set_session_execution_profile(&session_id, agent_profile_id, execution_profile)
        .await;
    Ok(Json(UpdateSessionResponse {
        session: session_detail_from_record(record),
    }))
}

pub(crate) async fn api_delete_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Result<Json<OkResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    let record = load_owned_session(&state, user.user_id, &session_id).await?;
    if let Some(active_task_id) = record.active_task_id.as_deref() {
        state
            .session_manager
            .cancel_task(active_task_id, &session_id)
            .await;
        abort_task_handle(&state, active_task_id).await;
    }
    for context_key in record.tracked_context_keys() {
        state
            .sandbox_control()
            .destroy_scope(web_session_sandbox_scope(user.user_id, &context_key))
            .await
            .map_err(|error| backend_unavailable_response(error.to_string()))?;
        state
            .session_manager
            .storage()
            .clear_agent_memory_for_flow(user.user_id, context_key, record.agent_flow_id.clone())
            .await
            .map_err(|error| backend_unavailable_response(error.to_string()))?;
    }
    state
        .web_store
        .delete_session(user.user_id, &session_id)
        .await
        .map_err(store_error_response)?;
    state.session_manager.delete_session(&session_id).await;
    if let Err(error) = reconcile_web_sandbox_orphans(&state, user.user_id).await {
        tracing::warn!(
            user_id = user.user_id,
            error = %error,
            "Web sandbox reconcile after delete_session failed"
        );
    }
    invalidate_session_summaries_cache(&state, user.user_id).await;
    Ok(Json(OkResponse::ok()))
}
