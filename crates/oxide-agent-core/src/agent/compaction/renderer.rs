//! Compaction renderer — the only model-facing compaction boundary.
//!
//! Produces `Vec<llm::Message>` from raw `AgentMessage` transcript plus
//! `CompactionState` and `RenderPolicy`. When state is empty, output is
//! identical to the base `AgentMessage` → `Message` conversion (identity
//! rendering) — no ref tags, no strategies, no block summaries.
//!
//! When active blocks exist, the renderer:
//! 1. Injects block summary as a synthetic user message at each active block's
//!    anchor (first direct message index).
//! 2. Skips all messages covered by active blocks' effective indices.
//! 3. Applies dedup and purge-errors strategies to non-covered messages.
//! 4. Injects `<mNNNN>` ref tags into each rendered message for LLM reference.
//!
//! Raw messages are never mutated — rendering is a pure function of
//! (raw messages, CompactionState, RenderPolicy).

use super::block::{CompressionBlock, SummaryPart};
use super::state::CompactionState;
use super::strategy::{
    RenderPolicy, compute_purge_error_inputs, compute_superseded_tool_results, ref_tag,
};
use crate::agent::memory::{AgentMessage, MessageRole};
use crate::llm::Message;
use std::collections::{BTreeMap, BTreeSet};

/// Renders raw agent memory + compaction state into model-facing LLM messages.
pub struct CompactionRenderer;

impl CompactionRenderer {
    /// Render raw messages + compaction state + policy into model-facing `Vec<Message>`.
    ///
    /// When `CompactionState` is empty, this is identity-equivalent to
    /// the base `AgentMessage` → `Message` conversion.
    #[must_use]
    pub fn render(
        messages: &[AgentMessage],
        state: &CompactionState,
        policy: &RenderPolicy,
    ) -> Vec<Message> {
        if !state.has_active_blocks() {
            return Self::convert_messages(messages);
        }

        Self::render_with_blocks(messages, state, policy)
    }

    /// Render with active block overlay: inject summaries, skip covered, apply strategies, inject refs.
    fn render_with_blocks(
        messages: &[AgentMessage],
        state: &CompactionState,
        policy: &RenderPolicy,
    ) -> Vec<Message> {
        // 1. Compute covered indices and anchor→block map from active blocks.
        let mut covered: BTreeSet<usize> = BTreeSet::new();
        let mut anchor_block: BTreeMap<usize, &CompressionBlock> = BTreeMap::new();
        for block in state.blocks().values().filter(|b| b.is_active()) {
            let effective = block.effective_message_indices(state);
            covered.extend(effective);
            anchor_block.insert(block.anchor_index(), block);
        }

        // 2. Compute strategy decisions for non-covered messages.
        let superseded = compute_superseded_tool_results(messages, policy);
        let purge_inputs = compute_purge_error_inputs(messages, policy);

        // 3. Walk through raw messages and build rendered output.
        let mut result = Vec::new();
        for (index, msg) in messages.iter().enumerate() {
            if let Some(block) = anchor_block.get(&index) {
                // Inject block summary at anchor.
                let summary_text = Self::render_block_summary(block, state);
                result.push(Message {
                    role: "user".to_string(),
                    content: summary_text,
                    content_parts: Vec::new(),
                    reasoning_content: None,
                    tool_call_id: None,
                    tool_call_correlation: None,
                    name: None,
                    tool_calls: None,
                });
            } else if covered.contains(&index) {
                // Skip covered non-anchor message.
                continue;
            } else {
                // Render normally with strategy application and ref injection.
                let mut message = Self::convert_one(msg);

                // Apply dedup: replace superseded tool result content.
                if superseded.contains(&index) {
                    message.content =
                        "[Output removed: superseded by a newer tool call with the same signature.]"
                            .to_string();
                }
                // Apply purge-errors: strip tool call arguments.
                if purge_inputs.contains(&index)
                    && let Some(tool_calls) = message.tool_calls.as_mut()
                {
                    for tc in tool_calls {
                        tc.function.arguments = "[stripped: old errored tool call]".to_string();
                    }
                }

                // Inject ref tag.
                let tag = ref_tag(index);
                if message.content.is_empty() {
                    message.content = tag;
                } else {
                    message.content = format!("{}\n{tag}", message.content);
                }

                result.push(message);
            }
        }

        result
    }

    /// Render a block's summary text, expanding `SummaryPart::BlockRef` recursively
    /// and appending missing consumed block summaries.
    fn render_block_summary(block: &CompressionBlock, state: &CompactionState) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut referenced: BTreeSet<super::refs::BlockRef> = BTreeSet::new();

        // Expand summary parts.
        for part in block.summary() {
            match part {
                SummaryPart::Text(text) => parts.push(text.clone()),
                SummaryPart::BlockRef(br) => {
                    referenced.insert(*br);
                    if let Some(consumed) = state.blocks().get(br) {
                        parts.push(Self::render_block_summary(consumed, state));
                    }
                }
            }
        }

        // Append missing consumed block summaries (consumed but not referenced).
        for consumed_ref in block.consumed_block_refs() {
            if !referenced.contains(consumed_ref)
                && let Some(consumed) = state.blocks().get(consumed_ref)
            {
                parts.push(Self::render_block_summary(consumed, state));
            }
        }

        format!(
            "[Compressed conversation section]\n{}\n[/Compressed conversation section]",
            parts.join("\n\n")
        )
    }

    /// Base conversion: `AgentMessage` → `llm::Message` (1:1, no compaction overlay).
    fn convert_messages(messages: &[AgentMessage]) -> Vec<Message> {
        messages.iter().map(Self::convert_one).collect()
    }

    /// Convert a single `AgentMessage` → `llm::Message` (1:1, no overlay).
    fn convert_one(msg: &AgentMessage) -> Message {
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
            tool_call_id: None,
            tool_call_correlation: msg.resolved_tool_call_correlation(),
            name: msg.tool_name.clone(),
            tool_calls: msg.tool_calls.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::compaction::block::SummaryPart;
    use crate::agent::compaction::engine::CompactionEngine;
    use crate::agent::compaction::refs::{BlockRef, MessageRef};
    use crate::agent::compaction::state::CompactionState;
    use crate::agent::memory::AgentMessage;
    use crate::llm::{ToolCall, ToolCallFunction};

    // --- Helpers ---

    fn user_msg(content: &str) -> AgentMessage {
        AgentMessage::user_task(content)
    }

    fn assistant_msg(content: &str) -> AgentMessage {
        AgentMessage::assistant(content)
    }

    fn tool_call_msg(id: &str, tool_name: &str, args: &str) -> AgentMessage {
        let call = ToolCall::new(
            id.to_string(),
            ToolCallFunction {
                name: tool_name.to_string(),
                arguments: args.to_string(),
            },
            false,
        );
        AgentMessage::assistant_with_tools("calling tool", vec![call])
    }

    fn tool_result_msg(id: &str, tool_name: &str, content: &str) -> AgentMessage {
        AgentMessage::tool(id, tool_name, content)
    }

    fn text_summary(s: &str) -> Vec<SummaryPart> {
        vec![SummaryPart::Text(s.into())]
    }

    fn default_policy() -> RenderPolicy {
        RenderPolicy::default()
    }

    fn simple_messages() -> Vec<AgentMessage> {
        vec![
            user_msg("task"),        // 0
            assistant_msg("think"),  // 1
            assistant_msg("answer"), // 2
            user_msg("follow-up"),   // 3
            assistant_msg("reply"),  // 4
        ]
    }

    // --- Empty state (identity) tests ---

    #[test]
    fn empty_state_render_matches_base_conversion() {
        let messages = simple_messages();
        let state = CompactionState::default();
        let rendered = CompactionRenderer::render(&messages, &state, &default_policy());
        assert_eq!(rendered.len(), messages.len());
        for (rendered_msg, original) in rendered.iter().zip(messages.iter()) {
            assert_eq!(rendered_msg.content, original.content);
        }
    }

    #[test]
    fn render_preserves_roles() {
        let messages = simple_messages();
        let state = CompactionState::default();
        let rendered = CompactionRenderer::render(&messages, &state, &default_policy());
        assert_eq!(rendered[0].role, "user");
        assert_eq!(rendered[1].role, "assistant");
        assert_eq!(rendered[2].role, "assistant");
        assert_eq!(rendered[3].role, "user");
        assert_eq!(rendered[4].role, "assistant");
    }

    #[test]
    fn render_preserves_reasoning() {
        let mut msg = AgentMessage::assistant("thinking...");
        msg.reasoning = Some("internal reasoning".to_string());
        let messages = vec![msg];
        let state = CompactionState::default();
        let rendered = CompactionRenderer::render(&messages, &state, &default_policy());
        assert_eq!(
            rendered[0].reasoning_content,
            Some("internal reasoning".to_string())
        );
    }

    #[test]
    fn render_empty_messages() {
        let state = CompactionState::default();
        let rendered = CompactionRenderer::render(&[], &state, &default_policy());
        assert!(rendered.is_empty());
    }

    #[test]
    fn render_identity_with_agent_memory_rendered_messages() {
        use crate::agent::memory::AgentMemory;
        let mut memory = AgentMemory::new(4096);
        memory.add_message(AgentMessage::user_task("test task"));
        memory.add_message(AgentMessage::assistant("response"));

        let direct = CompactionRenderer::render(
            memory.get_messages(),
            memory.compaction_state(),
            &default_policy(),
        );
        let via_memory = memory.rendered_messages();
        assert_eq!(direct.len(), via_memory.len());
        for (d, m) in direct.iter().zip(via_memory.iter()) {
            assert_eq!(d.role, m.role);
            assert_eq!(d.content, m.content);
        }
    }

    // --- Block rendering tests ---

    fn create_block(
        state: &mut CompactionState,
        messages: &[AgentMessage],
        start: usize,
        end: usize,
        summary: &str,
    ) -> BlockRef {
        CompactionEngine::apply_compression_at(
            state,
            messages,
            &crate::agent::compaction::CompressionSelection::Range {
                start: MessageRef::from_index(start),
                end: MessageRef::from_index(end),
            },
            text_summary(summary),
            0,
        )
        .expect("test block creation succeeds")
    }

    #[test]
    fn block_render_injects_summary_at_anchor() {
        let messages = simple_messages();
        let mut state = CompactionState::default();
        let _b1 = create_block(&mut state, &messages, 1, 2, "compressed middle");

        let rendered = CompactionRenderer::render(&messages, &state, &default_policy());

        // 0: user task (with ref tag)
        // 1-2: covered by block → replaced by summary at anchor (index 1)
        // 3: user follow-up (with ref tag)
        // 4: assistant reply (with ref tag)
        assert_eq!(rendered.len(), 4); // 0, summary, 3, 4

        assert_eq!(rendered[0].role, "user");
        assert!(rendered[0].content.contains("task"));
        assert!(rendered[0].content.contains("<m0001>"));

        assert_eq!(rendered[1].role, "user");
        assert!(
            rendered[1]
                .content
                .contains("[Compressed conversation section]")
        );
        assert!(rendered[1].content.contains("compressed middle"));

        assert_eq!(rendered[2].role, "user");
        assert!(rendered[2].content.contains("follow-up"));
        assert!(rendered[2].content.contains("<m0004>"));

        assert_eq!(rendered[3].role, "assistant");
        assert!(rendered[3].content.contains("reply"));
        assert!(rendered[3].content.contains("<m0005>"));
    }

    #[test]
    fn block_render_preserves_raw_messages() {
        let messages = simple_messages();
        let original_count = messages.len();
        let mut state = CompactionState::default();
        let _b1 = create_block(&mut state, &messages, 1, 2, "summary");

        let _rendered = CompactionRenderer::render(&messages, &state, &default_policy());

        // Raw messages are unchanged
        assert_eq!(messages.len(), original_count);
        assert_eq!(messages[1].content, "think");
        assert_eq!(messages[2].content, "answer");
    }

    #[test]
    fn block_render_nested_consumption() {
        let messages = simple_messages();
        let mut state = CompactionState::default();

        // b1 covers [1, 2]
        let b1 = create_block(&mut state, &messages, 1, 2, "b1 summary");
        // b2 covers [0, 4] consuming b1, references b1 in summary
        let _b2 = CompactionEngine::apply_compression_at(
            &mut state,
            &messages,
            &crate::agent::compaction::CompressionSelection::Range {
                start: MessageRef::from_index(0),
                end: MessageRef::from_index(4),
            },
            vec![
                SummaryPart::Text("b2 outer".into()),
                SummaryPart::BlockRef(b1),
            ],
            0,
        )
        .expect("nested block creation succeeds");

        let rendered = CompactionRenderer::render(&messages, &state, &default_policy());

        // Only b2 is active, covering all 5 messages → 1 rendered message (summary)
        assert_eq!(rendered.len(), 1);
        assert!(
            rendered[0]
                .content
                .contains("[Compressed conversation section]")
        );
        assert!(rendered[0].content.contains("b2 outer"));
        // b1's summary should be expanded inline via BlockRef
        assert!(rendered[0].content.contains("b1 summary"));
    }

    #[test]
    fn block_render_missing_consumed_ref_appended() {
        let messages = simple_messages();
        let mut state = CompactionState::default();

        // b1 covers [1, 2]
        let _b1 = create_block(&mut state, &messages, 1, 2, "b1 appended summary");
        // b2 covers [0, 4] consuming b1, but does NOT reference b1 in summary
        let _b2 = CompactionEngine::apply_compression_at(
            &mut state,
            &messages,
            &crate::agent::compaction::CompressionSelection::Range {
                start: MessageRef::from_index(0),
                end: MessageRef::from_index(4),
            },
            text_summary("b2 without ref"), // no BlockRef to b1
            0,
        )
        .expect("outer block creation succeeds");

        let rendered = CompactionRenderer::render(&messages, &state, &default_policy());

        assert_eq!(rendered.len(), 1);
        assert!(rendered[0].content.contains("b2 without ref"));
        // b1's summary should be appended (missing consumed ref)
        assert!(rendered[0].content.contains("b1 appended summary"));
    }

    #[test]
    fn block_render_multiple_non_overlapping_blocks() {
        let messages = vec![
            user_msg("t1"),      // 0
            assistant_msg("a1"), // 1
            user_msg("t2"),      // 2
            assistant_msg("a2"), // 3
            user_msg("t3"),      // 4
            assistant_msg("a3"), // 5
        ];
        let mut state = CompactionState::default();
        let _b1 = create_block(&mut state, &messages, 1, 1, "sum a1");
        let _b2 = create_block(&mut state, &messages, 3, 3, "sum a2");

        let rendered = CompactionRenderer::render(&messages, &state, &default_policy());

        // 0: user t1 (ref)
        // 1: summary b1
        // 2: user t2 (ref)
        // 3: summary b2
        // 4: user t3 (ref)
        // 5: assistant a3 (ref)
        assert_eq!(rendered.len(), 6);
        assert!(rendered[1].content.contains("sum a1"));
        assert!(rendered[3].content.contains("sum a2"));
        assert!(rendered[0].content.contains("<m0001>"));
        assert!(rendered[5].content.contains("<m0006>"));
    }

    // --- Block rendering with tool batches ---

    #[test]
    fn block_render_includes_full_tool_batch() {
        let messages = vec![
            user_msg("task"),                        // 0
            tool_call_msg("c1", "test", "{}"),       // 1
            tool_result_msg("c1", "test", "result"), // 2
            assistant_msg("answer"),                 // 3
        ];
        let mut state = CompactionState::default();
        // Block covers [1, 2] — full tool batch
        let _b1 = create_block(&mut state, &messages, 1, 2, "tool batch summary");

        let rendered = CompactionRenderer::render(&messages, &state, &default_policy());

        // 0: user task (ref)
        // 1: summary (covers tool call + result)
        // 3: assistant answer (ref)
        assert_eq!(rendered.len(), 3);
        assert!(rendered[1].content.contains("tool batch summary"));
        assert_eq!(rendered[2].role, "assistant");
    }

    // --- Strategy application tests ---

    #[test]
    fn render_applies_dedup_to_superseded_tool_result() {
        let messages = vec![
            user_msg("read a file"),                             // 0
            tool_call_msg("c1", "read_file", r#"{"path":"a"}"#), // 1
            tool_result_msg("c1", "read_file", "content v1"),    // 2
            assistant_msg("checking"),                           // 3
            tool_call_msg("c2", "read_file", r#"{"path":"a"}"#), // 4
            tool_result_msg("c2", "read_file", "content v2"),    // 5
        ];

        // Create a block covering message [3] so state is non-empty and strategies run.
        let mut state = CompactionState::default();
        let _b1 = create_block(&mut state, &messages, 3, 3, "assistant check");

        let policy = RenderPolicy {
            turn_protection: 0,
            ..RenderPolicy::default()
        };
        let rendered = CompactionRenderer::render(&messages, &state, &policy);

        // Block covers only [3], so only index 3 is replaced by summary.
        // Rendered: [0, 1, 2 (superseded), summary_b1, 4, 5]
        assert_eq!(rendered.len(), 6);

        // Index 2 should have placeholder content (superseded).
        assert!(rendered[2].content.contains("[Output removed: superseded"));
        assert!(!rendered[2].content.contains("content v1"));

        // Index 5 should still have original content.
        assert!(rendered[5].content.contains("content v2"));
    }

    #[test]
    fn render_injects_refs_on_non_covered_messages() {
        let messages = simple_messages();
        let mut state = CompactionState::default();
        let _b1 = create_block(&mut state, &messages, 1, 2, "compressed");

        let rendered = CompactionRenderer::render(&messages, &state, &default_policy());

        // Non-covered messages should have ref tags.
        assert!(rendered[0].content.contains("<m0001>"));
        // The synthetic summary should NOT have a ref tag.
        assert!(!rendered[1].content.contains("<m"));
        // Non-covered messages after block.
        assert!(rendered[2].content.contains("<m0004>"));
        assert!(rendered[3].content.contains("<m0005>"));
    }

    #[test]
    fn render_no_refs_for_empty_state() {
        let messages = simple_messages();
        let state = CompactionState::default();
        let rendered = CompactionRenderer::render(&messages, &state, &default_policy());
        // No ref tags for empty state (identity rendering).
        for msg in &rendered {
            assert!(!msg.content.contains("<m"));
        }
    }
}
