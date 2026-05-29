//! Search Budget Hook.
//!
//! Enforces a limit on the number of search tool calls per agent session.

use super::registry::Hook;
use super::types::{HookContext, HookEvent, HookResult};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Hook that limits the number of search tool calls.
pub struct SearchBudgetHook {
    limit: usize,
    count: AtomicUsize,
}

impl SearchBudgetHook {
    /// Create a new search budget hook with a limit.
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            count: AtomicUsize::new(0),
        }
    }

    fn is_search_tool(&self, tool_name: &str) -> bool {
        matches!(
            tool_name,
            "web_search" | "web_extract" | "duckduckgo_search" | "duckduckgo_news" | "web_markdown"
        )
    }
}

impl Hook for SearchBudgetHook {
    fn name(&self) -> &'static str {
        "search_budget"
    }

    fn handle(&self, event: &HookEvent, _context: &HookContext) -> HookResult {
        if let HookEvent::BeforeTool { tool_name, .. } = event {
            if self.is_search_tool(tool_name) {
                let current = self.count.fetch_add(1, Ordering::SeqCst) + 1;
                if current > self.limit {
                    return HookResult::Block {
                        reason: format!(
                            "Search budget exceeded ({}/{}). Please synthesize findings from existing data instead of searching more.",
                            current, self.limit
                        ),
                    };
                }
            }
        }

        HookResult::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::memory::AgentMemory;
    use crate::agent::providers::TodoList;

    #[test]
    fn counts_duckduckgo_search_against_budget() {
        let hook = SearchBudgetHook::new(1);
        let todos = TodoList::new();
        let memory = AgentMemory::new(1024);
        let context = HookContext::new(&todos, &memory, 0, 0, 1);

        let first = hook.handle(
            &HookEvent::BeforeTool {
                tool_name: "duckduckgo_search".to_string(),
                arguments: "{}".to_string(),
            },
            &context,
        );
        let second = hook.handle(
            &HookEvent::BeforeTool {
                tool_name: "duckduckgo_search".to_string(),
                arguments: "{}".to_string(),
            },
            &context,
        );

        assert!(matches!(first, HookResult::Continue));
        assert!(matches!(second, HookResult::Block { .. }));
    }

    #[test]
    fn counts_duckduckgo_news_against_budget() {
        let hook = SearchBudgetHook::new(0);
        let todos = TodoList::new();
        let memory = AgentMemory::new(1024);
        let context = HookContext::new(&todos, &memory, 0, 0, 1);

        let result = hook.handle(
            &HookEvent::BeforeTool {
                tool_name: "duckduckgo_news".to_string(),
                arguments: "{}".to_string(),
            },
            &context,
        );

        assert!(matches!(result, HookResult::Block { .. }));
    }
}
