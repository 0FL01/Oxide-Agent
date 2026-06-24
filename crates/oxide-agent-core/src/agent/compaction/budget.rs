//! Full request budget estimation for Agent Mode compaction checkpoints.

use super::types::{
    AgentMessageKind, BudgetEstimate, BudgetState, CompactionPolicy, CompactionRequest,
    CompactionRetention, HotMemoryBudget,
};
use crate::agent::context::AgentContext;
use crate::agent::memory::AgentMessage;
use crate::llm::{Message, ToolDefinition};
use std::sync::OnceLock;
use tiktoken_rs::cl100k_base;

/// Cached tokenizer to avoid repeated initialization.
/// cl100k_base() loads the BPE encoder from bytes on each call, which is expensive.
static TOKENIZER: OnceLock<tiktoken_rs::CoreBPE> = OnceLock::new();

/// Get the cached tokenizer, initializing it on first call.
fn get_tokenizer() -> &'static tiktoken_rs::CoreBPE {
    TOKENIZER.get_or_init(|| cl100k_base().expect("Failed to initialize cl100k tokenizer"))
}

/// Count tokens in text using the cached cl100k tokenizer.
/// This is a public wrapper for use by other modules (e.g., memory).
pub fn count_tokens_cached(text: &str) -> usize {
    get_tokenizer().encode_with_special_tokens(text).len()
}

/// Estimate the full request budget for the current checkpoint.
#[must_use]
pub fn estimate_request_budget(
    policy: &CompactionPolicy,
    request: &CompactionRequest<'_>,
    agent: &dyn AgentContext,
) -> BudgetEstimate {
    let system_prompt_tokens = estimate_text_tokens(request.system_prompt);
    let tool_schema_tokens = estimate_tool_tokens(request.tools);
    let hot_memory = estimate_hot_memory(agent);
    let context_window_tokens = agent.memory().max_tokens();
    let reserved_output_tokens = 0;
    let hard_reserve_tokens = policy.hard_reserve_tokens;
    let total_input_tokens = system_prompt_tokens
        .saturating_add(tool_schema_tokens)
        .saturating_add(hot_memory.rendered_tokens);
    let projected_total_tokens = total_input_tokens.saturating_add(hard_reserve_tokens);
    let headroom_tokens = context_window_tokens.saturating_sub(projected_total_tokens);
    let warning_threshold_tokens =
        percent_of(context_window_tokens, policy.warning_threshold_percent);
    let compact_threshold_tokens =
        percent_of(context_window_tokens, policy.compact_threshold_percent);
    let over_limit_threshold_tokens =
        percent_of(context_window_tokens, policy.over_limit_threshold_percent);
    let state = if projected_total_tokens >= over_limit_threshold_tokens {
        BudgetState::OverLimit
    } else if projected_total_tokens >= compact_threshold_tokens {
        BudgetState::ShouldCompact
    } else if projected_total_tokens >= warning_threshold_tokens {
        BudgetState::Warning
    } else {
        BudgetState::Healthy
    };

    BudgetEstimate {
        context_window_tokens,
        system_prompt_tokens,
        tool_schema_tokens,
        hot_memory,
        reserved_output_tokens,
        hard_reserve_tokens,
        total_input_tokens,
        projected_total_tokens,
        headroom_tokens,
        warning_threshold_tokens,
        compact_threshold_tokens,
        over_limit_threshold_tokens,
        state,
    }
}

fn estimate_hot_memory(agent: &dyn AgentContext) -> HotMemoryBudget {
    let messages = agent.memory().get_messages();
    let mut budget = HotMemoryBudget {
        rendered_tokens: 0,
        rendered_messages: 0,
        raw_tokens: 0,
        raw_messages: messages.len(),
        pinned_tokens: 0,
        protected_live_tokens: 0,
        prunable_artifact_tokens: 0,
        compactable_history_tokens: 0,
        runtime_context_tokens: 0,
    };

    for message in messages {
        let tokens = estimate_message_tokens(message);
        budget.raw_tokens = budget.raw_tokens.saturating_add(tokens);

        match message.retention() {
            CompactionRetention::Pinned => {
                budget.pinned_tokens = budget.pinned_tokens.saturating_add(tokens);
            }
            CompactionRetention::ProtectedLive => {
                budget.protected_live_tokens = budget.protected_live_tokens.saturating_add(tokens);
            }
            CompactionRetention::PrunableArtifact => {
                budget.prunable_artifact_tokens =
                    budget.prunable_artifact_tokens.saturating_add(tokens);
            }
            CompactionRetention::CompactableHistory => {
                budget.compactable_history_tokens =
                    budget.compactable_history_tokens.saturating_add(tokens);
            }
        }

        if message.resolved_kind() == AgentMessageKind::RuntimeContext {
            budget.runtime_context_tokens = budget.runtime_context_tokens.saturating_add(tokens);
        }
    }

    let rendered = agent.memory().rendered_messages();
    budget.rendered_messages = rendered.len();
    budget.rendered_tokens = estimate_rendered_messages_tokens(&rendered);

    budget
}

fn estimate_tool_tokens(tools: &[ToolDefinition]) -> usize {
    tools.iter().fold(0, |acc, tool| {
        let parameter_tokens = estimate_json_tokens(&tool.parameters);
        acc.saturating_add(estimate_text_tokens(&tool.name))
            .saturating_add(estimate_text_tokens(&tool.description))
            .saturating_add(parameter_tokens)
    })
}

pub(crate) fn estimate_message_tokens(message: &AgentMessage) -> usize {
    let reasoning_tokens = message.reasoning.as_deref().map_or(0, estimate_text_tokens);
    estimate_text_tokens(&message.content).saturating_add(reasoning_tokens)
}

fn estimate_rendered_messages_tokens(messages: &[Message]) -> usize {
    messages.iter().map(estimate_rendered_message_tokens).sum()
}

fn estimate_rendered_message_tokens(message: &Message) -> usize {
    let mut tokens = estimate_text_tokens(&message.content);
    if let Some(reasoning) = message.reasoning_content.as_deref() {
        tokens = tokens.saturating_add(estimate_text_tokens(reasoning));
    }
    if let Some(correlation) = message.resolved_tool_call_correlation() {
        tokens = tokens.saturating_add(estimate_text_tokens(correlation.wire_tool_call_id()));
    }
    if let Some(name) = message.name.as_deref() {
        tokens = tokens.saturating_add(estimate_text_tokens(name));
    }
    if let Some(tool_calls) = message.tool_calls.as_ref() {
        tokens = tokens.saturating_add(estimate_json_tokens(
            &serde_json::to_value(tool_calls).unwrap_or(serde_json::Value::Null),
        ));
    }
    tokens
}

fn estimate_json_tokens(value: &serde_json::Value) -> usize {
    serde_json::to_string(value)
        .ok()
        .as_deref()
        .map_or(0, estimate_text_tokens)
}

fn estimate_text_tokens(text: &str) -> usize {
    get_tokenizer().encode_with_special_tokens(text).len()
}

const fn percent_of(value: usize, percent: u8) -> usize {
    value.saturating_mul(percent as usize) / 100
}

#[cfg(test)]
mod tests {
    use super::estimate_request_budget;
    use crate::agent::compaction::{
        BudgetState, CompactionEngine, CompactionPolicy, CompactionRequest, CompactionTrigger,
        CompressionSelection, MessageRef, SummaryPart,
    };
    use crate::agent::memory::AgentMessage;
    use crate::agent::{AgentContext, EphemeralSession};
    use crate::llm::{ToolCall, ToolCallFunction, ToolDefinition};

    #[test]
    fn estimate_request_budget_accounts_for_request_components() {
        let mut session = EphemeralSession::new(4_000);
        session
            .memory_mut()
            .add_message(AgentMessage::topic_agents_md(
                "# Topic AGENTS\nUse safe deploys.",
            ));
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Deploy the hotfix"));
        // Tool result requires a preceding assistant message with tool call to avoid being dropped by history repair
        session
            .memory_mut()
            .add_message(AgentMessage::assistant_with_tools(
                "I'll run cargo check",
                vec![ToolCall::new(
                    "call-1".to_string(),
                    ToolCallFunction {
                        name: "execute_command".to_string(),
                        arguments: r#"{"command":"cargo check"}"#.to_string(),
                    },
                    false,
                )],
            ));
        session.memory_mut().add_message(AgentMessage::tool(
            "call-1",
            "execute_command",
            "cargo check output",
        ));
        let tools = [ToolDefinition {
            name: "execute_command".to_string(),
            description: "Run a shell command".to_string(),
            parameters: serde_json::json!({"type":"object","properties":{"command":{"type":"string"}}}),
        }];
        let request = CompactionRequest::new(
            CompactionTrigger::PreRun,
            "Deploy the hotfix",
            "System prompt with operational instructions",
            &tools,
            "demo-model",
            512,
            false,
        );

        let estimate = estimate_request_budget(&CompactionPolicy::default(), &request, &session);

        assert!(estimate.system_prompt_tokens > 0);
        assert!(estimate.tool_schema_tokens > 0);
        assert!(estimate.hot_memory.rendered_tokens > 0);
        assert!(estimate.hot_memory.raw_tokens > 0);
        assert!(estimate.hot_memory.rendered_tokens >= estimate.hot_memory.raw_tokens);
        assert!(estimate.hot_memory.pinned_tokens > 0);
        assert!(estimate.hot_memory.prunable_artifact_tokens > 0);
        assert_eq!(estimate.reserved_output_tokens, 0);
        assert_eq!(estimate.warning_threshold_tokens, 2_600);
        assert_eq!(estimate.compact_threshold_tokens, 3_400);
        assert_eq!(estimate.over_limit_threshold_tokens, 3_800);
        assert_eq!(
            estimate.total_input_tokens,
            estimate.system_prompt_tokens
                + estimate.tool_schema_tokens
                + estimate.hot_memory.rendered_tokens
        );
        assert_eq!(
            estimate.projected_total_tokens,
            estimate.total_input_tokens + estimate.hard_reserve_tokens
        );
    }

    #[test]
    fn estimate_request_budget_transitions_to_over_limit() {
        let mut session = EphemeralSession::new(1_000);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task(format!(
                "{}{}",
                "a".repeat(2_000),
                "b".repeat(2_000)
            )));

        let request = CompactionRequest::new(
            CompactionTrigger::PreRun,
            "huge task",
            "system prompt",
            &[],
            "demo-model",
            512,
            false,
        );

        let estimate = estimate_request_budget(&CompactionPolicy::default(), &request, &session);

        assert_eq!(estimate.state, BudgetState::OverLimit);
        assert_eq!(estimate.headroom_tokens, 0);
    }

    #[test]
    fn estimate_request_budget_uses_rendered_overlay_not_raw_transcript() {
        let mut session = EphemeralSession::new(20_000);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Investigate laptops"));
        session.memory_mut().add_message(AgentMessage::user(format!(
            "old raw crawl {}",
            "large ".repeat(12_000)
        )));
        session
            .memory_mut()
            .add_message(AgentMessage::user("recent requirement 1"));
        session
            .memory_mut()
            .add_message(AgentMessage::user("recent requirement 2"));
        session
            .memory_mut()
            .add_message(AgentMessage::user("recent requirement 3"));

        let messages = session.memory().get_messages().to_vec();
        CompactionEngine::apply_compression(
            session.memory_mut().compaction_state_mut(),
            &messages,
            &CompressionSelection::Range {
                start: MessageRef::from_index(1),
                end: MessageRef::from_index(1),
            },
            vec![SummaryPart::Text("old crawl summarized".to_string())],
        )
        .expect("test compaction block is valid");

        let request = CompactionRequest::new(
            CompactionTrigger::PreIteration,
            "Investigate laptops",
            "system prompt",
            &[],
            "demo-model",
            512,
            false,
        );

        let estimate = estimate_request_budget(&CompactionPolicy::default(), &request, &session);
        let raw_projected_total = estimate
            .system_prompt_tokens
            .saturating_add(estimate.tool_schema_tokens)
            .saturating_add(estimate.hot_memory.raw_tokens)
            .saturating_add(estimate.hard_reserve_tokens);

        assert!(estimate.hot_memory.raw_tokens > estimate.hot_memory.rendered_tokens);
        assert!(raw_projected_total >= estimate.compact_threshold_tokens);
        assert!(estimate.projected_total_tokens < estimate.warning_threshold_tokens);
        assert_eq!(estimate.state, BudgetState::Healthy);
    }
}
