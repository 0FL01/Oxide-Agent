use crate::agent::progress::{ProgressState, Step, StepStatus};

/// Render a progress state into Telegram-ready HTML.
pub fn render_progress_html(state: &ProgressState) -> String {
    let mut lines = Vec::new();

    let tokens_str = last_token_count(state)
        .map(crate::utils::format_tokens)
        .unwrap_or_else(|| "...".to_string());

    lines.push(format!(
        "🤖 <b>Oxide Agent</b> │ Iteration {}/{} │ {}",
        state.current_iteration, state.max_iterations, tokens_str
    ));
    lines.push(String::new());

    if let (Some(ref headline), Some(ref content)) =
        (&state.narrative_headline, &state.narrative_content)
    {
        lines.push(format!(
            "🧠 <b>{}</b>",
            html_escape::encode_text(&crate::utils::truncate_str(headline, 50))
        ));
        lines.push(format!(
            "   {}",
            html_escape::encode_text(&crate::utils::truncate_str(content, 150))
        ));
        lines.push(String::new());
    } else if let Some(ref thought) = state.current_thought {
        lines.push("💭 <i>Agent thoughts:</i>".to_string());
        lines.push(format!(
            "   {}",
            html_escape::encode_text(&crate::utils::truncate_str(thought, 120))
        ));
        lines.push(String::new());
    }

    if let Some(ref todos) = state.current_todos {
        if !todos.items.is_empty() {
            lines.push(format!(
                "📋 <b>Tasks [{}/{}]:</b>",
                todos.completed_count(),
                todos.items.len()
            ));
            for (i, item) in todos.items.iter().enumerate() {
                let status_icon = match item.status {
                    crate::agent::providers::TodoStatus::Completed => "✅",
                    crate::agent::providers::TodoStatus::InProgress => "🔄",
                    crate::agent::providers::TodoStatus::Pending => "⏳",
                    crate::agent::providers::TodoStatus::Cancelled => "❌",
                };
                let truncated = crate::utils::truncate_str(&item.description, 45);
                let desc = html_escape::encode_text(&truncated);
                lines.push(format!("  {} {}. {}", status_icon, i + 1, desc));
            }
        }
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

fn last_token_count(state: &ProgressState) -> Option<usize> {
    state.steps.iter().rev().find_map(|s| s.tokens)
}

#[cfg(test)]
mod tests {
    use crate::agent::loop_detection::LoopType;
    use crate::agent::progress::{AgentEvent, ProgressState};

    use super::render_progress_html;

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
}
