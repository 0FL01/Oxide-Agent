use super::SESSION_REGISTRY;
use oxide_agent_core::agent::compaction::AgentMessageKind;
use oxide_agent_core::agent::memory::AgentMessage;
use oxide_agent_core::agent::{
    AgentInputIntentClassification, AgentInputIntentSnapshot, AgentInputSessionStatus,
    classify_agent_input_intent,
};
use oxide_agent_core::agent::{AgentSession, AgentStatus, SessionId};
use oxide_agent_core::config::AgentSettings;
use oxide_agent_core::llm::LlmClient;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentInputIntent {
    StartNewTask { task: String },
    ContinueLastTask { user_context: String },
    ReviseLastAnswer { instruction: String },
}

#[derive(Debug, Clone)]
struct InputIntentSnapshot {
    status: AgentInputSessionStatus,
    last_task: Option<String>,
    final_response_after_last_task: Option<String>,
    has_activity_after_last_task: bool,
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
        AgentInputSessionStatus::Error | AgentInputSessionStatus::TimedOut => {
            return AgentInputIntent::ContinueLastTask {
                user_context: user_input,
            };
        }
        AgentInputSessionStatus::Idle | AgentInputSessionStatus::Completed => {}
    }

    if snapshot.final_response_after_last_task.is_none() && snapshot.has_activity_after_last_task {
        return AgentInputIntent::ContinueLastTask {
            user_context: user_input,
        };
    }

    let core_snapshot = AgentInputIntentSnapshot {
        status: snapshot.status,
        last_task: snapshot.last_task.clone(),
        final_response_after_last_task: snapshot.final_response_after_last_task.clone(),
    };
    if let Some(classified) =
        classify_agent_input_intent(&core_snapshot, &user_input, llm, settings).await
    {
        return match classified {
            AgentInputIntentClassification::StartNewTask => {
                AgentInputIntent::StartNewTask { task: user_input }
            }
            AgentInputIntentClassification::ContinueLastTask => {
                AgentInputIntent::ContinueLastTask {
                    user_context: user_input,
                }
            }
            AgentInputIntentClassification::ReviseLastAnswer => {
                AgentInputIntent::ReviseLastAnswer {
                    instruction: user_input,
                }
            }
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
        AgentStatus::Idle | AgentStatus::Processing { .. } => AgentInputSessionStatus::Idle,
        AgentStatus::Completed => AgentInputSessionStatus::Completed,
        AgentStatus::TimedOut => AgentInputSessionStatus::TimedOut,
        AgentStatus::Error(_) => AgentInputSessionStatus::Error,
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

        assert_eq!(snapshot.status, AgentInputSessionStatus::Completed);
        assert_eq!(
            snapshot.final_response_after_last_task.as_deref(),
            Some("final report")
        );
    }
}
