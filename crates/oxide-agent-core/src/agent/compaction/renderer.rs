//! Compaction renderer — the only model-facing compaction boundary.
//!
//! Produces `Vec<llm::Message>` from raw `AgentMessage` transcript plus
//! `CompactionState`. When state is empty, output is identical to the
//! base `AgentMessage` → `Message` conversion (identity rendering).
//!
//! Later phases add: block summary injection at anchors, message skipping
//! for compacted ranges, dedup/purge strategy application, and stable ref
//! injection.

use super::state::CompactionState;
use crate::agent::memory::{AgentMessage, MessageRole};
use crate::llm::Message;

/// Renders raw agent memory + compaction state into model-facing LLM messages.
pub struct CompactionRenderer;

impl CompactionRenderer {
    /// Render raw messages + compaction state into model-facing `Vec<Message>`.
    ///
    /// When `CompactionState` is empty, this is identity-equivalent to
    /// the base `AgentMessage` → `Message` conversion.
    #[must_use]
    pub fn render(messages: &[AgentMessage], state: &CompactionState) -> Vec<Message> {
        if state.is_empty() {
            Self::convert_messages(messages)
        } else {
            // Phase 4: apply compaction overlay (block summaries, pruning, refs)
            Self::convert_messages(messages)
        }
    }

    /// Base conversion: `AgentMessage` → `llm::Message` (1:1, no compaction overlay).
    fn convert_messages(messages: &[AgentMessage]) -> Vec<Message> {
        messages
            .iter()
            .map(|msg| {
                let role = match msg.role {
                    MessageRole::User => "user",
                    MessageRole::Assistant => "assistant",
                    MessageRole::System => "system",
                    MessageRole::Tool => "tool",
                };
                Message {
                    role: role.to_string(),
                    content: msg.content.clone(),
                    content_parts: Vec::new(),
                    reasoning_content: msg.reasoning.clone(),
                    tool_call_id: msg.tool_call_id.clone(),
                    tool_call_correlation: msg.resolved_tool_call_correlation(),
                    name: msg.tool_name.clone(),
                    tool_calls: msg.tool_calls.clone(),
                    tool_call_correlations: msg.resolved_tool_call_correlations(),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::memory::AgentMessage;

    fn sample_messages() -> Vec<AgentMessage> {
        vec![
            AgentMessage::user_task("Hello"),
            AgentMessage::assistant("Hi there"),
            AgentMessage::user_task("Do something"),
        ]
    }

    #[test]
    fn empty_state_render_matches_base_conversion() {
        let messages = sample_messages();
        let state = CompactionState::default();
        let rendered = CompactionRenderer::render(&messages, &state);
        assert_eq!(rendered.len(), messages.len());
        for (rendered_msg, original) in rendered.iter().zip(messages.iter()) {
            assert_eq!(rendered_msg.content, original.content);
        }
    }

    #[test]
    fn render_preserves_roles() {
        let messages = sample_messages();
        let state = CompactionState::default();
        let rendered = CompactionRenderer::render(&messages, &state);
        assert_eq!(rendered[0].role, "user");
        assert_eq!(rendered[1].role, "assistant");
        assert_eq!(rendered[2].role, "user");
    }

    #[test]
    fn render_preserves_reasoning() {
        let mut msg = AgentMessage::assistant("thinking...");
        msg.reasoning = Some("internal reasoning".to_string());
        let messages = vec![msg];
        let state = CompactionState::default();
        let rendered = CompactionRenderer::render(&messages, &state);
        assert_eq!(
            rendered[0].reasoning_content,
            Some("internal reasoning".to_string())
        );
    }

    #[test]
    fn render_empty_messages() {
        let state = CompactionState::default();
        let rendered = CompactionRenderer::render(&[], &state);
        assert!(rendered.is_empty());
    }

    #[test]
    fn render_identity_with_agent_memory_rendered_messages() {
        // AgentMemory::rendered_messages() should produce the same Vec<Message>
        // as CompactionRenderer::render with the memory's compaction state.
        use crate::agent::memory::AgentMemory;
        let mut memory = AgentMemory::new(4096);
        memory.add_message(AgentMessage::user_task("test task"));
        memory.add_message(AgentMessage::assistant("response"));

        let direct = CompactionRenderer::render(memory.get_messages(), memory.compaction_state());
        let via_memory = memory.rendered_messages();
        assert_eq!(direct.len(), via_memory.len());
        for (d, m) in direct.iter().zip(via_memory.iter()) {
            assert_eq!(d.role, m.role);
            assert_eq!(d.content, m.content);
        }
    }
}
