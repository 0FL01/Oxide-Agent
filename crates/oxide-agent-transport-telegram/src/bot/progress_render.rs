use oxide_agent_core::agent::progress::{
    AgentEventSource, LlmRetryState, ProgressState, Step, StepStatus,
};

/// Render a progress state into Telegram-ready HTML.
pub fn render_progress_html(state: &ProgressState) -> String {
    let mut lines = Vec::new();

    lines.push(format!(
        "🤖 <b>Oxide Agent</b> │ Iteration {}/{}",
        state.current_iteration, state.max_iterations
    ));
    lines.push(String::new());

    push_current_thought(&mut lines, state);
    push_todos(&mut lines, state);
    push_context(&mut lines, state);

    if let Some(warning) = &state.repeated_compaction_warning {
        lines.push(format!(
            "🗜 {}",
            html_escape::encode_text(&oxide_agent_core::utils::truncate_str(warning, 180))
        ));
    }

    if let Some(notice) = &state.provider_failover_notice {
        lines.push(format!(
            "↪️ {}",
            html_escape::encode_text(&oxide_agent_core::utils::truncate_str(notice, 180))
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

    // Render LLM retry status if active
    if let Some(retry) = &state.llm_retry {
        if !lines.last().is_some_and(String::is_empty) {
            lines.push(String::new());
        }
        push_llm_retry(&mut lines, retry);
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

fn push_current_thought(lines: &mut Vec<String>, state: &ProgressState) {
    if let Some(ref thought) = state.current_thought {
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
            oxide_agent_core::agent::providers::TodoStatus::BlockedOnUser => "⏸️",
            oxide_agent_core::agent::providers::TodoStatus::Pending => "⏳",
            oxide_agent_core::agent::providers::TodoStatus::Cancelled => "❌",
        };
        let truncated = oxide_agent_core::utils::truncate_str(&item.description, 45);
        let desc = html_escape::encode_text(&truncated);
        lines.push(format!("  {} {}. {}", status_icon, i + 1, desc));
    }
}

fn push_llm_retry(lines: &mut Vec<String>, retry: &LlmRetryState) {
    // Format wait time display
    let wait_display = if let Some(secs) = retry.wait_secs {
        if secs >= 60 {
            format!(" (~{}m {}s)", secs / 60, secs % 60)
        } else {
            format!(" (~{secs}s)")
        }
    } else {
        String::new()
    };

    let title = if retry.error_class.is_some() {
        "LLM retrying"
    } else {
        "Rate limited"
    };
    let provider = match retry.error_class.as_deref() {
        Some(error_class) => format!("{} [{}]", retry.provider, error_class),
        None => retry.provider.clone(),
    };
    let attempt_display = if retry.unbounded {
        format!("Attempt {} - retrying{}", retry.attempt, wait_display)
    } else {
        format!(
            "Attempt {}/{} - retrying{}",
            retry.attempt, retry.max_attempts, wait_display
        )
    };

    lines.push(format!(
        "🔄 <b>{}</b> ({})",
        title,
        html_escape::encode_text(&provider)
    ));
    lines.push(format!("   {attempt_display}"));
}

fn push_context(lines: &mut Vec<String>, state: &ProgressState) {
    if state.latest_token_snapshot.is_none()
        && state.last_compaction_status.is_none()
        && state.last_history_repair_status.is_none()
    {
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
    if let Some(status) = &state.last_history_repair_status {
        lines.push(format!(
            "   {}",
            html_escape::encode_text(&oxide_agent_core::utils::truncate_str(status, 160))
        ));
    }
}

fn format_grouped_steps(state: &ProgressState) -> Vec<String> {
    use std::collections::HashMap;

    let mut completed_counts: HashMap<&str, usize> = HashMap::new();
    let mut failed_counts: HashMap<&str, usize> = HashMap::new();

    for step in &state.steps {
        if let Some(ref tool_name) = step.tool_name {
            match step.status {
                StepStatus::Completed => {
                    *completed_counts.entry(tool_name.as_str()).or_insert(0) += 1;
                }
                StepStatus::Failed => {
                    *failed_counts.entry(tool_name.as_str()).or_insert(0) += 1;
                }
                StepStatus::Pending | StepStatus::InProgress => {}
            }
        }
    }

    let mut sorted_completed: Vec<_> = completed_counts.into_iter().collect();
    sorted_completed.sort_by(|a, b| b.1.cmp(&a.1));

    let mut sorted_failed: Vec<_> = failed_counts.into_iter().collect();
    sorted_failed.sort_by(|a, b| b.1.cmp(&a.1));

    sorted_completed
        .into_iter()
        .map(|(name, count)| {
            if count > 1 {
                format!("  ✅ {} ×{}", name, count)
            } else {
                format!("  ✅ {}", name)
            }
        })
        .chain(sorted_failed.into_iter().map(|(name, count)| {
            if count > 1 {
                format!("  ❌ {} ×{}", name, count)
            } else {
                format!("  ❌ {}", name)
            }
        }))
        .collect()
}

fn current_step(state: &ProgressState) -> Option<&Step> {
    state
        .steps
        .iter()
        .rfind(|s| s.status == StepStatus::InProgress)
}

fn format_snapshot_summary(snapshot: &oxide_agent_core::agent::progress::TokenSnapshot) -> String {
    format!(
        "flow {} | prompt {} | tools {}",
        oxide_agent_core::utils::format_tokens(snapshot.hot_memory_tokens),
        oxide_agent_core::utils::format_tokens(snapshot.system_prompt_tokens),
        oxide_agent_core::utils::format_tokens(snapshot.tool_schema_tokens),
    )
}

fn format_budget_breakdown(snapshot: &oxide_agent_core::agent::progress::TokenSnapshot) -> String {
    format!(
        "input {} + reserve {} = projected {} | {} free",
        oxide_agent_core::utils::format_tokens(snapshot.total_input_tokens),
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
        oxide_agent_core::agent::compaction::BudgetState::ShouldCompact => "compact soon",
        oxide_agent_core::agent::compaction::BudgetState::OverLimit => "over limit",
    }
}

#[cfg(test)]
mod tests {
    use oxide_agent_core::agent::compaction::BudgetState;
    use oxide_agent_core::agent::loop_detection::LoopType;
    use oxide_agent_core::agent::progress::{AgentEvent, ProgressState, TokenSnapshot};
    use oxide_agent_core::agent::providers::{TodoItem, TodoList, TodoStatus};
    use oxide_agent_core::llm::TokenUsage;

    use super::render_progress_html;

    fn sample_snapshot() -> TokenSnapshot {
        TokenSnapshot {
            hot_memory_tokens: 5_700,
            system_prompt_tokens: 1_200,
            tool_schema_tokens: 1_100,
            total_input_tokens: 8_000,
            reserved_output_tokens: 0,
            hard_reserve_tokens: 8_192,
            projected_total_tokens: 16_192,
            context_window_tokens: 200_000,
            headroom_tokens: 183_808,
            budget_state: BudgetState::Healthy,
            last_api_usage: Some(TokenUsage {
                prompt_tokens: 15_200,
                completion_tokens: 800,
                total_tokens: 16_000,
                ..TokenUsage::default()
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
        assert!(output.contains("flow 5.7k | prompt 1.2k | tools 1.1k"));
        assert!(output.contains("input 8k + reserve 8.2k = projected 16k | 184k free"));
        assert!(output.contains("Budget: healthy"));
        assert!(!output.contains("Last API usage:"));
    }

    #[test]
    fn renders_grouped_steps_and_current_step() {
        let mut state = ProgressState::new(100);

        state.update(AgentEvent::ToolCall {
            id: "tool-1".to_string(),
            source: Default::default(),
            name: "web_search".to_string(),
            input: "q1".to_string(),
            command_preview: None,
        });
        state.update(AgentEvent::ToolResult {
            id: "tool-1".to_string(),
            source: Default::default(),
            name: "web_search".to_string(),
            output: "result1".to_string(),
            success: true,
        });
        state.update(AgentEvent::ToolCall {
            id: "tool-2".to_string(),
            source: Default::default(),
            name: "web_search".to_string(),
            input: "q2".to_string(),
            command_preview: None,
        });
        state.update(AgentEvent::ToolResult {
            id: "tool-2".to_string(),
            source: Default::default(),
            name: "web_search".to_string(),
            output: "result2".to_string(),
            success: true,
        });
        state.update(AgentEvent::ToolCall {
            id: "tool-3".to_string(),
            source: Default::default(),
            name: "execute_command".to_string(),
            input: "{}".to_string(),
            command_preview: Some("ls -la".to_string()),
        });

        let output = render_progress_html(&state);

        assert!(output.contains("✅ web_search ×2"));
        assert!(output.contains("⏳ 🔧 ls -la"));
    }

    #[test]
    fn renders_todos_after_todos_updated_event() {
        let mut state = ProgressState::new(10);

        state.update(AgentEvent::TodosUpdated {
            source: AgentEventSource::Root,
            todos: TodoList {
                items: vec![
                    TodoItem {
                        description: "Finished task".to_string(),
                        status: TodoStatus::Completed,
                    },
                    TodoItem {
                        description: "Active task".to_string(),
                        status: TodoStatus::InProgress,
                    },
                ],
                updated_at: None,
            },
        });

        let output = render_progress_html(&state);

        assert!(output.contains("📋 <b>Tasks [1/2]:</b>"));
        assert!(output.contains("✅ 1. Finished task"));
        assert!(output.contains("🔄 2. Active task"));
    }

    #[test]
    fn renders_failed_tools_separately() {
        let mut state = ProgressState::new(100);

        state.update(AgentEvent::ToolCall {
            id: "tool-1".to_string(),
            source: Default::default(),
            name: "text_to_speech_en_file".to_string(),
            input: "{}".to_string(),
            command_preview: None,
        });
        state.update(AgentEvent::ToolResult {
            id: "tool-1".to_string(),
            source: Default::default(),
            name: "text_to_speech_en_file".to_string(),
            output: "Tool execution error: boom".to_string(),
            success: false,
        });

        let output = render_progress_html(&state);

        assert!(output.contains("❌ text_to_speech_en_file"));
        assert!(!output.contains("✅ text_to_speech_en_file"));
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
            id: "tool-1".to_string(),
            source: Default::default(),
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
    fn renders_runtime_compaction_status() {
        let mut state = ProgressState::new(10);

        state.update(AgentEvent::RuntimeCompactionStarted {
            reason: oxide_agent_core::agent::compaction::CompactionReason::Manual,
            phase: oxide_agent_core::agent::compaction::CompactionPhase::Manual,
            backend: oxide_agent_core::agent::compaction::CompactionBackend::LocalLlmSummary,
            provider: None,
            route: None,
            token_before: 2400,
            history_items_before: 12,
        });
        state.update(AgentEvent::RuntimeCompactionCompleted {
            reason: oxide_agent_core::agent::compaction::CompactionReason::Manual,
            phase: oxide_agent_core::agent::compaction::CompactionPhase::Manual,
            backend: oxide_agent_core::agent::compaction::CompactionBackend::LocalLlmSummary,
            provider: "mock".to_string(),
            route: "compact".to_string(),
            token_before: 2400,
            token_after: 900,
            history_items_before: 12,
            history_items_after: 4,
            generation: 1,
            repair_applied: false,
        });

        let output = render_progress_html(&state);

        assert!(output.contains("<b>Context:</b>"));
        assert!(output.contains("Compaction: compacted history"));
        assert!(output.contains("manual/manual"));
        assert!(output.contains("local_llm_summary"));
        assert!(output.contains("mock/compact"));
    }

    #[test]
    fn renders_rate_limit_retry() {
        let mut state = ProgressState::new(10);

        state.update(AgentEvent::RateLimitRetrying {
            attempt: 2,
            max_attempts: 5,
            unbounded: false,
            wait_secs: Some(30),
            provider: "mistral".to_string(),
        });

        let output = render_progress_html(&state);

        assert!(output.contains("🔄 <b>Rate limited</b>"));
        assert!(output.contains("mistral"));
        assert!(output.contains("Attempt 2/5"));
        assert!(output.contains("~30s"));
    }

    #[test]
    fn renders_rate_limit_retry_long_wait() {
        let mut state = ProgressState::new(10);

        state.update(AgentEvent::RateLimitRetrying {
            attempt: 1,
            max_attempts: 5,
            unbounded: false,
            wait_secs: Some(125),
            provider: "openrouter".to_string(),
        });

        let output = render_progress_html(&state);

        assert!(output.contains("🔄 <b>Rate limited</b>"));
        assert!(output.contains("openrouter"));
        assert!(output.contains("Attempt 1/5"));
        assert!(output.contains("~2m 5s"));
    }

    #[test]
    fn renders_unbounded_llm_retry_without_attempt_cap() {
        let mut state = ProgressState::new(10);

        state.update(AgentEvent::LlmRetrying {
            attempt: 16,
            max_attempts: 16,
            unbounded: true,
            wait_secs: Some(30),
            provider: "opencode-go".to_string(),
            error_class: "server_error".to_string(),
        });

        let output = render_progress_html(&state);

        assert!(output.contains("🔄 <b>LLM retrying</b>"));
        assert!(output.contains("opencode-go [server_error]"));
        assert!(output.contains("Attempt 16 - retrying"));
        assert!(!output.contains("Attempt 16/16"));
        assert!(!output.contains("Rate limited"));
    }
}
