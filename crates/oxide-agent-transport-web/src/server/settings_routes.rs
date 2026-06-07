use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use oxide_agent_web_contracts::{
    ErrorCode, ErrorEnvelope, UpdateUserSettingsRequest, UserSettingsResponse,
};
use std::sync::Arc;

use crate::persistence::WebUserRecord;

use super::agent_profiles::validate_optional_agent_profile_id;
use super::model_routes::canonical_model_selection;
use super::{
    api_error, authenticated_user, authenticated_user_with_csrf, store_error_response, AppState,
};

#[derive(Debug)]
pub(crate) enum UserSettingsCacheLoadError {
    Unauthorized,
    Store(String),
}

fn user_settings_cache_error_response(
    error: Arc<UserSettingsCacheLoadError>,
) -> (StatusCode, Json<ErrorEnvelope>) {
    match error.as_ref() {
        UserSettingsCacheLoadError::Unauthorized => api_error(
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthorized,
            "Authenticated web user no longer exists.",
            false,
        ),
        UserSettingsCacheLoadError::Store(message) => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::BackendUnavailable,
            format!("Web user settings are unavailable: {message}"),
            true,
        ),
    }
}

pub(crate) async fn api_get_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<UserSettingsResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user(&state, &headers).await?;
    if let Some(response) = state.user_settings_cache.get(&user.user_id).await {
        tracing::debug!(
            target: "oxide_agent_transport_web::web_perf",
            user_id = user.user_id,
            settings_cache_hit = true,
            "web settings cache checked"
        );
        return Ok(Json(response));
    }

    let response = cached_user_settings(&state, user.user_id)
        .await
        .map_err(user_settings_cache_error_response)?;
    tracing::debug!(
        target: "oxide_agent_transport_web::web_perf",
        user_id = user.user_id,
        settings_cache_hit = false,
        "web settings cache checked"
    );
    Ok(Json(response))
}

pub(crate) async fn prewarm_user_settings_cache(state: AppState, user_id: i64) {
    if let Err(error) = cached_user_settings(&state, user_id).await {
        tracing::debug!(
            target: "oxide_agent_transport_web::web_perf",
            user_id,
            error = ?error,
            "web settings cache prewarm failed"
        );
    }
}

async fn cached_user_settings(
    state: &AppState,
    user_id: i64,
) -> Result<UserSettingsResponse, Arc<UserSettingsCacheLoadError>> {
    let state = state.clone();
    let cache = state.user_settings_cache.clone();
    cache
        .try_get_with(user_id, async move {
            let record = load_user_record_for_cache(&state, user_id).await?;
            Ok(user_settings_response_from_record(&record))
        })
        .await
}

async fn load_user_record_for_cache(
    state: &AppState,
    user_id: i64,
) -> Result<WebUserRecord, UserSettingsCacheLoadError> {
    state
        .web_store
        .load_user(user_id)
        .await
        .map_err(|error| UserSettingsCacheLoadError::Store(error.to_string()))?
        .ok_or(UserSettingsCacheLoadError::Unauthorized)
}

pub(crate) async fn api_update_settings(
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
    let response = user_settings_response_from_record(&record);
    state
        .user_settings_cache
        .insert(user.user_id, response.clone())
        .await;
    Ok(Json(response))
}

fn user_settings_response_from_record(record: &WebUserRecord) -> UserSettingsResponse {
    UserSettingsResponse {
        default_model_selection: record.default_model_selection.clone(),
        default_agent_profile_id: record.default_agent_profile_id.clone(),
        default_effort: record.default_effort,
    }
}

pub(crate) async fn load_current_user_record(
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
