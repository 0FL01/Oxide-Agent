//! Prompt builders for context compaction.

use super::history::{PreviousCompactedSummary, is_any_compaction_summary_message};
use crate::agent::memory::{AgentMessage, MessageRole};

const MESSAGE_REASONING_PREVIEW_CHARS: usize = 500;
const LOCAL_COMPACT_MESSAGE_PREVIEW_CHARS: usize = 1_600;

/// Build the system prompt for Codex-style local LLM summary compaction.
#[must_use]
pub fn local_compaction_system_prompt() -> &'static str {
    r#"You create a compact handoff summary so another model instance can continue the same Oxide-Agent session.

Do not answer the user. Do not call tools. Do not output JSON.

Preserve only facts that matter for continuing the session:
- current goal and progress
- key decisions and constraints
- user preferences
- relevant files, commands, entities, and runtime state
- pending actions and remaining work
- risks, blockers, approvals, and open questions

Use any previous compacted summary only as source signal. Do not summarize old summaries as separate facts.
Do not invent state. If something is uncertain, say it is uncertain.

Output a structured handoff using this format:

GENERATION: <number, starting from 1>
GOAL: <current goal and progress>
FINDINGS: <key findings, decisions, user preferences, relevant state>
BLOCKERS: <risks, blockers, approvals, open questions>
NEXT_STEPS: <pending actions and remaining work>"#
}

/// Build the user payload for Codex-style local LLM summary compaction.
#[must_use]
pub fn build_local_compaction_user_message(
    task: &str,
    previous_summary: Option<&PreviousCompactedSummary>,
    messages: &[AgentMessage],
) -> String {
    let previous_summary_section = previous_summary
        .map(|summary| summary.content.trim())
        .filter(|summary| !summary.is_empty())
        .unwrap_or("none");
    let mut sections = vec![
        format!("## Current Task\n{}", task.trim()),
        format!("## Previous Compacted Summary\n{previous_summary_section}"),
    ];

    let message_sections = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| !is_any_compaction_summary_message(message))
        .map(|(index, message)| {
            let role = match message.role {
                MessageRole::System => "system",
                MessageRole::User => "user",
                MessageRole::Assistant => "assistant",
                MessageRole::Tool => "tool",
            };
            let mut block = format!(
                "### Message {index}\nrole: {role}\nkind: {:?}\ncontent:\n{}",
                message.resolved_kind(),
                truncate_chars(&message.content, LOCAL_COMPACT_MESSAGE_PREVIEW_CHARS)
            );
            if let Some(reasoning) = message.reasoning.as_deref() {
                block.push_str("\nreasoning:\n");
                block.push_str(&truncate_chars(reasoning, MESSAGE_REASONING_PREVIEW_CHARS));
            }
            block
        })
        .collect::<Vec<_>>();

    sections.push(if message_sections.is_empty() {
        "## Source History\nNo non-summary source messages.".to_string()
    } else {
        format!("## Source History\n{}", message_sections.join("\n\n"))
    });

    sections.join("\n\n")
}

fn truncate_chars(text: &str, limit: usize) -> String {
    let trimmed = text.trim();
    let mut truncated: String = trimmed.chars().take(limit).collect();
    if trimmed.chars().count() > limit {
        truncated.push_str("...");
    }
    truncated
}
