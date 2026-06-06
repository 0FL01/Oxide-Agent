//! Authentication, CSRF protection, rate limiting, and cookie helpers.

use super::{api_error, AppState, CachedAuthSession, AUTH_COOKIE_NAME, CSRF_HEADER_NAME};
use crate::auth::{current_user_for_token, hash_session_token, AuthError};
use axum::{
    http::{
        header::{COOKIE, HOST, ORIGIN, REFERER},
        HeaderMap, HeaderValue, StatusCode,
    },
    Json,
};
use oxide_agent_web_contracts::{
    CurrentUser, ErrorCode, ErrorEnvelope, WebSessionRecord, WebTaskRecord,
};
use std::time::Instant;

// ---------------------------------------------------------------------------
// Auth error mapping
// ---------------------------------------------------------------------------

pub(crate) fn auth_error_response(error: AuthError) -> (StatusCode, Json<ErrorEnvelope>) {
    match error {
        AuthError::Validation(message) => api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationError,
            message,
            false,
        ),
        AuthError::Unauthorized => api_error(
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthorized,
            "Unauthorized.",
            false,
        ),
        AuthError::InvalidCredentials => api_error(
            StatusCode::UNAUTHORIZED,
            ErrorCode::InvalidCredentials,
            "Invalid credentials.",
            false,
        ),
        AuthError::CsrfInvalid => api_error(
            StatusCode::FORBIDDEN,
            ErrorCode::CsrfInvalid,
            "Invalid CSRF token.",
            false,
        ),
        AuthError::RegistrationDisabled => api_error(
            StatusCode::FORBIDDEN,
            ErrorCode::RegistrationDisabled,
            "Registration is disabled.",
            false,
        ),
        AuthError::BootstrapUnavailable => api_error(
            StatusCode::NOT_FOUND,
            ErrorCode::BootstrapUnavailable,
            "Bootstrap is not available.",
            false,
        ),
        AuthError::Conflict(message) => {
            api_error(StatusCode::CONFLICT, ErrorCode::Conflict, message, false)
        }
        AuthError::StoreUnavailable(message) => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::BackendUnavailable,
            message,
            true,
        ),
    }
}

// ---------------------------------------------------------------------------
// Rate limiting
// ---------------------------------------------------------------------------

pub(crate) fn auth_rate_limit_key(headers: &HeaderMap, login: &str) -> String {
    let client_key = auth_client_key(headers);
    let login_key = login.trim().to_ascii_lowercase();
    format!("{client_key}:{login_key}")
}

fn auth_client_key(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .unwrap_or("unknown")
        .to_string()
}

pub(crate) async fn reject_auth_rate_limited(
    state: &AppState,
    key: &str,
) -> Result<(), (StatusCode, Json<ErrorEnvelope>)> {
    if state
        .auth_rate_limiter
        .lock()
        .await
        .is_limited(key, Instant::now())
    {
        return Err(api_error(
            StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::RateLimited,
            "Too many authentication attempts. Try again later.",
            true,
        ));
    }
    Ok(())
}

pub(crate) async fn record_auth_failure(state: &AppState, key: String) {
    state
        .auth_rate_limiter
        .lock()
        .await
        .record_failure(key, Instant::now());
}

pub(crate) async fn clear_auth_rate_limit(state: &AppState, key: &str) {
    state.auth_rate_limiter.lock().await.clear(key);
}

// ---------------------------------------------------------------------------
// CSRF origin validation
// ---------------------------------------------------------------------------

pub(crate) fn validate_csrf_request_origin(
    headers: &HeaderMap,
) -> Result<(), (StatusCode, Json<ErrorEnvelope>)> {
    let Some(supplied_origin) = csrf_supplied_origin(headers) else {
        return Ok(());
    };
    let Some(expected_origin) = csrf_expected_origin(headers) else {
        return Err(csrf_origin_error());
    };
    if supplied_origin.eq_ignore_ascii_case(&expected_origin) {
        return Ok(());
    }
    Err(csrf_origin_error())
}

fn csrf_supplied_origin(headers: &HeaderMap) -> Option<String> {
    headers
        .get(ORIGIN)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(trim_trailing_slash)
        .or_else(|| {
            headers
                .get(REFERER)
                .and_then(|value| value.to_str().ok())
                .and_then(origin_from_url)
        })
}

fn csrf_expected_origin(headers: &HeaderMap) -> Option<String> {
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(HOST))?
        .to_str()
        .ok()?
        .split(',')
        .next()?
        .trim();
    if host.is_empty() {
        return None;
    }
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            if super::is_production_run_mode() {
                "https"
            } else {
                "http"
            }
        });
    Some(format!("{proto}://{host}"))
}

fn origin_from_url(value: &str) -> Option<String> {
    let value = value.trim();
    let scheme_end = value.find("://")?;
    let after_scheme = scheme_end + 3;
    let host_end = value[after_scheme..]
        .find('/')
        .map_or(value.len(), |index| after_scheme + index);
    (host_end > after_scheme).then(|| trim_trailing_slash(&value[..host_end]))
}

fn trim_trailing_slash(value: &str) -> String {
    value.trim_end_matches('/').to_string()
}

fn csrf_origin_error() -> (StatusCode, Json<ErrorEnvelope>) {
    api_error(
        StatusCode::FORBIDDEN,
        ErrorCode::CsrfInvalid,
        "Invalid request origin.",
        false,
    )
}

// ---------------------------------------------------------------------------
// Auth extraction helpers
// ---------------------------------------------------------------------------

pub(crate) async fn authenticated_user(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<CurrentUser, (StatusCode, Json<ErrorEnvelope>)> {
    let raw_session_token = auth_cookie_value(headers).map_err(auth_error_response)?;
    let (user, _) = current_user_for_token_cached(state, &raw_session_token, chrono::Utc::now())
        .await
        .map_err(auth_error_response)?;
    Ok(user)
}

pub(crate) async fn authenticated_user_with_csrf(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<CurrentUser, (StatusCode, Json<ErrorEnvelope>)> {
    validate_csrf_request_origin(headers)?;
    let raw_session_token = auth_cookie_value(headers).map_err(auth_error_response)?;
    let csrf_token = csrf_header_value(headers).map_err(auth_error_response)?;
    let (user, auth_session) =
        current_user_for_token_cached(state, &raw_session_token, chrono::Utc::now())
            .await
            .map_err(auth_error_response)?;
    if auth_session.csrf_token != csrf_token {
        return Err(auth_error_response(AuthError::CsrfInvalid));
    }
    Ok(user)
}

pub(crate) async fn current_user_for_token_cached(
    state: &AppState,
    raw_session_token: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(CurrentUser, crate::persistence::WebAuthSessionRecord), AuthError> {
    let session_token_hash = hash_session_token(raw_session_token);
    if let Some(entry) = state.auth_cache.get(&session_token_hash).await {
        if entry.auth_session.revoked_at.is_some() || entry.auth_session.expires_at <= now {
            state.auth_cache.invalidate(&session_token_hash).await;
            tracing::debug!(
                target: "oxide_agent_transport_web::web_perf",
                auth_cache_hit = true,
                auth_cache_valid = false,
                user_id = entry.user.user_id,
                cache_age_ms = (now - entry.cached_at).num_milliseconds(),
                "web auth cache checked"
            );
            return Err(AuthError::Unauthorized);
        }

        tracing::debug!(
            target: "oxide_agent_transport_web::web_perf",
            auth_cache_hit = true,
            auth_cache_valid = true,
            user_id = entry.user.user_id,
            cache_age_ms = (now - entry.cached_at).num_milliseconds(),
            "web auth cache checked"
        );
        return Ok((entry.user, entry.auth_session));
    }

    let (user, auth_session) =
        current_user_for_token(state.web_store.as_ref(), raw_session_token, now).await?;
    state
        .auth_cache
        .insert(
            session_token_hash,
            CachedAuthSession {
                user: user.clone(),
                auth_session: auth_session.clone(),
                cached_at: now,
            },
        )
        .await;
    tracing::debug!(
        target: "oxide_agent_transport_web::web_perf",
        auth_cache_hit = false,
        auth_cache_valid = true,
        user_id = user.user_id,
        "web auth cache checked"
    );
    Ok((user, auth_session))
}

pub(crate) async fn cache_auth_session(
    state: &AppState,
    raw_session_token: &str,
    user: CurrentUser,
    auth_session: crate::persistence::WebAuthSessionRecord,
    now: chrono::DateTime<chrono::Utc>,
) {
    state
        .auth_cache
        .insert(
            hash_session_token(raw_session_token),
            CachedAuthSession {
                user,
                auth_session,
                cached_at: now,
            },
        )
        .await;
}

pub(crate) async fn invalidate_auth_session_cache(state: &AppState, raw_session_token: &str) {
    state
        .auth_cache
        .invalidate(&hash_session_token(raw_session_token))
        .await;
}

pub(crate) async fn load_owned_session(
    state: &AppState,
    user_id: i64,
    session_id: &str,
) -> Result<WebSessionRecord, (StatusCode, Json<ErrorEnvelope>)> {
    state
        .web_store
        .load_session(user_id, session_id)
        .await
        .map_err(store_error_response)?
        .ok_or_else(not_found_response)
}

pub(crate) async fn load_owned_task(
    state: &AppState,
    user_id: i64,
    session_id: &str,
    task_id: &str,
) -> Result<WebTaskRecord, (StatusCode, Json<ErrorEnvelope>)> {
    let mut task = state
        .web_store
        .load_task(user_id, session_id, task_id)
        .await
        .map_err(store_error_response)?
        .ok_or_else(not_found_response)?;
    task.normalize_version_lineage();
    Ok(task)
}

// ---------------------------------------------------------------------------
// Cookie helpers
// ---------------------------------------------------------------------------

pub(crate) fn auth_cookie_value(headers: &HeaderMap) -> Result<String, AuthError> {
    let cookie_header = headers
        .get(COOKIE)
        .and_then(|value| value.to_str().ok())
        .ok_or(AuthError::Unauthorized)?;
    cookie_header
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find_map(|(name, value)| (name == AUTH_COOKIE_NAME).then(|| value.to_string()))
        .filter(|value| !value.is_empty())
        .ok_or(AuthError::Unauthorized)
}

pub(crate) fn csrf_header_value(headers: &HeaderMap) -> Result<String, AuthError> {
    headers
        .get(CSRF_HEADER_NAME)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .ok_or(AuthError::CsrfInvalid)
}

pub(crate) fn auth_cookie_header(
    raw_session_token: &str,
    max_age_secs: i64,
) -> Result<HeaderValue, (StatusCode, Json<ErrorEnvelope>)> {
    cookie_header(format!(
        "{AUTH_COOKIE_NAME}={raw_session_token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age_secs}{}",
        secure_cookie_suffix()
    ))
}

pub(crate) fn expired_auth_cookie_header() -> Result<HeaderValue, (StatusCode, Json<ErrorEnvelope>)>
{
    cookie_header(format!(
        "{AUTH_COOKIE_NAME}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT{}",
        secure_cookie_suffix()
    ))
}

fn cookie_header(value: String) -> Result<HeaderValue, (StatusCode, Json<ErrorEnvelope>)> {
    HeaderValue::from_str(&value).map_err(|_| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Internal,
            "Failed to build auth cookie.",
            false,
        )
    })
}

fn secure_cookie_suffix() -> &'static str {
    if super::web_bool_env("OXIDE_WEB_COOKIE_SECURE") || super::is_production_run_mode() {
        "; Secure"
    } else {
        ""
    }
}

// ---------------------------------------------------------------------------
// Store/error helpers
// ---------------------------------------------------------------------------

pub(crate) fn store_error_response(
    error: crate::persistence::WebUiStoreError,
) -> (StatusCode, Json<ErrorEnvelope>) {
    match error {
        crate::persistence::WebUiStoreError::Conflict(message) => {
            api_error(StatusCode::CONFLICT, ErrorCode::Conflict, message, false)
        }
        crate::persistence::WebUiStoreError::Unavailable(message) => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::BackendUnavailable,
            message,
            true,
        ),
    }
}

pub(crate) fn not_found_response() -> (StatusCode, Json<ErrorEnvelope>) {
    api_error(
        StatusCode::NOT_FOUND,
        ErrorCode::NotFound,
        "Resource not found.",
        false,
    )
}
