//! Token snapshot helpers for runner progress and diagnostics.

use super::types::AgentRunnerContext;
use super::AgentRunner;
use crate::agent::compaction::{
    estimate_request_budget, CompactionPolicy, CompactionRequest, CompactionTrigger,
};
use crate::agent::progress::{AgentEvent, TokenSnapshot};
use tracing::info;

impl AgentRunner {
    pub(super) fn build_token_snapshot(
        ctx: &AgentRunnerContext<'_>,
        trigger: CompactionTrigger,
    ) -> TokenSnapshot {
        let request = CompactionRequest::new(
            trigger,
            ctx.task,
            ctx.system_prompt,
            ctx.tools,
            &ctx.config.model_name,
            ctx.config.model_max_output_tokens,
            ctx.config.is_sub_agent,
        );
        let policy = CompactionPolicy::default();
        let budget = estimate_request_budget(&policy, &request, ctx.agent);

        TokenSnapshot {
            hot_memory_tokens: budget.hot_memory.total_tokens,
            system_prompt_tokens: budget.system_prompt_tokens,
            tool_schema_tokens: budget.tool_schema_tokens,
            total_input_tokens: budget.total_input_tokens,
            reserved_output_tokens: budget.reserved_output_tokens,
            hard_reserve_tokens: budget.hard_reserve_tokens,
            projected_total_tokens: budget.projected_total_tokens,
            context_window_tokens: budget.context_window_tokens,
            headroom_tokens: budget.headroom_tokens,
            budget_state: budget.state,
            last_api_usage: ctx.agent.memory().api_usage().cloned(),
        }
    }

    pub(super) async fn emit_token_snapshot_update(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        snapshot: TokenSnapshot,
    ) {
        let Some(tx) = progress_tx else { return };
        let _ = tx.send(AgentEvent::TokenSnapshotUpdated { snapshot }).await;
    }

    pub(super) fn log_token_snapshot(
        ctx: &AgentRunnerContext<'_>,
        iteration: usize,
        phase: &str,
        snapshot: &TokenSnapshot,
    ) {
        let planned_provider = ctx
            .config
            .model_provider
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        info!(
            task_id = %ctx.task_id,
            iteration,
            phase,
            planned_provider = planned_provider,
            planned_model = %ctx.config.model_name,
            is_sub_agent = ctx.config.is_sub_agent,
            hot_memory_tokens = snapshot.hot_memory_tokens,
            system_prompt_tokens = snapshot.system_prompt_tokens,
            tool_schema_tokens = snapshot.tool_schema_tokens,
            total_input_tokens = snapshot.total_input_tokens,
            reserved_output_tokens = snapshot.reserved_output_tokens,
            projected_total_tokens = snapshot.projected_total_tokens,
            context_window_tokens = snapshot.context_window_tokens,
            headroom_tokens = snapshot.headroom_tokens,
            budget_state = ?snapshot.budget_state,
            last_api_prompt_tokens = snapshot.last_api_usage.as_ref().map(|usage| usage.prompt_tokens),
            last_api_completion_tokens = snapshot
                .last_api_usage
                .as_ref()
                .map(|usage| usage.completion_tokens),
            last_api_total_tokens = snapshot.last_api_usage.as_ref().map(|usage| usage.total_tokens),
            last_api_cached_tokens = snapshot.last_api_usage.as_ref().and_then(|usage| usage.cached_tokens),
            last_api_cache_creation_tokens = snapshot
                .last_api_usage
                .as_ref()
                .and_then(|usage| usage.cache_creation_tokens),
            "Agent request token snapshot"
        );
    }

    pub(super) fn refresh_messages_from_memory(ctx: &mut AgentRunnerContext<'_>) {
        *ctx.messages = Self::convert_memory_to_messages(ctx.agent.memory().get_messages());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::context::{AgentContext, EphemeralSession};
    use crate::agent::memory::AgentMessage;
    use crate::agent::runner::{AgentRunnerConfig, AgentRunnerContext};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[test]
    fn refresh_messages_from_memory_drops_transient_messages() {
        let tools = Vec::new();
        let mut session = EphemeralSession::new(1024);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("refresh transient context"));
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        messages.push(crate::llm::Message::system("temporary warning"));
        let mut ctx = AgentRunnerContext {
            task: "refresh transient context",
            system_prompt: "system prompt",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "refresh-transient-test",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 1, 1, 30, 256),
        };

        AgentRunner::refresh_messages_from_memory(&mut ctx);

        assert!(!ctx
            .messages
            .iter()
            .any(|message| message.role == "system" && message.content == "temporary warning"));
    }
}
