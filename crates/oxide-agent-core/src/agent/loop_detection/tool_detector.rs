//! Tool call loop detector with cycle detection.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use tracing::debug;

/// Maximum number of hashes retained for cycle analysis.
const MAX_HISTORY_MULTIPLIER: usize = 2;

/// Detects tool call loops by checking for repeating cycles in the call sequence.
///
/// Unlike a consecutive-identical detector, this catches cycles of any period
/// (e.g. A-B-A-B with period 2, A-B-C-A-B-C with period 3), not just A-A-A-A.
pub struct ToolCallDetector {
    history: Vec<String>,
    threshold: usize,
}

impl ToolCallDetector {
    /// Create a new tool call detector with a threshold.
    ///
    /// The threshold is the minimum number of entries in the history that must
    /// form a repeating periodic pattern before a loop is reported.
    #[must_use]
    pub fn new(threshold: usize) -> Self {
        Self {
            history: Vec::new(),
            threshold: threshold.max(2),
        }
    }

    /// Check if the given tool call forms a loop.
    pub fn check(&mut self, tool_name: &str, args: &str) -> bool {
        let key = Self::hash_tool_call(tool_name, args);
        let args_preview: String = args.chars().take(100).collect();

        self.history.push(key.clone());
        // Bound history to threshold * multiplier so we have enough data for
        // cycle detection without unbounded growth.
        let max_len = self.threshold * MAX_HISTORY_MULTIPLIER;
        if self.history.len() > max_len {
            self.history.drain(0..(self.history.len() - max_len));
        }

        let detected = self.detect_cycle();
        let hash_preview = &key[..8.min(key.len())];

        if detected {
            debug!(
                tool_name,
                args_preview,
                hash = hash_preview,
                history_len = self.history.len(),
                threshold = self.threshold,
                "tool_detector: CYCLE DETECTED"
            );
        } else {
            debug!(
                tool_name,
                args_preview,
                hash = hash_preview,
                history_len = self.history.len(),
                threshold = self.threshold,
                "tool_detector: no cycle"
            );
        }

        detected
    }

    /// Reset the detector state.
    pub fn reset(&mut self) {
        self.history.clear();
    }

    /// Check if the history tail is periodic with the given period.
    ///
    /// Returns true if the last `threshold` entries repeat with period `p`,
    /// i.e. `entry[i] == entry[i - p]` for all `i` in `[p, threshold)`.
    fn is_periodic(tail: &[String], p: usize) -> bool {
        if p == 0 || tail.len() < p + 1 {
            return false;
        }
        for i in p..tail.len() {
            if tail[i] != tail[i - p] {
                return false;
            }
        }
        true
    }

    /// Check the history for a repeating cycle of any period.
    ///
    /// Takes the last `threshold` entries and checks if they are periodic
    /// with any period `p` from 1 to `threshold / 2`. This catches:
    /// - `p=1`: consecutive identical calls (A-A-A-A-A)
    /// - `p=2`: alternating cycles (A-B-A-B-A)
    /// - `p=n`: arbitrary repeating patterns
    fn detect_cycle(&self) -> bool {
        let n = self.history.len();
        if n < self.threshold {
            return false;
        }
        let start = n - self.threshold;
        let tail = &self.history[start..];
        for p in 1..=self.threshold / 2 {
            if Self::is_periodic(tail, p) {
                debug!(
                    period = p,
                    history_len = n,
                    threshold = self.threshold,
                    "tool_detector: periodic pattern found"
                );
                return true;
            }
        }
        false
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
        canonicalize_tool_call_args(args).unwrap_or_else(|| args.to_string())
    }
}

pub(crate) fn canonicalize_tool_call_args(args: &str) -> Option<String> {
    serde_json::from_str::<Value>(args)
        .ok()
        .map(sort_json_value)
        .map(|value| value.to_string())
}

fn sort_json_value(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(sort_json_value).collect()),
        Value::Object(map) => {
            let sorted = map.into_iter().collect::<BTreeMap<_, _>>();
            let mut canonical = serde_json::Map::with_capacity(sorted.len());
            for (key, value) in sorted {
                canonical.insert(key, sort_json_value(value));
            }
            Value::Object(canonical)
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::{ToolCallDetector, canonicalize_tool_call_args};
    use proptest::prop_assert_eq;

    #[test]
    fn detects_consecutive_identical_at_threshold() {
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
        // History is [A, A, B] — not periodic with any period
    }

    #[test]
    fn resets_on_args_change() {
        let mut detector = ToolCallDetector::new(3);
        assert!(!detector.check("tool_a", r#"{"a":1}"#));
        assert!(!detector.check("tool_a", r#"{"a":2}"#));
        // History is [hash(1), hash(2)] — not periodic
    }

    #[test]
    fn detects_abab_cycle() {
        let mut detector = ToolCallDetector::new(5);
        // A-B-A-B-A: period 2, 5 entries
        assert!(!detector.check("tool_a", r#"{"x":1}"#)); // [A]
        assert!(!detector.check("tool_b", r#"{"y":2}"#)); // [A,B]
        assert!(!detector.check("tool_a", r#"{"x":1}"#)); // [A,B,A]
        assert!(!detector.check("tool_b", r#"{"y":2}"#)); // [A,B,A,B]
        assert!(detector.check("tool_a", r#"{"x":1}"#)); // [A,B,A,B,A] — periodic with p=2
    }

    #[test]
    fn detects_abc_abc_cycle() {
        let mut detector = ToolCallDetector::new(6);
        // A-B-C-A-B-C: period 3, 6 entries
        assert!(!detector.check("tool_a", r#"{"x":1}"#));
        assert!(!detector.check("tool_b", r#"{"y":2}"#));
        assert!(!detector.check("tool_c", r#"{"z":3}"#));
        assert!(!detector.check("tool_a", r#"{"x":1}"#));
        assert!(!detector.check("tool_b", r#"{"y":2}"#));
        assert!(detector.check("tool_c", r#"{"z":3}"#)); // [A,B,C,A,B,C] — periodic with p=3
    }

    #[test]
    fn does_not_detect_non_repeating_sequence() {
        let mut detector = ToolCallDetector::new(5);
        assert!(!detector.check("tool_a", r#"{"x":1}"#));
        assert!(!detector.check("tool_b", r#"{"y":2}"#));
        assert!(!detector.check("tool_c", r#"{"z":3}"#));
        assert!(!detector.check("tool_a", r#"{"x":4}"#));
        assert!(!detector.check("tool_b", r#"{"y":5}"#));
        // [A,B,C,A',B'] — not periodic (A≠A', B≠B')
    }

    #[test]
    fn canonicalize_tool_call_args_sorts_object_keys_recursively() {
        let left = canonicalize_tool_call_args(r#"{"b":2,"a":{"d":4,"c":3}}"#)
            .expect("left canonical args");
        let right = canonicalize_tool_call_args(r#"{"a":{"c":3,"d":4},"b":2}"#)
            .expect("right canonical args");

        assert_eq!(left, right);
    }

    #[test]
    fn detects_reordered_args_as_identical() {
        let mut detector = ToolCallDetector::new(3);
        assert!(!detector.check("tool", r#"{"a":1,"b":2}"#));
        assert!(!detector.check("tool", r#"{"b":2,"a":1}"#));
        // Canonicalized to same hash → consecutive identical
        assert!(detector.check("tool", r#"{"a":1,"b":2}"#));
    }

    proptest::proptest! {
        #[test]
        fn proptest_canonicalize_idempotent(json in proptest::string::string_regex("[a-z0-9{}\\[\\]:,\"]*").expect("valid regex")) {
            // canonicalize(canonicalize(x)) == canonicalize(x) for any valid JSON
            if let Some(first) = canonicalize_tool_call_args(&json)
                && let Some(second) = canonicalize_tool_call_args(&first)
            {
                prop_assert_eq!(first, second);
            }
        }

        #[test]
        fn proptest_canonicalize_key_order_independent(
            pairs in proptest::collection::vec(("[a-z]{1,3}", "[0-9]{1,3}"), 1..5)
        ) {
            // Deduplicate by key
            let mut unique: Vec<(String, String)> = pairs.into_iter().collect();
            unique.sort_by(|a, b| a.0.cmp(&b.0));
            unique.dedup_by(|a, b| a.0 == b.0);
            if unique.is_empty() {
                return Ok(());
            }

            let mut reversed = unique.clone();
            reversed.reverse();

            let json_a = format!(
                "{{{}}}",
                unique.iter().map(|(k, v)| format!("\"{k}\":{v}")).collect::<Vec<_>>().join(",")
            );
            let json_b = format!(
                "{{{}}}",
                reversed.iter().map(|(k, v)| format!("\"{k}\":{v}")).collect::<Vec<_>>().join(",")
            );

            let canon_a = canonicalize_tool_call_args(&json_a);
            let canon_b = canonicalize_tool_call_args(&json_b);
            prop_assert_eq!(canon_a, canon_b, "canonical forms differ for same key-value pairs");
        }
    }
}
