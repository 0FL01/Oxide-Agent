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
//! - `GET /api/v1/settings` — read current user's web settings
//! - `PATCH /api/v1/settings` — update current user's web settings
//! - `GET /api/v1/sessions` — list current user's web sessions
//! - `POST /api/v1/sessions` — create current user's web session
//! - `GET /api/v1/sessions/:session_id` — get current user's web session
//! - `PATCH /api/v1/sessions/:session_id` — rename current user's web session
//! - `DELETE /api/v1/sessions/:session_id` — delete current user's web session

mod auth_helpers;
mod auto_title;
mod converters;
mod router;
mod sse;
mod static_assets;
mod task_executor;
mod types;
use auth_helpers::*;
use converters::*;
pub use router::{build_router, serve};
pub use types::*;

use crate::auth::{
    bootstrap_user, change_password, create_auth_session_for_user, current_user_for_token,
    login_user, logout_session, register_user,
};
use crate::persistence::{WebUiStore, WebUserRecord};
use crate::session::{web_session_sandbox_scope, WebSessionRuntimeOptions};
use axum::{
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{
        header::{CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE, SET_COOKIE},
        HeaderMap, HeaderValue, StatusCode,
    },
    response::Response,
    Json,
};
use oxide_agent_core::agent::{
    parse_agent_profile, preprocessor::Preprocessor, AgentExecutionProfile, ToolAccessPolicy,
};
use oxide_agent_core::llm::DiscoveredLlmModel;
use oxide_agent_core::storage::{AgentProfileRecord, StorageError, UpsertAgentProfileOptions};
use oxide_agent_web_contracts::{
    AgentProfileSelection, AgentProfileView, AuthUserResponse, BootstrapRequest,
    CancelTaskResponse as ApiCancelTaskResponse, ChangePasswordRequest, CreateAgentProfileRequest,
    CreateAgentProfileResponse, CreateSessionRequest as ApiCreateSessionRequest,
    CreateSessionResponse as ApiCreateSessionResponse, CreateTaskRequest as ApiCreateTaskRequest,
    CreateTaskResponse as ApiCreateTaskResponse,
    CreateTaskVersionRequest as ApiCreateTaskVersionRequest,
    CreateTaskVersionResponse as ApiCreateTaskVersionResponse, CurrentUser, CurrentUserResponse,
    ErrorCode, ErrorEnvelope, GetSessionResponse, GetTaskProgressResponse, GetTaskResponse,
    ListAgentProfilesResponse, ListModelRoutesResponse, ListSessionsResponse, ListTasksResponse,
    LoginRequest, ModelRouteProtocolView, ModelRouteSourceView, ModelRouteView, ModelSelection,
    OkResponse, PersistedTaskEvent, PublicConfigResponse, RegisterRequest,
    ResumeTaskRequest as ApiResumeTaskRequest, ResumeTaskResponse as ApiResumeTaskResponse,
    TaskAttachment, TaskEventKind, TaskEventsResponse, TaskStatus as ApiTaskStatus,
    UpdateAgentProfileRequest, UpdateAgentProfileResponse, UpdateSessionProfileRequest,
    UpdateSessionRequest, UpdateSessionResponse, UpdateUserSettingsRequest,
    UploadTaskAttachmentsResponse, UserMessageEventPayload, UserSettingsResponse, WebSessionRecord,
    WebTaskRecord,
};
use serde::Deserialize;
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

const DEFAULT_OPENCODE_GO_QUALIFIED_MODEL_ID: &str = "opencode-go/kimi-k2.6";
const MAX_MODEL_SELECTION_CHARS: usize = 128;
const MAX_AGENT_PROFILE_ID_CHARS: usize = 64;
const MAX_AGENT_PROFILE_NAME_CHARS: usize = 80;
const MAX_AGENT_PROFILE_PROMPT_CHARS: usize = 32_000;
const AUTO_TITLE_SYNC_TIMEOUT_SECS: u64 = 5;

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

async fn api_list_model_routes(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ListModelRoutesResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    authenticated_user(&state, &headers).await?;
    Ok(Json(model_routes_response(&state, false).await))
}

async fn api_refresh_model_routes(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ListModelRoutesResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    authenticated_user_with_csrf(&state, &headers).await?;
    Ok(Json(model_routes_response(&state, true).await))
}

async fn api_get_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<UserSettingsResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user(&state, &headers).await?;
    let record = load_current_user_record(&state, user.user_id).await?;
    Ok(Json(user_settings_response_from_record(&record)))
}

async fn api_update_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UpdateUserSettingsRequest>,
) -> Result<Json<UserSettingsResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    let mut record = load_current_user_record(&state, user.user_id).await?;
    record.default_model_selection = request
        .default_model_selection
        .map(canonical_model_selection)
        .transpose()?;
    record.default_agent_profile_id = validate_optional_agent_profile_id(
        &state,
        user.user_id,
        request.default_agent_profile_id,
        true,
    )
    .await?;
    record.default_effort = request.default_effort;
    record.updated_at = chrono::Utc::now();
    state
        .web_store
        .save_user(record.clone())
        .await
        .map_err(store_error_response)?;
    Ok(Json(user_settings_response_from_record(&record)))
}

async fn model_routes_response(state: &AppState, refresh: bool) -> ListModelRoutesResponse {
    let llm = state.session_manager.llm_client();
    let mut models = Vec::new();
    if let Some(go_models) = if refresh {
        llm.refresh_opencode_go_models().await
    } else {
        llm.opencode_go_models().await
    } {
        models.extend(go_models);
    }
    if let Some(zen_models) = if refresh {
        llm.refresh_opencode_zen_models().await
    } else {
        llm.opencode_zen_models().await
    } {
        models.extend(zen_models);
    }
    let routes = models
        .into_iter()
        .map(model_route_view_from_discovered)
        .collect();

    ListModelRoutesResponse {
        provider_id: "opencode".to_string(),
        provider_available: opencode_provider_available(state),
        default_model_id: default_opencode_model_id(state),
        routes,
    }
}

fn model_route_view_from_discovered(model: DiscoveredLlmModel) -> ModelRouteView {
    let protocol = model_route_protocol_view(&model.protocol);
    ModelRouteView {
        provider_id: model.provider_id,
        model_id: model.model_id,
        qualified_id: model.qualified_id,
        display_name: model.display_name,
        protocol,
        source: model_route_source_view(&model.source),
        fetched_at: model.fetched_at,
        runnable: protocol != ModelRouteProtocolView::Unknown,
    }
}

fn model_route_protocol_view(value: &str) -> ModelRouteProtocolView {
    match value.trim().to_ascii_lowercase().as_str() {
        "openai_chat_completions" => ModelRouteProtocolView::OpenAiChatCompletions,
        "anthropic_messages" => ModelRouteProtocolView::AnthropicMessages,
        _ => ModelRouteProtocolView::Unknown,
    }
}

fn model_route_source_view(value: &str) -> ModelRouteSourceView {
    match value.trim().to_ascii_lowercase().as_str() {
        "network" => ModelRouteSourceView::Network,
        "cache" => ModelRouteSourceView::Cache,
        _ => ModelRouteSourceView::Fallback,
    }
}

fn opencode_provider_available(state: &AppState) -> bool {
    let llm = state.session_manager.llm_client();
    llm.is_provider_available("opencode-go")
        || llm.is_provider_available("opencode_go")
        || llm.is_provider_available("opencode-zen")
        || llm.is_provider_available("opencode_zen")
}

fn default_opencode_model_id(state: &AppState) -> Option<String> {
    state
        .session_manager
        .agent_settings()
        .get_configured_agent_model_routes()
        .into_iter()
        .find(|route| opencode_provider_prefix(&route.provider).is_some())
        .and_then(|route| qualified_opencode_model_id(&route.id, &route.provider))
}

fn default_session_model_selection(state: &AppState) -> ModelSelection {
    ModelSelection {
        qualified_id: default_opencode_model_id(state)
            .unwrap_or_else(|| DEFAULT_OPENCODE_GO_QUALIFIED_MODEL_ID.to_string()),
    }
}

fn user_settings_response_from_record(record: &WebUserRecord) -> UserSettingsResponse {
    UserSettingsResponse {
        default_model_selection: record.default_model_selection.clone(),
        default_agent_profile_id: record.default_agent_profile_id.clone(),
        default_effort: record.default_effort,
    }
}

async fn load_current_user_record(
    state: &AppState,
    user_id: i64,
) -> Result<WebUserRecord, (StatusCode, Json<ErrorEnvelope>)> {
    state
        .web_store
        .load_user(user_id)
        .await
        .map_err(store_error_response)?
        .ok_or_else(|| {
            api_error(
                StatusCode::UNAUTHORIZED,
                ErrorCode::Unauthorized,
                "Authenticated web user no longer exists.",
                false,
            )
        })
}

async fn api_list_agent_profiles(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ListAgentProfilesResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user(&state, &headers).await?;
    let mut profiles = state
        .session_manager
        .storage()
        .list_agent_profiles(user.user_id)
        .await
        .map_err(control_plane_storage_error_response)?
        .into_iter()
        .map(agent_profile_view_from_record)
        .collect::<Vec<_>>();
    profiles.sort_by(|left, right| {
        left.display_name
            .to_ascii_lowercase()
            .cmp(&right.display_name.to_ascii_lowercase())
            .then_with(|| left.agent_id.cmp(&right.agent_id))
    });
    Ok(Json(ListAgentProfilesResponse { profiles }))
}

async fn api_create_agent_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateAgentProfileRequest>,
) -> Result<Json<CreateAgentProfileResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    let display_name = validate_agent_profile_display_name(&request.display_name)?;
    let system_prompt = validate_agent_profile_system_prompt(&request.system_prompt)?;
    let agent_id = uuid::Uuid::new_v4().to_string();
    let record = state
        .session_manager
        .storage()
        .upsert_agent_profile(UpsertAgentProfileOptions {
            user_id: user.user_id,
            agent_id,
            profile: agent_profile_payload(&display_name, &system_prompt),
        })
        .await
        .map_err(control_plane_storage_error_response)?;
    Ok(Json(CreateAgentProfileResponse {
        profile: agent_profile_view_from_record(record),
    }))
}

async fn api_update_agent_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(agent_id): Path<String>,
    Json(request): Json<UpdateAgentProfileRequest>,
) -> Result<Json<UpdateAgentProfileResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    let agent_id = validate_agent_profile_id(&agent_id)?;
    load_owned_agent_profile(&state, user.user_id, &agent_id).await?;
    let display_name = validate_agent_profile_display_name(&request.display_name)?;
    let system_prompt = validate_agent_profile_system_prompt(&request.system_prompt)?;
    let record = state
        .session_manager
        .storage()
        .upsert_agent_profile(UpsertAgentProfileOptions {
            user_id: user.user_id,
            agent_id: agent_id.clone(),
            profile: agent_profile_payload(&display_name, &system_prompt),
        })
        .await
        .map_err(control_plane_storage_error_response)?;
    refresh_runtime_sessions_for_profile(&state, user.user_id, &agent_id).await?;
    Ok(Json(UpdateAgentProfileResponse {
        profile: agent_profile_view_from_record(record),
    }))
}

async fn api_delete_agent_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(agent_id): Path<String>,
) -> Result<Json<OkResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    let agent_id = validate_agent_profile_id(&agent_id)?;
    load_owned_agent_profile(&state, user.user_id, &agent_id).await?;
    state
        .session_manager
        .storage()
        .delete_agent_profile(user.user_id, agent_id.clone())
        .await
        .map_err(control_plane_storage_error_response)?;
    clear_agent_profile_references(&state, user.user_id, &agent_id).await?;
    Ok(Json(OkResponse { ok: true }))
}

async fn refresh_runtime_sessions_for_profile(
    state: &AppState,
    user_id: i64,
    agent_id: &str,
) -> Result<(), (StatusCode, Json<ErrorEnvelope>)> {
    let execution_profile =
        load_execution_profile_for_agent_profile_id(state, user_id, Some(agent_id))
            .await?
            .ok_or_else(not_found_response)?;
    let sessions = state
        .web_store
        .list_sessions(user_id)
        .await
        .map_err(store_error_response)?;
    for session in sessions {
        if session.agent_profile_id.as_deref() == Some(agent_id) {
            state
                .session_manager
                .set_session_execution_profile(
                    &session.session_id,
                    Some(agent_id.to_string()),
                    Some(execution_profile.clone()),
                )
                .await;
        }
    }
    Ok(())
}

async fn clear_agent_profile_references(
    state: &AppState,
    user_id: i64,
    agent_id: &str,
) -> Result<(), (StatusCode, Json<ErrorEnvelope>)> {
    let mut user_record = load_current_user_record(state, user_id).await?;
    if user_record.default_agent_profile_id.as_deref() == Some(agent_id) {
        user_record.default_agent_profile_id = None;
        user_record.updated_at = chrono::Utc::now();
        state
            .web_store
            .save_user(user_record)
            .await
            .map_err(store_error_response)?;
    }

    let sessions = state
        .web_store
        .list_sessions(user_id)
        .await
        .map_err(store_error_response)?;
    for mut session in sessions {
        if session.agent_profile_id.as_deref() != Some(agent_id) {
            continue;
        }
        session.agent_profile_id = None;
        session.updated_at = chrono::Utc::now();
        state
            .web_store
            .save_session(session.clone())
            .await
            .map_err(store_error_response)?;
        state
            .session_manager
            .set_session_execution_profile(&session.session_id, None, None)
            .await;
    }
    Ok(())
}

fn agent_profile_payload(display_name: &str, system_prompt: &str) -> serde_json::Value {
    serde_json::json!({
        "displayName": display_name,
        "systemPrompt": system_prompt,
    })
}

fn agent_profile_view_from_record(record: AgentProfileRecord) -> AgentProfileView {
    AgentProfileView {
        agent_id: record.agent_id,
        display_name: agent_profile_display_name(&record.profile),
        system_prompt: agent_profile_system_prompt(&record.profile),
        version: record.version,
        created_at: unix_secs_to_datetime(record.created_at),
        updated_at: unix_secs_to_datetime(record.updated_at),
    }
}

fn agent_profile_display_name(profile: &serde_json::Value) -> String {
    profile
        .get("displayName")
        .or_else(|| profile.get("display_name"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Untitled profile")
        .to_string()
}

fn agent_profile_system_prompt(profile: &serde_json::Value) -> String {
    profile
        .get("systemPrompt")
        .or_else(|| profile.get("system_prompt"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_string()
}

fn unix_secs_to_datetime(secs: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0).unwrap_or_else(|| {
        chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0)
            .expect("unix epoch timestamp is valid")
    })
}

fn validate_agent_profile_id(agent_id: &str) -> Result<String, (StatusCode, Json<ErrorEnvelope>)> {
    let agent_id = agent_id.trim();
    if agent_id.is_empty() {
        return Err(api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationError,
            "Agent profile id must not be empty.",
            false,
        ));
    }
    if agent_id.chars().count() > MAX_AGENT_PROFILE_ID_CHARS {
        return Err(api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationError,
            format!("Agent profile id must be at most {MAX_AGENT_PROFILE_ID_CHARS} characters."),
            false,
        ));
    }
    if !agent_id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationError,
            "Agent profile id may contain only ASCII letters, digits, '.', '_' or '-'.",
            false,
        ));
    }
    Ok(agent_id.to_string())
}

fn validate_agent_profile_display_name(
    display_name: &str,
) -> Result<String, (StatusCode, Json<ErrorEnvelope>)> {
    let display_name = display_name.trim();
    if display_name.is_empty() {
        return Err(api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationError,
            "Agent profile name must not be empty.",
            false,
        ));
    }
    if display_name.chars().count() > MAX_AGENT_PROFILE_NAME_CHARS {
        return Err(api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationError,
            format!(
                "Agent profile name must be at most {MAX_AGENT_PROFILE_NAME_CHARS} characters."
            ),
            false,
        ));
    }
    Ok(display_name.to_string())
}

fn validate_agent_profile_system_prompt(
    system_prompt: &str,
) -> Result<String, (StatusCode, Json<ErrorEnvelope>)> {
    let system_prompt = system_prompt.trim();
    if system_prompt.is_empty() {
        return Err(api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationError,
            "Agent profile system prompt must not be empty.",
            false,
        ));
    }
    if system_prompt.chars().count() > MAX_AGENT_PROFILE_PROMPT_CHARS {
        return Err(api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationError,
            format!("Agent profile system prompt must be at most {MAX_AGENT_PROFILE_PROMPT_CHARS} characters."),
            false,
        ));
    }
    Ok(system_prompt.to_string())
}

async fn load_owned_agent_profile(
    state: &AppState,
    user_id: i64,
    agent_id: &str,
) -> Result<AgentProfileRecord, (StatusCode, Json<ErrorEnvelope>)> {
    state
        .session_manager
        .storage()
        .get_agent_profile(user_id, agent_id.to_string())
        .await
        .map_err(control_plane_storage_error_response)?
        .ok_or_else(not_found_response)
}

async fn validate_optional_agent_profile_id(
    state: &AppState,
    user_id: i64,
    agent_profile_id: Option<String>,
    missing_is_error: bool,
) -> Result<Option<String>, (StatusCode, Json<ErrorEnvelope>)> {
    let Some(agent_profile_id) = agent_profile_id else {
        return Ok(None);
    };
    let agent_profile_id = validate_agent_profile_id(&agent_profile_id)?;
    let exists = state
        .session_manager
        .storage()
        .get_agent_profile(user_id, agent_profile_id.clone())
        .await
        .map_err(control_plane_storage_error_response)?
        .is_some();
    if exists {
        Ok(Some(agent_profile_id))
    } else if missing_is_error {
        Err(not_found_response())
    } else {
        Ok(None)
    }
}

async fn resolve_session_agent_profile_id(
    state: &AppState,
    user_id: i64,
    user_record: &WebUserRecord,
    selection: AgentProfileSelection,
) -> Result<Option<String>, (StatusCode, Json<ErrorEnvelope>)> {
    match selection {
        AgentProfileSelection::Default => {
            validate_optional_agent_profile_id(
                state,
                user_id,
                user_record.default_agent_profile_id.clone(),
                false,
            )
            .await
        }
        AgentProfileSelection::None => Ok(None),
        AgentProfileSelection::Profile { agent_profile_id } => {
            validate_optional_agent_profile_id(state, user_id, Some(agent_profile_id), true).await
        }
    }
}

fn execution_profile_from_agent_profile(record: &AgentProfileRecord) -> AgentExecutionProfile {
    let parsed_profile = parse_agent_profile(&record.profile);
    AgentExecutionProfile::new(
        Some(record.agent_id.clone()),
        parsed_profile.prompt_instructions,
        ToolAccessPolicy::default(),
    )
}

async fn load_execution_profile_for_agent_profile_id(
    state: &AppState,
    user_id: i64,
    agent_profile_id: Option<&str>,
) -> Result<Option<AgentExecutionProfile>, (StatusCode, Json<ErrorEnvelope>)> {
    let Some(agent_profile_id) = agent_profile_id else {
        return Ok(None);
    };
    let Some(record) = state
        .session_manager
        .storage()
        .get_agent_profile(user_id, agent_profile_id.to_string())
        .await
        .map_err(control_plane_storage_error_response)?
    else {
        return Ok(None);
    };
    Ok(Some(execution_profile_from_agent_profile(&record)))
}

fn control_plane_storage_error_response(error: StorageError) -> (StatusCode, Json<ErrorEnvelope>) {
    match error {
        StorageError::InvalidInput(message) => api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationError,
            message,
            false,
        ),
        StorageError::ConcurrencyConflict { .. } => api_error(
            StatusCode::CONFLICT,
            ErrorCode::Conflict,
            error.to_string(),
            false,
        ),
        _ => backend_unavailable_response(error.to_string()),
    }
}

fn canonical_model_selection(
    selection: ModelSelection,
) -> Result<ModelSelection, (StatusCode, Json<ErrorEnvelope>)> {
    let qualified_id = selection.qualified_id.trim();
    if qualified_id.chars().count() > MAX_MODEL_SELECTION_CHARS {
        return Err(api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationError,
            format!("Model selection must be at most {MAX_MODEL_SELECTION_CHARS} characters."),
            false,
        ));
    }
    let (prefix, model_id) = parse_opencode_model_selection(qualified_id).ok_or_else(|| {
        api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationError,
            "Model selection must be an OpenCode Go or Zen model id.",
            false,
        )
    })?;
    if model_id.is_empty() || model_id.contains('/') {
        return Err(api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationError,
            "Model selection must be an OpenCode Go or Zen model id.",
            false,
        ));
    }
    Ok(ModelSelection {
        qualified_id: format!("{prefix}/{model_id}"),
    })
}

fn parse_opencode_model_selection(value: &str) -> Option<(&'static str, &str)> {
    let value = value.trim();
    if let Some(model_id) = value.strip_prefix("opencode-go/") {
        return Some(("opencode-go", model_id.trim()));
    }
    if let Some(model_id) = value.strip_prefix("opencode-zen/") {
        return Some(("opencode-zen", model_id.trim()));
    }
    if value.contains('/') {
        return None;
    }
    Some(("opencode-go", value))
}

fn opencode_provider_prefix(provider: &str) -> Option<&'static str> {
    match provider
        .trim()
        .strip_prefix("llm-provider/")
        .unwrap_or(provider.trim())
        .replace('_', "-")
        .to_ascii_lowercase()
        .as_str()
    {
        "opencode-go" => Some("opencode-go"),
        "opencode-zen" => Some("opencode-zen"),
        _ => None,
    }
}

fn qualified_opencode_model_id(model_id: &str, provider: &str) -> Option<String> {
    let prefix = opencode_provider_prefix(provider)?;
    let model_id = model_id.trim();
    if model_id.starts_with("opencode-go/") || model_id.starts_with("opencode-zen/") {
        parse_opencode_model_selection(model_id).and_then(|(route_prefix, route_model_id)| {
            (route_prefix == prefix).then(|| format!("{route_prefix}/{route_model_id}"))
        })
    } else {
        Some(format!("{prefix}/{model_id}"))
    }
}

fn backend_unavailable_response(message: impl Into<String>) -> (StatusCode, Json<ErrorEnvelope>) {
    api_error(
        StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::BackendUnavailable,
        message.into(),
        true,
    )
}

fn is_web_session_sandbox_scope(scope: &str) -> bool {
    scope == "web" || scope.starts_with("web-session-")
}

async fn abort_task_handle(state: &AppState, task_id: &str) {
    let handle = {
        let mut handles = state.task_handles.write().await;
        handles.remove(task_id)
    };
    if let Some(handle) = handle {
        handle.abort();
    }
}

async fn reconcile_web_sandbox_orphans_with_sessions(
    state: &AppState,
    user_id: i64,
    sessions: &[WebSessionRecord],
) -> Result<u64, String> {
    let live_contexts = sessions
        .iter()
        .flat_map(WebSessionRecord::tracked_context_keys)
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
        .list_sessions(user_id)
        .await
        .map_err(|error| error.to_string())?;
    reconcile_web_sandbox_orphans_with_sessions(state, user_id, &sessions).await
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

fn task_preview_source(input_markdown: &str, attachments: &[TaskAttachment]) -> String {
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

fn build_task_execution_input(input_markdown: &str, attachments: &[TaskAttachment]) -> String {
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
    let session_records = state
        .web_store
        .list_sessions(user.user_id)
        .await
        .map_err(store_error_response)?;
    if let Err(error) =
        reconcile_web_sandbox_orphans_with_sessions(&state, user.user_id, &session_records).await
    {
        tracing::warn!(
            user_id = user.user_id,
            error = %error,
            "Web sandbox reconcile during list_sessions failed"
        );
    }
    let sessions = session_records
        .into_iter()
        .map(session_summary_from_record)
        .collect();
    Ok(Json(ListSessionsResponse { sessions }))
}

#[cfg(test)]
async fn api_create_session(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ApiCreateSessionResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    create_session_for_request(state, headers, ApiCreateSessionRequest::default()).await
}

async fn api_create_session_with_request(
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
    Ok(Json(ApiCreateSessionResponse {
        session: session_summary_from_record(record),
    }))
}

async fn api_upload_task_attachments(
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

async fn api_update_session_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(request): Json<UpdateSessionProfileRequest>,
) -> Result<Json<UpdateSessionResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    reject_active_task(&state, user.user_id, &session_id).await?;
    let mut record = load_owned_session(&state, user.user_id, &session_id).await?;
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
    state
        .session_manager
        .set_session_execution_profile(&session_id, agent_profile_id, execution_profile)
        .await;
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
    let attachments = validate_task_attachments(&request.attachments)?;
    let input_markdown =
        validate_task_input_with_attachments(&request.input_markdown, !attachments.is_empty())?;
    let execution_input = build_task_execution_input(&input_markdown, &attachments);
    reject_active_task(&state, user.user_id, &session_id).await?;

    ensure_runtime_session(&state, user.user_id, &session).await;
    let Some(running_task) = state
        .session_manager
        .register_task(&session_id, execution_input.clone())
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
        version_group_id: task_id.clone(),
        version_index: 1,
        parent_task_id: None,
        status: ApiTaskStatus::Running,
        input_markdown: input_markdown.clone(),
        attachments: attachments.clone(),
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

    let preview_source = task_preview_source(&input_markdown, &attachments);
    let preview = markdown_preview(&preview_source);
    let should_auto_title = is_first_task && !session.manually_renamed;

    session.active_task_id = Some(task_id.clone());
    session.last_task_status = Some(ApiTaskStatus::Running);
    session.last_preview = Some(preview.clone());
    if should_auto_title && !state.auto_title_enabled && session.title == WEB_SESSION_DEFAULT_TITLE
    {
        session.title = preview.clone();
    }
    session.updated_at = now;
    state
        .web_store
        .save_session(session)
        .await
        .map_err(store_error_response)?;

    if should_auto_title && state.auto_title_enabled {
        let auto_title_request = auto_title::AutoTitleRequest {
            user_id: user.user_id,
            session_id: session_id.clone(),
            first_user_message: preview_source,
            fallback_preview: preview,
        };
        match tokio::time::timeout(
            std::time::Duration::from_secs(AUTO_TITLE_SYNC_TIMEOUT_SECS),
            auto_title::generate_and_save_auto_title(state.clone(), auto_title_request),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!(session_id = %session_id, error = %error, "auto title generation failed before task start");
            }
            Err(_) => {
                tracing::warn!(session_id = %session_id, "auto title generation timed out before task start");
            }
        }
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
        task_executor::TaskRunRequest::Execute {
            input: execution_input,
            effort: request.effort,
        },
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

async fn api_download_task_file(
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
struct TaskFileDownloadQuery {
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

async fn api_create_task_version(
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
    let execution_input = build_task_execution_input(&input_markdown, &attachments);
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
        .ensure_scope_sandbox(web_session_sandbox_scope(
            user.user_id,
            &session.context_key,
        ))
        .await
        .map_err(|error| backend_unavailable_response(error.to_string()))?;
    state
        .web_store
        .save_session(session.clone())
        .await
        .map_err(store_error_response)?;

    recreate_runtime_session(&state, user.user_id, &session).await;
    let Some(running_task) = state
        .session_manager
        .register_task(&session_id, execution_input.clone())
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
    let task = WebTaskRecord {
        schema_version: WEB_TASK_SCHEMA_VERSION,
        task_id: version_task_id.clone(),
        session_id: session_id.clone(),
        user_id: user.user_id,
        version_group_id,
        version_index,
        parent_task_id: Some(parent_task.task_id.clone()),
        status: ApiTaskStatus::Running,
        input_markdown: input_markdown.clone(),
        attachments: attachments.clone(),
        input_edited_at: Some(now),
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
        .save_task(task.clone())
        .await
        .map_err(store_error_response)?;

    let preview_source = task_preview_source(&input_markdown, &attachments);
    let preview = markdown_preview(&preview_source);
    let old_preview = session.last_preview.clone();
    session.active_task_id = Some(version_task_id.clone());
    session.last_task_status = Some(ApiTaskStatus::Running);
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

    let persistence = task_executor::WebTaskPersistence {
        web_store: state.web_store.clone(),
        user_id: user.user_id,
        session_id: session_id.clone(),
        task_id: version_task_id,
    };
    task_executor::spawn_registered_task(
        state.clone(),
        session_id,
        running_task,
        task_executor::TaskRunRequest::Execute {
            input: execution_input,
            effort: request.effort,
        },
        Some(persistence),
    )
    .await;

    Ok(Json(ApiCreateTaskVersionResponse {
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
    let attachments = validate_task_attachments(&request.attachments)?;
    let input_markdown =
        validate_task_input_with_attachments(&request.input_markdown, !attachments.is_empty())?;
    let execution_input = build_task_execution_input(&input_markdown, &attachments);
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
        .register_existing_task(&session_id, &task_id, execution_input.clone())
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

    let mut session = session;
    session.active_task_id = Some(task_id.clone());
    session.last_task_status = Some(ApiTaskStatus::Running);
    session.last_preview = Some(markdown_preview(&task_preview_source(
        &input_markdown,
        &attachments,
    )));
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
        task_executor::TaskRunRequest::ResumeUserInput {
            input: execution_input,
            effort: request.effort,
        },
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
    abort_task_handle(&state, &task_id).await;

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

pub(crate) fn api_error(
    status: StatusCode,
    code: ErrorCode,
    message: impl Into<String>,
    retryable: bool,
) -> (StatusCode, Json<ErrorEnvelope>) {
    (status, Json(ErrorEnvelope::new(code, message, retryable)))
}

#[cfg(test)]
mod tests;
