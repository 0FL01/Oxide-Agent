//! Native browser sidecar binary entry point.

use anyhow::Context;
use oxide_browser_sidecar::{AppState, create_app, session::SessionManager};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let token = std::env::var("BROWSER_AGENT_SIDECAR_TOKEN")
        .context("BROWSER_AGENT_SIDECAR_TOKEN is required")?;

    let bind =
        std::env::var("BROWSER_AGENT_SIDECAR_BIND").unwrap_or_else(|_| "0.0.0.0:8787".to_string());

    let state = AppState {
        sessions: std::sync::Arc::new(SessionManager::default()),
    };

    let app = create_app(state, token);

    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("bind {bind}"))?;
    info!(%bind, "oxide-browser-sidecar listening");

    axum::serve(listener, app).await.context("axum serve")?;

    Ok(())
}
