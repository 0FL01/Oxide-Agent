//! Test infrastructure setup: AppState factory functions and task execution helpers.

use oxide_agent_core::config::AgentSettings;
use oxide_agent_core::llm::LlmClient;
use oxide_agent_runtime::SessionRegistry;
use oxide_agent_transport_web::session::WebSessionManager;
use oxide_agent_transport_web::AppState;
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
    let agent_settings = Arc::new({
        let mut s = AgentSettings::default();
        s.agent_model_id = Some("main-model".to_string());
        s.agent_model_provider = Some("zai".to_string());
        s.sub_agent_model_id = Some("glm-4.7".to_string());
        s.sub_agent_model_provider = Some("zai".to_string());
        s.narrator_model_id = Some("narrator-model".to_string());
        s.narrator_model_provider = Some("narrator".to_string());
        s.agent_timeout_secs = Some(5);
        s.sub_agent_timeout_secs = Some(5);
        s
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

/// Set up test infrastructure with the default ScriptedLlmProvider.
pub async fn setup_test() -> AppState {
    use oxide_agent_transport_web::scripted_llm::{ScriptedLlmProvider, ScriptedResponse};

    let scripted = Arc::new(ScriptedLlmProvider::new(vec![ScriptedResponse::Text(
        "Hello from scripted LLM!".to_string(),
    )]));

    let agent_settings = Arc::new({
        let mut s = AgentSettings::default();
        s.agent_model_id = Some("test-model".to_string());
        s.agent_model_provider = Some("scripted".to_string());
        s.agent_timeout_secs = Some(5);
        s
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

/// Execute a task directly via the session registry (bypasses HTTP layer).
pub async fn execute_task(
    session_manager: &WebSessionManager,
    session_id: &str,
    task_id: &str,
    task_text: &str,
) {
    use std::collections::hash_map::DefaultHasher;

    let meta = session_manager.get_session(session_id).await.unwrap();
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
