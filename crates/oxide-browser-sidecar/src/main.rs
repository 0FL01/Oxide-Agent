//! Native browser sidecar — replaces `chrome-agent-sidecar.py` + external
//! `chrome-agent` subprocess with a single Rust binary speaking CDP directly.
//!
//! CP1 scaffold: axum server with `/healthz` and bearer-token auth middleware.
//! Session, CDP, action, and observation routes are added in later checkpoints.

use anyhow::Context;
use axum::{
    Router,
    extract::Request,
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Json, Response},
    routing::get,
};
use serde_json::json;
use std::env;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let token = env::var("BROWSER_AGENT_SIDECAR_TOKEN")
        .context("BROWSER_AGENT_SIDECAR_TOKEN is required")?;

    let bind =
        env::var("BROWSER_AGENT_SIDECAR_BIND").unwrap_or_else(|_| "0.0.0.0:8787".to_string());

    // Healthz is exempt from auth (matches Python sidecar behavior).
    let authed_routes = Router::new()
        .route("/sessions", axum::routing::post(not_implemented))
        .route("/sessions/:id", axum::routing::delete(not_implemented))
        .route("/sessions/:id/goto", axum::routing::post(not_implemented))
        .route("/sessions/:id/observe", get(not_implemented))
        .route("/sessions/:id/action", axum::routing::post(not_implemented))
        .route("/sessions/:id/screenshot/latest", get(not_implemented))
        .route("/sessions/:id/debug/network", get(not_implemented))
        .route("/sessions/:id/debug/console", get(not_implemented))
        .route_layer(axum::middleware::from_fn_with_state(token, bearer_auth));

    let app = Router::new()
        .route("/healthz", get(healthz))
        .merge(authed_routes);

    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("bind {bind}"))?;
    info!(%bind, "oxide-browser-sidecar listening");

    axum::serve(listener, app).await.context("axum serve")?;

    Ok(())
}

async fn healthz() -> Json<serde_json::Value> {
    Json(json!({"ok": true, "native": true}))
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
