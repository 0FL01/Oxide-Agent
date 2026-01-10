//! Delegation Guard Hook.
//!
//! Prevents the main agent from delegating high-level cognitive tasks (analysis, reasoning)
//! to sub-agents, forcing it to use sub-agents only for mechanical tasks (retrieval, raw data).

use super::registry::Hook;
use super::types::{HookContext, HookEvent, HookResult};
use serde_json::Value;

/// Hook that blocks delegation of analytical tasks.
pub struct DelegationGuardHook {
    forbidden_keywords: Vec<&'static str>,
}

impl DelegationGuardHook {
    /// Create a new delegation guard hook.
    #[must_use]
    pub fn new() -> Self {
        Self {
            forbidden_keywords: vec![
                // English
                "why",
                "analyze",
                "explain",
                "review",
                "opinion",
                "reasoning",
                "architect",
                "evaluate",
                "compare",
                // Russian
                "почему",
                "анализ",
                "объясни",
                "обзор",
                "мнение",
                "архитект",
                "оцени",
                "сравни",
                "выясни",    // "find out" - often implies investigation + reasoning
                "эффективн", // "effective" - implies quality judgment
            ],
        }
    }

    fn check_task(&self, task: &str) -> Option<String> {
        let normalized = task.to_lowercase();
        for keyword in &self.forbidden_keywords {
            if normalized.contains(keyword) {
                return Some(keyword.to_string());
            }
        }
        None
    }
}

impl Default for DelegationGuardHook {
    fn default() -> Self {
        Self::new()
    }
}

impl Hook for DelegationGuardHook {
    fn name(&self) -> &'static str {
        "delegation_guard"
    }

    fn handle(&self, event: &HookEvent, _context: &HookContext) -> HookResult {
        let HookEvent::BeforeTool {
            tool_name,
            arguments,
        } = event
        else {
            return HookResult::Continue;
        };

        if tool_name != "delegate_to_sub_agent" {
            return HookResult::Continue;
        }

        // Parse arguments to get the 'task' field
        let task = match serde_json::from_str::<Value>(arguments) {
            Ok(json) => json
                .get("task")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            Err(_) => return HookResult::Continue, // Let the tool fail naturally on bad JSON
        };

        if let Some(keyword) = self.check_task(&task) {
            return HookResult::Block {
                reason: format!(
                    "⛔ Delegation Blocked: The task contains an analytical keyword ('{}'). \
                     Sub-agents are restricted to raw data retrieval (cloning, grep, list files). \
                     Please split the task: delegate the retrieval, but perform the analysis yourself.",
                    keyword
                ),
            };
        }

        HookResult::Continue
    }
}
