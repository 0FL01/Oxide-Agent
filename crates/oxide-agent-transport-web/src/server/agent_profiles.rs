use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use oxide_agent_core::agent::{parse_agent_profile, AgentExecutionProfile, ToolAccessPolicy};
use oxide_agent_core::storage::{AgentProfileRecord, StorageError, UpsertAgentProfileOptions};
use oxide_agent_web_contracts::{
    AgentProfileSelection, AgentProfileView, CreateAgentProfileRequest, CreateAgentProfileResponse,
    ErrorCode, ErrorEnvelope, ListAgentProfilesResponse, OkResponse, UpdateAgentProfileRequest,
    UpdateAgentProfileResponse,
};

use crate::persistence::WebUserRecord;

use super::settings_routes::load_current_user_record;
use super::{
    api_error, authenticated_user, authenticated_user_with_csrf, backend_unavailable_response,
    not_found_response, store_error_response, AppState,
};

const MAX_AGENT_PROFILE_ID_CHARS: usize = 64;
const MAX_AGENT_PROFILE_NAME_CHARS: usize = 80;
const MAX_AGENT_PROFILE_PROMPT_CHARS: usize = 32_000;

pub(crate) async fn api_list_agent_profiles(
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

pub(crate) async fn api_create_agent_profile(
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

pub(crate) async fn api_update_agent_profile(
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

pub(crate) async fn api_delete_agent_profile(
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

pub(crate) async fn validate_optional_agent_profile_id(
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

pub(crate) async fn resolve_session_agent_profile_id(
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

pub(crate) async fn load_execution_profile_for_agent_profile_id(
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
