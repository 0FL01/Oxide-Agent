//! Hot context health hook.

use super::registry::Hook;
use super::types::{HookContext, HookEvent, HookResult};
use crate::agent::compaction::HotContextLimits;

/// Warns when the hot context grows too large.
pub struct HotContextHealthHook {
    limits: HotContextLimits,
}

impl HotContextHealthHook {
    /// Create a hook with the default thresholds.
    #[must_use]
    pub fn new() -> Self {
        Self {
            limits: HotContextLimits::default(),
        }
    }

    #[must_use]
    /// Create a hook with custom thresholds.
    pub const fn with_limits(limits: HotContextLimits) -> Self {
        Self { limits }
    }

    fn effective_soft_limit(&self, context: &HookContext) -> usize {
        self.limits
            .soft_warning_tokens
            .min(context.max_tokens.saturating_mul(60) / 100)
    }

    fn effective_hard_limit(&self, context: &HookContext) -> usize {
        let soft_limit = self.effective_soft_limit(context);
        self.limits
            .hard_compaction_tokens
            .min(context.max_tokens.saturating_mul(80) / 100)
            .max(soft_limit.saturating_add(1))
    }

    fn build_notice(
        &self,
        context: &HookContext,
        current_tokens: usize,
        threshold_tokens: usize,
        hard: bool,
    ) -> String {
        let agent_kind = if context.is_sub_agent {
            "Sub-agent"
        } else {
            "Agent"
        };
        let compress_hint = if context.has_tool("compress") {
            "Use `compress` to trim older context before adding more work."
        } else {
            "Trim older context before adding more work."
        };

        if hard {
            format!(
                "{agent_kind} hot context hit the hard limit ({current_tokens}/{threshold_tokens} tokens). A compaction pass is running now. {compress_hint}"
            )
        } else {
            format!(
                "{agent_kind} hot context is getting large ({current_tokens}/{threshold_tokens} tokens). {compress_hint}"
            )
        }
    }
}

impl Default for HotContextHealthHook {
    fn default() -> Self {
        Self::new()
    }
}

impl Hook for HotContextHealthHook {
    fn name(&self) -> &'static str {
        "hot_context_health"
    }

    fn handle(&self, event: &HookEvent, context: &HookContext) -> HookResult {
        let HookEvent::BeforeIteration { .. } = event else {
            return HookResult::Continue;
        };

        let current_tokens = context.token_count;
        let soft_limit = self.effective_soft_limit(context);
        if current_tokens < soft_limit {
            return HookResult::Continue;
        }

        let notice = self.build_notice(context, current_tokens, soft_limit, false);
        let hard_limit = self.effective_hard_limit(context);
        if current_tokens >= hard_limit {
            let hard_notice = self.build_notice(context, current_tokens, hard_limit, true);
            return HookResult::RequestCompaction {
                reason: format!(
                    "Hot context reached the hard threshold ({} >= {} tokens)",
                    current_tokens, hard_limit
                ),
                context: Some(hard_notice),
            };
        }

        HookResult::InjectTransientContext(notice)
    }
}

#[cfg(test)]
mod tests {
    use super::HotContextHealthHook;
    use crate::agent::compaction::HotContextLimits;
    use crate::agent::hooks::{Hook, HookContext, HookEvent, HookResult};
    use crate::agent::memory::AgentMemory;
    use crate::agent::providers::TodoList;

    fn context(token_count: usize, max_tokens: usize) -> HookContext<'static> {
        let todos = Box::leak(Box::new(TodoList::new()));
        let memory = Box::leak(Box::new(AgentMemory::new(max_tokens)));
        HookContext::new(todos, memory, 0, 0, 4).with_tokens(token_count, max_tokens)
    }

    #[test]
    fn soft_limit_returns_transient_notice() {
        let hook = HotContextHealthHook::with_limits(HotContextLimits::new(10, 20));

        let result = hook.handle(
            &HookEvent::BeforeIteration { iteration: 1 },
            &context(12, 100),
        );

        assert!(matches!(result, HookResult::InjectTransientContext(_)));
    }

    #[test]
    fn hard_limit_requests_compaction() {
        let hook = HotContextHealthHook::with_limits(HotContextLimits::new(10, 20));

        let result = hook.handle(
            &HookEvent::BeforeIteration { iteration: 1 },
            &context(21, 100),
        );

        assert!(matches!(result, HookResult::RequestCompaction { .. }));
    }
}
