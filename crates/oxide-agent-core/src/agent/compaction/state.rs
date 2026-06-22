//! Compaction overlay state — tracks blocks, refs, and strategy decisions
//! that the renderer uses to produce compacted model context from raw transcript.
//!
//! Phase 1: empty state, identity rendering.
//! Phase 2: block id allocation counter, ref types.
//! Later phases add block graph and strategy state.

use super::refs::BlockRef;
use serde::{Deserialize, Serialize};

/// Persistent compaction overlay state stored alongside raw `AgentMemory` messages.
///
/// This is the mutable authority for compaction: the `CompactionEngine` (future)
/// is the only component that mutates this state. The `CompactionRenderer` reads
/// it to produce compacted model-facing context.
///
/// Serialized with `#[serde(default)]` on `AgentMemory`, so old checkpoints
/// without this field deserialize to `CompactionState::default()` (empty state).
/// Individual fields also carry `#[serde(default)]` so partial state from
/// earlier phases deserializes correctly.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompactionState {
    /// Next block id to allocate. 0 means no blocks have been created.
    /// Block ids start at 1 (`b1`, `b2`, …) — see `allocate_block_id`.
    #[serde(default)]
    next_block_id: u32,
    // Phase 3: blocks: BTreeMap<BlockRef, CompressionBlock>,
    // Phase 4: strategy state (dedup, purge-errors)
}

impl CompactionState {
    /// Returns true when no compaction overlay is active.
    /// The renderer treats this as identity (raw == rendered).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Allocate the next block id and advance the internal counter.
    ///
    /// Returns a `BlockRef` (`b1`, `b2`, …) for the new block. The counter is
    /// monotonic — it never resets except when `CompactionState` itself is
    /// reset to `default()` (on memory replacement, clear, or repair).
    pub fn allocate_block_id(&mut self) -> BlockRef {
        self.next_block_id = self.next_block_id.saturating_add(1);
        BlockRef::new(self.next_block_id)
    }

    /// Current next-block-id value (for diagnostics / tests).
    #[must_use]
    pub const fn next_block_id(&self) -> u32 {
        self.next_block_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_is_empty() {
        assert!(CompactionState::default().is_empty());
    }

    #[test]
    fn empty_state_round_trips_through_serde() {
        let state = CompactionState::default();
        let json = serde_json::to_string(&state).expect("serialize");
        let restored: CompactionState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(state, restored);
        assert!(restored.is_empty());
    }

    #[test]
    fn absent_field_deserializes_to_default() {
        // Simulates old AgentMemory JSON without compaction_state field.
        // serde(default) on the AgentMemory field handles this;
        // here we verify CompactionState itself deserializes from empty JSON.
        let json = "{}";
        let state: CompactionState = serde_json::from_str(json).expect("deserialize empty");
        assert_eq!(state, CompactionState::default());
        assert!(state.is_empty());
    }

    #[test]
    fn phase1_empty_json_deserializes_with_new_field() {
        // Phase 1 serialized CompactionState as `{}` (empty struct).
        // After adding next_block_id, this must still deserialize via serde(default).
        let json = "{}";
        let state: CompactionState = serde_json::from_str(json).expect("deserialize empty");
        assert_eq!(state.next_block_id(), 0);
        assert!(state.is_empty());
    }

    #[test]
    fn allocate_block_id_starts_at_one() {
        let mut state = CompactionState::default();
        let b1 = state.allocate_block_id();
        assert_eq!(b1.to_string(), "b1");
        assert_eq!(state.next_block_id(), 1);
        assert!(!state.is_empty());
    }

    #[test]
    fn allocate_block_id_is_monotonic() {
        let mut state = CompactionState::default();
        assert_eq!(state.allocate_block_id().as_u32(), 1);
        assert_eq!(state.allocate_block_id().as_u32(), 2);
        assert_eq!(state.allocate_block_id().as_u32(), 3);
        assert_eq!(state.next_block_id(), 3);
    }

    #[test]
    fn non_empty_state_round_trips_through_serde() {
        let mut state = CompactionState::default();
        state.allocate_block_id();
        state.allocate_block_id();
        assert!(!state.is_empty());

        let json = serde_json::to_string(&state).expect("serialize");
        let restored: CompactionState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(state, restored);
        assert_eq!(restored.next_block_id(), 2);
        assert!(!restored.is_empty());
    }

    #[test]
    fn partial_json_with_only_next_block_id_deserializes() {
        // Future-proofing: if Phase 3 adds fields, old JSON with only next_block_id
        // must still deserialize. serde(default) on each field ensures this.
        let json = r#"{"next_block_id": 5}"#;
        let state: CompactionState = serde_json::from_str(json).expect("deserialize partial");
        assert_eq!(state.next_block_id(), 5);
        assert!(!state.is_empty());
    }
}
