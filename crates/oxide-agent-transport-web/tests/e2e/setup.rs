//! Test infrastructure setup: AppState factory functions and task execution helpers.

use oxide_agent_core::config::AgentSettings;
use oxide_agent_core::llm::LlmClient;
use oxide_agent_core::sandbox::{SandboxManager, SandboxScope};
use oxide_agent_core::{agent::connect_postgres_memory_store, agent::PersistentMemoryStore};
use oxide_agent_runtime::SessionRegistry;
use oxide_agent_transport_web::session::WebSessionManager;
use oxide_agent_transport_web::AppState;
use std::env;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use super::providers::{ControlledNarratorProvider, SequencedZaiProvider};

/// Response sequence for delegated sub-agent tests.
///
/// Call order: main-agent (delegate_to_sub_agent) -> sub-agent (write_todos) ->
/// sub-agent (empty unstructured) -> main-agent (final text).
pub fn delegated_sub_agent_empty_content_responses() -> Vec<oxide_agent_core::llm::ChatResponse> {
    vec![
        super::helpers::tool_call_response(
            "delegate_to_sub_agent",
            serde_json::json!({
                "task": "Capture package status and finish.",
                "tools": ["write_todos"],
            }),
        ),
        super::helpers::tool_call_response(
            "write_todos",
            serde_json::json!({
                "todos": [
                    {
                        "description": "Capture package status",
                        "status": "completed"
                    }
                ]
            }),
        ),
        super::helpers::empty_unstructured_response(),
        super::helpers::unstructured_text_response("delegation complete"),
    ]
}

/// Set up AppState with custom ZAI and narrator LLM providers.
/// Uses two SequencedZaiProvider instances: one for the main-agent ("main-model"),
/// one for the sub-agent ("glm-4.7").
pub fn setup_web_test_with_custom_providers(
    zai_provider: Arc<SequencedZaiProvider>,
    narrator_provider: Arc<ControlledNarratorProvider>,
) -> AppState {
    let agent_settings = Arc::new(AgentSettings {
        agent_model_id: Some("main-model".to_string()),
        agent_model_provider: Some("zai".to_string()),
        sub_agent_model_id: Some("glm-4.7".to_string()),
        sub_agent_model_provider: Some("zai".to_string()),
        narrator_model_id: Some("narrator-model".to_string()),
        narrator_model_provider: Some("narrator".to_string()),
        agent_timeout_secs: Some(5),
        sub_agent_timeout_secs: Some(5),
        ..AgentSettings::default()
    });

    let llm = {
        let mut llm = LlmClient::new(&agent_settings);
        llm.register_provider("zai".to_string(), zai_provider);
        llm.register_provider("narrator".to_string(), narrator_provider);
        Arc::new(llm)
    };

    let registry = SessionRegistry::new();
    let session_manager = WebSessionManager::new(registry, llm, agent_settings);
    AppState::new(Arc::new(session_manager))
}

/// Set up AppState with a structured-output-capable main-agent route.
pub fn setup_web_test_with_structured_main_provider(
    provider: Arc<SequencedZaiProvider>,
    narrator_provider: Arc<ControlledNarratorProvider>,
) -> AppState {
    let agent_settings = Arc::new(AgentSettings {
        agent_model_id: Some("gemini-2.0-flash".to_string()),
        agent_model_provider: Some("gemini".to_string()),
        narrator_model_id: Some("narrator-model".to_string()),
        narrator_model_provider: Some("narrator".to_string()),
        agent_timeout_secs: Some(5),
        ..AgentSettings::default()
    });

    let llm = {
        let mut llm = LlmClient::new(&agent_settings);
        llm.register_provider("gemini".to_string(), provider);
        llm.register_provider("narrator".to_string(), narrator_provider);
        Arc::new(llm)
    };

    let registry = SessionRegistry::new();
    let session_manager = WebSessionManager::new(registry, llm, agent_settings);
    AppState::new(Arc::new(session_manager))
}

/// Set up test infrastructure with the default ScriptedLlmProvider.
pub async fn setup_test() -> AppState {
    use oxide_agent_transport_web::scripted_llm::{ScriptedLlmProvider, ScriptedResponse};

    let scripted = Arc::new(ScriptedLlmProvider::new(vec![ScriptedResponse::Text(
        "Hello from scripted LLM!".to_string(),
    )]));

    let agent_settings = Arc::new(AgentSettings {
        agent_model_id: Some("test-model".to_string()),
        agent_model_provider: Some("scripted".to_string()),
        agent_timeout_secs: Some(5),
        ..AgentSettings::default()
    });

    let llm = LlmClient::new(&agent_settings);
    let llm = {
        let mut llm = llm;
        llm.register_provider("scripted".to_string(), scripted);
        Arc::new(llm)
    };

    let registry = SessionRegistry::new();
    let session_manager = WebSessionManager::new(registry, llm, agent_settings);
    AppState::new(Arc::new(session_manager))
}

/// ZAI API base URL used by live E2E tests.
const ZAI_API_BASE: &str = "https://api.z.ai/api/coding/paas/v4/chat/completions";

/// Set up web transport state backed by the real ZAI provider.
pub fn setup_live_zai_test() -> anyhow::Result<AppState> {
    // Load .env file if present so that `ZAI_API_KEY` and `ZAI_API_BASE`
    // are available when the test is run from a fresh shell.
    let _ = dotenvy::dotenv();

    let api_key = env::var("ZAI_API_KEY")
        .ok()
        .filter(|value| !value.is_empty() && value != "dummy")
        .ok_or_else(|| anyhow::anyhow!("ZAI_API_KEY is required for live ZAI E2E tests"))?;

    let mut settings = AgentSettings {
        agent_model_id: Some("glm-4.7".to_string()),
        agent_model_provider: Some("zai".to_string()),
        agent_model_max_output_tokens: Some(32_000),
        agent_model_context_window_tokens: Some(200_000),
        agent_timeout_secs: Some(900),
        zai_api_key: Some(api_key),
        ..AgentSettings::default()
    };
    if let Ok(base) = env::var("ZAI_API_BASE") {
        if !base.is_empty() {
            settings.zai_api_base = base;
        }
    } else {
        settings.zai_api_base = ZAI_API_BASE.to_string();
    }
    let agent_settings = Arc::new(settings);

    let llm = Arc::new(LlmClient::new(&agent_settings));
    let registry = SessionRegistry::new();
    let session_manager = WebSessionManager::new(registry, llm, agent_settings);
    Ok(AppState::new(Arc::new(session_manager)))
}

pub async fn setup_live_zai_test_with_postgres(
) -> anyhow::Result<(AppState, Arc<dyn PersistentMemoryStore>)> {
    let _ = dotenvy::dotenv();

    let api_key = env::var("ZAI_API_KEY")
        .ok()
        .filter(|value| !value.is_empty() && value != "dummy")
        .ok_or_else(|| anyhow::anyhow!("ZAI_API_KEY is required for live ZAI E2E tests"))?;
    let memory_database_url = env::var("MEMORY_DATABASE_URL")
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("MEMORY_DATABASE_URL is required for Postgres memory E2E tests")
        })?;
    let embedding_dimensions = env::var("EMBEDDING_DIMENSIONS")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(768);

    let mut settings = AgentSettings {
        agent_model_id: Some("glm-4.7".to_string()),
        agent_model_provider: Some("zai".to_string()),
        memory_classifier_model: Some("glm-4.7".to_string()),
        memory_classifier_provider: Some("zai".to_string()),
        compaction_model_id: Some("glm-4.7".to_string()),
        compaction_model_provider: Some("zai".to_string()),
        agent_model_max_output_tokens: Some(32_000),
        agent_model_context_window_tokens: Some(200_000),
        embedding_dimensions: Some(embedding_dimensions),
        agent_timeout_secs: Some(900),
        zai_api_key: Some(api_key),
        memory_database_url: Some(memory_database_url),
        memory_database_auto_migrate: Some(true),
        ..AgentSettings::default()
    };
    if let Ok(base) = env::var("ZAI_API_BASE") {
        if !base.is_empty() {
            settings.zai_api_base = base;
        }
    } else {
        settings.zai_api_base = ZAI_API_BASE.to_string();
    }
    let agent_settings = Arc::new(settings);
    let memory_store = connect_postgres_memory_store(agent_settings.as_ref()).await?;

    let llm = Arc::new(LlmClient::new(&agent_settings));
    let registry = SessionRegistry::new();
    let session_manager = WebSessionManager::new(registry, llm, Arc::clone(&agent_settings))
        .with_persistent_memory_store(Arc::clone(&memory_store));
    Ok((AppState::new(Arc::new(session_manager)), memory_store))
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
