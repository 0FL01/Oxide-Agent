//! Core-only input intent classification for Agent Mode.

use crate::config::{AgentSettings, ModelInfo};
use crate::llm::{InternalTextPurpose, LlmClient};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};

const INTENT_CLASSIFIER_TIMEOUT_SECS: u64 = 8;
const INTENT_CLASSIFIER_MAX_OUTPUT_TOKENS: u32 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Snapshot status of the current Agent Mode session for follow-up intent classification.
pub enum AgentInputSessionStatus {
    /// The agent is idle or still processing without a terminal state.
    Idle,
    /// The agent completed the last task.
    Completed,
    /// The agent timed out while processing the last task.
    TimedOut,
    /// The agent ended the last task with an error.
    Error,
}

#[derive(Debug, Clone)]
/// Minimal transport-provided session context for core intent classification.
pub struct AgentInputIntentSnapshot {
    /// Current session status.
    pub status: AgentInputSessionStatus,
    /// Last user task known to the session.
    pub last_task: Option<String>,
    /// Final assistant response after the last task, if one exists.
    pub final_response_after_last_task: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// LLM-assisted classification of ambiguous Agent Mode user input.
pub enum AgentInputIntentClassification {
    /// Treat the input as an independent new task.
    StartNewTask,
    /// Treat the input as context for continuing the previous task.
    ContinueLastTask,
    /// Treat the input as a requested revision of the previous answer.
    ReviseLastAnswer,
}

#[derive(Debug, Deserialize)]
struct ClassifierResponse {
    intent: AgentInputIntentClassification,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WireClassifierIntent {
    StartNewTask,
    ContinueLastTask,
    ReviseLastAnswer,
}

impl<'de> Deserialize<'de> for AgentInputIntentClassification {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(match WireClassifierIntent::deserialize(deserializer)? {
            WireClassifierIntent::StartNewTask => Self::StartNewTask,
            WireClassifierIntent::ContinueLastTask => Self::ContinueLastTask,
            WireClassifierIntent::ReviseLastAnswer => Self::ReviseLastAnswer,
        })
    }
}

/// Classify ambiguous Agent Mode input using an internal text completion route.
pub async fn classify_agent_input_intent(
    snapshot: &AgentInputIntentSnapshot,
    user_input: &str,
    llm: &Arc<LlmClient>,
    settings: &Arc<AgentSettings>,
) -> Option<AgentInputIntentClassification> {
    let route = classifier_route(settings);
    if route.id.trim().is_empty() || route.provider.trim().is_empty() {
        return None;
    }

    let payload = json!({
        "last_task": snapshot.last_task.as_deref().unwrap_or_default(),
        "last_final_answer_preview": snapshot
            .final_response_after_last_task
            .as_deref()
            .map(|text| crate::utils::truncate_str(text, 1200))
            .unwrap_or_default(),
        "new_user_input": user_input,
        "session_status": match snapshot.status {
            AgentInputSessionStatus::Idle => "idle",
            AgentInputSessionStatus::Completed => "completed",
            AgentInputSessionStatus::TimedOut => "timed_out",
            AgentInputSessionStatus::Error => "error",
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
        llm.complete_internal_text(
            InternalTextPurpose::InputIntentClassification,
            system_prompt,
            &payload.to_string(),
            &route,
        ),
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
    use super::{AgentInputIntentClassification, parse_classifier_response};

    #[test]
    fn parse_classifier_response_accepts_wrapped_json() {
        let parsed = parse_classifier_response("result: {\"intent\":\"continue_last_task\"}\n")
            .expect("wrapped JSON should parse");

        assert_eq!(
            parsed.intent,
            AgentInputIntentClassification::ContinueLastTask
        );
    }
}
