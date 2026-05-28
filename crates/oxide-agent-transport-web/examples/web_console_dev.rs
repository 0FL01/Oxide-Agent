use oxide_agent_core::config::{AgentSettings, ModelInfo};
use oxide_agent_core::llm::LlmClient;
use oxide_agent_runtime::SessionRegistry;
use oxide_agent_transport_web::scripted_llm::ScriptedLlmProvider;
use oxide_agent_transport_web::session::WebSessionManager;
use oxide_agent_transport_web::{serve, AppState};
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    default_dev_env("OXIDE_WEB_REGISTRATION_ENABLED", "true");
    default_dev_env("OXIDE_WEB_ALLOW_IN_MEMORY_STORE", "true");

    let addr = env::var("OXIDE_WEB_DEV_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:3010".to_string())
        .parse::<SocketAddr>()?;

    let model_id = "opencode-go/deepseek-v4-flash".to_string();
    let agent_settings = Arc::new(AgentSettings {
        agent_model_id: Some(model_id.clone()),
        agent_model_provider: Some("opencode_go".to_string()),
        agent_model_routes: Some(vec![ModelInfo {
            id: model_id,
            provider: "opencode_go".to_string(),
            max_output_tokens: 32_000,
            context_window_tokens: 200_000,
            weight: 1,
        }]),
        agent_timeout_secs: Some(30),
        ..AgentSettings::default()
    });

    let mut llm = LlmClient::new(&agent_settings);
    llm.register_provider(
        "opencode_go".to_string(),
        Arc::new(ScriptedLlmProvider::new(Vec::new())),
    );

    let session_manager =
        WebSessionManager::new(SessionRegistry::new(), Arc::new(llm), agent_settings);
    let state = AppState::new(Arc::new(session_manager));

    println!("Oxide Agent web console dev server: http://{addr}");
    serve(state, addr).await;
    Ok(())
}

fn default_dev_env(key: &str, value: &str) {
    if env::var_os(key).is_none() {
        env::set_var(key, value);
    }
}
