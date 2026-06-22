//! Compaction overlay state — tracks blocks, refs, and strategy decisions
//! that the renderer uses to produce compacted model context from raw transcript.
//!
//! Phase 1: empty state, identity rendering.
//! Later phases add message refs, block graph, and strategy state.

use serde::{Deserialize, Serialize};

/// Persistent compaction overlay state stored alongside raw `AgentMemory` messages.
///
/// This is the mutable authority for compaction: the `CompactionEngine` (future)
/// is the only component that mutates this state. The `CompactionRenderer` reads
/// it to produce compacted model-facing context.
///
/// Serialized with `#[serde(default)]` on `AgentMemory`, so old checkpoints
/// without this field deserialize to `CompactionState::default()` (empty state).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompactionState {
    // Phase 2: message refs (mNNNN), block refs (bN)
    // Phase 3: block graph (active/consumed/parent blocks)
    // Phase 4: strategy state (dedup, purge-errors)
}

impl CompactionState {
    /// Returns true when no compaction overlay is active.
    /// The renderer treats this as identity (raw == rendered).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
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
}
