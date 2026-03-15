//! Workload Distributor Hook.
//!
//! Enforces the separation of duties between the Main Agent (Orchestrator) and Sub-Agents (Workers).
//!
//! Features:
//! 1. **Hard Blocking:** Prevents the Main Agent from executing heavy filesystem operations manually.
//!    If the Main Agent tries to `git clone` or `grep -r`, it gets blocked and told to delegate.
//! 2. **Context Injection:** Analyzes the user's prompt complexity and injects strict instructions
//!    to delegate routine work, replacing the older `ComplexityAnalyzerHook`.

use super::registry::Hook;
use super::types::{HookContext, HookEvent, HookResult};
use serde_json::Value;

/// Hook that distributes workload by blocking manual labor and encouraging delegation.
pub struct WorkloadDistributorHook {
    min_word_count: usize,
}

impl WorkloadDistributorHook {
    /// Create a new workload distributor hook.
    #[must_use]
    pub const fn new() -> Self {
        Self { min_word_count: 60 } // Slightly lower threshold than the old analyzer
    }

    fn is_heavy_command(&self, command: &str) -> Option<&'static str> {
        let normalized = command.trim();

        // Git operations that fetch data
        if normalized.starts_with("git clone") {
            return Some("git clone");
        }
        if normalized.starts_with("git fetch") {
            return Some("git fetch");
        }

        // Heavy search operations
        if normalized.contains("grep -r") || normalized.contains("grep -R") {
            return Some("recursive grep");
        }
        if normalized.starts_with("find")
            && (normalized.contains("-exec") || normalized.contains("-name"))
        {
            return Some("find search");
        }

        None
    }

    fn is_crawl4ai_tool(&self, tool_name: &str) -> bool {
        matches!(tool_name, "deep_crawl" | "web_markdown" | "web_pdf")
    }

    fn is_complex_prompt(&self, prompt: &str) -> bool {
        let normalized = prompt.to_lowercase();
        let word_count = normalized.split_whitespace().count();
        if word_count >= self.min_word_count {
            return true;
        }

        let keywords = [
            // Russian
            "исслед",
            "сравн",
            "обзор",
            "анализ",
            "отчет",
            "подбор",
            "репозитор",
            "код",
            "файлы",
            "сканир",
            "изучи",
            // English
            "compare",
            "research",
            "analysis",
            "overview",
            "report",
            "benchmark",
            "repo",
            "codebase",
            "scan",
            "investigate",
        ];

        if keywords.iter().any(|keyword| normalized.contains(keyword)) {
            return true;
        }

        // Multi-sentence complex request detection
        let sentence_markers = ["?", "!", "."];
        let sentence_hits: usize = sentence_markers
            .iter()
            .map(|marker| normalized.matches(marker).count())
            .sum();

        sentence_hits >= 3
    }

    fn can_delegate(&self, context: &HookContext) -> bool {
        context.has_tool("delegate_to_sub_agent")
    }
}

impl Default for WorkloadDistributorHook {
    fn default() -> Self {
        Self::new()
    }
}

impl Hook for WorkloadDistributorHook {
    fn name(&self) -> &'static str {
        "workload_distributor"
    }

    fn handle(&self, event: &HookEvent, context: &HookContext) -> HookResult {
        match event {
            // 1. Context Injection for Complex Prompts
            HookEvent::BeforeAgent { prompt } => {
                if self.is_complex_prompt(prompt) && self.can_delegate(context) {
                    return HookResult::InjectContext(
                        "[SYSTEM NOTICE: High Complexity Detected]\n\
                        You must SPLIT your workflow to handle this request efficiently:\n\
                        1. 🟢 DELEGATE retrieval tasks (git clone, grep, find, cat, deep_crawl, web_markdown) to `delegate_to_sub_agent`.\n\
                           - Goal: Get raw data/files/web content.\n\
                           - Forbidden for sub-agent: analysis, reasoning, explaining \"why\".\n\
                        2. 🧠 RETAIN analysis tasks for yourself.\n\
                           - Goal: Read the files/content returned by the sub-agent and perform high-level reasoning.\n\
                        Example of GOOD delegation: \"Use deep_crawl to find news about X\".\n\
                        Example of BAD delegation: \"Analyze why project X is failing\"."
                            .to_string(),
                    );
                }
            }

            // 2. Hard Blocking of Heavy Commands and Direct Search
            HookEvent::BeforeTool {
                tool_name,
                arguments,
            } => {
                // Sub-agents are allowed to run everything
                if context.is_sub_agent {
                    return HookResult::Continue;
                }

                if !self.can_delegate(context) {
                    return HookResult::Continue;
                }

                // Block direct Crawl4AI calls for Main Agent
                if self.is_crawl4ai_tool(tool_name) {
                    return HookResult::Block {
                        reason: format!(
                            "⛔ DIRECT SEARCH BLOCKED: You are trying to use '{}' directly. \
                            For efficiency and context saving, you MUST delegate web crawling/extraction to a sub-agent.\n\
                            ACTION REQUIRED: Use `delegate_to_sub_agent` with tool '{}' in the whitelist.",
                            tool_name, tool_name
                        ),
                    };
                }

                if tool_name == "execute_command" {
                    // Parse JSON arguments to get the command string
                    let command = match serde_json::from_str::<Value>(arguments) {
                        Ok(json) => json
                            .get("command")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        Err(_) => return HookResult::Continue,
                    };

                    if let Some(op) = self.is_heavy_command(&command) {
                        return HookResult::Block {
                            reason: format!(
                                "⛔ MANUAL LABOR DETECTED: You are trying to run a heavy operation ('{}') yourself. \
                                This wastes your context window.\n\
                                ACTION REQUIRED: Use `delegate_to_sub_agent` to run this command and summarize the results.",
                                op
                            ),
                        };
                    }
                }
            }
            _ => {}
        }

        HookResult::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::WorkloadDistributorHook;
    use crate::agent::hooks::{Hook, HookContext, HookEvent, HookResult};
    use crate::agent::memory::AgentMemory;
    use crate::agent::providers::TodoList;
    use crate::llm::ToolDefinition;
    use serde_json::json;

    fn hook_context<'a>(
        memory: &'a AgentMemory,
        todos: &'a TodoList,
        tools: &'a [ToolDefinition],
    ) -> HookContext<'a> {
        HookContext::new(todos, memory, 0, 0, 4).with_available_tools(tools)
    }

    fn tool(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: name.to_string(),
            parameters: json!({ "type": "object" }),
        }
    }

    #[test]
    fn does_not_inject_delegation_pressure_without_delegate_tool() {
        let hook = WorkloadDistributorHook::new();
        let memory = AgentMemory::new(1024);
        let todos = TodoList::new();
        let tools = [tool("execute_command")];

        let result = hook.handle(
            &HookEvent::BeforeAgent {
                prompt: "Исследуй кодовую базу, сравни варианты и подготовь отчет по архитектуре."
                    .to_string(),
            },
            &hook_context(&memory, &todos, &tools),
        );

        assert!(matches!(result, HookResult::Continue));
    }

    #[test]
    fn does_not_block_heavy_command_when_delegate_tool_is_unavailable() {
        let hook = WorkloadDistributorHook::new();
        let memory = AgentMemory::new(1024);
        let todos = TodoList::new();
        let tools = [tool("execute_command")];

        let result = hook.handle(
            &HookEvent::BeforeTool {
                tool_name: "execute_command".to_string(),
                arguments: r#"{"command":"git clone https://example.com/repo.git"}"#.to_string(),
            },
            &hook_context(&memory, &todos, &tools),
        );

        assert!(matches!(result, HookResult::Continue));
    }

    #[test]
    fn still_injects_delegation_pressure_when_delegate_tool_is_available() {
        let hook = WorkloadDistributorHook::new();
        let memory = AgentMemory::new(1024);
        let todos = TodoList::new();
        let tools = [tool("execute_command"), tool("delegate_to_sub_agent")];

        let result = hook.handle(
            &HookEvent::BeforeAgent {
                prompt: "Investigate the repository, compare approaches, and prepare a report."
                    .to_string(),
            },
            &hook_context(&memory, &todos, &tools),
        );

        assert!(matches!(result, HookResult::InjectContext(_)));
    }
}
