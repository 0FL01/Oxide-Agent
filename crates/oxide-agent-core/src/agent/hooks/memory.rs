//! Agent-native memory behavior hooks.

use super::registry::Hook;
use super::types::{HookContext, HookEvent, HookResult};
use crate::agent::compaction::AgentMessageKind;
use crate::agent::memory::{AgentMemory, MessageRole};
use crate::agent::memory_behavior::{
    MemoryBehaviorRuntime, ToolDerivedMemoryDraft, ToolDerivedMemoryKind, TopicMemoryPolicy,
};
use crate::agent::wiki_memory::planner::{
    extract_explicit_remember_payload, has_explicit_remember_intent,
};
use chrono::Utc;
use serde_json::Value;

const TITLE_MAX_CHARS: usize = 96;
const SHORT_DESCRIPTION_MAX_CHARS: usize = 160;
const CONTENT_MAX_CHARS: usize = 320;
const EVIDENCE_MAX_CHARS: usize = 220;
const EXPLICIT_REMEMBER_SOURCE: &str = "explicit_remember_capture";

/// Adds lightweight wiki-memory reminders when the task looks history- or policy-heavy.
pub struct RetrievalAdvisorHook;

impl RetrievalAdvisorHook {
    /// Create the durable wiki-memory retrieval advisor hook.
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
        if context.is_sub_agent {
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
                "- Durable wiki card: this request may depend on prior {} procedures, constraints, or decisions. Check the injected wiki memory before repeating work, and keep new durable facts concise enough to merge back into the wiki.",
                policy.context_label
            ));
        }
        if needs_history_card {
            lines.push(
                "- History card: if injected wiki context is insufficient, rely on current hot/session context rather than retired episode memory tools."
                    .to_string(),
            );
        }
        if normalized.contains("episode") {
            lines.push(
                "- Episode card: old typed episode lookup is disabled; preserve any still-relevant outcome as a concise wiki update instead."
                    .to_string(),
            );
        }

        HookResult::InjectTransientContext(lines.join("\n"))
    }
}

/// Captures wiki update candidates from selected tool calls without writing storage directly.
pub struct EpisodicExtractHook;

impl EpisodicExtractHook {
    /// Create the episodic extraction hook.
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
            kind: ToolDerivedMemoryKind::Preference,
            title: "Topic editing preference".to_string(),
            content: "In this topic, prefer incremental file changes over broad rewrites when updating code or configuration.".to_string(),
            short_description: "Prefer incremental file changes in this topic".to_string(),
            importance: 0.78,
            confidence: 0.7,
            source: "episodic_extract_hook".to_string(),
            reason: "repeated file-change pattern observed during one episode".to_string(),
            tags: vec![
                "preference".to_string(),
                "tool_pattern".to_string(),
                "file_edit".to_string(),
            ],
            evidence: vec![
                "Observed two successful file-change operations in one run.".to_string(),
            ],
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
        if context.is_sub_agent {
            return HookResult::Continue;
        }
        let Some(runtime) = context.memory_behavior else {
            return HookResult::Continue;
        };
        let policy = TopicMemoryPolicy::from_scope(context.memory_scope);

        match event {
            HookEvent::AfterTool { tool_name, result } => match tool_name.as_str() {
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
                "execute_command" | "ssh_exec" | "ssh_sudo_exec"
                    if policy.allow_failure_capture =>
                {
                    if let Some(draft) = failure_from_command_result(tool_name, result) {
                        runtime.record_draft(draft);
                    }
                }
                _ => {}
            },
            HookEvent::AfterAgent { response } => {
                if let Some(result) =
                    capture_explicit_remember_candidate(context, response, runtime)
                {
                    return result;
                }
            }
            _ => {}
        }

        HookResult::Continue
    }
}

fn capture_explicit_remember_candidate(
    context: &HookContext<'_>,
    response: &str,
    runtime: &MemoryBehaviorRuntime,
) -> Option<HookResult> {
    let task = latest_user_task(context.memory)?;
    if !has_explicit_remember_intent(task) {
        return None;
    }

    let evidence = collect_tool_evidence(context.memory);
    let volatile = requests_volatile_current_fact(task);
    if volatile && evidence.is_empty() {
        if !context.at_continuation_limit() {
            return Some(HookResult::ForceIteration {
                reason: "Volatile facts need tool verification before saving to durable wiki memory."
                    .to_string(),
                context: Some(
                    "Before finishing, verify the requested current/volatile fact with an available tool. Then answer with the exact verified value, observation time, and source so it can be saved correctly."
                        .to_string(),
                ),
            });
        }

        runtime.record_draft(explicit_remember_draft(
            task,
            response,
            evidence,
            0.45,
            "explicit remember request for current/volatile data reached the continuation limit without tool evidence",
            vec![
                "explicit-remember".to_string(),
                "fact".to_string(),
                "volatile".to_string(),
                "unverified".to_string(),
            ],
        ));
        return Some(HookResult::Continue);
    }

    if volatile && response_is_generic_confirmation(response) && !context.at_continuation_limit() {
        return Some(HookResult::ForceIteration {
            reason: "Verified volatile facts must be restated in the final answer before durable save."
                .to_string(),
            context: Some(
                "You already have enough evidence. State the exact verified value, observation time, and source in the final answer, then finish."
                    .to_string(),
            ),
        });
    }

    let confidence = if volatile { 0.9 } else { 0.88 };
    let mut tags = vec!["explicit-remember".to_string(), "fact".to_string()];
    if volatile {
        tags.push("volatile".to_string());
        tags.push("verified".to_string());
    }
    runtime.record_draft(explicit_remember_draft(
        task,
        response,
        evidence,
        confidence,
        "captured final explicit remember payload for durable wiki memory",
        tags,
    ));
    Some(HookResult::Continue)
}

fn explicit_remember_draft(
    task: &str,
    response: &str,
    evidence: Vec<String>,
    confidence: f32,
    reason: &str,
    tags: Vec<String>,
) -> ToolDerivedMemoryDraft {
    let content = explicit_remember_content(task, response, &evidence);
    ToolDerivedMemoryDraft {
        kind: ToolDerivedMemoryKind::Fact,
        title: truncate_chars(&title_from_content(&content), TITLE_MAX_CHARS),
        content: truncate_chars(&content, CONTENT_MAX_CHARS),
        short_description: truncate_chars(&content, SHORT_DESCRIPTION_MAX_CHARS),
        importance: 0.84,
        confidence,
        source: EXPLICIT_REMEMBER_SOURCE.to_string(),
        reason: reason.to_string(),
        tags,
        evidence,
        captured_at: Utc::now(),
    }
}

fn explicit_remember_content(task: &str, response: &str, evidence: &[String]) -> String {
    if !response_is_generic_confirmation(response) {
        return response.trim().to_string();
    }
    if let Some(payload) = extract_explicit_remember_payload(task)
        && !payload.trim().is_empty()
    {
        return payload;
    }
    if let Some(first_evidence) = evidence.first() {
        return format!(
            "Verified memory candidate for '{}'. See evidence: {}",
            task.trim(),
            first_evidence
        );
    }
    task.trim().to_string()
}

fn title_from_content(content: &str) -> String {
    let line = content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("explicit remember");
    let cleaned = line
        .trim_start_matches("TL;DR")
        .trim_start_matches(':')
        .trim();
    if cleaned.is_empty() {
        "explicit remember".to_string()
    } else {
        cleaned.to_string()
    }
}

fn latest_user_task(memory: &AgentMemory) -> Option<&str> {
    memory
        .get_messages()
        .iter()
        .rev()
        .find(|message| message.kind == AgentMessageKind::UserTask)
        .map(|message| message.content.trim())
        .filter(|task| !task.is_empty())
}

fn collect_tool_evidence(memory: &AgentMemory) -> Vec<String> {
    let messages = memory.get_messages();
    let start = messages
        .iter()
        .rposition(|message| message.kind == AgentMessageKind::UserTask)
        .map(|index| index + 1)
        .unwrap_or(0);

    messages[start..]
        .iter()
        .filter(|message| message.role == MessageRole::Tool)
        .filter_map(|message| {
            let tool_name = message.tool_name.as_deref().unwrap_or("tool");
            let preview = tool_result_preview(message)?;
            Some(truncate_chars(
                &format!("Tool: {tool_name}. Evidence: {preview}"),
                EVIDENCE_MAX_CHARS,
            ))
        })
        .take(3)
        .collect()
}

fn tool_result_preview(message: &crate::agent::memory::AgentMessage) -> Option<String> {
    let preview = message
        .externalized_payload
        .as_ref()
        .map(|payload| payload.preview.as_str())
        .or_else(|| {
            message
                .pruned_artifact
                .as_ref()
                .map(|artifact| artifact.preview.as_str())
        })
        .unwrap_or(message.content.as_str())
        .replace('\n', " ");
    let trimmed = preview.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn requests_volatile_current_fact(task: &str) -> bool {
    let normalized = task.to_lowercase();
    contains_any(
        &normalized,
        &[
            "current",
            "latest",
            "right now",
            "сейчас",
            "текущ",
            "актуаль",
            "курс",
            "price",
            "rate",
        ],
    )
}

fn response_is_generic_confirmation(response: &str) -> bool {
    let trimmed = response.trim();
    if trimmed.is_empty() {
        return true;
    }
    if trimmed.chars().any(|ch| ch.is_ascii_digit()) || trimmed.contains("http") {
        return false;
    }
    let normalized = trimmed.to_lowercase();
    if normalized.len() > 80 {
        return false;
    }
    contains_any(
        &normalized,
        &[
            "remembered",
            "saved",
            "memorized",
            "got it",
            "noted",
            "done",
            "запомнил",
            "запомнила",
            "сохранил",
            "сохранила",
            "записал",
            "записала",
        ],
    )
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
        kind: ToolDerivedMemoryKind::Procedure,
        title: truncate_chars("Sandbox file update workflow", TITLE_MAX_CHARS),
        content: truncate_chars(
            &format!(
                "When generating or replacing sandbox artifacts, write the target file directly at '{}' and verify the result before relying on it.",
                path
            ),
            CONTENT_MAX_CHARS,
        ),
        short_description: truncate_chars(
            &format!(
                "Direct sandbox write for {} ({} bytes)",
                path, bytes_written
            ),
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
        evidence: vec![truncate_chars(
            &format!("Tool: write_file. Path: {path}. Bytes written: {bytes_written}."),
            EVIDENCE_MAX_CHARS,
        )],
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
        kind: ToolDerivedMemoryKind::Procedure,
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
        reason: format!(
            "captured successful {} result as a reusable edit procedure",
            tool_name
        ),
        tags: vec![
            "procedure".to_string(),
            format!("tool:{tool_name}"),
            "file_edit".to_string(),
            "ssh".to_string(),
        ],
        evidence: vec![truncate_chars(
            &format!("Tool: {tool_name}. Path: {path}. Status: {status}."),
            EVIDENCE_MAX_CHARS,
        )],
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
        kind: ToolDerivedMemoryKind::Fact,
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
        evidence: vec![truncate_chars(
            &format!(
                "Tool: {tool_name}. Command: {command}. Outcome: {exit_suffix}. Detail: {detail}."
            ),
            EVIDENCE_MAX_CHARS,
        )],
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
    use crate::agent::memory::{AgentMemory, AgentMessage};
    use crate::agent::memory_behavior::{MemoryBehaviorRuntime, ToolDerivedMemoryKind};
    use crate::agent::providers::TodoList;
    use crate::agent::session::AgentMemoryScope;
    use crate::llm::{ToolCall, ToolCallFunction, ToolDefinition};

    fn tool(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: String::new(),
            parameters: serde_json::json!({"type": "object"}),
        }
    }

    fn tool_call(name: &str) -> ToolCall {
        ToolCall::new(
            "call-1",
            ToolCallFunction {
                name: name.to_string(),
                arguments: "{}".to_string(),
            },
            false,
        )
    }

    fn context<'a>(
        scope: &'a AgentMemoryScope,
        runtime: &'a MemoryBehaviorRuntime,
        tools: &'a [ToolDefinition],
        memory: &'a AgentMemory,
        continuation_count: usize,
    ) -> HookContext<'a> {
        let todos = Box::leak(Box::new(TodoList::new()));
        HookContext::new(todos, memory, 0, continuation_count, 4)
            .with_memory_scope(Some(scope))
            .with_memory_behavior(Some(runtime))
            .with_available_tools(tools)
    }

    fn memory_with_messages(messages: Vec<AgentMessage>) -> &'static AgentMemory {
        let mut memory = AgentMemory::new(4096);
        for message in messages {
            memory.add_message(message);
        }
        Box::leak(Box::new(memory))
    }

    #[test]
    fn retrieval_advisor_injects_history_card_for_topic_scope() {
        let hook = RetrievalAdvisorHook::new();
        let scope = AgentMemoryScope::new(7, "topic-a", "flow-a");
        let runtime = MemoryBehaviorRuntime::new();
        let tools = Vec::new();
        let memory = memory_with_messages(Vec::new());

        let result = hook.handle(
            &HookEvent::BeforeAgent {
                prompt: "What happened earlier in this thread and why did it regress again?"
                    .to_string(),
            },
            &context(&scope, &runtime, &tools, memory, 0),
        );

        match result {
            HookResult::InjectTransientContext(text) => {
                assert!(text.contains("Durable wiki card"));
                assert!(text.contains("History card"));
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn retrieval_advisor_omits_history_card_for_synthetic_scope() {
        let hook = RetrievalAdvisorHook::new();
        let scope = AgentMemoryScope::new(7, "session:123", "agent-mode");
        let runtime = MemoryBehaviorRuntime::new();
        let tools = Vec::new();
        let memory = memory_with_messages(Vec::new());

        let result = hook.handle(
            &HookEvent::BeforeAgent {
                prompt: "Check what happened earlier in this thread".to_string(),
            },
            &context(&scope, &runtime, &tools, memory, 0),
        );

        match result {
            HookResult::InjectTransientContext(text) => {
                assert!(text.contains("Durable wiki card"));
                assert!(!text.contains("History card"));
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
        let memory = memory_with_messages(Vec::new());

        let result = hook.handle(
            &HookEvent::AfterTool {
                tool_name: "ssh_apply_file_edit".to_string(),
                result: r#"{"ok":true,"path":"/etc/app/config.toml","status":"updated"}"#
                    .to_string(),
            },
            &context(&scope, &runtime, &tools, memory, 0),
        );

        assert!(matches!(result, HookResult::Continue));
        let drafts = runtime.snapshot();
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].kind, ToolDerivedMemoryKind::Procedure);
        assert!(drafts[0].content.contains("/etc/app/config.toml"));
    }

    #[test]
    fn episodic_extract_ignores_command_failures_by_default() {
        let hook = EpisodicExtractHook::new();
        let scope = AgentMemoryScope::new(7, "topic-a", "flow-a");
        let runtime = MemoryBehaviorRuntime::new();
        let tools = vec![tool("execute_command")];
        let memory = memory_with_messages(Vec::new());

        hook.handle(
            &HookEvent::AfterTool {
                tool_name: "execute_command".to_string(),
                result: r#"{"ok":false,"command":"cargo test","error":"missing env"}"#.to_string(),
            },
            &context(&scope, &runtime, &tools, memory, 0),
        );

        let drafts = runtime.snapshot();
        assert!(drafts.is_empty());
    }

    #[test]
    fn episodic_extract_emits_preference_after_repeated_topic_edits() {
        let hook = EpisodicExtractHook::new();
        let scope = AgentMemoryScope::new(7, "topic-a", "flow-a");
        let runtime = MemoryBehaviorRuntime::new();
        let tools = vec![tool("ssh_apply_file_edit")];
        let memory = memory_with_messages(Vec::new());
        let context = context(&scope, &runtime, &tools, memory, 0);

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
        assert!(
            drafts
                .iter()
                .any(|draft| draft.kind == ToolDerivedMemoryKind::Preference)
        );
    }

    #[test]
    fn episodic_extract_forces_verification_before_saving_volatile_fact() {
        let hook = EpisodicExtractHook::new();
        let scope = AgentMemoryScope::new(7, "topic-a", "flow-a");
        let runtime = MemoryBehaviorRuntime::new();
        let tools = vec![tool("tavily_search")];
        let memory = memory_with_messages(vec![AgentMessage::user_task(
            "Сохрани текущий курс BTC USDT в память",
        )]);

        let result = hook.handle(
            &HookEvent::AfterAgent {
                response: "Сохранил".to_string(),
            },
            &context(&scope, &runtime, &tools, memory, 0),
        );

        assert!(matches!(result, HookResult::ForceIteration { .. }));
        assert!(runtime.snapshot().is_empty());
    }

    #[test]
    fn episodic_extract_captures_explicit_final_answer_with_tool_evidence() {
        let hook = EpisodicExtractHook::new();
        let scope = AgentMemoryScope::new(7, "topic-a", "flow-a");
        let runtime = MemoryBehaviorRuntime::new();
        let tools = vec![tool("tavily_search")];
        let memory = memory_with_messages(vec![
            AgentMessage::user_task("Сохрани текущий курс BTC USDT в память"),
            AgentMessage::assistant_with_tools("searching", vec![tool_call("tavily_search")]),
            AgentMessage::tool(
                "call-1",
                "tavily_search",
                "BTC/USDT = 80 104.40 from https://example.com",
            ),
        ]);

        let result = hook.handle(
            &HookEvent::AfterAgent {
                response: "BTC/USDT = 80 104.40. Source: https://example.com".to_string(),
            },
            &context(&scope, &runtime, &tools, memory, 0),
        );

        assert!(matches!(result, HookResult::Continue));
        let drafts = runtime.snapshot();
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].source, "explicit_remember_capture");
        assert!(drafts[0].content.contains("80 104.40"));
        assert!(!drafts[0].evidence.is_empty());
    }

    #[test]
    fn episodic_extract_forces_restatement_when_verified_response_is_generic() {
        let hook = EpisodicExtractHook::new();
        let scope = AgentMemoryScope::new(7, "topic-a", "flow-a");
        let runtime = MemoryBehaviorRuntime::new();
        let tools = vec![tool("tavily_search")];
        let memory = memory_with_messages(vec![
            AgentMessage::user_task("Сохрани текущий курс BTC USDT в память"),
            AgentMessage::assistant_with_tools("searching", vec![tool_call("tavily_search")]),
            AgentMessage::tool(
                "call-1",
                "tavily_search",
                "BTC/USDT = 80 104.40 from https://example.com",
            ),
        ]);

        let result = hook.handle(
            &HookEvent::AfterAgent {
                response: "Сохранил".to_string(),
            },
            &context(&scope, &runtime, &tools, memory, 0),
        );

        assert!(matches!(result, HookResult::ForceIteration { .. }));
        assert!(runtime.snapshot().is_empty());
    }

    #[test]
    fn episodic_extract_downgrades_unverified_volatile_fact_at_continuation_limit() {
        let hook = EpisodicExtractHook::new();
        let scope = AgentMemoryScope::new(7, "topic-a", "flow-a");
        let runtime = MemoryBehaviorRuntime::new();
        let tools = vec![tool("tavily_search")];
        let memory = memory_with_messages(vec![AgentMessage::user_task(
            "Сохрани текущий курс BTC USDT в память",
        )]);

        let result = hook.handle(
            &HookEvent::AfterAgent {
                response: "BTC/USDT = 80 104.40".to_string(),
            },
            &context(&scope, &runtime, &tools, memory, 4),
        );

        assert!(matches!(result, HookResult::Continue));
        let drafts = runtime.snapshot();
        assert_eq!(drafts.len(), 1);
        assert!(drafts[0].tags.iter().any(|tag| tag == "unverified"));
        assert_eq!(drafts[0].confidence, 0.45);
    }
}
