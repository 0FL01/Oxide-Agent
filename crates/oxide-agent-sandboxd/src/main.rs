use dotenvy::dotenv;
use oxide_agent_core::sandbox::SandboxBrokerServer;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();
    init_logging();

    let server = SandboxBrokerServer::bind_default().await?;
    info!(socket_path = %server.socket_path().display(), "Starting sandbox broker");
    server.serve().await?;
    Ok(())
}

fn init_logging() {
    let debug_mode = std::env::var("DEBUG_MODE")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    let filter = if debug_mode {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("debug"))
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new("oxide_agent_core=info,oxide_agent_sandboxd=info,bollard=warn")
        })
    };

    tracing_subscriber::fmt().with_env_filter(filter).init();
}
