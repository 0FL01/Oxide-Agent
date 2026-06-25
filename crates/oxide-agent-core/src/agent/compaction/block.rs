//! Compression block — the core data structure for range/message compression.
//!
//! A `CompressionBlock` records a set of raw messages that were compressed into
//! a summary. Blocks form a nesting graph: a new block can consume existing
//! active blocks, subsuming their ranges. Consumed blocks become inactive.
//!
//! The renderer (Phase 4) uses the block graph to decide which messages to skip
//! and where to inject summaries. The engine (Phase 3) is the only component
//! that creates, activates, and deactivates blocks.

use super::refs::{BlockRef, MessageRef};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use super::state::CompactionState;

/// Structured summary part — replaces free-form text with typed nodes.
///
/// The LLM produces a sequence of `SummaryPart` values in the compress tool
/// call. The engine validates `BlockRef` parts against consumed blocks. The
/// renderer (Phase 4) expands `BlockRef` parts into the consumed block's
/// summary text.
///
/// This replaces DCP's regex-based `(bN)` placeholder parsing with a typed
/// AST — no string matching over LLM output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SummaryPart {
    /// Plain text summary content.
    Text(String),
    /// Reference to a consumed block whose summary should be nested inline.
    BlockRef(BlockRef),
}

/// Selection of messages to compress, expressed in stable `MessageRef` form.
///
/// The LLM provides refs (not internal indices); the engine resolves them.
/// Both modes resolve to a set of raw message indices for block creation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompressionSelection {
    /// Compress a contiguous range of messages.
    Range {
        /// First message in the range (inclusive).
        start: MessageRef,
        /// Last message in the range (inclusive).
        end: MessageRef,
    },
    /// Compress specific individual messages (not necessarily contiguous).
    Messages {
        /// Message refs to compress.
        refs: Vec<MessageRef>,
    },
}

/// A compression block — records a set of raw messages replaced by a summary.
///
/// Created by `CompactionEngine::apply_compression`. Stored in
/// `CompactionState::blocks`. Active blocks are visible to the renderer;
/// consumed blocks are inactive (subsumed by a newer block).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompressionBlock {
    /// Block reference (`b1`, `b2`, …).
    block_ref: BlockRef,
    /// Whether this block is active (its summary is injected in rendered context).
    /// Set to `false` when consumed by a newer block.
    #[serde(default = "default_active")]
    active: bool,
    /// Direct message indices covered by this block.
    /// For range mode: contiguous range. For message mode: specific indices.
    direct_message_indices: Vec<usize>,
    /// Block refs consumed (nested) by this block.
    /// Their effective indices are subsumed by this block.
    #[serde(default)]
    consumed_block_refs: Vec<BlockRef>,
    /// Block ref that deactivated this block (if consumed by a newer block).
    #[serde(default)]
    deactivated_by_block_ref: Option<BlockRef>,
    /// Structured summary parts (text + nested block references).
    summary: Vec<SummaryPart>,
    /// Creation timestamp (Unix seconds).
    created_at_unix: i64,
}

fn default_active() -> bool {
    true
}

impl CompressionBlock {
    /// Block reference.
    #[must_use]
    pub const fn block_ref(&self) -> BlockRef {
        self.block_ref
    }

    /// Whether this block is active.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    /// Direct message indices covered by this block.
    #[must_use]
    pub fn direct_message_indices(&self) -> &[usize] {
        &self.direct_message_indices
    }

    /// Consumed block refs.
    #[must_use]
    pub fn consumed_block_refs(&self) -> &[BlockRef] {
        &self.consumed_block_refs
    }

    /// Block ref that deactivated this block, if consumed by a newer block.
    #[must_use]
    pub fn deactivated_by_block_ref(&self) -> Option<BlockRef> {
        self.deactivated_by_block_ref
    }

    /// Summary parts.
    #[must_use]
    pub fn summary(&self) -> &[SummaryPart] {
        &self.summary
    }

    /// Anchor index — where the summary is injected in rendered context.
    /// Always the first direct message index.
    #[must_use]
    pub fn anchor_index(&self) -> usize {
        *self.direct_message_indices.first().unwrap_or(&0)
    }

    /// Compute effective message indices: direct + all consumed blocks' effective.
    ///
    /// Walks the consumed-block graph recursively. For personal-use scale
    /// (small block graphs), on-the-fly computation is correct and avoids
    /// redundant stored data that could drift.
    #[must_use]
    pub fn effective_message_indices(&self, state: &CompactionState) -> BTreeSet<usize> {
        let mut indices: BTreeSet<usize> = self.direct_message_indices.iter().copied().collect();
        for consumed_ref in &self.consumed_block_refs {
            if let Some(consumed) = state.blocks().get(consumed_ref) {
                indices.extend(consumed.effective_message_indices(state));
            }
        }
        indices
    }

    /// Collect all block refs transitively consumed by this block (including self).
    ///
    /// Used for diagnostics and summary validation.
    #[must_use]
    pub fn transitive_consumed_refs(&self, state: &CompactionState) -> BTreeSet<BlockRef> {
        let mut refs = BTreeSet::new();
        refs.insert(self.block_ref);
        for consumed_ref in &self.consumed_block_refs {
            if let Some(consumed) = state.blocks().get(consumed_ref) {
                refs.extend(consumed.transitive_consumed_refs(state));
            }
        }
        refs
    }
}

/// Internal constructor used by the engine.
pub(super) fn new_block(
    block_ref: BlockRef,
    direct_message_indices: Vec<usize>,
    consumed_block_refs: Vec<BlockRef>,
    summary: Vec<SummaryPart>,
    created_at_unix: i64,
) -> CompressionBlock {
    CompressionBlock {
        block_ref,
        active: true,
        direct_message_indices,
        consumed_block_refs,
        deactivated_by_block_ref: None,
        summary,
        created_at_unix,
    }
}

/// Mark a block as consumed by a newer block (engine internal).
pub(super) fn mark_consumed(block: &mut CompressionBlock, consumed_by: BlockRef) {
    block.active = false;
    block.deactivated_by_block_ref = Some(consumed_by);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::compaction::refs::BlockRef;

    fn dummy_block(ref_n: u32, indices: Vec<usize>, consumed: Vec<BlockRef>) -> CompressionBlock {
        new_block(
            BlockRef::new(ref_n),
            indices,
            consumed,
            vec![SummaryPart::Text("summary".into())],
            0,
        )
    }

    #[test]
    fn block_is_active_by_default() {
        let block = dummy_block(1, vec![0, 1, 2], vec![]);
        assert!(block.is_active());
        assert_eq!(block.deactivated_by_block_ref, None);
    }

    #[test]
    fn anchor_index_is_first_direct() {
        let block = dummy_block(1, vec![3, 4, 5], vec![]);
        assert_eq!(block.anchor_index(), 3);
    }

    #[test]
    fn effective_indices_no_consumed() {
        let state = CompactionState::default();
        let block = dummy_block(1, vec![2, 3, 4], vec![]);
        let effective = block.effective_message_indices(&state);
        assert_eq!(effective, [2, 3, 4].into_iter().collect::<BTreeSet<_>>());
    }

    #[test]
    fn effective_indices_with_consumed() {
        let mut state = CompactionState::default();
        let b1 = new_block(
            BlockRef::new(1),
            vec![0, 1],
            vec![],
            vec![SummaryPart::Text("b1".into())],
            0,
        );
        state.blocks_mut().insert(BlockRef::new(1), b1);

        let b2 = dummy_block(2, vec![2, 3, 4], vec![BlockRef::new(1)]);
        let effective = b2.effective_message_indices(&state);
        assert_eq!(
            effective,
            [0, 1, 2, 3, 4].into_iter().collect::<BTreeSet<_>>()
        );
    }

    #[test]
    fn effective_indices_nested_consumption() {
        // b1 covers [0,1], b2 covers [2,3] consuming b1, b3 covers [0..=5] consuming b2
        let mut state = CompactionState::default();
        let b1 = new_block(
            BlockRef::new(1),
            vec![0, 1],
            vec![],
            vec![SummaryPart::Text("b1".into())],
            0,
        );
        state.blocks_mut().insert(BlockRef::new(1), b1);

        let b2 = new_block(
            BlockRef::new(2),
            vec![2, 3],
            vec![BlockRef::new(1)],
            vec![SummaryPart::Text("b2".into())],
            0,
        );
        state.blocks_mut().insert(BlockRef::new(2), b2);

        let b3 = dummy_block(3, vec![4, 5], vec![BlockRef::new(2)]);
        let effective = b3.effective_message_indices(&state);
        assert_eq!(
            effective,
            [0, 1, 2, 3, 4, 5].into_iter().collect::<BTreeSet<_>>()
        );
    }

    #[test]
    fn transitive_consumed_refs_includes_self() {
        let mut state = CompactionState::default();
        let b1 = new_block(
            BlockRef::new(1),
            vec![0, 1],
            vec![],
            vec![SummaryPart::Text("b1".into())],
            0,
        );
        state.blocks_mut().insert(BlockRef::new(1), b1);

        let b2 = dummy_block(2, vec![2, 3], vec![BlockRef::new(1)]);
        let refs = b2.transitive_consumed_refs(&state);
        assert!(refs.contains(&BlockRef::new(1)));
        assert!(refs.contains(&BlockRef::new(2)));
    }

    #[test]
    fn mark_consumed_deactivates() {
        let mut block = dummy_block(1, vec![0, 1], vec![]);
        assert!(block.is_active());
        mark_consumed(&mut block, BlockRef::new(2));
        assert!(!block.is_active());
        assert_eq!(block.deactivated_by_block_ref, Some(BlockRef::new(2)));
    }

    #[test]
    fn summary_part_serde_round_trip() {
        let parts = vec![
            SummaryPart::Text("hello".into()),
            SummaryPart::BlockRef(BlockRef::new(3)),
        ];
        let json = serde_json::to_string(&parts).expect("serialize");
        let restored: Vec<SummaryPart> = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parts, restored);
    }

    #[test]
    fn compression_selection_serde_round_trip() {
        let range = CompressionSelection::Range {
            start: MessageRef::from_index(0),
            end: MessageRef::from_index(5),
        };
        let json = serde_json::to_string(&range).expect("serialize");
        let restored: CompressionSelection = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(range, restored);

        let messages = CompressionSelection::Messages {
            refs: vec![MessageRef::from_index(0), MessageRef::from_index(3)],
        };
        let json = serde_json::to_string(&messages).expect("serialize");
        let restored: CompressionSelection = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(messages, restored);
    }

    #[test]
    fn block_serde_round_trip() {
        let block = new_block(
            BlockRef::new(1),
            vec![0, 1, 2],
            vec![BlockRef::new(2)],
            vec![
                SummaryPart::Text("summary text".into()),
                SummaryPart::BlockRef(BlockRef::new(2)),
            ],
            1234567890,
        );
        let json = serde_json::to_string(&block).expect("serialize");
        let restored: CompressionBlock = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(block, restored);
    }
}
