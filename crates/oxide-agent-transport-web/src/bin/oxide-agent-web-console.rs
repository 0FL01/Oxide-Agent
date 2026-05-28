use dotenvy::dotenv;
use oxide_agent_core::config::AgentSettings;
use oxide_agent_core::llm::LlmClient;
use oxide_agent_runtime::SessionRegistry;
use oxide_agent_transport_web::session::WebSessionManager;
use oxide_agent_transport_web::{serve, AppState};
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();
    init_logging();

    let addr = env::var("OXIDE_WEB_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:3010".to_string())
        .parse::<SocketAddr>()?;

    let agent_settings = Arc::new(AgentSettings::new()?);
    let llm = Arc::new(LlmClient::new(agent_settings.as_ref()));
    let provider_names = llm.configured_provider_names();
    if provider_names.is_empty() {
        warn!(
            "No LLM providers are configured; web tasks will fail until provider credentials are set"
        );
    } else {
        info!(providers = ?provider_names, "Configured LLM providers");
    }

    let state = build_app_state(SessionRegistry::new(), llm, agent_settings).await?;
    info!("Starting Oxide Agent web console at http://{addr}");
    serve(state, addr).await;
    Ok(())
}

async fn build_app_state(
    registry: SessionRegistry,
    llm: Arc<LlmClient>,
    agent_settings: Arc<AgentSettings>,
) -> Result<AppState, Box<dyn std::error::Error>> {
    if use_r2_web_store() {
        return build_r2_app_state(registry, llm, agent_settings).await;
    }

    let session_manager = WebSessionManager::new(registry, llm, agent_settings);
    Ok(AppState::new(Arc::new(session_manager)))
}

#[cfg(feature = "storage-s3-r2")]
async fn build_r2_app_state(
    registry: SessionRegistry,
    llm: Arc<LlmClient>,
    agent_settings: Arc<AgentSettings>,
) -> Result<AppState, Box<dyn std::error::Error>> {
    oxide_agent_transport_web::build_r2_backed_app_state(registry, llm, agent_settings)
        .await
        .map_err(Into::into)
}

#[cfg(not(feature = "storage-s3-r2"))]
async fn build_r2_app_state(
    _registry: SessionRegistry,
    _llm: Arc<LlmClient>,
    _agent_settings: Arc<AgentSettings>,
) -> Result<AppState, Box<dyn std::error::Error>> {
    Err("OXIDE_WEB_STORE=r2 requires the storage-s3-r2 feature".into())
}

fn use_r2_web_store() -> bool {
    env::var("OXIDE_WEB_STORE").is_ok_and(|value| value.trim().eq_ignore_ascii_case("r2"))
        || web_bool_env("OXIDE_WEB_REQUIRE_DURABLE_STORAGE")
}

fn web_bool_env(key: &str) -> bool {
    env::var(key).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(
            "oxide_agent_core=info,oxide_agent_transport_web=info,oxide_agent_runtime=info,hyper=warn,h2=error,reqwest=warn,tokio=warn,tower=warn",
        )
    });
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
