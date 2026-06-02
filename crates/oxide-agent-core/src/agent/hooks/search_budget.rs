//! Search Budget Hook.
//!
//! Enforces a limit on the number of search tool calls per agent session.

use super::registry::Hook;
use super::types::{HookContext, HookEvent, HookResult};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use url::Url;

/// Hook that limits the number of search tool calls.
pub struct SearchBudgetHook {
    limit: usize,
    count: AtomicUsize,
    duckduckgo_unavailable: AtomicBool,
    blocked_web_markdown_hosts: Mutex<HashSet<String>>,
}

impl SearchBudgetHook {
    /// Create a new search budget hook with a limit.
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            count: AtomicUsize::new(0),
            duckduckgo_unavailable: AtomicBool::new(false),
            blocked_web_markdown_hosts: Mutex::new(HashSet::new()),
        }
    }

    fn is_search_tool(&self, tool_name: &str) -> bool {
        matches!(
            tool_name,
            "web_search"
                | "web_extract"
                | "duckduckgo_search"
                | "duckduckgo_news"
                | "searxng_search"
        )
    }

    fn is_duckduckgo_tool(tool_name: &str) -> bool {
        matches!(tool_name, "duckduckgo_search" | "duckduckgo_news")
    }

    fn result_marks_duckduckgo_unavailable(result: &str) -> bool {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(result) else {
            return false;
        };
        let Some(payload) = value.get("structured_payload") else {
            return false;
        };
        if payload.get("provider").and_then(|value| value.as_str()) != Some("duckduckgo") {
            return false;
        }
        matches!(
            payload.get("error_kind").and_then(|value| value.as_str()),
            Some("blocked" | "rate_limited")
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
        if payload.get("provider").and_then(|value| value.as_str()) != Some("web_markdown") {
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

    fn handle(&self, event: &HookEvent, _context: &HookContext) -> HookResult {
        match event {
            HookEvent::BeforeTool {
                tool_name,
                arguments,
            } => {
                if Self::is_duckduckgo_tool(tool_name)
                    && self.duckduckgo_unavailable.load(Ordering::SeqCst)
                {
                    return HookResult::Block {
                        reason: concat!(
                            "DuckDuckGo is temporarily unavailable in this task because it returned ",
                            "a block/rate-limit signal. Do not retry DuckDuckGo with rewritten ",
                            "queries; use `searxng_search` instead or synthesize from existing data."
                        )
                        .to_string(),
                    };
                }

                if tool_name == "web_markdown" {
                    if let Some(host) = Self::web_markdown_host_from_arguments(arguments) {
                        if self
                            .blocked_web_markdown_hosts
                            .lock()
                            .expect("blocked_web_markdown_hosts poisoned")
                            .contains(&host)
                        {
                            return HookResult::Block {
                                reason: format!(
                                    "web_markdown is temporarily unavailable for host {host} in this task because the site returned an anti-bot challenge. Do not retry this host with the lightweight fetcher; use another source."
                                ),
                            };
                        }
                    }
                }

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
            HookEvent::AfterTool { tool_name, result } if Self::is_duckduckgo_tool(tool_name) => {
                if Self::result_marks_duckduckgo_unavailable(result) {
                    self.duckduckgo_unavailable.store(true, Ordering::SeqCst);
                }
            }
            HookEvent::AfterTool { tool_name, result } if tool_name == "web_markdown" => {
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

    #[test]
    fn blocks_repeated_duckduckgo_after_block_signal() {
        let hook = SearchBudgetHook::new(10);
        let todos = TodoList::new();
        let memory = AgentMemory::new(1024);
        let context = HookContext::new(&todos, &memory, 0, 0, 1);

        let result = serde_json::json!({
            "structured_payload": {
                "provider": "duckduckgo",
                "kind": "search",
                "error_kind": "blocked"
            }
        })
        .to_string();
        assert!(matches!(
            hook.handle(
                &HookEvent::AfterTool {
                    tool_name: "duckduckgo_search".to_string(),
                    result,
                },
                &context,
            ),
            HookResult::Continue
        ));

        let blocked = hook.handle(
            &HookEvent::BeforeTool {
                tool_name: "duckduckgo_search".to_string(),
                arguments: "{}".to_string(),
            },
            &context,
        );

        assert!(matches!(blocked, HookResult::Block { .. }));
    }

    #[test]
    fn web_markdown_does_not_consume_search_budget() {
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
                tool_name: "duckduckgo_search".to_string(),
                arguments: "{}".to_string(),
            },
            &context,
        );

        assert!(matches!(markdown, HookResult::Continue));
        assert!(matches!(search, HookResult::Continue));
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
