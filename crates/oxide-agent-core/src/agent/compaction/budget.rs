//! Full request budget estimation for Agent Mode compaction checkpoints.

use super::types::{
    AgentMessageKind, BudgetEstimate, BudgetState, CompactionPolicy, CompactionRequest,
    CompactionRetention, HotMemoryBudget,
};
use crate::agent::context::AgentContext;
use crate::agent::memory::AgentMessage;
use crate::llm::ToolDefinition;
use tiktoken_rs::cl100k_base;

/// Estimate the full request budget for the current checkpoint.
#[must_use]
pub fn estimate_request_budget(
    policy: &CompactionPolicy,
    request: &CompactionRequest<'_>,
    agent: &dyn AgentContext,
) -> BudgetEstimate {
    let system_prompt_tokens = estimate_text_tokens(request.system_prompt);
    let tool_schema_tokens = estimate_tool_tokens(request.tools);
    let hot_memory = estimate_hot_memory(agent.memory().get_messages());
    let loaded_skill_tokens = agent.skill_token_count();
    let context_window_tokens = agent.memory().max_tokens();
    let reserved_output_tokens =
        usize::try_from(request.model_max_output_tokens).unwrap_or(usize::MAX);
    let hard_reserve_tokens = policy.hard_reserve_tokens;
    let total_input_tokens = system_prompt_tokens
        .saturating_add(tool_schema_tokens)
        .saturating_add(hot_memory.total_tokens);
    let projected_total_tokens = total_input_tokens
        .saturating_add(reserved_output_tokens)
        .saturating_add(hard_reserve_tokens);
    let headroom_tokens = context_window_tokens.saturating_sub(projected_total_tokens);
    let warning_threshold_tokens =
        percent_of(context_window_tokens, policy.warning_threshold_percent);
    let prune_threshold_tokens = percent_of(context_window_tokens, policy.prune_threshold_percent);
    let compact_threshold_tokens =
        percent_of(context_window_tokens, policy.compact_threshold_percent);
    let state = if projected_total_tokens >= context_window_tokens {
        BudgetState::OverLimit
    } else if projected_total_tokens >= compact_threshold_tokens {
        BudgetState::ShouldCompact
    } else if projected_total_tokens >= prune_threshold_tokens {
        BudgetState::ShouldPrune
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
        loaded_skill_tokens,
        reserved_output_tokens,
        hard_reserve_tokens,
        total_input_tokens,
        projected_total_tokens,
        headroom_tokens,
        warning_threshold_tokens,
        prune_threshold_tokens,
        compact_threshold_tokens,
        state,
    }
}

fn estimate_hot_memory(messages: &[AgentMessage]) -> HotMemoryBudget {
    let mut budget = HotMemoryBudget {
        total_tokens: 0,
        total_messages: messages.len(),
        pinned_tokens: 0,
        protected_live_tokens: 0,
        prunable_artifact_tokens: 0,
        compactable_history_tokens: 0,
        skill_context_tokens: 0,
        runtime_context_tokens: 0,
    };

    for message in messages {
        let tokens = estimate_message_tokens(message);
        budget.total_tokens = budget.total_tokens.saturating_add(tokens);

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

        match message.resolved_kind() {
            AgentMessageKind::SkillContext => {
                budget.skill_context_tokens = budget.skill_context_tokens.saturating_add(tokens);
            }
            AgentMessageKind::RuntimeContext => {
                budget.runtime_context_tokens =
                    budget.runtime_context_tokens.saturating_add(tokens);
            }
            _ => {}
        }
    }

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

fn estimate_json_tokens(value: &serde_json::Value) -> usize {
    serde_json::to_string(value)
        .ok()
        .as_deref()
        .map_or(0, estimate_text_tokens)
}

fn estimate_text_tokens(text: &str) -> usize {
    cl100k_base().map_or(text.len() / 4, |bpe| {
        bpe.encode_with_special_tokens(text).len()
    })
}

const fn percent_of(value: usize, percent: u8) -> usize {
    value.saturating_mul(percent as usize) / 100
}

#[cfg(test)]
mod tests {
    use super::estimate_request_budget;
    use crate::agent::compaction::{
        BudgetState, CompactionPolicy, CompactionRequest, CompactionTrigger,
    };
    use crate::agent::memory::AgentMessage;
    use crate::agent::{AgentContext, EphemeralSession};
    use crate::llm::ToolDefinition;

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
        session.memory_mut().add_message(AgentMessage::tool(
            "call-1",
            "execute_command",
            "cargo check",
        ));
        assert!(session.register_loaded_skill("release", 321));

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
        assert!(estimate.hot_memory.total_tokens > 0);
        assert!(estimate.hot_memory.pinned_tokens > 0);
        assert!(estimate.hot_memory.prunable_artifact_tokens > 0);
        assert_eq!(estimate.loaded_skill_tokens, 321);
        assert_eq!(estimate.reserved_output_tokens, 512);
        assert_eq!(
            estimate.total_input_tokens,
            estimate.system_prompt_tokens
                + estimate.tool_schema_tokens
                + estimate.hot_memory.total_tokens
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
}
