//! Tool call loop detector.

use sha2::{Digest, Sha256};

/// Detects consecutive identical tool calls using hashing.
pub struct ToolCallDetector {
    last_key: Option<String>,
    repetition_count: usize,
    threshold: usize,
}

impl ToolCallDetector {
    /// Create a new tool call detector with a threshold.
    #[must_use]
    pub fn new(threshold: usize) -> Self {
        Self {
            last_key: None,
            repetition_count: 0,
            threshold: threshold.max(1),
        }
    }

    /// Check if the given tool call forms a loop.
    pub fn check(&mut self, tool_name: &str, args: &str) -> bool {
        let key = Self::hash_tool_call(tool_name, args);
        if self.last_key.as_deref() == Some(&key) {
            self.repetition_count = self.repetition_count.saturating_add(1);
        } else {
            self.last_key = Some(key);
            self.repetition_count = 1;
        }

        self.repetition_count >= self.threshold
    }

    /// Reset the detector state.
    pub fn reset(&mut self) {
        self.last_key = None;
        self.repetition_count = 0;
    }

    #[cfg(test)]
    fn repetition_count(&self) -> usize {
        self.repetition_count
    }

    fn hash_tool_call(tool_name: &str, args: &str) -> String {
        let normalized_args = Self::normalize_args(args);
        let mut hasher = Sha256::new();
        hasher.update(tool_name.as_bytes());
        hasher.update(b":");
        hasher.update(normalized_args.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    fn normalize_args(args: &str) -> String {
        serde_json::from_str::<serde_json::Value>(args)
            .map(|value| value.to_string())
            .unwrap_or_else(|_| args.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::ToolCallDetector;

    #[test]
    fn detects_at_threshold() {
        let mut detector = ToolCallDetector::new(5);
        for _ in 0..4 {
            assert!(!detector.check("test_tool", r#"{"param": "value"}"#));
        }
        assert!(detector.check("test_tool", r#"{"param": "value"}"#));
    }

    #[test]
    fn resets_on_tool_change() {
        let mut detector = ToolCallDetector::new(3);
        assert!(!detector.check("tool_a", r#"{"a":1}"#));
        assert!(!detector.check("tool_a", r#"{"a":1}"#));
        assert!(!detector.check("tool_b", r#"{"a":1}"#));
        assert_eq!(detector.repetition_count(), 1);
    }

    #[test]
    fn resets_on_args_change() {
        let mut detector = ToolCallDetector::new(3);
        assert!(!detector.check("tool_a", r#"{"a":1}"#));
        assert!(!detector.check("tool_a", r#"{"a":2}"#));
        assert_eq!(detector.repetition_count(), 1);
    }
}
