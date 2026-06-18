//! Native browser sidecar library — shared between the binary and tests.
//!
//! Replaces `chrome-agent-sidecar.py` + external `chrome-agent` subprocess
//! with a single Rust binary speaking CDP directly.

pub mod browser;
pub mod cdp;
pub mod session;

use axum::{
    Router,
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Json, Response},
    routing::{delete, get, post},
};
use oxide_browser_contracts::{
    CloseSessionRequest, CloseSessionResponse, CreateSessionRequest, CreateSessionResponse,
};
use serde_json::json;
use std::sync::Arc;

use session::SessionManager;

/// Shared state passed to all route handlers.
#[derive(Clone)]
pub struct AppState {
    pub sessions: Arc<SessionManager>,
}

/// Build the axum router with all routes (healthz + authed session routes).
pub fn create_app(state: AppState, token: String) -> Router {
    let authed_routes = Router::new()
        .route("/sessions", post(create_session))
        .route("/sessions/:id", delete(close_session))
        .route("/sessions/:id/goto", post(not_implemented))
        .route("/sessions/:id/observe", get(not_implemented))
        .route("/sessions/:id/action", post(not_implemented))
        .route("/sessions/:id/screenshot/latest", get(not_implemented))
        .route("/sessions/:id/debug/network", get(not_implemented))
        .route("/sessions/:id/debug/console", get(not_implemented))
        .route_layer(axum::middleware::from_fn_with_state(token, bearer_auth))
        .with_state(state);

    Router::new()
        .route("/healthz", get(healthz))
        .merge(authed_routes)
}

async fn healthz() -> Json<serde_json::Value> {
    Json(json!({"ok": true, "native": true}))
}

async fn create_session(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> Json<CreateSessionResponse> {
    let resp = state.sessions.create(req).await;
    Json(resp)
}

async fn close_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<CloseSessionRequest>,
) -> Json<CloseSessionResponse> {
    let resp = state.sessions.close(&id, req).await;
    Json(resp)
}

async fn not_implemented() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(
            json!({"ok": false, "code": "not_implemented", "message": "endpoint not yet implemented"}),
        ),
    )
}

async fn bearer_auth(
    axum::extract::State(expected_token): axum::extract::State<String>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Response {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if auth == format!("Bearer {expected_token}") {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({"ok": false, "code": "unauthorized", "message": "invalid or missing bearer token"})),
        )
            .into_response()
    }
}
