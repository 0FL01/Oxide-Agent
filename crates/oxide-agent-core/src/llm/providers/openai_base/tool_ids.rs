//! Tool call ID mapping for Mistral API compatibility.
//!
//! Mistral requires tool call IDs to be at most 9 alphanumeric characters.
//! This module provides bidirectional mapping between original UUID-based IDs
//! and Mistral-compatible truncated IDs.
//!
//! Moved from `mistral/id_mapper.rs` during the profile migration.

// Some methods are not yet wired into OpenAIBaseProvider chat flow
// (checkpoints 3-6). Kept alive via mistral re-export.
#![allow(dead_code)]

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Maps between original tool call IDs (UUID format) and Mistral-compatible IDs
/// (9 alphanumeric characters).
#[derive(Debug, Clone, Default)]
pub(crate) struct ToolCallIdMapper {
    /// original_id -> mistral_id (9 chars)
    original_to_mistral: HashMap<String, String>,
    /// mistral_id (9 chars) -> original_id
    mistral_to_original: HashMap<String, String>,
}

impl ToolCallIdMapper {
    /// Create a new empty mapper.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Normalize an ID for Mistral API (at most 9 alphanumeric chars).
    ///
    /// Algorithm: take last 9 alphanumeric characters from the original ID.
    /// Example: `call_44456aeb-f16d-4c5e-8f38-f1243acb9e14` -> `43acb9e14`.
    pub(crate) fn normalize_for_mistral(id: &str) -> String {
        let alphanumeric: String = id.chars().filter(|c| c.is_alphanumeric()).collect();
        let start = alphanumeric.len().saturating_sub(9);
        alphanumeric[start..].to_string()
    }

    /// Register a mapping between original ID and its Mistral version.
    ///
    /// Returns the Mistral-compatible ID for convenience.
    ///
    /// If the truncated ID collides with a mapping to a *different* original
    /// ID, a stable base36 fallback is generated to avoid ID confusion.
    pub(crate) fn register(&mut self, original_id: String) -> String {
        // Check if already registered
        if let Some(mistral_id) = self.original_to_mistral.get(&original_id) {
            return mistral_id.clone();
        }

        let truncated = Self::normalize_for_mistral(&original_id);

        let mistral_id = match self.mistral_to_original.get(&truncated) {
            // No collision -- use truncated ID.
            None => truncated,
            // Maps to the same original (shouldn't reach here after the early
            // return, but be defensive).
            Some(existing) if existing == &original_id => truncated,
            // Collision with a different original -- generate stable fallback.
            Some(_) => Self::find_collision_free_id(&original_id, &self.mistral_to_original),
        };

        // Store bidirectional mapping
        self.original_to_mistral
            .insert(original_id.clone(), mistral_id.clone());
        self.mistral_to_original
            .insert(mistral_id.clone(), original_id);

        mistral_id
    }

    /// Find a collision-free 9-char alphanumeric ID for an original ID.
    ///
    /// Uses a stable base36 hash of the original ID. If the hash also collides
    /// (astronomically unlikely with 36^9 keyspace), tries salted variants.
    fn find_collision_free_id(original_id: &str, existing: &HashMap<String, String>) -> String {
        let candidate = Self::stable_fallback(original_id);
        if !existing.contains_key(&candidate) {
            return candidate;
        }
        // Try salted variants -- unreachable in practice.
        for i in 1..=35u64 {
            let salted = Self::stable_fallback(&format!("{original_id}#{i}"));
            if !existing.contains_key(&salted) {
                return salted;
            }
        }
        // All 35 salts exhausted -- unreachable with 36^9 keyspace.
        candidate
    }

    /// Generate a deterministic 9-char base36 ID from the original ID.
    ///
    /// Used as a collision-free fallback when the standard truncation
    /// produces a Mistral ID that already maps to a different original.
    fn stable_fallback(original_id: &str) -> String {
        let mut hasher = DefaultHasher::new();
        original_id.hash(&mut hasher);
        let hash = hasher.finish();
        let encoded = Self::to_base36(hash);
        // Take last 9 chars (padded with leading zeros if shorter).
        let start = encoded.len().saturating_sub(9);
        encoded[start..].to_string()
    }

    /// Encode a `u64` as lowercase base36, zero-padded to at least 9 chars.
    fn to_base36(mut value: u64) -> String {
        const CHARSET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
        if value == 0 {
            return "000000000".to_string();
        }
        let mut chars = Vec::with_capacity(13);
        while value > 0 {
            chars.push(CHARSET[(value % 36) as usize]);
            value /= 36;
        }
        chars.reverse();
        let s = String::from_utf8(chars).unwrap_or_else(|_| "000000000".to_string());
        if s.len() < 9 { format!("{s:0>9}") } else { s }
    }

    /// Get or register the Mistral-compatible ID for an original ID.
    ///
    /// If not registered, creates and registers the mapping.
    pub(crate) fn mistral_id_for(&mut self, original_id: &str) -> String {
        if let Some(mistral_id) = self.original_to_mistral.get(original_id) {
            return mistral_id.clone();
        }
        self.register(original_id.to_string())
    }

    /// Get original ID from a Mistral ID.
    ///
    /// Returns the original ID if found, otherwise returns the input unchanged
    /// (handles cases where Mistral generates its own IDs).
    #[must_use]
    pub(crate) fn to_original(&self, mistral_id: &str) -> String {
        self.mistral_to_original
            .get(mistral_id)
            .cloned()
            .unwrap_or_else(|| mistral_id.to_string())
    }

    /// Check if a Mistral ID is known (has a mapping to original).
    #[must_use]
    pub(crate) fn has_mistral_id(&self, mistral_id: &str) -> bool {
        self.mistral_to_original.contains_key(mistral_id)
    }

    /// Clear all mappings.
    pub(crate) fn clear(&mut self) {
        self.original_to_mistral.clear();
        self.mistral_to_original.clear();
    }

    /// Get the number of registered mappings.
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.original_to_mistral.len()
    }

    /// Check if mapper is empty.
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.original_to_mistral.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_for_mistral() {
        // UUID format
        assert_eq!(
            ToolCallIdMapper::normalize_for_mistral("call_44456aeb-f16d-4c5e-8f38-f1243acb9e14"),
            "43acb9e14"
        );

        // Short ID (less than 9 chars after filtering)
        assert_eq!(
            ToolCallIdMapper::normalize_for_mistral("call_abc"),
            "callabc"
        );

        // Exactly 9 alphanumeric chars
        assert_eq!(
            ToolCallIdMapper::normalize_for_mistral("call_abcdefghi"),
            "abcdefghi"
        );

        // Already normalized (9 chars)
        assert_eq!(
            ToolCallIdMapper::normalize_for_mistral("43acb9e14"),
            "43acb9e14"
        );
    }

    #[test]
    fn test_bidirectional_mapping() {
        let mut mapper = ToolCallIdMapper::new();

        let original = "call_44456aeb-f16d-4c5e-8f38-f1243acb9e14";
        let mistral = mapper.register(original.to_string());

        assert_eq!(mistral, "43acb9e14");
        assert_eq!(mapper.mistral_id_for(original), "43acb9e14");
        assert_eq!(mapper.to_original(&mistral), original);
    }

    #[test]
    fn test_to_original_unknown_id() {
        let mapper = ToolCallIdMapper::new();

        // Unknown Mistral ID returns as-is
        assert_eq!(mapper.to_original("unknown123"), "unknown123");
    }

    #[test]
    fn test_duplicate_registration() {
        let mut mapper = ToolCallIdMapper::new();

        let original = "call_test123";
        let mistral1 = mapper.register(original.to_string());
        let mistral2 = mapper.register(original.to_string());

        // Should return same Mistral ID
        assert_eq!(mistral1, mistral2);
        assert_eq!(mapper.len(), 1);
    }

    #[test]
    fn test_collision_handling() {
        let mut mapper = ToolCallIdMapper::new();

        // Two different originals that both normalize to "43acb9e14":
        //   "call_44456aeb-f16d-4c5e-8f38-f1243acb9e14" -> last 9 alnum -> "43acb9e14"
        //   "prefix_43acb9e14"                           -> last 9 alnum -> "43acb9e14"
        let original_a = "call_44456aeb-f16d-4c5e-8f38-f1243acb9e14";
        let original_b = "prefix_43acb9e14";

        let id_a = mapper.register(original_a.to_string());
        assert_eq!(id_a, "43acb9e14");

        let id_b = mapper.register(original_b.to_string());
        // Collision: id_b must differ from id_a.
        assert_ne!(id_b, id_a);
        // Must still be valid for Mistral: <=9 alphanumeric chars.
        assert!(id_b.len() <= 9);
        assert!(id_b.chars().all(|c| c.is_alphanumeric()));

        // Both originals must round-trip correctly.
        assert_eq!(mapper.to_original(&id_a), original_a);
        assert_eq!(mapper.to_original(&id_b), original_b);
        assert_eq!(mapper.len(), 2);
    }

    #[test]
    fn test_stable_fallback_is_deterministic() {
        // Same input always produces the same fallback ID.
        let a = ToolCallIdMapper::stable_fallback("some_unique_id");
        let b = ToolCallIdMapper::stable_fallback("some_unique_id");
        assert_eq!(a, b);
        assert!(a.len() <= 9);
        assert!(a.chars().all(|c| c.is_alphanumeric()));

        // Different inputs produce different fallbacks.
        let c = ToolCallIdMapper::stable_fallback("different_id");
        assert_ne!(a, c);
    }
}
