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

    fn handle(&self, event: &HookEvent, _context: &HookContext) -> HookResult {
        match event {
            // 1. Context Injection for Complex Prompts
            HookEvent::BeforeAgent { prompt } => {
                if self.is_complex_prompt(prompt) {
                    return HookResult::InjectContext(
                        "[SYSTEM NOTICE: High Complexity Detected]\n\
                        DO NOT perform heavy lifting (git clone, mass file reading, searching) yourself.\n\
                        Use `delegate_to_sub_agent` for ALL data gathering and exploration.\n\
                        Your role is ORCHESTRATOR & ANALYST. Delegate the manual labor."
                            .to_string(),
                    );
                }
            }

            // 2. Hard Blocking of Heavy Commands
            HookEvent::BeforeTool {
                tool_name,
                arguments,
            } => {
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
