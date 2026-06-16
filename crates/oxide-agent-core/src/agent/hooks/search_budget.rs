//! Search Budget Hook.
//!
//! Enforces a limit on the number of search tool calls per agent session.

use super::registry::Hook;
use super::types::{HookContext, HookEvent, HookResult};
use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use url::Url;

/// Hook that limits the number of search tool calls.
pub struct SearchBudgetHook {
    limit: usize,
    count: AtomicUsize,
    brave_search_unavailable: AtomicBool,
    blocked_web_markdown_hosts: Mutex<HashSet<String>>,
}

impl SearchBudgetHook {
    /// Create a new search budget hook with a limit.
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            count: AtomicUsize::new(0),
            brave_search_unavailable: AtomicBool::new(false),
            blocked_web_markdown_hosts: Mutex::new(HashSet::new()),
        }
    }

    fn is_search_tool(&self, tool_name: &str) -> bool {
        matches!(
            tool_name,
            "web_search" | "web_extract" | "brave_search" | "web_crawler" | "web_markdown"
        )
    }

    fn is_brave_search_tool(tool_name: &str) -> bool {
        matches!(tool_name, "brave_search")
    }

    fn result_marks_brave_search_unavailable(result: &str) -> bool {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(result) else {
            return false;
        };
        let Some(payload) = value.get("structured_payload") else {
            return false;
        };
        if payload.get("provider").and_then(|value| value.as_str()) != Some("brave_search") {
            return false;
        }

        payload
            .get("provider_unavailable")
            .and_then(|value| value.as_bool())
            == Some(true)
            || matches!(
                payload.get("error_kind").and_then(|value| value.as_str()),
                Some(
                    "rate_limited" | "auth" | "missing_api_key" | "server" | "network" | "timeout"
                )
            )
    }

    fn web_markdown_host_from_arguments(arguments: &str) -> Option<String> {
        let value = serde_json::from_str::<serde_json::Value>(arguments).ok()?;
        let url = value.get("url")?.as_str()?;
        Self::canonical_host_from_url(url)
    }

    fn canonical_host_from_url(raw_url: &str) -> Option<String> {
        Url::parse(raw_url)
            .ok()?
            .host_str()
            .map(|host| host.trim_end_matches('.').to_ascii_lowercase())
    }

    fn result_marks_web_markdown_host_unavailable(result: &str) -> Option<String> {
        let value = serde_json::from_str::<serde_json::Value>(result).ok()?;
        let payload = value.get("structured_payload")?;
        if !matches!(
            payload.get("provider").and_then(|value| value.as_str()),
            Some("web_markdown" | "web_crawler")
        ) {
            return None;
        }
        if payload.get("error_kind").and_then(|value| value.as_str()) != Some("anti_bot") {
            return None;
        }
        payload
            .get("host")
            .and_then(|value| value.as_str())
            .map(|host| host.trim_end_matches('.').to_ascii_lowercase())
    }
}

impl Hook for SearchBudgetHook {
    fn name(&self) -> &'static str {
        "search_budget"
    }

    fn handle(&self, event: &HookEvent, context: &HookContext) -> HookResult {
        match event {
            HookEvent::BeforeTool {
                tool_name,
                arguments,
            } => {
                if Self::is_brave_search_tool(tool_name)
                    && self.brave_search_unavailable.load(Ordering::SeqCst)
                {
                    return HookResult::Block {
                        reason: concat!(
                            "Brave Search is unavailable in this task. Do not retry ",
                            "brave_search with rewritten queries; use web_search fallback."
                        )
                        .to_string(),
                    };
                }

                if matches!(tool_name.as_str(), "web_markdown" | "web_crawler")
                    && let Some(host) = Self::web_markdown_host_from_arguments(arguments)
                    && self
                        .blocked_web_markdown_hosts
                        .lock()
                        .expect("blocked_web_markdown_hosts poisoned")
                        .contains(&host)
                {
                    return HookResult::Block {
                        reason: format!(
                            "lightweight URL fetch is temporarily unavailable for host {host} in this task because the site returned an anti-bot challenge. Do not retry this host with the same fetch path; use another source."
                        ),
                    };
                }

                if self.is_search_tool(tool_name) {
                    let limit = context.search_limit.unwrap_or(self.limit);
                    let current = self.count.fetch_add(1, Ordering::SeqCst) + 1;
                    if current > limit {
                        return HookResult::Block {
                            reason: format!(
                                "Search budget exceeded ({}/{}). Please synthesize findings from existing data instead of searching more.",
                                current, limit
                            ),
                        };
                    }
                }
            }
            HookEvent::AfterTool { tool_name, result } if Self::is_brave_search_tool(tool_name) => {
                if Self::result_marks_brave_search_unavailable(result) {
                    self.brave_search_unavailable.store(true, Ordering::SeqCst);
                }
            }
            HookEvent::AfterTool { tool_name, result }
                if matches!(tool_name.as_str(), "web_markdown" | "web_crawler") =>
            {
                if let Some(host) = Self::result_marks_web_markdown_host_unavailable(result) {
                    self.blocked_web_markdown_hosts
                        .lock()
                        .expect("blocked_web_markdown_hosts poisoned")
                        .insert(host);
                }
            }
            _ => {}
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
    fn counts_brave_search_against_budget() {
        let hook = SearchBudgetHook::new(0);
        let todos = TodoList::new();
        let memory = AgentMemory::new(1024);
        let context = HookContext::new(&todos, &memory, 0, 0, 1);

        let result = hook.handle(
            &HookEvent::BeforeTool {
                tool_name: "brave_search".to_string(),
                arguments: "{}".to_string(),
            },
            &context,
        );

        assert!(matches!(result, HookResult::Block { .. }));
    }

    #[test]
    fn counts_web_fetchers_against_budget() {
        let hook = SearchBudgetHook::new(1);
        let todos = TodoList::new();
        let memory = AgentMemory::new(1024);
        let context = HookContext::new(&todos, &memory, 0, 0, 1);

        let first = hook.handle(
            &HookEvent::BeforeTool {
                tool_name: "web_crawler".to_string(),
                arguments: "{}".to_string(),
            },
            &context,
        );
        let second = hook.handle(
            &HookEvent::BeforeTool {
                tool_name: "web_markdown".to_string(),
                arguments: r#"{"url":"https://example.com"}"#.to_string(),
            },
            &context,
        );

        assert!(matches!(first, HookResult::Continue));
        assert!(matches!(second, HookResult::Block { .. }));
    }

    #[test]
    fn context_search_limit_can_lower_hook_default() {
        let hook = SearchBudgetHook::new(10);
        let todos = TodoList::new();
        let memory = AgentMemory::new(1024);
        let context = HookContext::new(&todos, &memory, 0, 0, 1).with_search_limit(1);

        let first = hook.handle(
            &HookEvent::BeforeTool {
                tool_name: "web_search".to_string(),
                arguments: "{}".to_string(),
            },
            &context,
        );
        let second = hook.handle(
            &HookEvent::BeforeTool {
                tool_name: "web_search".to_string(),
                arguments: "{}".to_string(),
            },
            &context,
        );

        assert!(matches!(first, HookResult::Continue));
        assert!(matches!(second, HookResult::Block { .. }));
    }

    #[test]
    fn blocks_repeated_brave_search_after_unavailable_payload() {
        let hook = SearchBudgetHook::new(10);
        let todos = TodoList::new();
        let memory = AgentMemory::new(1024);
        let context = HookContext::new(&todos, &memory, 0, 0, 1);

        let result = serde_json::json!({
            "structured_payload": {
                "provider": "brave_search",
                "kind": "search",
                "error_kind": "rate_limited",
                "provider_unavailable": true
            }
        })
        .to_string();
        assert!(matches!(
            hook.handle(
                &HookEvent::AfterTool {
                    tool_name: "brave_search".to_string(),
                    result,
                },
                &context,
            ),
            HookResult::Continue
        ));

        let blocked = hook.handle(
            &HookEvent::BeforeTool {
                tool_name: "brave_search".to_string(),
                arguments: "{}".to_string(),
            },
            &context,
        );

        assert!(matches!(blocked, HookResult::Block { .. }));
        if let HookResult::Block { reason } = blocked {
            assert!(reason.contains("use web_search fallback"));
        }
    }

    #[test]
    fn allows_web_search_after_brave_search_failure() {
        let hook = SearchBudgetHook::new(10);
        let todos = TodoList::new();
        let memory = AgentMemory::new(1024);
        let context = HookContext::new(&todos, &memory, 0, 0, 1);

        let result = serde_json::json!({
            "structured_payload": {
                "provider": "brave_search",
                "kind": "search",
                "error_kind": "server"
            }
        })
        .to_string();
        assert!(matches!(
            hook.handle(
                &HookEvent::AfterTool {
                    tool_name: "brave_search".to_string(),
                    result,
                },
                &context,
            ),
            HookResult::Continue
        ));

        let web_search = hook.handle(
            &HookEvent::BeforeTool {
                tool_name: "web_search".to_string(),
                arguments: "{}".to_string(),
            },
            &context,
        );

        assert!(matches!(web_search, HookResult::Continue));
    }

    #[test]
    fn web_markdown_consumes_search_budget() {
        let hook = SearchBudgetHook::new(1);
        let todos = TodoList::new();
        let memory = AgentMemory::new(1024);
        let context = HookContext::new(&todos, &memory, 0, 0, 1);

        let markdown = hook.handle(
            &HookEvent::BeforeTool {
                tool_name: "web_markdown".to_string(),
                arguments: r#"{"url":"https://example.com/article"}"#.to_string(),
            },
            &context,
        );
        let search = hook.handle(
            &HookEvent::BeforeTool {
                tool_name: "web_search".to_string(),
                arguments: "{}".to_string(),
            },
            &context,
        );

        assert!(matches!(markdown, HookResult::Continue));
        assert!(matches!(search, HookResult::Block { .. }));
    }

    #[test]
    fn blocks_repeated_web_markdown_host_after_antibot_signal() {
        let hook = SearchBudgetHook::new(10);
        let todos = TodoList::new();
        let memory = AgentMemory::new(1024);
        let context = HookContext::new(&todos, &memory, 0, 0, 1);

        let result = serde_json::json!({
            "structured_payload": {
                "provider": "web_markdown",
                "kind": "fetch",
                "host": "ftbwiki.org",
                "error_kind": "anti_bot",
                "provider_unavailable": true
            }
        })
        .to_string();
        assert!(matches!(
            hook.handle(
                &HookEvent::AfterTool {
                    tool_name: "web_markdown".to_string(),
                    result,
                },
                &context,
            ),
            HookResult::Continue
        ));

        let blocked = hook.handle(
            &HookEvent::BeforeTool {
                tool_name: "web_markdown".to_string(),
                arguments: r#"{"url":"https://ftbwiki.org/Pech"}"#.to_string(),
            },
            &context,
        );
        let other_host = hook.handle(
            &HookEvent::BeforeTool {
                tool_name: "web_markdown".to_string(),
                arguments: r#"{"url":"https://thaumcraft.wiki/Pech"}"#.to_string(),
            },
            &context,
        );

        assert!(matches!(blocked, HookResult::Block { .. }));
        assert!(matches!(other_host, HookResult::Continue));
    }
}
