//! Hook for generating a structured report when a timeout is reached.

use super::registry::Hook;
use super::types::{HookContext, HookEvent, HookResult};
use crate::agent::memory::{AgentMemory, MessageRole};
use serde_json::json;

/// Hook that catches Timeout events and returns a structured JSON report.
pub struct TimeoutReportHook;

impl TimeoutReportHook {
    /// Create a new timeout report hook.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for TimeoutReportHook {
    fn default() -> Self {
        Self::new()
    }
}

impl TimeoutReportHook {
    fn build_report(&self, context: &HookContext) -> String {
        let report = json!({
            "status": "timeout",
            "termination_reason": "Soft timeout reached",
            "note": "Agent did not finish the task within the time limit. Partial results included.",
            "stats": {
                "iterations": context.iteration,
                "continuation_count": context.continuation_count,
                "tokens_used": context.token_count,
                "max_tokens": context.max_tokens,
            },
            "todos": &context.todos,
            "recent_messages": summarize_recent_messages(context.memory),
        });

        serde_json::to_string_pretty(&report)
            .unwrap_or_else(|_| "{\"status\": \"timeout\"}".to_string())
    }
}

const MAX_REPORT_MESSAGES: usize = 5;
const MAX_REPORT_CHARS: usize = 500;

fn summarize_recent_messages(memory: &AgentMemory) -> Vec<serde_json::Value> {
    let mut items = Vec::new();
    for message in memory.get_messages().iter().rev().take(MAX_REPORT_MESSAGES) {
        let content = crate::utils::truncate_str(&message.content, MAX_REPORT_CHARS);
        let reasoning = message
            .reasoning
            .as_ref()
            .map(|text| crate::utils::truncate_str(text, MAX_REPORT_CHARS));

        items.push(json!({
            "role": role_label(&message.role),
            "content": content,
            "reasoning": reasoning,
            "tool_name": message.tool_name.as_deref(),
        }));
    }
    items.reverse();
    items
}

fn role_label(role: &MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    }
}

impl Hook for TimeoutReportHook {
    fn name(&self) -> &'static str {
        "TimeoutReportHook"
    }

    fn handle(&self, event: &HookEvent, context: &HookContext) -> HookResult {
        if matches!(event, HookEvent::Timeout) {
            return HookResult::Finish(self.build_report(context));
        }
        HookResult::Continue
    }
}
