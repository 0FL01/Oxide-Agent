//! Completion Check Hook - ensures all todos are completed before finishing
//!
//! This hook forces the agent to continue iterating if there are
//! incomplete todos in the list, unless the remaining work is blocked on user input.

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
        let HookEvent::AfterAgent { response } = event else {
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

        if research_has_evidence(context) && response_is_research_status_only(response) {
            info!(
                response_chars = response.trim().chars().count(),
                "Forcing continuation because research final response is only status/offer text"
            );
            return HookResult::ForceIteration {
                reason: "Research final answer is incomplete: status/offer text instead of the requested answer."
                    .to_string(),
                context: Some(research_status_only_context()),
            };
        }

        // If no todos, allow completion
        if context.todos.items.is_empty() {
            return HookResult::Continue;
        }

        // CRITICAL: LLMs are inherently "lazy" and will often try to finish early
        // to save tokens or effort, even if tasks remain.
        // This deterministic check is MANDATORY to guarantee work completion.
        // DO NOT relax this check or allow the agent to self-judge its completion
        // if there are pending items in the todo list.
        // We previously tried to relax this, which led to significant task skipping.
        // Check if all todos are complete
        if context.todos.is_complete() {
            info!(
                completed = context.todos.completed_count(),
                total = context.todos.items.len(),
                "All todos completed"
            );
            return HookResult::Continue;
        }

        if context.todos.all_incomplete_items_blocked_on_user() {
            info!(
                blocked = context.todos.blocked_count(),
                total = context.todos.items.len(),
                "Allowing completion because remaining todos are blocked on user input"
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

fn research_has_evidence(context: &HookContext<'_>) -> bool {
    context.research_runtime.is_some_and(|runtime| {
        let snapshot = runtime.snapshot();
        !snapshot.evidence_documents.is_empty() || !snapshot.fetched_sources.is_empty()
    })
}

pub(crate) fn response_is_research_status_only(response: &str) -> bool {
    let trimmed = response.trim();
    if trimmed.is_empty() {
        return true;
    }

    let normalized = trimmed.to_lowercase();
    let has_status_offer = contains_any(
        &normalized,
        &[
            "отчёт готов",
            "отчет готов",
            "если хотите",
            "могу дополнительно",
            "могу проверить",
            "могу разобрать",
            "могу сравнить",
            "дать команду",
            "report is ready",
            "i can also",
            "i can additionally",
            "would you like",
            "if you want",
        ],
    );
    if !has_status_offer {
        return false;
    }

    !looks_like_substantive_research_answer(trimmed, &normalized)
}

fn looks_like_substantive_research_answer(trimmed: &str, normalized: &str) -> bool {
    normalized.contains("tl;dr")
        || normalized.contains("tl:dr")
        || contains_any(
            normalized,
            &[
                "итог",
                "таблица",
                "не подтвержден",
                "не подтверждён",
                "проверенн",
                "doc-",
                "checked sources",
                "not confirmed",
            ],
        )
        || trimmed.lines().any(|line| line.matches('|').count() >= 2)
}

fn research_status_only_context() -> String {
    [
        "The previous final response was only a status/offer, not the requested research answer.",
        "Do not say that the report is ready.",
        "Do not offer optional follow-up instead of answering.",
        "Write the final answer now.",
        "Required:",
        "- Start with TL;DR.",
        "- Answer the original user task directly.",
        "- Include the requested comparison/table if possible.",
        "- For unavailable metrics, write that they are not confirmed in checked sources, in the user's language.",
        "- Use only fetched evidence and verified/proof-not-found constraints.",
    ]
    .join("\n")
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::memory::AgentMemory;
    use crate::agent::providers::{TodoItem, TodoList, TodoStatus};
    use crate::agent::research::ResearchRuntime;
    use crate::agent::tool_runtime::{
        OutputTruncationMetadata, ToolCallId, ToolName, ToolOutput, ToolOutputIdentity,
        ToolOutputStatus,
    };
    use crate::config::AGENT_CONTINUATION_LIMIT;
    use crate::llm::InvocationId;
    use chrono::Utc;
    use serde_json::json;

    fn create_context(todos: &TodoList, continuation_count: usize) -> HookContext<'_> {
        let memory = Box::leak(Box::new(AgentMemory::new(1000)));
        HookContext::new(
            todos,
            memory,
            0,
            continuation_count,
            AGENT_CONTINUATION_LIMIT,
        )
    }

    fn runtime_with_evidence_document() -> ResearchRuntime {
        let runtime = ResearchRuntime::new();
        let now = Utc::now();
        let identity = ToolOutputIdentity {
            tool_call_id: ToolCallId::from("call-1"),
            provider_tool_call_id: None,
            invocation_id: InvocationId::new("invocation-1"),
            tool_name: ToolName::from("crawl4ai_markdown"),
            batch_index: 0,
        };
        let mut output = ToolOutput::terminal(
            identity,
            ToolOutputStatus::Success,
            now,
            now,
            OutputTruncationMetadata::new(4096, 4096, 4096),
        );
        output.structured_payload = Some(json!({
            "provider": "crawl4ai_markdown",
            "kind": "fetch",
            "url": "https://huggingface.co/example/model",
            "final_url": "https://huggingface.co/example/model",
            "status_code": 200,
            "markdown": "# Model card\nConfirmed model card text.",
            "source_kind": "model_card"
        }));
        runtime.record_tool_output(&output);
        runtime
    }

    #[test]
    fn test_empty_todos_allows_completion() {
        let hook = CompletionCheckHook::new();
        let todos = TodoList::new();
        let context = create_context(&todos, 0);
        let event = HookEvent::AfterAgent {
            response: "Done!".to_string(),
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
        };

        let result = hook.handle(&event, &context);
        assert!(matches!(result, HookResult::Continue));
    }

    #[test]
    fn research_status_only_response_forces_iteration_even_when_todos_complete() {
        let hook = CompletionCheckHook::new();
        let mut todos = TodoList::new();
        todos.items.push(TodoItem {
            description: "Research comparison".to_string(),
            status: TodoStatus::Completed,
        });
        let memory = AgentMemory::new(1000);
        let runtime = runtime_with_evidence_document();
        let context = HookContext::new(&todos, &memory, 0, 0, AGENT_CONTINUATION_LIMIT)
            .with_research_runtime(Some(&runtime));
        let event = HookEvent::AfterAgent {
            response: "Отчёт готов. Если хотите, могу дополнительно проверить цифры на RTX 4090."
                .to_string(),
        };

        let result = hook.handle(&event, &context);
        match result {
            HookResult::ForceIteration { reason, context } => {
                assert!(reason.contains("Research final answer is incomplete"));
                assert!(context.is_some_and(|value| value.contains("Start with TL;DR")));
            }
            other => {
                panic!("expected research status-only response to force iteration, got {other:?}")
            }
        }
    }

    #[test]
    fn research_substantive_answer_is_allowed_when_todos_complete() {
        let hook = CompletionCheckHook::new();
        let mut todos = TodoList::new();
        todos.items.push(TodoItem {
            description: "Research comparison".to_string(),
            status: TodoStatus::Completed,
        });
        let memory = AgentMemory::new(1000);
        let runtime = runtime_with_evidence_document();
        let context = HookContext::new(&todos, &memory, 0, 0, AGENT_CONTINUATION_LIMIT)
            .with_research_runtime(Some(&runtime));
        let event = HookEvent::AfterAgent {
            response: "TL;DR: speed, prompt eval and VRAM are not confirmed in checked sources.\n\n| Metric | DiffusionGemma | Gemma 4 |\n| --- | --- | --- |\n| VRAM | not confirmed in checked sources | not confirmed in checked sources |"
                .to_string(),
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
        };

        let result = hook.handle(&event, &context);
        assert!(matches!(result, HookResult::Continue)); // Should allow despite incomplete
    }

    #[test]
    fn test_only_blocked_on_user_todos_allow_completion() {
        let hook = CompletionCheckHook::new();
        let mut todos = TodoList::new();
        todos.items.push(TodoItem {
            description: "Need APK link from user".to_string(),
            status: TodoStatus::BlockedOnUser,
        });

        let context = create_context(&todos, 0);
        let event = HookEvent::AfterAgent {
            response: "Waiting for the user".to_string(),
        };

        let result = hook.handle(&event, &context);
        assert!(matches!(result, HookResult::Continue));
    }

    #[test]
    fn test_pending_and_blocked_todos_still_force_iteration() {
        let hook = CompletionCheckHook::new();
        let mut todos = TodoList::new();
        todos.items.push(TodoItem {
            description: "Need APK link from user".to_string(),
            status: TodoStatus::BlockedOnUser,
        });
        todos.items.push(TodoItem {
            description: "Repack APK".to_string(),
            status: TodoStatus::Pending,
        });

        let context = create_context(&todos, 0);
        let event = HookEvent::AfterAgent {
            response: "Done".to_string(),
        };

        let result = hook.handle(&event, &context);
        assert!(matches!(result, HookResult::ForceIteration { .. }));
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
