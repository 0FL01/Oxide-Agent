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

mod agent_profiles;
mod auth_helpers;
mod auth_routes;
mod auto_title;
mod converters;
mod model_routes;
mod router;
mod search_probe;
mod session_routes;
mod settings_routes;
mod sse;
mod static_assets;
mod task_executor;
mod task_routes;
mod types;

pub(crate) use agent_profiles::{
    api_create_agent_profile, api_delete_agent_profile, api_list_agent_profiles,
    api_update_agent_profile, load_execution_profile_for_agent_profile_id,
    resolve_session_agent_profile_id, validate_optional_agent_profile_id,
};
use auth_helpers::*;
pub(crate) use auth_routes::{
    api_bootstrap, api_change_password, api_login, api_logout, api_me, api_register,
};
use converters::*;
pub(crate) use model_routes::{
    api_list_model_routes, api_refresh_model_routes, canonical_model_selection,
    default_session_model_selection,
};
pub use router::{build_router, serve};
#[cfg(test)]
pub(crate) use session_routes::api_create_session;
pub(crate) use session_routes::{
    api_create_session_with_request, api_delete_session, api_get_session, api_list_sessions,
    api_update_session, api_update_session_profile, api_upload_large_input,
    api_upload_task_attachments, invalidate_session_summaries_cache,
};
pub(crate) use settings_routes::{api_get_settings, api_update_settings, load_current_user_record};
pub(crate) use task_routes::{
    api_cancel_task, api_create_task, api_create_task_version, api_download_task_file,
    api_get_task, api_get_task_events, api_get_task_progress, api_list_tasks, api_resume_task,
};
#[cfg(test)]
pub(crate) use task_routes::{
    build_task_agent_user_input, build_task_execution_input, task_preview_source,
};
pub use types::*;

use axum::{Json, extract::State, http::StatusCode};
use oxide_agent_web_contracts::{ErrorCode, ErrorEnvelope, PublicConfigResponse};

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
        max_task_input_chars: MAX_TASK_INPUT_CHARS,
        large_input_attachments_supported: state.large_input_attachments_supported(),
    })
}

fn backend_unavailable_response(message: impl Into<String>) -> (StatusCode, Json<ErrorEnvelope>) {
    api_error(
        StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::BackendUnavailable,
        message.into(),
        true,
    )
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
