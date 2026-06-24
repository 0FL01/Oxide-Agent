//! Compaction engine — the single mutation authority for `CompactionState`.
//!
//! The engine validates selections (tool-batch safety, non-overlap, summary
//! block refs) and creates/consumes blocks. It does NOT modify raw messages
//! or render anything — that is the renderer's job (Phase 4).
//!
//! All triggers (admission, pre-LLM budget, agent compress, manual,
//! model-downshift, typed overflow) go through this one engine.

use super::block::{CompressionBlock, CompressionSelection, SummaryPart, mark_consumed, new_block};
use super::refs::{BlockRef, MessageRef};
use super::state::CompactionState;
use crate::agent::compaction::AgentMessageKind;
use crate::agent::memory::AgentMessage;
use std::collections::{BTreeSet, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

/// Errors from the compaction engine.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompactionError {
    /// A message ref in the selection is stale or out of range.
    #[error("message ref {0} is stale or out of range")]
    InvalidMessageRef(MessageRef),
    /// Range start must be <= end.
    #[error("range start {start} must be <= end {end}")]
    InvalidRange {
        /// Start ref.
        start: MessageRef,
        /// End ref.
        end: MessageRef,
    },
    /// Selection contains no messages.
    #[error("selection is empty")]
    EmptySelection,
    /// Selection overlaps with an active block that is not being consumed.
    #[error("selection overlaps with active block {0}")]
    OverlapsActiveBlock(BlockRef),
    /// Selection splits a tool-call/result pair at the given index.
    #[error("selection splits tool-call/result pair at index {index}")]
    SplitsToolBatch {
        /// Index where the split occurs.
        index: usize,
    },
    /// Summary references a block that is not a consumed block.
    #[error("summary references block {0} which is not a consumed block")]
    InvalidBlockRef(BlockRef),
    /// Summary references the same block more than once.
    #[error("summary references block {0} more than once")]
    DuplicateBlockRef(BlockRef),
}

/// The compaction engine — sole mutation authority for `CompactionState`.
///
/// Stateless: all state lives in `CompactionState`. The engine validates
/// and mutates the state in a single `apply_compression` call.
pub struct CompactionEngine;

impl CompactionEngine {
    /// Apply a compression: validate, create a new block, consume existing blocks.
    ///
    /// Returns the new block's `BlockRef`. Does NOT modify raw messages or
    /// render anything — only mutates `CompactionState`.
    pub fn apply_compression(
        state: &mut CompactionState,
        messages: &[AgentMessage],
        selection: &CompressionSelection,
        summary: Vec<SummaryPart>,
    ) -> Result<BlockRef, CompactionError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Self::apply_compression_at(state, messages, selection, summary, now)
    }

    /// Internal entry point with explicit timestamp (for deterministic tests).
    pub(super) fn apply_compression_at(
        state: &mut CompactionState,
        messages: &[AgentMessage],
        selection: &CompressionSelection,
        summary: Vec<SummaryPart>,
        created_at_unix: i64,
    ) -> Result<BlockRef, CompactionError> {
        // 1. Resolve selection refs to indices.
        let new_indices = Self::resolve_selection(messages, selection)?;

        // 2. Validate tool-batch safety (selection must not split any batch).
        Self::validate_tool_batch_safety(messages, &new_indices)?;

        // 3. Find consumed blocks (active blocks whose direct indices are
        //    fully within the new selection).
        let consumed = Self::find_consumed_blocks(state, &new_indices);

        // 4. Validate non-overlap with active blocks that are NOT consumed.
        Self::validate_non_overlap(state, &new_indices, &consumed)?;

        // 5. Validate summary block refs (no invented or duplicate refs).
        Self::validate_summary_block_refs(&summary, &consumed)?;

        // 6. Allocate block id and create the block.
        let block_ref = state.allocate_block_id();
        let direct_indices: Vec<usize> = new_indices.into_iter().collect();
        let block = new_block(
            block_ref,
            direct_indices,
            consumed.clone(),
            summary,
            created_at_unix,
        );

        // 7. Mark consumed blocks as inactive.
        for consumed_ref in &consumed {
            if let Some(consumed_block) = state.blocks_mut().get_mut(consumed_ref) {
                mark_consumed(consumed_block, block_ref);
            }
        }

        // 8. Insert the new block.
        state.blocks_mut().insert(block_ref, block);

        Ok(block_ref)
    }

    // -----------------------------------------------------------------------
    // Step 1: Resolve selection
    // -----------------------------------------------------------------------

    fn resolve_selection(
        messages: &[AgentMessage],
        selection: &CompressionSelection,
    ) -> Result<BTreeSet<usize>, CompactionError> {
        let message_count = messages.len();
        match selection {
            CompressionSelection::Range { start, end } => {
                let start_idx = start
                    .resolve(message_count)
                    .ok_or(CompactionError::InvalidMessageRef(*start))?;
                let end_idx = end
                    .resolve(message_count)
                    .ok_or(CompactionError::InvalidMessageRef(*end))?;
                if start_idx > end_idx {
                    return Err(CompactionError::InvalidRange {
                        start: *start,
                        end: *end,
                    });
                }
                Ok((start_idx..=end_idx).collect())
            }
            CompressionSelection::Messages { refs } => {
                if refs.is_empty() {
                    return Err(CompactionError::EmptySelection);
                }
                let mut indices = BTreeSet::new();
                for reff in refs {
                    let idx = reff
                        .resolve(message_count)
                        .ok_or(CompactionError::InvalidMessageRef(*reff))?;
                    indices.insert(idx);
                }
                if indices.is_empty() {
                    return Err(CompactionError::EmptySelection);
                }
                Ok(indices)
            }
        }
    }

    // -----------------------------------------------------------------------
    // Step 2: Tool-batch safety
    // -----------------------------------------------------------------------

    /// Find all tool batches: (tool_call_index, last_tool_result_index).
    ///
    /// A tool batch is an `AssistantToolCall` followed by consecutive
    /// `ToolResult` messages. If the tool call has no results, the batch is
    /// just the tool call itself (terminal open batch).
    fn find_tool_batches(messages: &[AgentMessage]) -> Vec<(usize, usize)> {
        let mut batches = Vec::new();
        let mut i = 0;
        while i < messages.len() {
            if messages[i].resolved_kind() == AgentMessageKind::AssistantToolCall {
                let tc_start = i;
                i += 1;
                while i < messages.len()
                    && messages[i].resolved_kind() == AgentMessageKind::ToolResult
                {
                    i += 1;
                }
                batches.push((tc_start, i - 1));
            } else {
                i += 1;
            }
        }
        batches
    }

    /// Validate that the selection does not split any tool-call/result pair.
    fn validate_tool_batch_safety(
        messages: &[AgentMessage],
        indices: &BTreeSet<usize>,
    ) -> Result<(), CompactionError> {
        for (tc_start, tc_end) in Self::find_tool_batches(messages) {
            let batch_indices: BTreeSet<usize> = (tc_start..=tc_end).collect();
            let selected_in_batch: Vec<usize> =
                batch_indices.intersection(indices).copied().collect();

            if selected_in_batch.is_empty() {
                // Batch is fully outside the selection — OK.
                continue;
            }
            if selected_in_batch.len() == batch_indices.len() {
                // Batch is fully inside the selection — OK.
                continue;
            }
            // Partial overlap — the selection splits the batch.
            return Err(CompactionError::SplitsToolBatch {
                index: selected_in_batch[0],
            });
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Step 3: Find consumed blocks
    // -----------------------------------------------------------------------

    /// Find active blocks whose direct indices are fully within the new selection.
    fn find_consumed_blocks(
        state: &CompactionState,
        new_indices: &BTreeSet<usize>,
    ) -> Vec<BlockRef> {
        state
            .blocks()
            .values()
            .filter(|b| b.is_active())
            .filter(|b| {
                b.direct_message_indices()
                    .iter()
                    .all(|i| new_indices.contains(i))
            })
            .map(CompressionBlock::block_ref)
            .collect()
    }

    // -----------------------------------------------------------------------
    // Step 4: Non-overlap validation
    // -----------------------------------------------------------------------

    /// Validate that the selection does not overlap with active blocks
    /// that are not being consumed.
    fn validate_non_overlap(
        state: &CompactionState,
        new_indices: &BTreeSet<usize>,
        consumed: &[BlockRef],
    ) -> Result<(), CompactionError> {
        let consumed_set: HashSet<BlockRef> = consumed.iter().copied().collect();
        for block in state.blocks().values() {
            if !block.is_active() || consumed_set.contains(&block.block_ref()) {
                continue;
            }
            let effective = block.effective_message_indices(state);
            if !effective.is_disjoint(new_indices) {
                return Err(CompactionError::OverlapsActiveBlock(block.block_ref()));
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Step 5: Summary block ref validation
    // -----------------------------------------------------------------------

    /// Validate that summary BlockRef parts reference only consumed blocks
    /// and that no block ref is duplicated.
    ///
    /// Missing consumed refs are OK — the renderer (Phase 4) appends their
    /// summaries automatically. Invented or duplicate refs are rejected.
    fn validate_summary_block_refs(
        summary: &[SummaryPart],
        consumed: &[BlockRef],
    ) -> Result<(), CompactionError> {
        let consumed_set: HashSet<BlockRef> = consumed.iter().copied().collect();
        let mut seen = HashSet::new();

        for part in summary {
            if let SummaryPart::BlockRef(r) = part {
                if !consumed_set.contains(r) {
                    return Err(CompactionError::InvalidBlockRef(*r));
                }
                if !seen.insert(*r) {
                    return Err(CompactionError::DuplicateBlockRef(*r));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::compaction::refs::{BlockRef, MessageRef};
    use crate::agent::memory::AgentMessage;
    use crate::llm::{ToolCall, ToolCallFunction};

    // --- Test helpers ---

    fn user_msg(content: &str) -> AgentMessage {
        AgentMessage::user(content)
    }

    fn assistant_msg(content: &str) -> AgentMessage {
        AgentMessage::assistant(content)
    }

    fn tool_call_msg(content: &str) -> AgentMessage {
        let call = ToolCall::new(
            "call-test".to_string(),
            ToolCallFunction {
                name: "test_tool".to_string(),
                arguments: "{}".to_string(),
            },
            false,
        );
        AgentMessage::assistant_with_tools(content, vec![call])
    }

    fn tool_result_msg(content: &str) -> AgentMessage {
        AgentMessage::tool("call-test", "test_tool", content)
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

    fn messages_with_tool_batch() -> Vec<AgentMessage> {
        vec![
            user_msg("task"),           // 0
            tool_call_msg("call tool"), // 1
            tool_result_msg("result"),  // 2
            assistant_msg("answer"),    // 3
            user_msg("next"),           // 4
        ]
    }

    fn text_summary(s: &str) -> Vec<SummaryPart> {
        vec![SummaryPart::Text(s.into())]
    }

    // --- Step 1: resolve_selection ---

    #[test]
    fn resolve_range_valid() {
        let messages = simple_messages();
        let sel = CompressionSelection::Range {
            start: MessageRef::from_index(1),
            end: MessageRef::from_index(3),
        };
        let indices =
            CompactionEngine::resolve_selection(&messages, &sel).expect("valid range resolves");
        assert_eq!(indices, [1, 2, 3].into_iter().collect::<BTreeSet<_>>());
    }

    #[test]
    fn resolve_range_single_message() {
        let messages = simple_messages();
        let sel = CompressionSelection::Range {
            start: MessageRef::from_index(2),
            end: MessageRef::from_index(2),
        };
        let indices = CompactionEngine::resolve_selection(&messages, &sel)
            .expect("single-message range resolves");
        assert_eq!(indices, [2].into_iter().collect::<BTreeSet<_>>());
    }

    #[test]
    fn resolve_range_stale_ref() {
        let messages = simple_messages();
        let sel = CompressionSelection::Range {
            start: MessageRef::from_index(10),
            end: MessageRef::from_index(12),
        };
        assert_eq!(
            CompactionEngine::resolve_selection(&messages, &sel),
            Err(CompactionError::InvalidMessageRef(MessageRef::from_index(
                10
            )))
        );
    }

    #[test]
    fn resolve_range_inverted() {
        let messages = simple_messages();
        let sel = CompressionSelection::Range {
            start: MessageRef::from_index(3),
            end: MessageRef::from_index(1),
        };
        assert!(matches!(
            CompactionEngine::resolve_selection(&messages, &sel),
            Err(CompactionError::InvalidRange { .. })
        ));
    }

    #[test]
    fn resolve_messages_valid() {
        let messages = simple_messages();
        let sel = CompressionSelection::Messages {
            refs: vec![MessageRef::from_index(0), MessageRef::from_index(2)],
        };
        let indices = CompactionEngine::resolve_selection(&messages, &sel)
            .expect("message selection resolves");
        assert_eq!(indices, [0, 2].into_iter().collect::<BTreeSet<_>>());
    }

    #[test]
    fn resolve_messages_deduplicates() {
        let messages = simple_messages();
        let sel = CompressionSelection::Messages {
            refs: vec![
                MessageRef::from_index(1),
                MessageRef::from_index(1),
                MessageRef::from_index(2),
            ],
        };
        let indices = CompactionEngine::resolve_selection(&messages, &sel)
            .expect("duplicate refs are deduplicated");
        assert_eq!(indices, [1, 2].into_iter().collect::<BTreeSet<_>>());
    }

    #[test]
    fn resolve_messages_empty() {
        let messages = simple_messages();
        let sel = CompressionSelection::Messages { refs: vec![] };
        assert_eq!(
            CompactionEngine::resolve_selection(&messages, &sel),
            Err(CompactionError::EmptySelection)
        );
    }

    #[test]
    fn resolve_messages_stale_ref() {
        let messages = simple_messages();
        let sel = CompressionSelection::Messages {
            refs: vec![MessageRef::from_index(99)],
        };
        assert_eq!(
            CompactionEngine::resolve_selection(&messages, &sel),
            Err(CompactionError::InvalidMessageRef(MessageRef::from_index(
                99
            )))
        );
    }

    // --- Step 2: tool-batch safety ---

    #[test]
    fn tool_batch_safety_range_fully_includes_batch() {
        let messages = messages_with_tool_batch();
        // Range [1, 2] fully includes tool batch (1=call, 2=result)
        let indices = [1, 2].into_iter().collect::<BTreeSet<_>>();
        assert!(CompactionEngine::validate_tool_batch_safety(&messages, &indices).is_ok());
    }

    #[test]
    fn tool_batch_safety_range_excludes_batch() {
        let messages = messages_with_tool_batch();
        // Range [3, 4] fully excludes tool batch
        let indices = [3, 4].into_iter().collect::<BTreeSet<_>>();
        assert!(CompactionEngine::validate_tool_batch_safety(&messages, &indices).is_ok());
    }

    #[test]
    fn tool_batch_safety_range_starts_in_batch() {
        let messages = messages_with_tool_batch();
        // Range [2, 3] starts at tool result (middle of batch)
        let indices = [2, 3].into_iter().collect::<BTreeSet<_>>();
        assert!(matches!(
            CompactionEngine::validate_tool_batch_safety(&messages, &indices),
            Err(CompactionError::SplitsToolBatch { .. })
        ));
    }

    #[test]
    fn tool_batch_safety_range_ends_in_batch() {
        let messages = messages_with_tool_batch();
        // Range [0, 1] ends at tool call (missing result)
        let indices = [0, 1].into_iter().collect::<BTreeSet<_>>();
        assert!(matches!(
            CompactionEngine::validate_tool_batch_safety(&messages, &indices),
            Err(CompactionError::SplitsToolBatch { .. })
        ));
    }

    #[test]
    fn tool_batch_safety_messages_partial_batch() {
        let messages = messages_with_tool_batch();
        // Select only the tool call, not the result
        let indices = [1].into_iter().collect::<BTreeSet<_>>();
        assert!(matches!(
            CompactionEngine::validate_tool_batch_safety(&messages, &indices),
            Err(CompactionError::SplitsToolBatch { .. })
        ));
    }

    #[test]
    fn tool_batch_safety_messages_full_batch() {
        let messages = messages_with_tool_batch();
        // Select both tool call and result
        let indices = [1, 2].into_iter().collect::<BTreeSet<_>>();
        assert!(CompactionEngine::validate_tool_batch_safety(&messages, &indices).is_ok());
    }

    #[test]
    fn tool_batch_safety_no_batches() {
        let messages = simple_messages();
        let indices = [1, 2, 3].into_iter().collect::<BTreeSet<_>>();
        assert!(CompactionEngine::validate_tool_batch_safety(&messages, &indices).is_ok());
    }

    #[test]
    fn tool_batch_safety_multiple_batches() {
        let messages = vec![
            tool_call_msg("call1"),     // 0
            tool_result_msg("result1"), // 1
            assistant_msg("think"),     // 2
            tool_call_msg("call2"),     // 3
            tool_result_msg("result2"), // 4
        ];
        // Select [0, 1] — full first batch
        let indices = [0, 1].into_iter().collect::<BTreeSet<_>>();
        assert!(CompactionEngine::validate_tool_batch_safety(&messages, &indices).is_ok());
        // Select [3, 4] — full second batch
        let indices = [3, 4].into_iter().collect::<BTreeSet<_>>();
        assert!(CompactionEngine::validate_tool_batch_safety(&messages, &indices).is_ok());
        // Select [1, 2, 3] — splits both batches
        let indices = [1, 2, 3].into_iter().collect::<BTreeSet<_>>();
        assert!(matches!(
            CompactionEngine::validate_tool_batch_safety(&messages, &indices),
            Err(CompactionError::SplitsToolBatch { .. })
        ));
    }

    // --- Step 3+4: consumed blocks + non-overlap ---

    #[test]
    fn apply_compression_creates_block() {
        let mut state = CompactionState::default();
        let messages = simple_messages();
        let sel = CompressionSelection::Range {
            start: MessageRef::from_index(1),
            end: MessageRef::from_index(3),
        };
        let block_ref = CompactionEngine::apply_compression_at(
            &mut state,
            &messages,
            &sel,
            text_summary("s"),
            0,
        )
        .expect("compression creates block");
        assert_eq!(block_ref, BlockRef::new(1));
        assert!(state.has_active_blocks());
        let block = state
            .blocks()
            .get(&block_ref)
            .expect("created block exists");
        assert!(block.is_active());
        assert_eq!(block.direct_message_indices(), &[1, 2, 3]);
        assert!(block.consumed_block_refs().is_empty());
    }

    #[test]
    fn apply_compression_consumes_existing_block() {
        let mut state = CompactionState::default();
        let messages = simple_messages();

        // Create b1 covering [1, 2]
        let sel1 = CompressionSelection::Range {
            start: MessageRef::from_index(1),
            end: MessageRef::from_index(2),
        };
        let b1 = CompactionEngine::apply_compression_at(
            &mut state,
            &messages,
            &sel1,
            text_summary("b1"),
            0,
        )
        .expect("first compression creates block");

        // Create b2 covering [1, 4] — consumes b1
        let sel2 = CompressionSelection::Range {
            start: MessageRef::from_index(1),
            end: MessageRef::from_index(4),
        };
        let b2 = CompactionEngine::apply_compression_at(
            &mut state,
            &messages,
            &sel2,
            text_summary("b2"),
            0,
        )
        .expect("second compression consumes first block");

        // b1 should be inactive (consumed by b2)
        let block1 = state.blocks().get(&b1).expect("first block exists");
        assert!(!block1.is_active());
        assert_eq!(block1.deactivated_by_block_ref(), Some(b2));

        // b2 should be active and consume b1
        let block2 = state.blocks().get(&b2).expect("second block exists");
        assert!(block2.is_active());
        assert_eq!(block2.consumed_block_refs(), &[b1]);
    }

    #[test]
    fn apply_compression_rejects_overlap_with_active_block() {
        let mut state = CompactionState::default();
        let messages = simple_messages();

        // Create b1 covering [1, 2]
        let sel1 = CompressionSelection::Range {
            start: MessageRef::from_index(1),
            end: MessageRef::from_index(2),
        };
        CompactionEngine::apply_compression_at(&mut state, &messages, &sel1, text_summary("b1"), 0)
            .expect("first compression creates block");

        // Try to create b2 covering [2, 3] — partially overlaps b1
        let sel2 = CompressionSelection::Range {
            start: MessageRef::from_index(2),
            end: MessageRef::from_index(3),
        };
        let err = CompactionEngine::apply_compression_at(
            &mut state,
            &messages,
            &sel2,
            text_summary("b2"),
            0,
        )
        .expect_err("overlapping compression is rejected");
        assert!(matches!(err, CompactionError::OverlapsActiveBlock(_)));
    }

    #[test]
    fn apply_compression_allows_non_overlapping_block() {
        let mut state = CompactionState::default();
        let messages = simple_messages();

        // Create b1 covering [1, 2]
        let sel1 = CompressionSelection::Range {
            start: MessageRef::from_index(1),
            end: MessageRef::from_index(2),
        };
        CompactionEngine::apply_compression_at(&mut state, &messages, &sel1, text_summary("b1"), 0)
            .expect("first compression creates block");

        // Create b2 covering [3, 4] — no overlap
        let sel2 = CompressionSelection::Range {
            start: MessageRef::from_index(3),
            end: MessageRef::from_index(4),
        };
        let b2 = CompactionEngine::apply_compression_at(
            &mut state,
            &messages,
            &sel2,
            text_summary("b2"),
            0,
        )
        .expect("non-overlapping compression creates block");

        // Both blocks should be active
        assert!(
            state
                .blocks()
                .get(&BlockRef::new(1))
                .expect("first block exists")
                .is_active()
        );
        assert!(
            state
                .blocks()
                .get(&b2)
                .expect("second block exists")
                .is_active()
        );
    }

    // --- Step 5: summary block ref validation ---

    #[test]
    fn summary_valid_block_ref() {
        let mut state = CompactionState::default();
        let messages = simple_messages();

        // Create b1 covering [1, 2]
        let sel1 = CompressionSelection::Range {
            start: MessageRef::from_index(1),
            end: MessageRef::from_index(2),
        };
        let b1 = CompactionEngine::apply_compression_at(
            &mut state,
            &messages,
            &sel1,
            text_summary("b1"),
            0,
        )
        .expect("first compression creates block");

        // Create b2 covering [1, 4] consuming b1, with valid block ref in summary
        let sel2 = CompressionSelection::Range {
            start: MessageRef::from_index(1),
            end: MessageRef::from_index(4),
        };
        let summary = vec![
            SummaryPart::Text("new summary".into()),
            SummaryPart::BlockRef(b1),
        ];
        let b2 = CompactionEngine::apply_compression_at(&mut state, &messages, &sel2, summary, 0)
            .expect("summary with consumed block ref is accepted");

        let block2 = state.blocks().get(&b2).expect("second block exists");
        assert_eq!(block2.consumed_block_refs(), &[b1]);
    }

    #[test]
    fn summary_invented_block_ref_rejected() {
        let mut state = CompactionState::default();
        let messages = simple_messages();

        // No blocks exist yet — try to reference b1 in summary
        let sel = CompressionSelection::Range {
            start: MessageRef::from_index(1),
            end: MessageRef::from_index(3),
        };
        let summary = vec![
            SummaryPart::Text("summary".into()),
            SummaryPart::BlockRef(BlockRef::new(1)),
        ];
        let err = CompactionEngine::apply_compression_at(&mut state, &messages, &sel, summary, 0)
            .expect_err("invented block ref is rejected");
        assert_eq!(err, CompactionError::InvalidBlockRef(BlockRef::new(1)));
    }

    #[test]
    fn summary_duplicate_block_ref_rejected() {
        let mut state = CompactionState::default();
        let messages = simple_messages();

        // Create b1 covering [1, 2]
        let sel1 = CompressionSelection::Range {
            start: MessageRef::from_index(1),
            end: MessageRef::from_index(2),
        };
        let b1 = CompactionEngine::apply_compression_at(
            &mut state,
            &messages,
            &sel1,
            text_summary("b1"),
            0,
        )
        .expect("first compression creates block");

        // Create b2 covering [1, 4] with duplicate block ref
        let sel2 = CompressionSelection::Range {
            start: MessageRef::from_index(1),
            end: MessageRef::from_index(4),
        };
        let summary = vec![
            SummaryPart::BlockRef(b1),
            SummaryPart::Text("dup".into()),
            SummaryPart::BlockRef(b1),
        ];
        let err = CompactionEngine::apply_compression_at(&mut state, &messages, &sel2, summary, 0)
            .expect_err("duplicate block ref is rejected");
        assert_eq!(err, CompactionError::DuplicateBlockRef(b1));
    }

    #[test]
    fn summary_missing_consumed_ref_ok() {
        // Missing consumed refs are OK — the renderer appends them.
        let mut state = CompactionState::default();
        let messages = simple_messages();

        // Create b1 covering [1, 2]
        let sel1 = CompressionSelection::Range {
            start: MessageRef::from_index(1),
            end: MessageRef::from_index(2),
        };
        let b1 = CompactionEngine::apply_compression_at(
            &mut state,
            &messages,
            &sel1,
            text_summary("b1"),
            0,
        )
        .expect("first compression creates block");

        // Create b2 covering [1, 4] consuming b1, but don't reference b1 in summary
        let sel2 = CompressionSelection::Range {
            start: MessageRef::from_index(1),
            end: MessageRef::from_index(4),
        };
        let summary = text_summary("no block ref");
        let b2 = CompactionEngine::apply_compression_at(&mut state, &messages, &sel2, summary, 0)
            .expect("missing consumed ref is accepted");

        // Should succeed — b1 is still consumed
        let block2 = state.blocks().get(&b2).expect("second block exists");
        assert_eq!(block2.consumed_block_refs(), &[b1]);
    }

    // --- Integration: full apply_compression flow ---

    #[test]
    fn apply_compression_rejects_tool_batch_split() {
        let mut state = CompactionState::default();
        let messages = messages_with_tool_batch();

        // Range [2, 3] starts at tool result (splits batch)
        let sel = CompressionSelection::Range {
            start: MessageRef::from_index(2),
            end: MessageRef::from_index(3),
        };
        let err = CompactionEngine::apply_compression_at(
            &mut state,
            &messages,
            &sel,
            text_summary("s"),
            0,
        )
        .expect_err("tool-batch split is rejected");
        assert!(matches!(err, CompactionError::SplitsToolBatch { .. }));
        // State should not be modified
        assert!(!state.has_active_blocks());
    }

    #[test]
    fn apply_compression_rejects_stale_ref() {
        let mut state = CompactionState::default();
        let messages = simple_messages();
        let sel = CompressionSelection::Range {
            start: MessageRef::from_index(99),
            end: MessageRef::from_index(100),
        };
        let err = CompactionEngine::apply_compression_at(
            &mut state,
            &messages,
            &sel,
            text_summary("s"),
            0,
        )
        .expect_err("stale ref is rejected");
        assert!(matches!(err, CompactionError::InvalidMessageRef(_)));
    }

    #[test]
    fn apply_compression_message_mode() {
        let mut state = CompactionState::default();
        let messages = simple_messages();
        let sel = CompressionSelection::Messages {
            refs: vec![MessageRef::from_index(1), MessageRef::from_index(3)],
        };
        let block_ref = CompactionEngine::apply_compression_at(
            &mut state,
            &messages,
            &sel,
            text_summary("s"),
            0,
        )
        .expect("message-mode compression creates block");
        let block = state
            .blocks()
            .get(&block_ref)
            .expect("created block exists");
        assert_eq!(block.direct_message_indices(), &[1, 3]);
    }

    #[test]
    fn apply_compression_nested_consumption() {
        // b1 covers [0,1], b2 covers [0..3] consuming b1, b3 covers [0..4] consuming b2
        let mut state = CompactionState::default();
        let messages = simple_messages();

        let b1 = CompactionEngine::apply_compression_at(
            &mut state,
            &messages,
            &CompressionSelection::Range {
                start: MessageRef::from_index(0),
                end: MessageRef::from_index(1),
            },
            text_summary("b1"),
            0,
        )
        .expect("first nested compression creates block");

        let b2 = CompactionEngine::apply_compression_at(
            &mut state,
            &messages,
            &CompressionSelection::Range {
                start: MessageRef::from_index(0),
                end: MessageRef::from_index(2),
            },
            vec![SummaryPart::Text("b2".into()), SummaryPart::BlockRef(b1)],
            0,
        )
        .expect("second nested compression creates block");

        let b3 = CompactionEngine::apply_compression_at(
            &mut state,
            &messages,
            &CompressionSelection::Range {
                start: MessageRef::from_index(0),
                end: MessageRef::from_index(4),
            },
            vec![SummaryPart::Text("b3".into()), SummaryPart::BlockRef(b2)],
            0,
        )
        .expect("third nested compression creates block");

        // b1 and b2 are inactive, b3 is active
        assert!(
            !state
                .blocks()
                .get(&b1)
                .expect("first block exists")
                .is_active()
        );
        assert!(
            !state
                .blocks()
                .get(&b2)
                .expect("second block exists")
                .is_active()
        );
        assert!(
            state
                .blocks()
                .get(&b3)
                .expect("third block exists")
                .is_active()
        );

        // b3's effective indices include all messages via b2 → b1
        let effective = state
            .blocks()
            .get(&b3)
            .expect("third block exists")
            .effective_message_indices(&state);
        assert_eq!(
            effective,
            [0, 1, 2, 3, 4].into_iter().collect::<BTreeSet<_>>()
        );
    }

    #[test]
    fn apply_compression_no_mutation_on_error() {
        let mut state = CompactionState::default();
        let messages = messages_with_tool_batch();

        // Try to split a tool batch — should fail without allocating a block id
        let initial_next_id = state.next_block_id();
        let sel = CompressionSelection::Range {
            start: MessageRef::from_index(2),
            end: MessageRef::from_index(3),
        };
        let _ = CompactionEngine::apply_compression_at(
            &mut state,
            &messages,
            &sel,
            text_summary("s"),
            0,
        );
        // next_block_id should NOT have advanced (validation before allocation)
        assert_eq!(state.next_block_id(), initial_next_id);
        assert!(state.blocks().is_empty());
    }
}
