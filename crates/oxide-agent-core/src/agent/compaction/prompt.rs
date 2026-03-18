//! Prompt builders for the compaction summary sidecar model.

use super::types::{CompactionRetention, CompactionSnapshot};
use crate::agent::memory::{AgentMessage, MessageRole};

const MESSAGE_CONTENT_PREVIEW_CHARS: usize = 2_000;
const MESSAGE_REASONING_PREVIEW_CHARS: usize = 500;

/// Build the system prompt for the compaction summary model.
#[must_use]
pub fn compaction_system_prompt() -> &'static str {
    r#"You summarize old AI-agent working history for later context compaction.

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
- Summarize only the provided compactable history.
- Preserve technical facts, constraints, decisions, and unfinished work.
- Keep each list item short and information-dense.
- Do not invent files, commands, approvals, or conclusions.
- If a field has no evidence, return an empty string for goal or an empty array.
- Output JSON only. No markdown fences, no prose."#
}

/// Build the user payload for the compaction summary model.
#[must_use]
pub fn build_compaction_user_message(
    snapshot: &CompactionSnapshot,
    messages: &[AgentMessage],
) -> String {
    let mut sections = Vec::new();

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
        sections.push(block);
    }

    if sections.is_empty() {
        "No compactable history entries.".to_string()
    } else {
        sections.join("\n\n")
    }
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
        let prompt = build_compaction_user_message(&snapshot, &messages);

        assert!(prompt.contains("Older request"));
        assert!(prompt.contains("Older response"));
        assert!(!prompt.contains("Pinned task"));
        assert!(!prompt.contains("Recent request 1"));
        assert!(!prompt.contains("Recent response 2"));
        assert!(!prompt.contains("tool output"));
    }
}
