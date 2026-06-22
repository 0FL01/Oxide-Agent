//! Stable visible references for compaction.
//!
//! The renderer injects `MessageRef` (`mNNNN`) tags into model-visible context
//! so the LLM can specify compression ranges. The `CompactionEngine` resolves
//! refs to raw message indices — the LLM never provides internal ids, storage
//! keys, or downstream state.
//!
//! `BlockRef` (`bN`) identifies compression blocks. The engine allocates block
//! ids; the LLM sees them in summary text and may reference them in
//! recompression requests. Block refs are resolved by the engine against
//! `CompactionState` — the LLM cannot invent them.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// MessageRef
// ---------------------------------------------------------------------------

/// Stable visible reference to a raw message in the agent transcript.
///
/// Format: `mNNNN` (4-digit zero-padded, 1-indexed). `m0001` → `messages[0]`.
///
/// Assigned by the renderer based on current message index. Resolved by the
/// compaction engine. Refs are stable across appends (existing indices don't
/// shift). When the raw message array is replaced or repaired,
/// `CompactionState` is reset and refs recompute from scratch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageRef(u32);

impl MessageRef {
    /// Create a ref from a 0-indexed message position.
    #[must_use]
    pub fn from_index(index: usize) -> Self {
        Self((index + 1) as u32)
    }

    /// Convert back to 0-indexed message position.
    #[must_use]
    pub fn to_index(self) -> usize {
        (self.0 - 1) as usize
    }

    /// Raw numeric value (1-indexed).
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self.0
    }

    /// Resolve to a valid 0-indexed position, or `None` if the ref is stale
    /// (points beyond the current message count).
    #[must_use]
    pub fn resolve(self, message_count: usize) -> Option<usize> {
        let index = self.to_index();
        (index < message_count).then_some(index)
    }
}

impl fmt::Display for MessageRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "m{:04}", self.0)
    }
}

impl FromStr for MessageRef {
    type Err = MessageRefParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let rest = s
            .strip_prefix('m')
            .or_else(|| s.strip_prefix('M'))
            .ok_or(MessageRefParseError::MissingPrefix)?;
        let n: u32 = rest
            .parse()
            .map_err(|_| MessageRefParseError::InvalidNumber)?;
        if n == 0 {
            return Err(MessageRefParseError::ZeroRef);
        }
        Ok(Self(n))
    }
}

/// Error when parsing a `MessageRef` from a string.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MessageRefParseError {
    /// String does not start with `m` or `M`.
    #[error("message ref must start with 'm'")]
    MissingPrefix,
    /// Numeric portion is not a valid positive integer.
    #[error("message ref number must be a positive integer")]
    InvalidNumber,
    /// Ref value is zero (`m0000` is not valid — refs are 1-indexed).
    #[error("message ref must be >= 1 (m0000 is not valid)")]
    ZeroRef,
}

// ---------------------------------------------------------------------------
// BlockRef
// ---------------------------------------------------------------------------

/// Stable visible reference to a compression block.
///
/// Format: `bN` (no zero-padding, 1-indexed). `b1` → first block, `b2` → second.
///
/// Allocated by `CompactionEngine` via `CompactionState::allocate_block_id`.
/// Stored in `CompactionState`. The LLM sees block refs in summary text and
/// may reference them in recompression. The engine resolves them against
/// `CompactionState` — invented or stale refs are rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BlockRef(u32);

impl BlockRef {
    /// Create a ref from a raw block number (must be >= 1).
    #[must_use]
    pub const fn new(n: u32) -> Self {
        Self(n)
    }

    /// Raw numeric value (1-indexed).
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

impl fmt::Display for BlockRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "b{}", self.0)
    }
}

impl FromStr for BlockRef {
    type Err = BlockRefParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let rest = s
            .strip_prefix('b')
            .or_else(|| s.strip_prefix('B'))
            .ok_or(BlockRefParseError::MissingPrefix)?;
        let n: u32 = rest
            .parse()
            .map_err(|_| BlockRefParseError::InvalidNumber)?;
        if n == 0 {
            return Err(BlockRefParseError::ZeroRef);
        }
        Ok(Self(n))
    }
}

/// Error when parsing a `BlockRef` from a string.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BlockRefParseError {
    /// String does not start with `b` or `B`.
    #[error("block ref must start with 'b'")]
    MissingPrefix,
    /// Numeric portion is not a valid positive integer.
    #[error("block ref number must be a positive integer")]
    InvalidNumber,
    /// Ref value is zero (`b0` is not valid — refs are 1-indexed).
    #[error("block ref must be >= 1 (b0 is not valid)")]
    ZeroRef,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- MessageRef ---

    #[test]
    fn message_ref_from_index_formats_correctly() {
        assert_eq!(MessageRef::from_index(0).to_string(), "m0001");
        assert_eq!(MessageRef::from_index(1).to_string(), "m0002");
        assert_eq!(MessageRef::from_index(42).to_string(), "m0043");
    }

    #[test]
    fn message_ref_handles_large_indices() {
        // Beyond 9999, formatting extends naturally — no data loss.
        let big = MessageRef::from_index(9999);
        assert_eq!(big.to_string(), "m10000");
        let parsed: MessageRef = big.to_string().parse().unwrap();
        assert_eq!(big, parsed);
    }

    #[test]
    fn message_ref_to_index_round_trips() {
        for i in [0usize, 1, 5, 99, 999, 9999] {
            assert_eq!(MessageRef::from_index(i).to_index(), i);
        }
    }

    #[test]
    fn message_ref_parse_round_trip() {
        for i in [0usize, 1, 5, 99, 999] {
            let reff = MessageRef::from_index(i);
            let parsed: MessageRef = reff.to_string().parse().unwrap();
            assert_eq!(reff, parsed);
        }
    }

    #[test]
    fn message_ref_parse_case_insensitive_prefix() {
        let upper: MessageRef = "M0001".parse().unwrap();
        assert_eq!(upper, MessageRef::from_index(0));
    }

    #[test]
    fn message_ref_parse_invalid() {
        assert!("".parse::<MessageRef>().is_err());
        assert!("m".parse::<MessageRef>().is_err());
        assert!("m0000".parse::<MessageRef>().is_err());
        assert!("mabc".parse::<MessageRef>().is_err());
        assert!("1".parse::<MessageRef>().is_err());
        assert!("x0001".parse::<MessageRef>().is_err());
        assert!("m-1".parse::<MessageRef>().is_err());
    }

    #[test]
    fn message_ref_resolve_valid() {
        let reff = MessageRef::from_index(2);
        assert_eq!(reff.resolve(5), Some(2));
        assert_eq!(reff.resolve(3), Some(2));
    }

    #[test]
    fn message_ref_resolve_stale() {
        let reff = MessageRef::from_index(10);
        assert_eq!(reff.resolve(5), None);
        assert_eq!(reff.resolve(11), Some(10));
    }

    #[test]
    fn message_ref_resolve_boundary() {
        // message_count=5 means valid indices 0..=4
        let reff = MessageRef::from_index(4);
        assert_eq!(reff.resolve(5), Some(4));
        assert_eq!(reff.resolve(4), None);
    }

    #[test]
    fn message_ref_serde_round_trip() {
        let reff = MessageRef::from_index(42);
        let json = serde_json::to_string(&reff).expect("serialize");
        let restored: MessageRef = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(reff, restored);
    }

    #[test]
    fn message_ref_as_u32() {
        assert_eq!(MessageRef::from_index(0).as_u32(), 1);
        assert_eq!(MessageRef::from_index(41).as_u32(), 42);
    }

    // --- BlockRef ---

    #[test]
    fn block_ref_display() {
        assert_eq!(BlockRef::new(1).to_string(), "b1");
        assert_eq!(BlockRef::new(42).to_string(), "b42");
    }

    #[test]
    fn block_ref_parse_round_trip() {
        for n in [1u32, 2, 5, 99, 999] {
            let reff = BlockRef::new(n);
            let parsed: BlockRef = reff.to_string().parse().unwrap();
            assert_eq!(reff, parsed);
        }
    }

    #[test]
    fn block_ref_parse_case_insensitive_prefix() {
        let upper: BlockRef = "B1".parse().unwrap();
        assert_eq!(upper, BlockRef::new(1));
    }

    #[test]
    fn block_ref_parse_invalid() {
        assert!("".parse::<BlockRef>().is_err());
        assert!("b".parse::<BlockRef>().is_err());
        assert!("b0".parse::<BlockRef>().is_err());
        assert!("bxyz".parse::<BlockRef>().is_err());
        assert!("1".parse::<BlockRef>().is_err());
        assert!("b-1".parse::<BlockRef>().is_err());
    }

    #[test]
    fn block_ref_serde_round_trip() {
        let reff = BlockRef::new(42);
        let json = serde_json::to_string(&reff).expect("serialize");
        let restored: BlockRef = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(reff, restored);
    }

    #[test]
    fn block_ref_as_u32() {
        assert_eq!(BlockRef::new(1).as_u32(), 1);
        assert_eq!(BlockRef::new(42).as_u32(), 42);
    }

    #[test]
    fn block_ref_ordering() {
        // BlockRef derives Ord — useful for BTreeMap keys in Phase 3.
        assert!(BlockRef::new(1) < BlockRef::new(2));
        assert!(BlockRef::new(99) > BlockRef::new(5));
    }
}
