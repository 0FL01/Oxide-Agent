use axum::{
    body::Body,
    http::{header::CONTENT_SECURITY_POLICY, HeaderValue, Request},
    middleware::{self, Next},
    response::Response,
    routing::{delete, get, patch, post},
    Router,
};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

use super::types::is_production_run_mode;
use super::{
    api_bootstrap, api_cancel_task, api_change_password, api_create_agent_profile,
    api_create_session_with_request, api_create_task, api_create_task_version,
    api_delete_agent_profile, api_delete_session, api_download_task_file, api_get_session,
    api_get_settings, api_get_task, api_get_task_events, api_get_task_progress,
    api_list_agent_profiles, api_list_model_routes, api_list_sessions, api_list_tasks, api_login,
    api_logout, api_me, api_public_config, api_refresh_model_routes, api_register, api_resume_task,
    api_update_agent_profile, api_update_session, api_update_session_profile, api_update_settings,
    api_upload_task_attachments, health, sse, static_assets, AppState,
};

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
