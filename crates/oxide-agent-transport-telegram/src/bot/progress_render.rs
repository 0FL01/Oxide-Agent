use oxide_agent_core::agent::progress::{ProgressState, Step, StepStatus};

/// Render a progress state into Telegram-ready HTML.
pub fn render_progress_html(state: &ProgressState) -> String {
    let mut lines = Vec::new();

    let tokens_str = state
        .latest_token_snapshot
        .as_ref()
        .map(format_header_tokens)
        .unwrap_or_else(|| "...".to_string());

    lines.push(format!(
        "🤖 <b>Oxide Agent</b> │ Iteration {}/{} │ {}",
        state.current_iteration, state.max_iterations, tokens_str
    ));
    lines.push(String::new());

    push_narrative_or_thought(&mut lines, state);
    push_todos(&mut lines, state);
    push_context(&mut lines, state);

    if let Some(warning) = &state.repeated_compaction_warning {
        lines.push(format!(
            "🗜 {}",
            html_escape::encode_text(&oxide_agent_core::utils::truncate_str(warning, 180))
        ));
    }

    let grouped = format_grouped_steps(state);
    if !grouped.is_empty() {
        lines.push(String::new());
        lines.push("🔧 <b>Tools:</b>".to_string());
        lines.extend(grouped);
    }

    if let Some(step) = current_step(state) {
        if !lines.last().is_some_and(String::is_empty) {
            lines.push(String::new());
        }
        lines.push(format!(
            "⏳ {}",
            html_escape::encode_text(&step.description)
        ));
    }

    if state.is_finished {
        lines.push("\n✅ <b>Task completed</b>".to_string());
    } else if let Some(ref e) = state.error {
        lines.push(format!(
            "\n❌ <b>Error:</b> {}",
            html_escape::encode_text(e)
        ));
    }

    lines.join("\n")
}

fn push_narrative_or_thought(lines: &mut Vec<String>, state: &ProgressState) {
    if let (Some(ref headline), Some(ref content)) =
        (&state.narrative_headline, &state.narrative_content)
    {
        lines.push(format!(
            "🧠 <b>{}</b>",
            html_escape::encode_text(&oxide_agent_core::utils::truncate_str(headline, 50))
        ));
        lines.push(format!(
            "   {}",
            html_escape::encode_text(&oxide_agent_core::utils::truncate_str(content, 150))
        ));
        lines.push(String::new());
    } else if let Some(ref thought) = state.current_thought {
        lines.push("💭 <i>Agent thoughts:</i>".to_string());
        lines.push(format!(
            "   {}",
            html_escape::encode_text(&oxide_agent_core::utils::truncate_str(thought, 120))
        ));
        lines.push(String::new());
    }
}

fn push_todos(lines: &mut Vec<String>, state: &ProgressState) {
    let Some(ref todos) = state.current_todos else {
        return;
    };
    if todos.items.is_empty() {
        return;
    }

    lines.push(format!(
        "📋 <b>Tasks [{}/{}]:</b>",
        todos.completed_count(),
        todos.items.len()
    ));
    for (i, item) in todos.items.iter().enumerate() {
        let status_icon = match item.status {
            oxide_agent_core::agent::providers::TodoStatus::Completed => "✅",
            oxide_agent_core::agent::providers::TodoStatus::InProgress => "🔄",
            oxide_agent_core::agent::providers::TodoStatus::Pending => "⏳",
            oxide_agent_core::agent::providers::TodoStatus::Cancelled => "❌",
        };
        let truncated = oxide_agent_core::utils::truncate_str(&item.description, 45);
        let desc = html_escape::encode_text(&truncated);
        lines.push(format!("  {} {}. {}", status_icon, i + 1, desc));
    }
}

fn push_context(lines: &mut Vec<String>, state: &ProgressState) {
    if state.latest_token_snapshot.is_none() && state.last_compaction_status.is_none() {
        return;
    }
    if !lines.last().is_some_and(String::is_empty) {
        lines.push(String::new());
    }

    lines.push("🗜 <b>Context:</b>".to_string());
    if let Some(snapshot) = &state.latest_token_snapshot {
        push_budget_breakdown(lines, snapshot);
    }
    if let Some(status) = &state.last_compaction_status {
        lines.push(format!(
            "   {}",
            html_escape::encode_text(&oxide_agent_core::utils::truncate_str(status, 160))
        ));
    }
}

fn format_grouped_steps(state: &ProgressState) -> Vec<String> {
    use std::collections::HashMap;

    let mut counts: HashMap<&str, usize> = HashMap::new();

    for step in &state.steps {
        if step.status == StepStatus::Completed {
            if let Some(ref tool_name) = step.tool_name {
                *counts.entry(tool_name.as_str()).or_insert(0) += 1;
            }
        }
    }

    let mut sorted: Vec<_> = counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));

    sorted
        .into_iter()
        .map(|(name, count)| {
            if count > 1 {
                format!("  ✅ {} ×{}", name, count)
            } else {
                format!("  ✅ {}", name)
            }
        })
        .collect()
}

fn current_step(state: &ProgressState) -> Option<&Step> {
    state
        .steps
        .iter()
        .rfind(|s| s.status == StepStatus::InProgress)
}

fn format_header_tokens(snapshot: &oxide_agent_core::agent::progress::TokenSnapshot) -> String {
    format!(
        "ctx {} + p{} + t{} + s{} / {}",
        oxide_agent_core::utils::format_tokens(snapshot.hot_memory_tokens),
        oxide_agent_core::utils::format_tokens(snapshot.system_prompt_tokens),
        oxide_agent_core::utils::format_tokens(snapshot.tool_schema_tokens),
        oxide_agent_core::utils::format_tokens(snapshot.loaded_skill_tokens),
        oxide_agent_core::utils::format_tokens(snapshot.context_window_tokens)
    )
}

fn format_snapshot_summary(snapshot: &oxide_agent_core::agent::progress::TokenSnapshot) -> String {
    format!(
        "flow {} | prompt {} | tools {} | skills {}",
        oxide_agent_core::utils::format_tokens(snapshot.hot_memory_tokens),
        oxide_agent_core::utils::format_tokens(snapshot.system_prompt_tokens),
        oxide_agent_core::utils::format_tokens(snapshot.tool_schema_tokens),
        oxide_agent_core::utils::format_tokens(snapshot.loaded_skill_tokens),
    )
}

fn format_budget_breakdown(snapshot: &oxide_agent_core::agent::progress::TokenSnapshot) -> String {
    format!(
        "📤 {} + 🛡️ {} = 📊 {} | 🟢 {} free",
        oxide_agent_core::utils::format_tokens(snapshot.reserved_output_tokens),
        oxide_agent_core::utils::format_tokens(snapshot.hard_reserve_tokens),
        oxide_agent_core::utils::format_tokens(snapshot.projected_total_tokens),
        oxide_agent_core::utils::format_tokens(snapshot.headroom_tokens),
    )
}

fn format_budget_status(snapshot: &oxide_agent_core::agent::progress::TokenSnapshot) -> String {
    format!("Budget: {}", budget_state_label(snapshot.budget_state))
}

fn push_budget_breakdown(
    lines: &mut Vec<String>,
    snapshot: &oxide_agent_core::agent::progress::TokenSnapshot,
) {
    lines.push(format!(
        "   {}",
        html_escape::encode_text(&format_snapshot_summary(snapshot))
    ));
    lines.push(format!(
        "   {}",
        html_escape::encode_text(&format_budget_breakdown(snapshot))
    ));
    lines.push(format!(
        "   {}",
        html_escape::encode_text(&format_budget_status(snapshot))
    ));
}

fn budget_state_label(state: oxide_agent_core::agent::compaction::BudgetState) -> &'static str {
    match state {
        oxide_agent_core::agent::compaction::BudgetState::Healthy => "healthy",
        oxide_agent_core::agent::compaction::BudgetState::Warning => "warning",
        oxide_agent_core::agent::compaction::BudgetState::ShouldPrune => "prune soon",
        oxide_agent_core::agent::compaction::BudgetState::ShouldCompact => "compact soon",
        oxide_agent_core::agent::compaction::BudgetState::OverLimit => "over limit",
    }
}

#[cfg(test)]
mod tests {
    use oxide_agent_core::agent::compaction::BudgetState;
    use oxide_agent_core::agent::loop_detection::LoopType;
    use oxide_agent_core::agent::progress::{AgentEvent, ProgressState, TokenSnapshot};
    use oxide_agent_core::llm::TokenUsage;

    use super::render_progress_html;

    fn sample_snapshot() -> TokenSnapshot {
        TokenSnapshot {
            hot_memory_tokens: 5_700,
            system_prompt_tokens: 1_200,
            tool_schema_tokens: 1_100,
            loaded_skill_tokens: 0,
            total_input_tokens: 8_000,
            reserved_output_tokens: 8_000,
            hard_reserve_tokens: 8_192,
            projected_total_tokens: 24_192,
            context_window_tokens: 200_000,
            headroom_tokens: 175_808,
            budget_state: BudgetState::Healthy,
            last_api_usage: Some(TokenUsage {
                prompt_tokens: 15_200,
                completion_tokens: 800,
                total_tokens: 16_000,
            }),
        }
    }

    #[test]
    fn renders_minimal_state_header() {
        let state = ProgressState::new(5);
        let output = render_progress_html(&state);

        assert!(output.contains("🤖 <b>Oxide Agent</b>"));
        assert!(output.contains("Iteration 0/5"));
        assert!(!output.contains("Task completed"));
        assert!(!output.contains("<b>Error:</b>"));
    }

    #[test]
    fn renders_projected_budget_without_api_usage() {
        let mut state = ProgressState::new(5);
        state.update(AgentEvent::Thinking {
            snapshot: sample_snapshot(),
        });

        let output = render_progress_html(&state);

        assert!(output.contains("Iteration 1/5"));
        assert!(output.contains("ctx 5.7k + p1.2k + t1.1k + s0 / 200k"));
        assert!(output.contains("flow 5.7k | prompt 1.2k | tools 1.1k | skills 0"));
        assert!(output.contains("📤 8k + 🛡️ 8.2k = 📊 24k | 🟢 176k free"));
        assert!(output.contains("Budget: healthy"));
        assert!(!output.contains("Last API usage:"));
    }

    #[test]
    fn renders_grouped_steps_and_current_step() {
        let mut state = ProgressState::new(100);

        state.update(AgentEvent::ToolCall {
            name: "web_search".to_string(),
            input: "q1".to_string(),
            command_preview: None,
        });
        state.update(AgentEvent::ToolResult {
            name: "web_search".to_string(),
            output: "result1".to_string(),
        });
        state.update(AgentEvent::ToolCall {
            name: "web_search".to_string(),
            input: "q2".to_string(),
            command_preview: None,
        });
        state.update(AgentEvent::ToolResult {
            name: "web_search".to_string(),
            output: "result2".to_string(),
        });
        state.update(AgentEvent::ToolCall {
            name: "execute_command".to_string(),
            input: "{}".to_string(),
            command_preview: Some("ls -la".to_string()),
        });

        let output = render_progress_html(&state);

        assert!(output.contains("✅ web_search ×2"));
        assert!(output.contains("⏳ 🔧 ls -la"));
    }

    #[test]
    fn renders_loop_error() {
        let mut state = ProgressState::new(10);

        state.update(AgentEvent::LoopDetected {
            loop_type: LoopType::ToolCallLoop,
            iteration: 3,
        });

        let output = render_progress_html(&state);

        assert!(output.contains("❌ <b>Error:</b>"));
        assert!(output.contains("Loop detected"));
    }

    #[test]
    fn renders_waiting_for_approval_step() {
        let mut state = ProgressState::new(10);

        state.update(AgentEvent::ToolCall {
            name: "ssh_sudo_exec".to_string(),
            input: "{}".to_string(),
            command_preview: None,
        });
        state.update(AgentEvent::WaitingForApproval {
            tool_name: "ssh_sudo_exec".to_string(),
            target_name: "n-de1".to_string(),
            summary: "sudo exec on n-de1: journalctl -p err -n 10 --no-pager".to_string(),
        });

        let output = render_progress_html(&state);

        assert!(output.contains("SSH approval pending for n-de1"));
        assert!(!output.contains("Execution: ssh_sudo_exec"));
    }

    #[test]
    fn renders_compaction_status_and_warning() {
        let mut state = ProgressState::new(10);

        state.update(AgentEvent::CompactionStarted {
            trigger: oxide_agent_core::agent::CompactionTrigger::Manual,
        });
        state.update(AgentEvent::CompactionCompleted {
            trigger: oxide_agent_core::agent::CompactionTrigger::Manual,
            applied: true,
            externalized_count: 1,
            pruned_count: 2,
            reclaimed_tokens: 1800,
            archived_chunk_count: 1,
            summary_updated: true,
        });
        state.update(AgentEvent::RepeatedCompactionWarning {
            kind: oxide_agent_core::agent::RepeatedCompactionKind::Compaction,
            count: 2,
        });

        let output = render_progress_html(&state);

        assert!(output.contains("<b>Context:</b>"));
        assert!(output.contains("Compaction: refreshed summary and rebuilt active context"));
        assert!(output.contains("🗜 History compaction: 2x"));
    }
}
