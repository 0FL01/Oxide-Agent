//! Delegation Guard Hook.
//!
//! Prevents the main agent from delegating high-level cognitive tasks (analysis, reasoning)
//! to sub-agents, forcing it to use sub-agents only for mechanical tasks (retrieval, raw data).

use super::registry::Hook;
use super::types::{HookContext, HookEvent, HookResult};
use lazy_regex::lazy_regex;
use serde_json::Value;

/// Hook that blocks delegation of analytical tasks.
pub struct DelegationGuardHook {}

impl DelegationGuardHook {
    /// Create a new delegation guard hook.
    #[must_use]
    pub fn new() -> Self {
        Self {}
    }

    /// Checks if a task delegation should be blocked.
    ///
    /// Returns `Some(reason)` if the task contains analytical keywords and should be blocked.
    /// Returns `None` if the task starts with retrieval verbs (whitelisted).
    pub fn check_task(&self, task: &str) -> Option<String> {
        // 1. Whitelist: Explicit retrieval verbs (The "Safe" Path)
        // If the task starts with a mechanical verb, we assume it's a retrieval task
        // and bypass the blocklist. This prevents blocking queries like "Find files about architecture".
        // Supports RU/EN: find/search/grep/list/read -> найди/поиск/перечисли/прочитай
        // Added: collect/gather/compile/extract/retrieve -> собери/извлеки/получи
        static RE_RETRIEVAL_INTENT: lazy_regex::Lazy<regex::Regex> = lazy_regex!(
            r"(?iu)^\s*(?:please\s+|kindly\s+)?(?:find|search|grep|locate|list|ls|cat|read|get|fetch|download|clone|collect|gather|compile|extract|retrieve|найди|найди|поиск|искать|перечисли|список|покажи|скачай|загрузи|прочитай|выведи|собери)\b"
        );

        if RE_RETRIEVAL_INTENT.is_match(task) {
            return None;
        }

        // 2. Blocklist: Analytical keywords (The "Guard" Path)
        // Only checked if the task didn't pass the whitelist.
        // Matches: analyze/explain/review/evaluate/compare/reason + RU: анализ/объясни/оцени/сравни
        // Also matches prefixed forms: проанализ/объясни/оцени/сравни (with prefixes)
        static RE_ANALYTICAL_INTENT: lazy_regex::Lazy<regex::Regex> = lazy_regex!(
            r"(?iu)\b(why|analyz\w*|explain\w*|review\w*|opinion\w*|reason\w*|evaluate\w*|compare\w*|почему|анализ\w*|проанализ\w*|объясн\w*|обзор\w*|мнени\w*|оцени\w*|процени\w*|сравни\w*|просравни\w*|выясни\w*|эффективн\w*)\b"
        );

        RE_ANALYTICAL_INTENT
            .captures(task)
            .and_then(|captures| captures.get(1).map(|m| m.as_str().to_string()))
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
