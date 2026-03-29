//! Tool call ID mapping for Mistral API compatibility
//!
//! Mistral requires tool call IDs to be exactly 9 alphanumeric characters.
//! This module provides bidirectional mapping between original UUID-based IDs
//! and Mistral-compatible truncated IDs.

use std::collections::HashMap;

/// Maps between original tool call IDs (UUID format) and Mistral-compatible IDs (9 chars)
#[derive(Debug, Clone, Default)]
pub struct ToolCallIdMapper {
    /// original_id -> mistral_id (9 chars)
    original_to_mistral: HashMap<String, String>,
    /// mistral_id (9 chars) -> original_id
    mistral_to_original: HashMap<String, String>,
}

impl ToolCallIdMapper {
    /// Create a new empty mapper
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Normalize an ID for Mistral API (9 alphanumeric chars)
    ///
    /// Algorithm: take last 9 alphanumeric characters from the original ID
    /// Example: "call_44456aeb-f16d-4c5e-8f38-f1243acb9e14" -> "43acb9e14"
    pub fn normalize_for_mistral(id: &str) -> String {
        let alphanumeric: String = id.chars().filter(|c| c.is_alphanumeric()).collect();
        let start = alphanumeric.len().saturating_sub(9);
        alphanumeric[start..].to_string()
    }

    /// Register a mapping between original ID and its Mistral version
    ///
    /// Returns the Mistral-compatible ID for convenience
    pub fn register(&mut self, original_id: String) -> String {
        // Check if already registered
        if let Some(mistral_id) = self.original_to_mistral.get(&original_id) {
            return mistral_id.clone();
        }

        let mistral_id = Self::normalize_for_mistral(&original_id);

        // Store bidirectional mapping
        self.original_to_mistral
            .insert(original_id.clone(), mistral_id.clone());
        self.mistral_to_original
            .insert(mistral_id.clone(), original_id);

        mistral_id
    }

    /// Get or register the Mistral-compatible ID for an original ID.
    ///
    /// If not registered, creates and registers the mapping.
    pub fn mistral_id_for(&mut self, original_id: &str) -> String {
        if let Some(mistral_id) = self.original_to_mistral.get(original_id) {
            return mistral_id.clone();
        }
        self.register(original_id.to_string())
    }

    /// Get original ID from a Mistral ID
    ///
    /// Returns the original ID if found, otherwise returns the input unchanged
    /// (handles cases where Mistral generates its own IDs)
    #[must_use]
    pub fn to_original(&self, mistral_id: &str) -> String {
        self.mistral_to_original
            .get(mistral_id)
            .cloned()
            .unwrap_or_else(|| mistral_id.to_string())
    }

    /// Check if a Mistral ID is known (has a mapping to original)
    #[must_use]
    pub fn has_mistral_id(&self, mistral_id: &str) -> bool {
        self.mistral_to_original.contains_key(mistral_id)
    }

    /// Clear all mappings (useful for testing or session reset)
    pub fn clear(&mut self) {
        self.original_to_mistral.clear();
        self.mistral_to_original.clear();
    }

    /// Get the number of registered mappings
    #[must_use]
    pub fn len(&self) -> usize {
        self.original_to_mistral.len()
    }

    /// Check if mapper is empty
    #[must_use]
    pub fn is_empty(&self) -> bool {
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
}
