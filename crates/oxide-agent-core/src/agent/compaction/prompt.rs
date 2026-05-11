//! Prompt builders for the compaction summary sidecar model.

use super::types::{CompactionRetention, CompactionSnapshot, CompactionSummary};
use crate::agent::memory::{AgentMessage, MessageRole};

const MESSAGE_CONTENT_PREVIEW_CHARS: usize = 2_000;
const MESSAGE_REASONING_PREVIEW_CHARS: usize = 500;

/// Build the system prompt for the compaction summary model.
#[must_use]
pub fn compaction_system_prompt() -> &'static str {
    r#"You update a structured summary of older AI-agent working history for later context compaction.

You will receive:
- the previous structured summary, if one already exists
- the new compactable history entries that are about to leave the hot context

Return ONLY valid JSON with this exact schema:
{
  "goal": "string",
  "constraints": ["string"],
  "decisions": ["string"],
  "discoveries": ["string"],
  "relevant_files_entities": ["string"],
  "remaining_work": ["string"],
  "risks": ["string"]
}

Rules:
- Return the full updated summary, not a delta.
- Use the previous structured summary as context when provided.
- Remove remaining work that is already completed by the new history.
- Remove risks that are no longer relevant.
- Merge overlapping decisions, discoveries, risks, and files/entities.
- Preserve technical facts, constraints, decisions, and unfinished work that still matter.
- Keep each list item short and information-dense.
- Do not invent files, commands, approvals, or conclusions.
- If a field has no evidence, return an empty string for goal or an empty array.
- Output JSON only. No markdown fences, no prose."#
}

/// Build the user payload for the compaction summary model.
#[must_use]
pub fn build_compaction_user_message(
    previous_summary: Option<&CompactionSummary>,
    snapshot: &CompactionSnapshot,
    messages: &[AgentMessage],
) -> String {
    let mut history_sections = Vec::new();

    for entry in &snapshot.entries {
        if entry.retention != CompactionRetention::CompactableHistory
            || entry.preserve_in_raw_window
        {
            continue;
        }

        let Some(message) = messages.get(entry.index) else {
            continue;
        };
        let role = match message.role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        };
        let mut block = format!(
            "## Entry {}\nrole: {}\nkind: {:?}\ntokens: {}\ncontent:\n{}",
            entry.index,
            role,
            entry.kind,
            entry.estimated_tokens,
            truncate_chars(&message.content, MESSAGE_CONTENT_PREVIEW_CHARS)
        );
        if let Some(reasoning) = message.reasoning.as_deref() {
            block.push_str("\nreasoning:\n");
            block.push_str(&truncate_chars(reasoning, MESSAGE_REASONING_PREVIEW_CHARS));
        }
        history_sections.push(block);
    }

    let previous_summary_section = previous_summary
        .and_then(|summary| serde_json::to_string_pretty(summary).ok())
        .filter(|summary| !summary.trim().is_empty())
        .unwrap_or_else(|| "none".to_string());
    let history_section = if history_sections.is_empty() {
        "No new compactable history entries.".to_string()
    } else {
        history_sections.join("\n\n")
    };

    format!(
        "## Previous Structured Summary\n{}\n\n## New Compactable History\n{}",
        previous_summary_section, history_section
    )
}

fn truncate_chars(text: &str, limit: usize) -> String {
    let trimmed = text.trim();
    let mut truncated: String = trimmed.chars().take(limit).collect();
    if trimmed.chars().count() > limit {
        truncated.push_str("...");
    }
    truncated
}

#[cfg(test)]
mod tests {
    use super::build_compaction_user_message;
    use crate::agent::compaction::classify_hot_memory;
    use crate::agent::compaction::CompactionSummary;
    use crate::agent::memory::AgentMessage;

    #[test]
    fn build_compaction_user_message_only_includes_old_compactable_history() {
        let messages = vec![
            AgentMessage::user_task("Pinned task"),
            AgentMessage::user("Older request"),
            AgentMessage::assistant("Older response"),
            AgentMessage::user("Recent request 1"),
            AgentMessage::assistant("Recent response 1"),
            AgentMessage::user("Recent request 2"),
            AgentMessage::assistant("Recent response 2"),
            AgentMessage::tool("call-1", "search", "tool output"),
        ];

        let snapshot = classify_hot_memory(&messages);
        let prompt = build_compaction_user_message(None, &snapshot, &messages);

        assert!(prompt.contains("## Previous Structured Summary"));
        assert!(prompt.contains("none"));
        assert!(prompt.contains("## New Compactable History"));
        assert!(prompt.contains("Older request"));
        assert!(prompt.contains("Older response"));
        assert!(!prompt.contains("Pinned task"));
        assert!(!prompt.contains("Recent request 1"));
        assert!(!prompt.contains("Recent response 2"));
        assert!(!prompt.contains("tool output"));
    }

    #[test]
    fn build_compaction_user_message_includes_previous_summary_as_json() {
        let messages = vec![
            AgentMessage::user("Older request"),
            AgentMessage::assistant("Older response"),
            AgentMessage::user("Recent request 1"),
            AgentMessage::assistant("Recent response 1"),
            AgentMessage::user("Recent request 2"),
            AgentMessage::assistant("Recent response 2"),
        ];
        let snapshot = classify_hot_memory(&messages);
        let previous_summary = CompactionSummary {
            goal: "Ship stage 9".to_string(),
            risks: vec!["Old risk".to_string()],
            ..CompactionSummary::default()
        };

        let prompt = build_compaction_user_message(Some(&previous_summary), &snapshot, &messages);

        assert!(prompt.contains("\"goal\": \"Ship stage 9\""));
        assert!(prompt.contains("\"risks\": [\n    \"Old risk\""));
        assert!(prompt.contains("Older request"));
    }
}
