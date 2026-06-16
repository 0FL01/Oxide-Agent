//! Test infrastructure setup: AppState factory functions and task execution helpers.

use oxide_agent_core::config::{AgentSettings, ModelInfo};
use oxide_agent_core::llm::LlmClient;
use oxide_agent_core::sandbox::{SandboxManager, SandboxScope};
use oxide_agent_runtime::SessionRegistry;
use oxide_agent_transport_web::AppState;
use oxide_agent_transport_web::session::WebSessionManager;
use std::env;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use super::providers::SequencedLlmProvider;

/// Response sequence for async sub-agent spawn tests.
///
/// Call order starts with main-agent `spawn_sub_agents`. The background sub-agent
/// and main-agent continuation can race, so the remaining scripted responses are
/// plain final text and are safe for either model.
pub fn async_sub_agent_spawn_responses() -> Vec<oxide_agent_core::llm::ChatResponse> {
    vec![
        super::helpers::tool_call_response(
            "spawn_sub_agents",
            serde_json::json!({
                "tasks": [
                    {
                        "task": "Capture package status and finish.",
                        "tools": ["write_todos"]
                    }
                ]
            }),
        ),
        super::helpers::structured_final_answer_response("sub-agent spawned"),
        super::helpers::structured_final_answer_response("delegation complete"),
    ]
}

/// Set up AppState with a custom scripted LLM provider.
/// Uses one SequencedLlmProvider for both main-agent and sub-agent model ids.
pub fn setup_web_test_with_custom_providers(llm_provider: Arc<SequencedLlmProvider>) -> AppState {
    let model_id = "opencode-go/deepseek-v4-flash".to_string();
    let agent_settings = Arc::new(AgentSettings {
        agent_model_id: Some(model_id.clone()),
        agent_model_provider: Some("opencode_go".to_string()),
        agent_model_routes: Some(vec![ModelInfo {
            id: model_id.clone(),
            provider: "opencode_go".to_string(),
            max_output_tokens: 32_000,
            context_window_tokens: 200_000,
            weight: 1,
        }]),
        sub_agent_model_id: Some(model_id.clone()),
        sub_agent_model_provider: Some("opencode_go".to_string()),
        sub_agent_model_routes: Some(vec![ModelInfo {
            id: model_id,
            provider: "opencode_go".to_string(),
            max_output_tokens: 32_000,
            context_window_tokens: 200_000,
            weight: 1,
        }]),
        agent_timeout_secs: Some(5),
        sub_agent_timeout_secs: Some(5),
        ..AgentSettings::default()
    });

    let llm = {
        let mut llm = LlmClient::new(&agent_settings);
        llm.register_provider("opencode_go".to_string(), llm_provider.clone());
        llm.register_provider("opencode-go".to_string(), llm_provider.clone());
        llm.register_provider("llm-provider/opencode-go".to_string(), llm_provider);
        Arc::new(llm)
    };

    let registry = SessionRegistry::new();
    let session_manager = WebSessionManager::new(registry, llm, agent_settings);
    let mut state = AppState::new(Arc::new(session_manager));
    state.auto_title_enabled = false;
    state
}

/// Set up AppState with a structured-output-capable main-agent route.
pub fn setup_web_test_with_structured_main_provider(
    provider: Arc<SequencedLlmProvider>,
) -> AppState {
    let agent_settings = Arc::new(AgentSettings {
        agent_model_id: Some("google/gemini-2.0-flash".to_string()),
        agent_model_provider: Some("opencode_go".to_string()),
        agent_model_routes: Some(vec![ModelInfo {
            id: "google/gemini-2.0-flash".to_string(),
            provider: "opencode_go".to_string(),
            max_output_tokens: 32_000,
            context_window_tokens: 200_000,
            weight: 1,
        }]),
        agent_timeout_secs: Some(5),
        ..AgentSettings::default()
    });

    let llm = {
        let mut llm = LlmClient::new(&agent_settings);
        llm.register_provider("opencode_go".to_string(), provider.clone());
        llm.register_provider("opencode-go".to_string(), provider.clone());
        llm.register_provider("llm-provider/opencode-go".to_string(), provider);
        Arc::new(llm)
    };

    let registry = SessionRegistry::new();
    let session_manager = WebSessionManager::new(registry, llm, agent_settings);
    let mut state = AppState::new(Arc::new(session_manager));
    state.auto_title_enabled = false;
    state
}

/// Set up test infrastructure with the default ScriptedLlmProvider.
pub async fn setup_test() -> AppState {
    use oxide_agent_transport_web::scripted_llm::{ScriptedLlmProvider, ScriptedResponse};

    let scripted = Arc::new(ScriptedLlmProvider::new(vec![ScriptedResponse::Text(
        "Hello from scripted LLM!".to_string(),
    )]));

    let agent_settings = Arc::new(AgentSettings {
        agent_model_id: Some("opencode-go/deepseek-v4-flash".to_string()),
        agent_model_provider: Some("opencode_go".to_string()),
        agent_model_routes: Some(vec![ModelInfo {
            id: "opencode-go/deepseek-v4-flash".to_string(),
            provider: "opencode_go".to_string(),
            max_output_tokens: 32_000,
            context_window_tokens: 200_000,
            weight: 1,
        }]),
        agent_timeout_secs: Some(5),
        ..AgentSettings::default()
    });

    let llm = LlmClient::new(&agent_settings);
    let llm = {
        let mut llm = llm;
        llm.register_provider("opencode_go".to_string(), scripted.clone());
        llm.register_provider("opencode-go".to_string(), scripted.clone());
        llm.register_provider("llm-provider/opencode-go".to_string(), scripted);
        Arc::new(llm)
    };

    let registry = SessionRegistry::new();
    let session_manager = WebSessionManager::new(registry, llm, agent_settings);
    let mut state = AppState::new(Arc::new(session_manager));
    state.auto_title_enabled = false;
    state
}

/// OpenAI Base provider env index used by live ZAI E2E tests.
const LIVE_ZAI_OPENAI_BASE_INDEX: usize = 1;
const LIVE_ZAI_OPENAI_BASE_PROVIDER: &str = "openai-base:zai";

/// Set up web transport state backed by the real ZAI OpenAI Base profile.
pub fn setup_live_zai_test() -> anyhow::Result<AppState> {
    // Load .env file if present so that `OPENAI_BASE_PROVIDERS__1__*`
    // variables are available when the test is run from a fresh shell.
    let _ = dotenvy::dotenv();

    let env_prefix = format!("OPENAI_BASE_PROVIDERS__{LIVE_ZAI_OPENAI_BASE_INDEX}__");
    let api_key_name = format!("{env_prefix}API_KEY");
    let _api_key = env::var(&api_key_name)
        .ok()
        .filter(|value| !value.is_empty() && value != "dummy")
        .ok_or_else(|| anyhow::anyhow!("{api_key_name} is required for live ZAI E2E tests"))?;
    let required = [
        (format!("{env_prefix}NAME"), "zai"),
        (
            format!("{env_prefix}API_BASE"),
            "https://api.z.ai/api/coding/paas/v4",
        ),
        (format!("{env_prefix}PROFILE"), "zai"),
    ];
    for (name, expected) in required {
        let actual = env::var(&name)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!("{name}={expected} is required for live ZAI E2E tests")
            })?;
        anyhow::ensure!(
            actual == expected,
            "{name} must be {expected:?} for live ZAI E2E tests, got {actual:?}"
        );
    }

    let settings = AgentSettings {
        agent_model_id: Some("glm-4.7".to_string()),
        agent_model_provider: Some(LIVE_ZAI_OPENAI_BASE_PROVIDER.to_string()),
        agent_model_max_output_tokens: Some(32_000),
        agent_model_context_window_tokens: Some(200_000),
        agent_timeout_secs: Some(900),
        ..AgentSettings::default()
    };
    let agent_settings = Arc::new(settings);

    let llm = Arc::new(LlmClient::new(&agent_settings));
    let registry = SessionRegistry::new();
    let session_manager = WebSessionManager::new(registry, llm, agent_settings);
    Ok(AppState::new(Arc::new(session_manager)))
}

/// Best-effort cleanup for the persistent web sandbox container used by a test user.
pub async fn cleanup_web_sandbox(user_id: i64) -> anyhow::Result<bool> {
    let scope = SandboxScope::new(user_id, "web");
    SandboxManager::delete_sandbox_by_name(user_id, &scope.container_name()).await
}

/// Execute a task directly via the session registry (bypasses HTTP layer).
pub async fn execute_task(
    session_manager: &WebSessionManager,
    session_id: &str,
    task_id: &str,
    task_text: &str,
) {
    use std::collections::hash_map::DefaultHasher;

    let meta = session_manager
        .get_session(session_id)
        .await
        .expect("session metadata should exist");
    let sid = {
        let mut h = DefaultHasher::new();
        session_id.hash(&mut h);
        meta.user_id.hash(&mut h);
        oxide_agent_core::agent::SessionId::from(h.finish() as i64)
    };

    let executor_arc = session_manager
        .session_registry()
        .get(&sid)
        .await
        .expect("session not found in registry");

    let (tx, _rx) = tokio::sync::mpsc::channel(100);

    let result = {
        let mut executor = executor_arc.write().await;
        executor.execute(task_text, Some(tx)).await
    };

    match result {
        Ok(_) => {
            session_manager.complete_task(task_id, session_id).await;
            tracing::info!(task_id, "Task completed");
        }
        Err(e) => {
            session_manager.fail_task(task_id, session_id).await;
            tracing::info!(task_id, error = %e, "Task failed");
        }
    }
}
