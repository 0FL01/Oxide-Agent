use oxide_agent_core::agent::progress::{LlmRetryState, ProgressState, Step, StepStatus};
use oxide_agent_core::agent::providers::TodoStatus;

const THOUGHT_CHARS: usize = 240;
const ITEM_CHARS: usize = 120;
const STATUS_CHARS: usize = 120;

/// Render one compact, intrinsically bounded Telegram progress message.
/// Terminal text is deliberately excluded: `AgentExecutionOutcome` owns the
/// final replacement of the progress anchor.
pub fn render_progress_html(state: &ProgressState) -> String {
    let mut lines = vec![format!(
        "🤖 <b>Oxide Agent</b> │ Iteration {}/{}",
        state.current_iteration, state.max_iterations
    )];

    if !push_browser_milestone(&mut lines, state) {
        push_current_thought(&mut lines, state);
    }
    push_current_todo(&mut lines, state);
    push_current_step(&mut lines, state);
    if let Some(retry) = &state.llm_retry {
        push_llm_retry(&mut lines, retry);
    }
    if let Some(status) = state
        .last_compaction_status
        .as_deref()
        .or(state.last_history_repair_status.as_deref())
    {
        lines.push(format!("🗜 {}", escaped(status, STATUS_CHARS)));
    }

    lines.join("\n")
}

fn push_current_thought(lines: &mut Vec<String>, state: &ProgressState) {
    let Some(thought) = state.current_thought.as_deref() else {
        return;
    };
    if thought.starts_with("Browser") {
        return;
    }
    lines.push(format!("💭 {}", escaped(thought, THOUGHT_CHARS)));
}

fn push_current_todo(lines: &mut Vec<String>, state: &ProgressState) {
    let Some(todos) = state.current_todos.as_ref() else {
        return;
    };
    let current = todos
        .items
        .iter()
        .find(|item| item.status == TodoStatus::InProgress)
        .or_else(|| {
            todos
                .items
                .iter()
                .find(|item| item.status == TodoStatus::BlockedOnUser)
        });
    let Some(current) = current else {
        return;
    };
    lines.push(format!(
        "📋 {}/{} │ {}",
        todos.completed_count(),
        todos.items.len(),
        escaped(&current.description, ITEM_CHARS)
    ));
}

fn push_current_step(lines: &mut Vec<String>, state: &ProgressState) {
    let Some(step) = current_step(state) else {
        return;
    };
    lines.push(format!("⏳ {}", escaped(&step.description, ITEM_CHARS)));
}

fn push_llm_retry(lines: &mut Vec<String>, retry: &LlmRetryState) {
    let title = if retry.error_class.is_some() {
        "LLM retry"
    } else {
        "Rate limit"
    };
    let attempts = if retry.unbounded {
        format!("attempt {}", retry.attempt)
    } else {
        format!("attempt {}/{}", retry.attempt, retry.max_attempts)
    };
    let wait = retry.wait_secs.map_or_else(String::new, |seconds| {
        if seconds >= 60 {
            format!(", wait {}m {}s", seconds / 60, seconds % 60)
        } else {
            format!(", wait {seconds}s")
        }
    });
    let class = retry
        .error_class
        .as_deref()
        .map(|class| format!(" [{}]", escaped(class, 60)))
        .unwrap_or_default();
    lines.push(format!(
        "🔄 <b>{title}</b> │ {}{class} │ {attempts}{wait}",
        escaped(&retry.provider, 80)
    ));
}

fn current_step(state: &ProgressState) -> Option<&Step> {
    state
        .steps
        .iter()
        .rfind(|step| step.status == StepStatus::InProgress)
}

fn escaped(value: &str, max_chars: usize) -> String {
    html_escape::encode_text(&oxide_agent_core::utils::truncate_str(value, max_chars)).into_owned()
}

fn push_browser_milestone(lines: &mut Vec<String>, state: &ProgressState) -> bool {
    let Some(thought) = state.current_thought.as_deref() else {
        return false;
    };
    let Some(milestone) = BrowserMilestone::parse(thought) else {
        return false;
    };
    lines.push(format!(
        "🌐 <b>Browser</b> │ {}",
        escaped(&milestone.summary(), 180)
    ));
    if let Some(reason) = milestone.blocked_reason() {
        lines.push(format!("⏸ {}", escaped(reason, 180)));
    }
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserMilestoneKind {
    Action,
    Verification,
    Recovery,
}

impl BrowserMilestoneKind {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "BrowserAction" => Some(Self::Action),
            "BrowserVerification" => Some(Self::Verification),
            "BrowserRecovery" => Some(Self::Recovery),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BrowserMilestone<'a> {
    kind: BrowserMilestoneKind,
    session_id: Option<&'a str>,
    action_seq: Option<&'a str>,
    status: Option<&'a str>,
    action_kind: Option<&'a str>,
}

impl<'a> BrowserMilestone<'a> {
    fn parse(summary: &'a str) -> Option<Self> {
        let (kind, rest) = summary.split_once(' ')?;
        let mut milestone = Self {
            kind: BrowserMilestoneKind::parse(kind)?,
            session_id: None,
            action_seq: None,
            status: None,
            action_kind: None,
        };
        for part in rest.split_whitespace() {
            let Some((key, value)) = part.split_once('=') else {
                continue;
            };
            match key {
                "session_id" => milestone.session_id = Some(value),
                "action_seq" => milestone.action_seq = Some(value),
                "status" => milestone.status = Some(value),
                "kind" => milestone.action_kind = Some(value),
                _ => {}
            }
        }
        Some(milestone)
    }

    fn summary(&self) -> String {
        let seq = self
            .action_seq
            .map(|value| format!(" step {value}"))
            .unwrap_or_default();
        let session = self
            .session_id
            .map(|value| format!(" ({value})"))
            .unwrap_or_default();
        match self.kind {
            BrowserMilestoneKind::Action => format!(
                "Action{seq}: {}{session}",
                self.action_kind.unwrap_or("planned")
            ),
            BrowserMilestoneKind::Verification => format!(
                "Verification{seq}: {}{session}",
                self.status.unwrap_or("unknown")
            ),
            BrowserMilestoneKind::Recovery => format!(
                "Recovery{seq}: {} {}{session}",
                self.status.unwrap_or("unknown"),
                self.action_kind.unwrap_or("unknown")
            ),
        }
    }

    fn blocked_reason(&self) -> Option<&'static str> {
        match self.kind {
            BrowserMilestoneKind::Verification
                if matches!(
                    self.status,
                    Some("NeedsUser" | "VerificationFailed" | "Timeout")
                ) =>
            {
                Some("browser progress stopped; user input or diagnostics are required")
            }
            BrowserMilestoneKind::Recovery
                if matches!(self.status, Some("SafeStopped" | "RepeatedLoopStopped")) =>
            {
                Some("bounded recovery could not continue safely")
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use oxide_agent_core::agent::progress::{AgentEvent, AgentEventSource, ProgressState};
    use oxide_agent_core::agent::providers::{TodoItem, TodoList, TodoStatus};

    use super::render_progress_html;

    #[test]
    fn progress_is_live_only_even_after_terminal_events() {
        let mut state = ProgressState::new(5);
        state.update(AgentEvent::Error("boom".to_string()));
        state.update(AgentEvent::Finished);

        let output = render_progress_html(&state);

        assert!(output.contains("Iteration 0/5"));
        assert!(!output.contains("Task completed"));
        assert!(!output.contains("boom"));
    }

    #[test]
    fn progress_is_bounded_and_escapes_dynamic_content() {
        let hostile = "<script>&".repeat(1000);
        let mut state = ProgressState::new(100);
        state.current_thought = Some(hostile.clone());
        state.current_todos = Some(TodoList {
            items: (0..200)
                .map(|index| TodoItem {
                    description: format!("{hostile}-{index}"),
                    status: if index == 150 {
                        TodoStatus::InProgress
                    } else {
                        TodoStatus::Pending
                    },
                })
                .collect(),
            updated_at: None,
        });

        let output = render_progress_html(&state);

        assert!(output.chars().count() < 4000);
        assert!(!output.contains("<script>"));
        assert!(output.contains("&lt;script&gt;&amp;"));
        assert!(output.contains("0/200"));
        assert!(!output.contains("-149"));
    }

    #[test]
    fn renders_current_operation_and_retry_without_tool_history() {
        let mut state = ProgressState::new(10);
        state.update(AgentEvent::ToolCall {
            id: "tool-1".to_string(),
            source: Default::default(),
            name: "web_search".to_string(),
            input: "query".to_string(),
            command_preview: None,
        });
        state.update(AgentEvent::RateLimitRetrying {
            attempt: 2,
            max_attempts: 5,
            unbounded: false,
            wait_secs: Some(30),
            provider: "openrouter".to_string(),
        });

        let output = render_progress_html(&state);

        assert!(output.contains("⏳"));
        assert!(output.contains("Rate limit"));
        assert!(output.contains("attempt 2/5, wait 30s"));
        assert!(!output.contains("Tools:"));
    }

    #[test]
    fn renders_browser_milestone_without_generic_thought() {
        let mut state = ProgressState::new(10);
        state.update(AgentEvent::Reasoning {
            source: AgentEventSource::Root,
            summary: "BrowserRecovery session_id=browser-1 action_seq=7 status=SafeStopped kind=LowConfidence"
                .to_string(),
        });

        let output = render_progress_html(&state);

        assert!(output.contains("<b>Browser</b>"));
        assert!(output.contains("Recovery step 7: SafeStopped LowConfidence"));
        assert!(output.contains("bounded recovery could not continue safely"));
        assert!(!output.contains("💭"));
    }
}
