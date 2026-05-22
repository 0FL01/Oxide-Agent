use super::SESSION_REGISTRY;
use oxide_agent_core::agent::compaction::AgentMessageKind;
use oxide_agent_core::agent::memory::AgentMessage;
use oxide_agent_core::agent::{AgentSession, AgentStatus, SessionId};
use oxide_agent_core::config::{AgentSettings, ModelInfo};
use oxide_agent_core::llm::LlmClient;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};

const INTENT_CLASSIFIER_TIMEOUT_SECS: u64 = 8;
const INTENT_CLASSIFIER_MAX_OUTPUT_TOKENS: u32 = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentInputIntent {
    StartNewTask { task: String },
    ContinueLastTask { user_context: String },
    ReviseLastAnswer { instruction: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputSessionStatus {
    Idle,
    Completed,
    TimedOut,
    Error,
}

#[derive(Debug, Clone)]
struct InputIntentSnapshot {
    status: InputSessionStatus,
    last_task: Option<String>,
    final_response_after_last_task: Option<String>,
    has_activity_after_last_task: bool,
}

#[derive(Debug, Deserialize)]
struct ClassifierResponse {
    intent: ClassifierIntent,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ClassifierIntent {
    StartNewTask,
    ContinueLastTask,
    ReviseLastAnswer,
}

pub(crate) async fn resolve_agent_input_intent(
    session_id: SessionId,
    user_input: String,
    llm: &Arc<LlmClient>,
    settings: &Arc<AgentSettings>,
) -> AgentInputIntent {
    let Some(snapshot) = load_input_intent_snapshot(session_id).await else {
        return AgentInputIntent::StartNewTask { task: user_input };
    };

    if snapshot.last_task.is_none() {
        return AgentInputIntent::StartNewTask { task: user_input };
    }

    match snapshot.status {
        InputSessionStatus::Error | InputSessionStatus::TimedOut => {
            return AgentInputIntent::ContinueLastTask {
                user_context: user_input,
            };
        }
        InputSessionStatus::Idle | InputSessionStatus::Completed => {}
    }

    if snapshot.final_response_after_last_task.is_none() && snapshot.has_activity_after_last_task {
        return AgentInputIntent::ContinueLastTask {
            user_context: user_input,
        };
    }

    if let Some(classified) =
        classify_ambiguous_input_intent(&snapshot, &user_input, llm, settings).await
    {
        return match classified {
            ClassifierIntent::StartNewTask => AgentInputIntent::StartNewTask { task: user_input },
            ClassifierIntent::ContinueLastTask => AgentInputIntent::ContinueLastTask {
                user_context: user_input,
            },
            ClassifierIntent::ReviseLastAnswer => AgentInputIntent::ReviseLastAnswer {
                instruction: user_input,
            },
        };
    }

    if snapshot.final_response_after_last_task.is_some() {
        AgentInputIntent::StartNewTask { task: user_input }
    } else {
        AgentInputIntent::ContinueLastTask {
            user_context: user_input,
        }
    }
}

async fn load_input_intent_snapshot(session_id: SessionId) -> Option<InputIntentSnapshot> {
    let executor_arc = SESSION_REGISTRY.get(&session_id).await?;
    let executor = executor_arc.read().await;
    Some(snapshot_from_session(executor.session()))
}

fn snapshot_from_session(session: &AgentSession) -> InputIntentSnapshot {
    let status = match &session.status {
        AgentStatus::Idle | AgentStatus::Processing { .. } => InputSessionStatus::Idle,
        AgentStatus::Completed => InputSessionStatus::Completed,
        AgentStatus::TimedOut => InputSessionStatus::TimedOut,
        AgentStatus::Error(_) => InputSessionStatus::Error,
    };
    let messages = session.memory.get_messages();
    let last_task = session
        .last_task
        .clone()
        .or_else(|| latest_user_task(messages).map(ToOwned::to_owned));
    let (final_response_after_last_task, has_activity_after_last_task) =
        last_task_activity(messages);

    InputIntentSnapshot {
        status,
        last_task,
        final_response_after_last_task,
        has_activity_after_last_task,
    }
}

fn latest_user_task(messages: &[AgentMessage]) -> Option<&str> {
    messages
        .iter()
        .rev()
        .find(|message| {
            message.resolved_kind() == AgentMessageKind::UserTask
                && !message.content.trim().is_empty()
        })
        .map(|message| message.content.as_str())
}

fn last_task_activity(messages: &[AgentMessage]) -> (Option<String>, bool) {
    let Some(last_task_index) = messages
        .iter()
        .rposition(|message| message.resolved_kind() == AgentMessageKind::UserTask)
    else {
        return (None, false);
    };

    let mut final_response = None;
    let mut has_activity = false;
    for message in &messages[last_task_index + 1..] {
        has_activity = true;
        match message.resolved_kind() {
            AgentMessageKind::AssistantResponse | AgentMessageKind::AssistantReasoning => {
                if !message.content.trim().is_empty() {
                    final_response = Some(message.content.clone());
                }
            }
            _ => {}
        }
    }

    (final_response, has_activity)
}

async fn classify_ambiguous_input_intent(
    snapshot: &InputIntentSnapshot,
    user_input: &str,
    llm: &Arc<LlmClient>,
    settings: &Arc<AgentSettings>,
) -> Option<ClassifierIntent> {
    let route = classifier_route(settings);
    if route.id.trim().is_empty() || route.provider.trim().is_empty() {
        return None;
    }

    let payload = json!({
        "last_task": snapshot.last_task.as_deref().unwrap_or_default(),
        "last_final_answer_preview": snapshot
            .final_response_after_last_task
            .as_deref()
            .map(|text| oxide_agent_core::utils::truncate_str(text, 1200))
            .unwrap_or_default(),
        "new_user_input": user_input,
        "session_status": match snapshot.status {
            InputSessionStatus::Idle => "idle",
            InputSessionStatus::Completed => "completed",
            InputSessionStatus::TimedOut => "timed_out",
            InputSessionStatus::Error => "error",
        },
    });

    let system_prompt = concat!(
        "Classify the user's next Agent Mode input. ",
        "Return only JSON: {\"intent\":\"start_new_task|continue_last_task|revise_last_answer\"}. ",
        "Use continue_last_task when the user wants the previous unfinished task to proceed. ",
        "Use revise_last_answer when the user comments on or asks to expand/fix the previous answer. ",
        "Use start_new_task when the user is asking for a different task."
    );

    let classification = tokio::time::timeout(
        Duration::from_secs(INTENT_CLASSIFIER_TIMEOUT_SECS),
        llm.chat_completion_for_model_info(system_prompt, &[], &payload.to_string(), &route),
    )
    .await;

    let raw = match classification {
        Ok(Ok(raw)) => raw,
        Ok(Err(error)) => {
            debug!(error = %error, "Agent input intent classifier failed; using deterministic fallback");
            return None;
        }
        Err(_) => {
            warn!(
                timeout_secs = INTENT_CLASSIFIER_TIMEOUT_SECS,
                "Agent input intent classifier timed out; using deterministic fallback"
            );
            return None;
        }
    };

    parse_classifier_response(&raw).map(|response| response.intent)
}

fn classifier_route(settings: &AgentSettings) -> ModelInfo {
    let mut route = settings.get_configured_agent_model();
    route.max_output_tokens = route
        .max_output_tokens
        .clamp(64, INTENT_CLASSIFIER_MAX_OUTPUT_TOKENS);
    route
}

fn parse_classifier_response(raw: &str) -> Option<ClassifierResponse> {
    if let Ok(parsed) = serde_json::from_str::<ClassifierResponse>(raw.trim()) {
        return Some(parsed);
    }

    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str::<ClassifierResponse>(&raw[start..=end]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_agent_core::agent::memory::AgentMessage;

    #[test]
    fn snapshot_detects_incomplete_last_task_without_text_heuristics() {
        let mut session = AgentSession::new(SessionId::from(7_i64));
        session
            .memory
            .add_message(AgentMessage::user_task("research netcup"));
        session
            .memory
            .add_message(AgentMessage::assistant_with_tools(
                "",
                vec![oxide_agent_core::llm::ToolCall::new(
                    "call-1",
                    oxide_agent_core::llm::ToolCallFunction {
                        name: "web_search".to_string(),
                        arguments: "{}".to_string(),
                    },
                    false,
                )],
            ));
        session
            .memory
            .add_message(AgentMessage::tool("call-1", "web_search", "results"));
        session.restore_last_task_from_memory();

        let snapshot = snapshot_from_session(&session);

        assert_eq!(snapshot.last_task.as_deref(), Some("research netcup"));
        assert!(snapshot.has_activity_after_last_task);
        assert!(snapshot.final_response_after_last_task.is_none());
    }

    #[test]
    fn snapshot_detects_final_response_after_last_task() {
        let mut session = AgentSession::new(SessionId::from(7_i64));
        session
            .memory
            .add_message(AgentMessage::user_task("research netcup"));
        session
            .memory
            .add_message(AgentMessage::assistant("final report"));
        session.complete();

        let snapshot = snapshot_from_session(&session);

        assert_eq!(snapshot.status, InputSessionStatus::Completed);
        assert_eq!(
            snapshot.final_response_after_last_task.as_deref(),
            Some("final report")
        );
    }

    #[test]
    fn parses_classifier_json_with_surrounding_text() {
        let parsed =
            parse_classifier_response("Answer:\n{\"intent\":\"revise_last_answer\"}\nThanks")
                .expect("classifier response should parse");

        assert!(matches!(parsed.intent, ClassifierIntent::ReviseLastAnswer));
    }
}
