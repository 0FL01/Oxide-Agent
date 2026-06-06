use axum::{
    extract::State,
    http::{header::SET_COOKIE, HeaderMap, StatusCode},
    Json,
};
use oxide_agent_web_contracts::{
    AuthUserResponse, BootstrapRequest, ChangePasswordRequest, CurrentUser, CurrentUserResponse,
    ErrorEnvelope, LoginRequest, OkResponse, RegisterRequest,
};

use super::{
    auth_cookie_header, auth_cookie_value, auth_error_response, auth_rate_limit_key,
    cache_auth_session, clear_auth_rate_limit, csrf_header_value, current_user_for_token_cached,
    expired_auth_cookie_header, invalidate_auth_session_cache, record_auth_failure,
    reject_auth_rate_limited, validate_csrf_request_origin, web_bool_env, web_env_value, AppState,
};
use crate::auth::{
    bootstrap_user, change_password, create_auth_session_for_user, login_user, logout_session,
    register_user, AuthError, AUTH_SESSION_TTL_SECS,
};

pub(crate) async fn api_register(
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
    let user = rate_limited_auth_result(&state, rate_limit_key, result).await?;
    auth_session_response(&state, user, chrono::Utc::now()).await
}

pub(crate) async fn api_bootstrap(
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
    let user = rate_limited_auth_result(&state, rate_limit_key, result).await?;
    auth_session_response(&state, user, chrono::Utc::now()).await
}

pub(crate) async fn api_login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<LoginRequest>,
) -> Result<(HeaderMap, Json<AuthUserResponse>), (StatusCode, Json<ErrorEnvelope>)> {
    let rate_limit_key = auth_rate_limit_key(&headers, &request.login);
    reject_auth_rate_limited(&state, &rate_limit_key).await?;
    let result = login_user(state.web_store.as_ref(), request, chrono::Utc::now()).await;
    let (user, auth_session, raw_session_token) =
        rate_limited_auth_result(&state, rate_limit_key, result).await?;
    cache_auth_session(
        &state,
        &raw_session_token,
        user.clone(),
        auth_session.clone(),
        chrono::Utc::now(),
    )
    .await;
    auth_session_response_with_token(user, auth_session.csrf_token, &raw_session_token)
}

async fn rate_limited_auth_result<T>(
    state: &AppState,
    rate_limit_key: String,
    result: Result<T, AuthError>,
) -> Result<T, (StatusCode, Json<ErrorEnvelope>)> {
    match result {
        Ok(value) => {
            clear_auth_rate_limit(state, &rate_limit_key).await;
            Ok(value)
        }
        Err(error) => {
            record_auth_failure(state, rate_limit_key).await;
            Err(auth_error_response(error))
        }
    }
}

fn auth_session_response_with_token(
    user: CurrentUser,
    csrf_token: String,
    raw_session_token: &str,
) -> Result<(HeaderMap, Json<AuthUserResponse>), (StatusCode, Json<ErrorEnvelope>)> {
    let mut headers = HeaderMap::new();
    headers.insert(
        SET_COOKIE,
        auth_cookie_header(raw_session_token, AUTH_SESSION_TTL_SECS)?,
    );
    Ok((
        headers,
        Json(AuthUserResponse {
            user,
            csrf_token: Some(csrf_token),
        }),
    ))
}

async fn auth_session_response(
    state: &AppState,
    user: CurrentUser,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(HeaderMap, Json<AuthUserResponse>), (StatusCode, Json<ErrorEnvelope>)> {
    let (auth_session, raw_session_token) =
        create_auth_session_for_user(state.web_store.as_ref(), user.user_id, now)
            .await
            .map_err(auth_error_response)?;
    cache_auth_session(
        state,
        &raw_session_token,
        user.clone(),
        auth_session.clone(),
        now,
    )
    .await;
    auth_session_response_with_token(user, auth_session.csrf_token, &raw_session_token)
}

pub(crate) async fn api_me(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<CurrentUserResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let raw_session_token = auth_cookie_value(&headers).map_err(auth_error_response)?;
    let (user, auth_session) =
        current_user_for_token_cached(&state, &raw_session_token, chrono::Utc::now())
            .await
            .map_err(auth_error_response)?;
    Ok(Json(CurrentUserResponse {
        user,
        csrf_token: auth_session.csrf_token,
    }))
}

pub(crate) async fn api_logout(
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
    invalidate_auth_session_cache(&state, &raw_session_token).await;

    let mut response_headers = HeaderMap::new();
    response_headers.insert(SET_COOKIE, expired_auth_cookie_header()?);
    Ok((response_headers, Json(OkResponse::ok())))
}

pub(crate) async fn api_change_password(
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
    state.auth_cache.invalidate_all();
    Ok(Json(OkResponse::ok()))
}
