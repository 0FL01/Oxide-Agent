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
use crate::persistence::{WebUiStore, WebUserRecord};
use crate::session::{web_session_sandbox_scope, WebSessionRuntimeOptions};
use axum::{
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{
        header::{
            CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_SECURITY_POLICY,
            CONTENT_TYPE, SET_COOKIE,
        },
        HeaderMap, HeaderValue, Request, StatusCode,
    },
    middleware::{self, Next},
    response::Response,
    routing::{delete, get, patch, post},
    Json, Router,
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
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

const DEFAULT_OPENCODE_GO_QUALIFIED_MODEL_ID: &str = "opencode-go/kimi-k2.6";
const MAX_MODEL_SELECTION_CHARS: usize = 128;
const MAX_AGENT_PROFILE_ID_CHARS: usize = 64;
const MAX_AGENT_PROFILE_NAME_CHARS: usize = 80;
const MAX_AGENT_PROFILE_PROMPT_CHARS: usize = 32_000;

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
        .map(|session| session.context_key.clone())
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
        context_key,
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
    state
        .sandbox_control()
        .destroy_scope(web_session_sandbox_scope(user.user_id, &record.context_key))
        .await
        .map_err(|error| backend_unavailable_response(error.to_string()))?;
    state
        .session_manager
        .storage()
        .clear_agent_memory_for_flow(
            user.user_id,
            record.context_key.clone(),
            record.agent_flow_id.clone(),
        )
        .await
        .map_err(|error| backend_unavailable_response(error.to_string()))?;
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
                first_user_message: preview_source,
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
        task_executor::TaskRunRequest::Execute(execution_input),
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
        task_executor::TaskRunRequest::Execute(execution_input),
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
        task_executor::TaskRunRequest::ResumeUserInput(execution_input),
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
        .route(
            "/api/v1/settings",
            get(api_get_settings).patch(api_update_settings),
        )
        .route("/api/v1/model-routes", get(api_list_model_routes))
        .route(
            "/api/v1/model-routes/refresh",
            post(api_refresh_model_routes),
        )
        .route("/api/v1/agent-profiles", get(api_list_agent_profiles))
        .route("/api/v1/agent-profiles", post(api_create_agent_profile))
        .route(
            "/api/v1/agent-profiles/:agent_id",
            patch(api_update_agent_profile),
        )
        .route(
            "/api/v1/agent-profiles/:agent_id",
            delete(api_delete_agent_profile),
        )
        .route("/api/v1/sessions", get(api_list_sessions))
        .route("/api/v1/sessions", post(api_create_session_with_request))
        .route("/api/v1/sessions/:session_id", get(api_get_session))
        .route("/api/v1/sessions/:session_id", patch(api_update_session))
        .route(
            "/api/v1/sessions/:session_id/profile",
            patch(api_update_session_profile),
        )
        .route("/api/v1/sessions/:session_id", delete(api_delete_session))
        .route(
            "/api/v1/sessions/:session_id/uploads",
            post(api_upload_task_attachments),
        )
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
            "/api/v1/sessions/:session_id/tasks/:task_id/files/:file_id",
            get(api_download_task_file),
        )
        .route(
            "/api/v1/sessions/:session_id/tasks/:task_id/stream",
            get(sse::api_sse_task_stream),
        )
        .route(
            "/api/v1/sessions/:session_id/tasks/:task_id/versions",
            post(api_create_task_version),
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
    use async_trait::async_trait;
    use axum::http::HeaderMap;
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::Instant;

    use oxide_agent_core::agent::progress::{FileDeliveryKind, LlmRetryState, ProgressState};
    use oxide_agent_core::agent::AgentMemory;
    use oxide_agent_core::agent::{TodoItem, TodoList, TodoStatus};
    use oxide_agent_core::config::{AgentSettings, ModelInfo};
    use oxide_agent_core::llm::LlmClient;
    use oxide_agent_core::sandbox::{SandboxContainerRecord, SandboxScope};
    use oxide_agent_runtime::SessionRegistry;
    use oxide_agent_web_contracts::{
        AgentProfileSelection, CreateAgentProfileRequest,
        CreateSessionRequest as ApiCreateSessionRequest,
        CreateTaskVersionRequest as ApiCreateTaskVersionRequest, ErrorCode, LoginRequest,
        ModelSelection, PersistedTaskEvent, ProgressSnapshot, RegisterRequest, TaskAttachment,
        TaskEventKind, TaskStatus as ApiTaskStatus, UpdateSessionProfileRequest,
        UpdateUserSettingsRequest, WebTaskRecord,
    };
    #[cfg(feature = "profile-lite")]
    use oxide_agent_web_contracts::{
        CreateTaskRequest as ApiCreateTaskRequest, PendingUserInputView,
        ResumeTaskRequest as ApiResumeTaskRequest, UserInputKind as ApiUserInputKind,
    };
    use tokio::sync::mpsc;

    use crate::persistence::{WebTaskFileRecord, WEB_TASK_FILE_SCHEMA_VERSION};

    use super::{
        api_cancel_task, api_create_agent_profile, api_create_session,
        api_create_session_with_request, api_create_task_version, api_delete_session,
        api_get_session, api_get_settings, api_get_task_events, api_get_task_progress,
        api_list_sessions, api_update_session_profile, api_update_settings, auth_cookie_value,
        csrf_header_value, parse_web_bool, AppState, TaskEventsQuery, WebAssetsConfig,
        WebSandboxControl, WebStartupError, AUTH_COOKIE_NAME, WEB_TASK_SCHEMA_VERSION,
    };
    #[cfg(feature = "profile-lite")]
    use super::{api_create_task, api_get_task, api_list_tasks, api_resume_task};
    use crate::auth::{login_user, register_user};
    use crate::scripted_llm::{ScriptedLlmProvider, ScriptedResponse};
    use crate::session::WebSessionManager;

    #[derive(Clone, Default)]
    struct FakeSandboxControl {
        state: Arc<Mutex<FakeSandboxState>>,
    }

    #[derive(Default)]
    struct FakeSandboxState {
        ensured_scopes: Vec<SandboxScope>,
        destroyed_scopes: Vec<SandboxScope>,
        deleted_names: Vec<String>,
        sandboxes: Vec<SandboxContainerRecord>,
    }

    impl FakeSandboxControl {
        fn with_sandboxes(sandboxes: Vec<SandboxContainerRecord>) -> Self {
            Self {
                state: Arc::new(Mutex::new(FakeSandboxState {
                    sandboxes,
                    ..FakeSandboxState::default()
                })),
            }
        }

        fn ensured_scopes(&self) -> Vec<SandboxScope> {
            self.state
                .lock()
                .expect("fake sandbox state")
                .ensured_scopes
                .clone()
        }

        fn destroyed_scopes(&self) -> Vec<SandboxScope> {
            self.state
                .lock()
                .expect("fake sandbox state")
                .destroyed_scopes
                .clone()
        }

        fn deleted_names(&self) -> Vec<String> {
            self.state
                .lock()
                .expect("fake sandbox state")
                .deleted_names
                .clone()
        }
    }

    fn fake_sandbox_record(scope: SandboxScope) -> SandboxContainerRecord {
        SandboxContainerRecord {
            container_id: scope.stable_name(),
            container_name: scope.container_name(),
            image: Some("fake-image".to_string()),
            created_at: None,
            state: Some("running".to_string()),
            status: Some("running".to_string()),
            running: true,
            user_id: Some(scope.owner_id()),
            scope: Some(scope.namespace().to_string()),
            chat_id: scope.chat_id(),
            thread_id: scope.thread_id(),
            labels: scope.docker_labels(),
        }
    }

    #[async_trait]
    impl WebSandboxControl for FakeSandboxControl {
        async fn destroy_scope(&self, scope: SandboxScope) -> anyhow::Result<()> {
            let mut state = self.state.lock().expect("fake sandbox state");
            state.destroyed_scopes.push(scope.clone());
            state
                .sandboxes
                .retain(|sandbox| sandbox.container_name != scope.container_name());
            Ok(())
        }

        async fn list_user_sandboxes(
            &self,
            user_id: i64,
        ) -> anyhow::Result<Vec<SandboxContainerRecord>> {
            let state = self.state.lock().expect("fake sandbox state");
            Ok(state
                .sandboxes
                .iter()
                .filter(|sandbox| sandbox.user_id == Some(user_id))
                .cloned()
                .collect())
        }

        async fn ensure_scope_sandbox(
            &self,
            scope: SandboxScope,
        ) -> anyhow::Result<SandboxContainerRecord> {
            let mut state = self.state.lock().expect("fake sandbox state");
            state.ensured_scopes.push(scope.clone());
            let record = fake_sandbox_record(scope);
            state
                .sandboxes
                .retain(|sandbox| sandbox.container_name != record.container_name);
            state.sandboxes.push(record.clone());
            Ok(record)
        }

        async fn delete_sandbox_by_name(
            &self,
            user_id: i64,
            container_name: &str,
        ) -> anyhow::Result<bool> {
            let mut state = self.state.lock().expect("fake sandbox state");
            state.deleted_names.push(container_name.to_string());
            let before = state.sandboxes.len();
            state.sandboxes.retain(|sandbox| {
                !(sandbox.user_id == Some(user_id) && sandbox.container_name == container_name)
            });
            Ok(before != state.sandboxes.len())
        }
    }

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

    #[tokio::test]
    async fn api_list_model_routes_returns_empty_models_when_discovery_is_unavailable() {
        let _lock = web_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _guard = EnvGuard::capture(&[
            "OPENCODE_API_KEY",
            "OPENCODE_ZEN_API_KEY",
            "OPENCODE_GO_API_KEY",
            "OPENCODE_GO_MODELS_URL",
            "OPENCODE_ZEN_MODELS_URL",
            "LLM_HTTP_TIMEOUT_SECS",
        ]);
        std::env::set_var("OPENCODE_API_KEY", "test-opencode-key");
        std::env::remove_var("OPENCODE_ZEN_API_KEY");
        std::env::remove_var("OPENCODE_GO_API_KEY");
        std::env::set_var("OPENCODE_GO_MODELS_URL", "http://127.0.0.1:9/models");
        std::env::set_var("OPENCODE_ZEN_MODELS_URL", "http://127.0.0.1:9/models");
        std::env::set_var("LLM_HTTP_TIMEOUT_SECS", "1");

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

        let axum::Json(response) = super::api_list_model_routes(
            axum::extract::State(state.clone()),
            auth_headers(&token, None),
        )
        .await
        .expect("model routes response");

        assert!(response.provider_available);
        assert_eq!(
            response.default_model_id.as_deref(),
            Some("opencode-go/deepseek-v4-flash")
        );
        assert!(response.routes.is_empty());

        let axum::Json(refreshed) = super::api_refresh_model_routes(
            axum::extract::State(state),
            auth_headers(&token, Some(&auth_session.csrf_token)),
        )
        .await
        .expect("refresh model routes response");
        assert!(refreshed.routes.is_empty());
    }

    #[tokio::test]
    async fn api_settings_round_trips_default_model_selection() {
        let state = test_app_state();
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

        let axum::Json(initial) = api_get_settings(
            axum::extract::State(state.clone()),
            auth_headers(&token, None),
        )
        .await
        .expect("settings response");
        assert_eq!(initial.default_model_selection, None);

        let selected = ModelSelection {
            qualified_id: "kimi-k2.6".to_string(),
        };
        let axum::Json(updated) = api_update_settings(
            axum::extract::State(state.clone()),
            auth_headers(&token, Some(&auth_session.csrf_token)),
            axum::Json(UpdateUserSettingsRequest {
                default_model_selection: Some(selected),
                default_agent_profile_id: None,
            }),
        )
        .await
        .expect("update settings");
        assert_eq!(
            updated.default_model_selection,
            Some(ModelSelection {
                qualified_id: "opencode-go/kimi-k2.6".to_string(),
            })
        );
        let stored = state
            .web_store
            .load_user(user.user_id)
            .await
            .expect("load user")
            .expect("user exists");
        assert_eq!(
            stored.default_model_selection,
            updated.default_model_selection
        );

        let axum::Json(updated_zen) = api_update_settings(
            axum::extract::State(state.clone()),
            auth_headers(&token, Some(&auth_session.csrf_token)),
            axum::Json(UpdateUserSettingsRequest {
                default_model_selection: Some(ModelSelection {
                    qualified_id: "opencode-zen/deepseek-v4-flash-free".to_string(),
                }),
                default_agent_profile_id: None,
            }),
        )
        .await
        .expect("update zen settings");
        assert_eq!(
            updated_zen.default_model_selection,
            Some(ModelSelection {
                qualified_id: "opencode-zen/deepseek-v4-flash-free".to_string(),
            })
        );

        let (status, axum::Json(error)) = api_update_settings(
            axum::extract::State(state),
            auth_headers(&token, Some(&auth_session.csrf_token)),
            axum::Json(UpdateUserSettingsRequest {
                default_model_selection: Some(ModelSelection {
                    qualified_id: "other-provider/model".to_string(),
                }),
                default_agent_profile_id: None,
            }),
        )
        .await
        .expect_err("non-opencode model selection should fail");
        assert_eq!(status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(error.error.code, ErrorCode::ValidationError);
    }

    #[tokio::test]
    async fn api_create_session_persists_request_user_default_and_fallback_model_selection() {
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
        let (user, auth_session, token) = login_user(
            state.web_store.as_ref(),
            LoginRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            now,
        )
        .await
        .expect("login user");

        let axum::Json(fallback_created) = api_create_session(
            axum::extract::State(state.clone()),
            auth_headers(&token, Some(&auth_session.csrf_token)),
        )
        .await
        .expect("create fallback session");
        let fallback_record = state
            .web_store
            .load_session(user.user_id, &fallback_created.session.session_id)
            .await
            .expect("load fallback session")
            .expect("fallback session exists");
        assert_eq!(
            fallback_record.model_selection,
            Some(ModelSelection {
                qualified_id: "opencode-go/deepseek-v4-flash".to_string(),
            })
        );

        let axum::Json(_) = api_update_settings(
            axum::extract::State(state.clone()),
            auth_headers(&token, Some(&auth_session.csrf_token)),
            axum::Json(UpdateUserSettingsRequest {
                default_model_selection: Some(ModelSelection {
                    qualified_id: "opencode-go/kimi-k2.6".to_string(),
                }),
                default_agent_profile_id: None,
            }),
        )
        .await
        .expect("save user default");
        let axum::Json(default_created) = api_create_session(
            axum::extract::State(state.clone()),
            auth_headers(&token, Some(&auth_session.csrf_token)),
        )
        .await
        .expect("create default session");
        let default_record = state
            .web_store
            .load_session(user.user_id, &default_created.session.session_id)
            .await
            .expect("load default session")
            .expect("default session exists");
        assert_eq!(
            default_record.model_selection,
            Some(ModelSelection {
                qualified_id: "opencode-go/kimi-k2.6".to_string(),
            })
        );

        let axum::Json(request_created) = api_create_session_with_request(
            axum::extract::State(state.clone()),
            auth_headers(&token, Some(&auth_session.csrf_token)),
            axum::Json(ApiCreateSessionRequest {
                model_selection: Some(ModelSelection {
                    qualified_id: "glm-5".to_string(),
                }),
                agent_profile_selection: AgentProfileSelection::Default,
            }),
        )
        .await
        .expect("create request-selected session");
        let request_record = state
            .web_store
            .load_session(user.user_id, &request_created.session.session_id)
            .await
            .expect("load request session")
            .expect("request session exists");
        assert_eq!(
            request_record.model_selection,
            Some(ModelSelection {
                qualified_id: "opencode-go/glm-5".to_string(),
            })
        );
    }

    #[tokio::test]
    async fn api_agent_profile_default_and_session_selection_persist() {
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
        let (user, auth_session, token) = login_user(
            state.web_store.as_ref(),
            LoginRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            now,
        )
        .await
        .expect("login user");

        let axum::Json(created_profile) = api_create_agent_profile(
            axum::extract::State(state.clone()),
            auth_headers(&token, Some(&auth_session.csrf_token)),
            axum::Json(CreateAgentProfileRequest {
                display_name: "Reviewer".to_string(),
                system_prompt: "Focus on review notes.".to_string(),
            }),
        )
        .await
        .expect("create agent profile");
        assert_eq!(created_profile.profile.display_name, "Reviewer");

        let _ = api_update_settings(
            axum::extract::State(state.clone()),
            auth_headers(&token, Some(&auth_session.csrf_token)),
            axum::Json(UpdateUserSettingsRequest {
                default_model_selection: None,
                default_agent_profile_id: Some(created_profile.profile.agent_id.clone()),
            }),
        )
        .await
        .expect("save default profile");

        let axum::Json(default_created) = api_create_session(
            axum::extract::State(state.clone()),
            auth_headers(&token, Some(&auth_session.csrf_token)),
        )
        .await
        .expect("create default-profile session");
        let default_record = state
            .web_store
            .load_session(user.user_id, &default_created.session.session_id)
            .await
            .expect("load default-profile session")
            .expect("session exists");
        assert_eq!(
            default_record.agent_profile_id,
            Some(created_profile.profile.agent_id.clone())
        );

        let axum::Json(explicit_created) = api_create_session_with_request(
            axum::extract::State(state.clone()),
            auth_headers(&token, Some(&auth_session.csrf_token)),
            axum::Json(ApiCreateSessionRequest {
                model_selection: None,
                agent_profile_selection: AgentProfileSelection::None,
            }),
        )
        .await
        .expect("create no-profile session");
        let explicit_record = state
            .web_store
            .load_session(user.user_id, &explicit_created.session.session_id)
            .await
            .expect("load no-profile session")
            .expect("session exists");
        assert_eq!(explicit_record.agent_profile_id, None);

        let axum::Json(updated_session) = api_update_session_profile(
            axum::extract::State(state.clone()),
            auth_headers(&token, Some(&auth_session.csrf_token)),
            axum::extract::Path(explicit_created.session.session_id.clone()),
            axum::Json(UpdateSessionProfileRequest {
                agent_profile_id: Some(created_profile.profile.agent_id.clone()),
            }),
        )
        .await
        .expect("select profile for existing session");
        assert_eq!(
            updated_session.session.agent_profile_id,
            Some(created_profile.profile.agent_id.clone())
        );
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
        let (state, sandbox_control) =
            test_app_state_with_responses(vec![ScriptedResponse::Text("ok".to_string())]);
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
        assert_eq!(sandbox_control.ensured_scopes().len(), 1);
        assert_eq!(
            sandbox_control.ensured_scopes()[0].namespace(),
            record.context_key,
            "web session sandbox should be scoped per session context"
        );

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
    async fn api_download_task_file_serves_owned_file_and_supports_inline_preview() {
        use tower::Service as _;

        let state = test_app_state();
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
        let task_id = "task-download".to_string();
        state
            .web_store
            .save_task(task_record(
                user.user_id,
                &session_id,
                &task_id,
                ApiTaskStatus::Completed,
                "Deliver a file",
                now,
            ))
            .await
            .expect("save task");

        let file_id = "file-1".to_string();
        state
            .web_store
            .save_task_file(
                WebTaskFileRecord {
                    schema_version: WEB_TASK_FILE_SCHEMA_VERSION,
                    user_id: user.user_id,
                    session_id: session_id.clone(),
                    task_id: task_id.clone(),
                    file_id: file_id.clone(),
                    file_name: "report\n2026.pdf".to_string(),
                    content_type: "application/pdf".to_string(),
                    size_bytes: 7,
                    delivery_kind: FileDeliveryKind::Document,
                    created_at: now,
                },
                b"pdf-ish".to_vec(),
            )
            .await
            .expect("save task file");

        let mut app = super::build_router(state.clone());
        let response = app
            .call(
                axum::http::Request::builder()
                    .method(axum::http::Method::GET)
                    .uri(format!(
                        "/api/v1/sessions/{session_id}/tasks/{task_id}/files/{file_id}"
                    ))
                    .header(
                        axum::http::header::COOKIE,
                        format!("{AUTH_COOKIE_NAME}={token}"),
                    )
                    .body(axum::body::Body::empty())
                    .expect("download request"),
            )
            .await
            .expect("download response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert_eq!(
            response.headers()[axum::http::header::CACHE_CONTROL],
            axum::http::HeaderValue::from_static("private, no-store")
        );
        assert_eq!(
            response.headers()[axum::http::header::CONTENT_TYPE],
            axum::http::HeaderValue::from_static("application/pdf")
        );
        assert_eq!(
            response.headers()[axum::http::header::CONTENT_LENGTH],
            axum::http::HeaderValue::from_static("7")
        );
        assert_eq!(
            response.headers()[axum::http::header::CONTENT_DISPOSITION],
            axum::http::HeaderValue::from_static("attachment; filename=\"report_2026.pdf\"")
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read download body");
        assert_eq!(body.as_ref(), b"pdf-ish");

        let response = app
            .call(
                axum::http::Request::builder()
                    .method(axum::http::Method::GET)
                    .uri(format!(
                        "/api/v1/sessions/{session_id}/tasks/{task_id}/files/{file_id}?disposition=inline"
                    ))
                    .header(
                        axum::http::header::COOKIE,
                        format!("{AUTH_COOKIE_NAME}={token}"),
                    )
                    .body(axum::body::Body::empty())
                    .expect("inline preview request"),
            )
            .await
            .expect("inline preview response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert_eq!(
            response.headers()[axum::http::header::CONTENT_DISPOSITION],
            axum::http::HeaderValue::from_static("inline; filename=\"report_2026.pdf\"")
        );
    }

    #[tokio::test]
    async fn api_download_task_file_hides_foreign_or_missing_files() {
        use tower::Service as _;

        let state = test_app_state();
        let now = chrono::Utc::now();
        let owner = register_user(
            state.web_store.as_ref(),
            RegisterRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            true,
            now,
        )
        .await
        .expect("register owner");
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
        let (_, owner_session, owner_token) = login_user(
            state.web_store.as_ref(),
            LoginRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            now,
        )
        .await
        .expect("login owner");
        let (_, _, foreign_token) = login_user(
            state.web_store.as_ref(),
            LoginRequest {
                login: "bob".to_string(),
                password: "another correct horse battery staple".to_string(),
            },
            now,
        )
        .await
        .expect("login foreign user");

        let axum::Json(created_session) = api_create_session(
            axum::extract::State(state.clone()),
            auth_headers(&owner_token, Some(&owner_session.csrf_token)),
        )
        .await
        .expect("create session");
        let session_id = created_session.session.session_id;
        let task_id = "task-download".to_string();
        state
            .web_store
            .save_task(task_record(
                owner.user_id,
                &session_id,
                &task_id,
                ApiTaskStatus::Completed,
                "Deliver a file",
                now,
            ))
            .await
            .expect("save task");
        state
            .web_store
            .save_task_file(
                WebTaskFileRecord {
                    schema_version: WEB_TASK_FILE_SCHEMA_VERSION,
                    user_id: owner.user_id,
                    session_id: session_id.clone(),
                    task_id: task_id.clone(),
                    file_id: "file-1".to_string(),
                    file_name: "report.pdf".to_string(),
                    content_type: "application/pdf".to_string(),
                    size_bytes: 7,
                    delivery_kind: FileDeliveryKind::Document,
                    created_at: now,
                },
                b"pdf-ish".to_vec(),
            )
            .await
            .expect("save task file");

        let mut app = super::build_router(state);
        let foreign_response = app
            .call(
                axum::http::Request::builder()
                    .method(axum::http::Method::GET)
                    .uri(format!(
                        "/api/v1/sessions/{session_id}/tasks/{task_id}/files/file-1"
                    ))
                    .header(
                        axum::http::header::COOKIE,
                        format!("{AUTH_COOKIE_NAME}={foreign_token}"),
                    )
                    .body(axum::body::Body::empty())
                    .expect("foreign request"),
            )
            .await
            .expect("foreign response");
        assert_eq!(foreign_response.status(), axum::http::StatusCode::NOT_FOUND);

        let missing_response = app
            .call(
                axum::http::Request::builder()
                    .method(axum::http::Method::GET)
                    .uri(format!(
                        "/api/v1/sessions/{session_id}/tasks/{task_id}/files/missing"
                    ))
                    .header(
                        axum::http::header::COOKIE,
                        format!("{AUTH_COOKIE_NAME}={owner_token}"),
                    )
                    .body(axum::body::Body::empty())
                    .expect("missing file request"),
            )
            .await
            .expect("missing file response");
        assert_eq!(missing_response.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_create_session_prunes_orphan_web_sandboxes() {
        let (mut state, _) =
            test_app_state_with_responses(vec![ScriptedResponse::Text("ok".to_string())]);
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
        let sandbox_control = FakeSandboxControl::with_sandboxes(vec![
            fake_sandbox_record(SandboxScope::new(user.user_id, "web")),
            fake_sandbox_record(SandboxScope::new(user.user_id, "web-session-orphan")),
            fake_sandbox_record(SandboxScope::new(user.user_id, "topic-live")),
        ]);
        state.set_sandbox_control(Arc::new(sandbox_control.clone()));

        let axum::Json(created) = api_create_session(
            axum::extract::State(state.clone()),
            auth_headers(&token, Some(&auth_session.csrf_token)),
        )
        .await
        .expect("create session");

        let deleted_names = sandbox_control.deleted_names();
        assert!(deleted_names
            .iter()
            .any(|name| name == &SandboxScope::new(user.user_id, "web").container_name()));
        assert!(deleted_names.iter().any(|name| {
            name == &SandboxScope::new(user.user_id, "web-session-orphan").container_name()
        }));
        assert!(!deleted_names.iter().any(|name| {
            name == &SandboxScope::new(
                user.user_id,
                format!("web-session-{}", created.session.session_id),
            )
            .container_name()
        }));
    }

    #[tokio::test]
    async fn api_delete_session_destroys_web_sandbox_and_clears_flow_memory() {
        let (mut state, _) =
            test_app_state_with_responses(vec![ScriptedResponse::Text("ok".to_string())]);
        let sandbox_control = FakeSandboxControl::default();
        state.set_sandbox_control(Arc::new(sandbox_control.clone()));
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

        let axum::Json(created) = api_create_session(
            axum::extract::State(state.clone()),
            auth_headers(&token, Some(&auth_session.csrf_token)),
        )
        .await
        .expect("create session");
        let record = state
            .web_store
            .load_session(user.user_id, &created.session.session_id)
            .await
            .expect("load session")
            .expect("session exists");
        let memory = AgentMemory::new(usize::MAX);
        state
            .session_manager
            .storage()
            .save_agent_memory_for_flow(
                user.user_id,
                record.context_key.clone(),
                record.agent_flow_id.clone(),
                &memory,
            )
            .await
            .expect("save flow memory");

        let axum::Json(response) = api_delete_session(
            axum::extract::State(state.clone()),
            auth_headers(&token, Some(&auth_session.csrf_token)),
            axum::extract::Path(created.session.session_id.clone()),
        )
        .await
        .expect("delete session");

        assert!(response.ok);
        assert!(state
            .web_store
            .load_session(user.user_id, &created.session.session_id)
            .await
            .expect("load deleted session")
            .is_none());
        assert!(state
            .session_manager
            .storage()
            .load_agent_memory_for_flow(
                user.user_id,
                record.context_key.clone(),
                record.agent_flow_id.clone(),
            )
            .await
            .expect("load flow memory")
            .is_none());
        assert_eq!(sandbox_control.destroyed_scopes().len(), 1);
        assert_eq!(
            sandbox_control.destroyed_scopes()[0].namespace(),
            record.context_key,
            "delete session should destroy the per-session sandbox"
        );
    }

    #[tokio::test]
    async fn api_create_task_version_and_cancel_task_are_auth_scoped_and_status_checked() {
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

        let axum::Json(versioned) = api_create_task_version(
            axum::extract::State(state.clone()),
            auth_headers(&token_one, Some(&session_one.csrf_token)),
            axum::extract::Path((session_id.clone(), "task-completed".to_string())),
            axum::Json(ApiCreateTaskVersionRequest {
                input_markdown: "Edited prompt".to_string(),
                attachments: Vec::new(),
            }),
        )
        .await
        .expect("create task version");
        assert_eq!(versioned.task.input_markdown, "Edited prompt");
        assert!(versioned.task.input_edited_at.is_some());
        assert_eq!(versioned.task.version_group_id, "task-completed");
        assert_eq!(versioned.task.version_index, 2);
        assert_eq!(
            versioned.task.parent_task_id.as_deref(),
            Some("task-completed")
        );
        assert_ne!(versioned.task.task_id, "task-completed");

        let original = state
            .web_store
            .load_task(user_one.user_id, &session_id, "task-completed")
            .await
            .expect("load original task")
            .expect("original task exists");
        assert_eq!(original.input_markdown, "Original prompt");
        assert!(original.input_edited_at.is_none());

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

        let edit_running = api_create_task_version(
            axum::extract::State(state.clone()),
            auth_headers(&token_one, Some(&session_one.csrf_token)),
            axum::extract::Path((session_id.clone(), "task-running".to_string())),
            axum::Json(ApiCreateTaskVersionRequest {
                input_markdown: "Should fail".to_string(),
                attachments: Vec::new(),
            }),
        )
        .await;
        let (status, axum::Json(error)) = edit_running.expect_err("running edit should fail");
        assert_eq!(status, axum::http::StatusCode::CONFLICT);
        assert_eq!(error.error.code, ErrorCode::TaskActive);

        let edit_non_latest = api_create_task_version(
            axum::extract::State(state.clone()),
            auth_headers(&token_one, Some(&session_one.csrf_token)),
            axum::extract::Path((session_id.clone(), "task-completed".to_string())),
            axum::Json(ApiCreateTaskVersionRequest {
                input_markdown: "Should also fail".to_string(),
                attachments: Vec::new(),
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
                attachments: Vec::new(),
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
                attachments: Vec::new(),
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
                attachments: Vec::new(),
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
        ])
        .0;
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
                attachments: Vec::new(),
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

        let resume_attachments = vec![TaskAttachment {
            file_name: "scope.txt".to_string(),
            mime_type: Some("text/plain".to_string()),
            size_bytes: 17,
            sandbox_path: "/workspace/uploads/scope.txt".to_string(),
        }];
        let axum::Json(resumed) = api_resume_task(
            axum::extract::State(state.clone()),
            auth_headers(&token, Some(&auth_session.csrf_token)),
            axum::extract::Path((session_id.clone(), task_id.clone())),
            axum::Json(ApiResumeTaskRequest {
                input_markdown: "Scope is GPT-5.4-mini".to_string(),
                attachments: resume_attachments.clone(),
            }),
        )
        .await
        .expect("resume waiting task");
        assert_eq!(resumed.task.task_id, task_id);
        assert_eq!(resumed.task.status, ApiTaskStatus::Running);

        let persisted_events = state
            .web_store
            .list_task_events(user.user_id, &session_id, &task_id, 0, 256)
            .await
            .expect("list task events")
            .events;
        let resume_event = persisted_events
            .iter()
            .find(|event| event.kind == TaskEventKind::UserMessage)
            .expect("resume user message event exists");
        let payload: UserMessageEventPayload =
            serde_json::from_value(resume_event.payload.clone()).expect("payload parses");
        assert_eq!(payload.input_markdown, "Scope is GPT-5.4-mini");
        assert_eq!(payload.attachments, resume_attachments);

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
        test_app_state_with_responses(vec![ScriptedResponse::Text("ok".to_string())]).0
    }

    fn test_app_state_with_responses(
        responses: Vec<ScriptedResponse>,
    ) -> (AppState, FakeSandboxControl) {
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
        let sandbox_control = FakeSandboxControl::default();
        state.set_sandbox_control(Arc::new(sandbox_control.clone()));
        state.auto_title_enabled = false;
        (state, sandbox_control)
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

    #[test]
    fn task_preview_source_falls_back_to_attachment_names() {
        let attachments = vec![TaskAttachment {
            file_name: "report.csv".to_string(),
            mime_type: Some("text/csv".to_string()),
            size_bytes: 42,
            sandbox_path: "/workspace/uploads/demo-report.csv".to_string(),
        }];

        assert_eq!(
            super::task_preview_source("   ", &attachments),
            "Attachment: report.csv"
        );
    }

    #[test]
    fn build_task_execution_input_embeds_attachment_paths() {
        let attachments = vec![TaskAttachment {
            file_name: "report.csv".to_string(),
            mime_type: Some("text/csv".to_string()),
            size_bytes: 42,
            sandbox_path: "/workspace/uploads/demo-report.csv".to_string(),
        }];

        let execution_input = super::build_task_execution_input("Analyze this", &attachments);

        assert!(execution_input.contains("Analyze this"));
        assert!(execution_input.contains("report.csv"));
        assert!(execution_input.contains("/workspace/uploads/demo-report.csv"));
        assert!(execution_input.contains("sandbox-local"));
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
            version_group_id: task_id.to_string(),
            version_index: 1,
            parent_task_id: None,
            status,
            input_markdown: input_markdown.to_string(),
            attachments: Vec::new(),
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
