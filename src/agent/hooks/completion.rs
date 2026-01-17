//! Completion Check Hook - ensures all todos are completed before finishing
//!
//! This hook forces the agent to continue iterating if there are
//! incomplete todos in the list.

use super::registry::Hook;
use super::types::{HookContext, HookEvent, HookResult};
use tracing::info;

/// Hook that checks if all todos are completed
///
/// When the agent tries to finish (`AfterAgent` event), this hook checks
/// the todo list. If there are incomplete items and we haven't exceeded
/// the continuation limit, it forces another iteration.
pub struct CompletionCheckHook;

impl CompletionCheckHook {
    /// Create a new completion check hook
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for CompletionCheckHook {
    fn default() -> Self {
        Self::new()
    }
}

impl Hook for CompletionCheckHook {
    fn name(&self) -> &'static str {
        "completion_check"
    }

    fn handle(&self, event: &HookEvent, context: &HookContext) -> HookResult {
        // Only handle AfterAgent events
        let HookEvent::AfterAgent {
            response: _,
            has_final_answer,
        } = event
        else {
            return HookResult::Continue;
        };

        // Check if we've reached the continuation limit
        if context.at_continuation_limit() {
            info!(
                continuation_count = context.continuation_count,
                max = context.max_continuations,
                "Continuation limit reached, allowing completion"
            );
            return HookResult::Continue;
        }

        // If the agent explicitly provided a final answer, trust it and allow exit
        if *has_final_answer {
            info!("Agent provided a final answer, allowing completion");
            return HookResult::Continue;
        }

        // If no todos, allow completion
        if context.todos.items.is_empty() {
            return HookResult::Continue;
        }

        // Check if all todos are complete
        if context.todos.is_complete() {
            info!(
                completed = context.todos.completed_count(),
                total = context.todos.items.len(),
                "All todos completed"
            );
            return HookResult::Continue;
        }

        // Todos are incomplete - force continuation
        let pending = context.todos.pending_count();
        let total = context.todos.items.len();
        let completed = context.todos.completed_count();

        let reason = format!(
            "Not all tasks are completed ({completed}/{total} done, {pending} remaining). Continue working on remaining tasks."
        );

        let todo_context = context.todos.to_context_string();

        info!(
            pending = pending,
            completed = completed,
            total = total,
            "Forcing continuation due to incomplete todos"
        );

        HookResult::ForceIteration {
            reason,
            context: Some(todo_context),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::providers::{TodoItem, TodoList, TodoStatus};
    use crate::config::AGENT_CONTINUATION_LIMIT;

    fn create_context(todos: &TodoList, continuation_count: usize) -> HookContext<'_> {
        HookContext::new(todos, 0, continuation_count, AGENT_CONTINUATION_LIMIT)
    }

    #[test]
    fn test_empty_todos_allows_completion() {
        let hook = CompletionCheckHook::new();
        let todos = TodoList::new();
        let context = create_context(&todos, 0);
        let event = HookEvent::AfterAgent {
            response: "Done!".to_string(),
            has_final_answer: false,
        };

        let result = hook.handle(&event, &context);
        assert!(matches!(result, HookResult::Continue));
    }

    #[test]
    fn test_all_completed_allows_completion() {
        let hook = CompletionCheckHook::new();
        let mut todos = TodoList::new();
        todos.items.push(TodoItem {
            description: "Task 1".to_string(),
            status: TodoStatus::Completed,
        });
        todos.items.push(TodoItem {
            description: "Task 2".to_string(),
            status: TodoStatus::Completed,
        });

        let context = create_context(&todos, 0);
        let event = HookEvent::AfterAgent {
            response: "Done!".to_string(),
            has_final_answer: false,
        };

        let result = hook.handle(&event, &context);
        assert!(matches!(result, HookResult::Continue));
    }

    #[test]
    fn test_incomplete_todos_forces_iteration() {
        let hook = CompletionCheckHook::new();
        let mut todos = TodoList::new();
        todos.items.push(TodoItem {
            description: "Task 1".to_string(),
            status: TodoStatus::Completed,
        });
        todos.items.push(TodoItem {
            description: "Task 2".to_string(),
            status: TodoStatus::Pending,
        });

        let context = create_context(&todos, 0);
        let event = HookEvent::AfterAgent {
            response: "Done!".to_string(),
            has_final_answer: false,
        };

        let result = hook.handle(&event, &context);
        assert!(matches!(result, HookResult::ForceIteration { .. }));

        if let HookResult::ForceIteration { reason, context } = result {
            assert!(reason.contains("1/2"));
            assert!(context.is_some());
        }
    }

    #[test]
    fn test_continuation_limit_allows_completion() {
        let hook = CompletionCheckHook::new();
        let mut todos = TodoList::new();
        todos.items.push(TodoItem {
            description: "Task 1".to_string(),
            status: TodoStatus::Pending, // Incomplete!
        });

        // At continuation limit
        let context = create_context(&todos, AGENT_CONTINUATION_LIMIT);
        let event = HookEvent::AfterAgent {
            response: "Done!".to_string(),
            has_final_answer: false,
        };

        let result = hook.handle(&event, &context);
        assert!(matches!(result, HookResult::Continue)); // Should allow despite incomplete
    }

    #[test]
    fn test_ignores_non_after_agent_events() {
        let hook = CompletionCheckHook::new();
        let mut todos = TodoList::new();
        todos.items.push(TodoItem {
            description: "Task 1".to_string(),
            status: TodoStatus::Pending,
        });

        let context = create_context(&todos, 0);
        let event = HookEvent::BeforeAgent {
            prompt: "test".to_string(),
        };

        let result = hook.handle(&event, &context);
        assert!(matches!(result, HookResult::Continue));
    }
}
