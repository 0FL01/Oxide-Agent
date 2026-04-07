//! Stage-14 agent-native memory hooks.

use super::registry::Hook;
use super::types::{HookContext, HookEvent, HookResult};
use crate::agent::persistent_memory::{
    MemoryBehaviorRuntime, ToolDerivedMemoryDraft, TopicMemoryPolicy,
};
use chrono::Utc;
use oxide_agent_memory::MemoryType;
use serde_json::Value;

const TITLE_MAX_CHARS: usize = 96;
const SHORT_DESCRIPTION_MAX_CHARS: usize = 160;
const CONTENT_MAX_CHARS: usize = 320;

/// Suggests durable-memory tools when the user task looks history- or policy-heavy.
pub struct RetrievalAdvisorHook;

impl RetrievalAdvisorHook {
    /// Create the Stage-14 durable-memory retrieval advisor hook.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for RetrievalAdvisorHook {
    fn default() -> Self {
        Self::new()
    }
}

impl Hook for RetrievalAdvisorHook {
    fn name(&self) -> &'static str {
        "retrieval_advisor"
    }

    fn handle(&self, event: &HookEvent, context: &HookContext) -> HookResult {
        let HookEvent::BeforeAgent { prompt } = event else {
            return HookResult::Continue;
        };
        if context.is_sub_agent || !context.has_tool("memory_search") {
            return HookResult::Continue;
        }

        let policy = TopicMemoryPolicy::from_scope(context.memory_scope);
        if !policy.allow_manual_read_advice {
            return HookResult::Continue;
        }

        let normalized = prompt.to_ascii_lowercase();
        let needs_memory_card = contains_any(
            &normalized,
            &[
                "remember",
                "previous",
                "earlier",
                "before",
                "again",
                "policy",
                "constraint",
                "decision",
                "procedure",
                "workflow",
                "how did",
                "why did",
                "regression",
                "incident",
                "debug",
            ],
        );
        let needs_history_card = policy.allow_history_cards
            && contains_any(
                &normalized,
                &[
                    "thread",
                    "history",
                    "transcript",
                    "what happened",
                    "earlier",
                    "previous",
                    "before",
                    "incident",
                ],
            );

        if !needs_memory_card && !needs_history_card {
            return HookResult::Continue;
        }

        let mut lines = vec!["[Memory advisor]".to_string()];
        if needs_memory_card {
            lines.push(format!(
                "- Durable memory card: this request may depend on prior {} procedures, constraints, or decisions. Consider `memory_search` with a focused query before repeating work.",
                policy.context_label
            ));
        }
        if needs_history_card && context.has_tool("memory_read_thread_summary") {
            lines.push(
                "- History card: if you need prior episode context, start with `memory_read_thread_summary`; use `memory_read_thread_window` only when you need exact older turns."
                    .to_string(),
            );
        }
        if normalized.contains("episode") && context.has_tool("memory_read_episode") {
            lines.push(
                "- Episode card: if you already know the episode id, `memory_read_episode` gives the compact finalized record."
                    .to_string(),
            );
        }

        HookResult::InjectTransientContext(lines.join("\n"))
    }
}

/// Captures reusable memory candidates from selected tool calls without writing storage directly.
pub struct EpisodicExtractHook;

impl EpisodicExtractHook {
    /// Create the Stage-14 episodic extraction hook.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    fn maybe_record_preference(
        &self,
        runtime: &MemoryBehaviorRuntime,
        policy: &TopicMemoryPolicy,
        tool_name: &str,
    ) {
        if !policy.allow_preference_capture {
            return;
        }
        let pattern = match tool_name {
            "write_file" | "ssh_apply_file_edit" | "apply_file_edit" => "file_change_flow",
            _ => return,
        };
        if !runtime.observe_pattern(pattern, 2) {
            return;
        }
        runtime.record_draft(ToolDerivedMemoryDraft {
            memory_type: MemoryType::Preference,
            title: "Topic editing preference".to_string(),
            content: "In this topic, prefer incremental file changes over broad rewrites when updating code or configuration.".to_string(),
            short_description: "Prefer incremental file changes in this topic".to_string(),
            importance: 0.78,
            confidence: 0.7,
            source: "episodic_extract_hook".to_string(),
            reason: "repeated file-change pattern observed during one episode".to_string(),
            tags: vec!["preference".to_string(), "tool_pattern".to_string(), "file_edit".to_string()],
            captured_at: Utc::now(),
        });
    }
}

impl Default for EpisodicExtractHook {
    fn default() -> Self {
        Self::new()
    }
}

impl Hook for EpisodicExtractHook {
    fn name(&self) -> &'static str {
        "episodic_extract"
    }

    fn handle(&self, event: &HookEvent, context: &HookContext) -> HookResult {
        let HookEvent::AfterTool { tool_name, result } = event else {
            return HookResult::Continue;
        };
        if context.is_sub_agent {
            return HookResult::Continue;
        }
        let Some(runtime) = context.memory_behavior else {
            return HookResult::Continue;
        };
        let policy = TopicMemoryPolicy::from_scope(context.memory_scope);

        match tool_name.as_str() {
            "write_file" if policy.allow_procedure_capture => {
                if let Some(draft) = procedure_from_write_result(result) {
                    runtime.record_draft(draft);
                    self.maybe_record_preference(runtime, &policy, tool_name);
                }
            }
            "ssh_apply_file_edit" | "apply_file_edit" if policy.allow_procedure_capture => {
                if let Some(draft) = procedure_from_edit_result(tool_name, result) {
                    runtime.record_draft(draft);
                    self.maybe_record_preference(runtime, &policy, tool_name);
                }
            }
            "execute_command" | "ssh_exec" | "ssh_sudo_exec" if policy.allow_failure_capture => {
                if let Some(draft) = failure_from_command_result(tool_name, result) {
                    runtime.record_draft(draft);
                }
            }
            _ => {}
        }

        HookResult::Continue
    }
}

fn procedure_from_write_result(result: &str) -> Option<ToolDerivedMemoryDraft> {
    let payload = parse_json(result)?;
    if !payload.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    let path = string_field(&payload, "path")?;
    let bytes_written = payload
        .get("bytes_written")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    Some(ToolDerivedMemoryDraft {
        memory_type: MemoryType::Procedure,
        title: truncate_chars("Sandbox file update workflow", TITLE_MAX_CHARS),
        content: truncate_chars(
            &format!(
                "When generating or replacing sandbox artifacts, write the target file directly at '{}' and verify the result before relying on it.",
                path
            ),
            CONTENT_MAX_CHARS,
        ),
        short_description: truncate_chars(
            &format!("Direct sandbox write for {} ({} bytes)", path, bytes_written),
            SHORT_DESCRIPTION_MAX_CHARS,
        ),
        importance: 0.76,
        confidence: 0.72,
        source: "episodic_extract_hook".to_string(),
        reason: "captured successful sandbox file write as a reusable procedure".to_string(),
        tags: vec![
            "procedure".to_string(),
            "tool:write_file".to_string(),
            "sandbox".to_string(),
        ],
        captured_at: Utc::now(),
    })
}

fn procedure_from_edit_result(tool_name: &str, result: &str) -> Option<ToolDerivedMemoryDraft> {
    let payload = parse_json(result)?;
    if !payload.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    let path = string_field(&payload, "path")?;
    let status = string_field(&payload, "status").unwrap_or("updated");

    Some(ToolDerivedMemoryDraft {
        memory_type: MemoryType::Procedure,
        title: truncate_chars("Remote targeted edit workflow", TITLE_MAX_CHARS),
        content: truncate_chars(
            &format!(
                "For remote file changes in this topic, prefer targeted edits for '{}' instead of broad rewrites so the change stays reviewable and low-risk.",
                path
            ),
            CONTENT_MAX_CHARS,
        ),
        short_description: truncate_chars(
            &format!("{} remote file edit for {}", status, path),
            SHORT_DESCRIPTION_MAX_CHARS,
        ),
        importance: 0.82,
        confidence: 0.78,
        source: "episodic_extract_hook".to_string(),
        reason: format!("captured successful {} result as a reusable edit procedure", tool_name),
        tags: vec![
            "procedure".to_string(),
            format!("tool:{tool_name}"),
            "file_edit".to_string(),
            "ssh".to_string(),
        ],
        captured_at: Utc::now(),
    })
}

fn failure_from_command_result(tool_name: &str, result: &str) -> Option<ToolDerivedMemoryDraft> {
    let payload = parse_json(result)?;
    let ok = payload.get("ok").and_then(Value::as_bool).unwrap_or(false);
    let exit_code = payload.get("exit_code").and_then(Value::as_i64);
    if ok && exit_code.unwrap_or(0) == 0 {
        return None;
    }

    let command = string_field(&payload, "command").unwrap_or("command");
    let stderr = string_field(&payload, "stderr").unwrap_or_default();
    let error = string_field(&payload, "error").unwrap_or_default();
    let stdout = string_field(&payload, "stdout").unwrap_or_default();
    let detail = first_non_empty(&[error, stderr, stdout]).unwrap_or("no diagnostic output");
    let exit_suffix = exit_code
        .map(|code| format!("exit code {}", code))
        .unwrap_or_else(|| "execution error".to_string());

    Some(ToolDerivedMemoryDraft {
        memory_type: MemoryType::Fact,
        title: truncate_chars(
            &format!("Command failure: {}", truncate_chars(command, 48)),
            TITLE_MAX_CHARS,
        ),
        content: truncate_chars(
            &format!(
                "{} via {} failed with {}. Key diagnostic: {}.",
                if command == "command" {
                    "Command execution"
                } else {
                    command
                },
                tool_name,
                exit_suffix,
                detail
            ),
            CONTENT_MAX_CHARS,
        ),
        short_description: truncate_chars(
            &format!("{} failed: {}", command, detail),
            SHORT_DESCRIPTION_MAX_CHARS,
        ),
        importance: 0.74,
        confidence: 0.69,
        source: "episodic_extract_hook".to_string(),
        reason: format!(
            "captured failing {} result as a reusable failure fact",
            tool_name
        ),
        tags: vec![
            "fact".to_string(),
            "failure".to_string(),
            format!("tool:{tool_name}"),
            "exec".to_string(),
        ],
        captured_at: Utc::now(),
    })
}

fn parse_json(result: &str) -> Option<Value> {
    serde_json::from_str(result).ok()
}

fn string_field<'a>(payload: &'a Value, field: &str) -> Option<&'a str> {
    payload.get(field).and_then(Value::as_str).map(str::trim)
}

fn first_non_empty<'a>(values: &[&'a str]) -> Option<&'a str> {
    values
        .iter()
        .copied()
        .find(|value| !value.trim().is_empty())
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::{EpisodicExtractHook, RetrievalAdvisorHook};
    use crate::agent::hooks::{Hook, HookContext, HookEvent, HookResult};
    use crate::agent::memory::AgentMemory;
    use crate::agent::persistent_memory::MemoryBehaviorRuntime;
    use crate::agent::providers::TodoList;
    use crate::agent::session::AgentMemoryScope;
    use crate::llm::ToolDefinition;
    use oxide_agent_memory::MemoryType;

    fn tool(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: String::new(),
            parameters: serde_json::json!({"type": "object"}),
        }
    }

    fn context<'a>(
        scope: &'a AgentMemoryScope,
        runtime: &'a MemoryBehaviorRuntime,
        tools: &'a [ToolDefinition],
    ) -> HookContext<'a> {
        let todos = Box::leak(Box::new(TodoList::new()));
        let memory = Box::leak(Box::new(AgentMemory::new(4096)));
        HookContext::new(todos, memory, 0, 0, 4)
            .with_memory_scope(Some(scope))
            .with_memory_behavior(Some(runtime))
            .with_available_tools(tools)
    }

    #[test]
    fn retrieval_advisor_injects_history_card_for_topic_scope() {
        let hook = RetrievalAdvisorHook::new();
        let scope = AgentMemoryScope::new(7, "topic-a", "flow-a");
        let runtime = MemoryBehaviorRuntime::new();
        let tools = vec![
            tool("memory_search"),
            tool("memory_read_thread_summary"),
            tool("memory_read_thread_window"),
            tool("memory_read_episode"),
        ];

        let result = hook.handle(
            &HookEvent::BeforeAgent {
                prompt: "What happened earlier in this thread and why did it regress again?"
                    .to_string(),
            },
            &context(&scope, &runtime, &tools),
        );

        match result {
            HookResult::InjectTransientContext(text) => {
                assert!(text.contains("memory_search"));
                assert!(text.contains("memory_read_thread_summary"));
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn retrieval_advisor_omits_history_card_for_synthetic_scope() {
        let hook = RetrievalAdvisorHook::new();
        let scope = AgentMemoryScope::new(7, "session:123", "agent-mode");
        let runtime = MemoryBehaviorRuntime::new();
        let tools = vec![tool("memory_search"), tool("memory_read_thread_summary")];

        let result = hook.handle(
            &HookEvent::BeforeAgent {
                prompt: "Check what happened earlier in this thread".to_string(),
            },
            &context(&scope, &runtime, &tools),
        );

        match result {
            HookResult::InjectTransientContext(text) => {
                assert!(text.contains("memory_search"));
                assert!(!text.contains("memory_read_thread_summary"));
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn episodic_extract_captures_procedure_after_file_edit() {
        let hook = EpisodicExtractHook::new();
        let scope = AgentMemoryScope::new(7, "topic-a", "flow-a");
        let runtime = MemoryBehaviorRuntime::new();
        let tools = vec![tool("ssh_apply_file_edit")];

        let result = hook.handle(
            &HookEvent::AfterTool {
                tool_name: "ssh_apply_file_edit".to_string(),
                result: r#"{"ok":true,"path":"/etc/app/config.toml","status":"updated"}"#
                    .to_string(),
            },
            &context(&scope, &runtime, &tools),
        );

        assert!(matches!(result, HookResult::Continue));
        let drafts = runtime.snapshot();
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].memory_type, MemoryType::Procedure);
        assert!(drafts[0].content.contains("/etc/app/config.toml"));
    }

    #[test]
    fn episodic_extract_captures_failure_after_command_error() {
        let hook = EpisodicExtractHook::new();
        let scope = AgentMemoryScope::new(7, "topic-a", "flow-a");
        let runtime = MemoryBehaviorRuntime::new();
        let tools = vec![tool("execute_command")];

        hook.handle(
            &HookEvent::AfterTool {
                tool_name: "execute_command".to_string(),
                result: r#"{"ok":false,"command":"cargo test","error":"missing env"}"#.to_string(),
            },
            &context(&scope, &runtime, &tools),
        );

        let drafts = runtime.snapshot();
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].memory_type, MemoryType::Fact);
        assert!(drafts[0].content.contains("cargo test"));
    }

    #[test]
    fn episodic_extract_emits_preference_after_repeated_topic_edits() {
        let hook = EpisodicExtractHook::new();
        let scope = AgentMemoryScope::new(7, "topic-a", "flow-a");
        let runtime = MemoryBehaviorRuntime::new();
        let tools = vec![tool("ssh_apply_file_edit")];
        let context = context(&scope, &runtime, &tools);

        hook.handle(
            &HookEvent::AfterTool {
                tool_name: "ssh_apply_file_edit".to_string(),
                result: r#"{"ok":true,"path":"/etc/app/a.toml","status":"updated"}"#.to_string(),
            },
            &context,
        );
        hook.handle(
            &HookEvent::AfterTool {
                tool_name: "ssh_apply_file_edit".to_string(),
                result: r#"{"ok":true,"path":"/etc/app/b.toml","status":"updated"}"#.to_string(),
            },
            &context,
        );

        let drafts = runtime.snapshot();
        assert!(drafts
            .iter()
            .any(|draft| draft.memory_type == MemoryType::Preference));
    }
}
