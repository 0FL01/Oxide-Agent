//! Content loop detector.

use lazy_regex::lazy_regex;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use tracing::debug;

const DEFAULT_MAX_DISTANCE_MULTIPLIER: usize = 5;

static RE_BULLET_LIST: lazy_regex::Lazy<regex::Regex> = lazy_regex!(r"(?m)(^|\n)\s*[*\-+]\s");
static RE_BLOCKQUOTE: lazy_regex::Lazy<regex::Regex> = lazy_regex!(r"(?m)(^|\n)\s*>\s");
static RE_DIVIDER: lazy_regex::Lazy<regex::Regex> = lazy_regex!(r"(?m)(^|\n)\s*[-=]{3,}\s*$");

/// Detects repeated content chunks within a sliding window.
pub struct ContentLoopDetector {
    history: Vec<char>,
    chunk_stats: HashMap<String, Vec<usize>>,
    last_index: usize,
    in_code_block: bool,
    chunk_size: usize,
    loop_threshold: usize,
    max_history_length: usize,
    max_distance_multiplier: usize,
}

impl ContentLoopDetector {
    /// Create a new content loop detector using config values.
    #[must_use]
    pub fn new(chunk_size: usize, loop_threshold: usize, max_history_length: usize) -> Self {
        Self {
            history: Vec::new(),
            chunk_stats: HashMap::new(),
            last_index: 0,
            in_code_block: false,
            chunk_size: chunk_size.max(1),
            loop_threshold: loop_threshold.max(2),
            max_history_length: max_history_length.max(1),
            max_distance_multiplier: DEFAULT_MAX_DISTANCE_MULTIPLIER,
        }
    }

    /// Check a new content fragment for looping behavior.
    pub fn check(&mut self, content: &str) -> bool {
        let has_code_fence = content.contains("```");
        self.update_code_block_state(content);

        if has_code_fence || self.in_code_block {
            debug!(
                in_code_block = self.in_code_block,
                has_code_fence, "content_detector: skipping (code block)"
            );
            self.reset_tracking();
            return false;
        }

        if self.should_skip_tracking(content) {
            debug!("content_detector: skipping (table/list/header)");
            self.reset_tracking();
            return false;
        }

        self.history.extend(content.chars());
        self.truncate_if_needed();

        debug!(
            history_len = self.history.len(),
            chunk_size = self.chunk_size,
            last_index = self.last_index,
            "content_detector: processing content"
        );

        while self.last_index + self.chunk_size <= self.history.len() {
            let chunk = self.chunk_at(self.last_index);
            let hash = Self::hash_chunk(&chunk);
            if self.check_chunk_loop(self.last_index, &chunk, &hash) {
                debug!(
                    chunk_preview = %chunk.chars().take(30).collect::<String>(),
                    "content_detector: LOOP DETECTED!"
                );
                return true;
            }
            self.last_index = self.last_index.saturating_add(1);
        }

        false
    }

    /// Reset the detector state (including history).
    pub fn reset(&mut self) {
        self.history.clear();
        self.chunk_stats.clear();
        self.last_index = 0;
        self.in_code_block = false;
    }

    /// Reset tracking state but preserve code block context.
    pub fn reset_tracking(&mut self) {
        self.history.clear();
        self.chunk_stats.clear();
        self.last_index = 0;
    }

    #[cfg(test)]
    fn history_len(&self) -> usize {
        self.history.len()
    }

    fn update_code_block_state(&mut self, content: &str) {
        let fences = content.matches("```").count();
        if !fences.is_multiple_of(2) {
            self.in_code_block = !self.in_code_block;
        }
    }

    fn should_skip_tracking(&self, content: &str) -> bool {
        content.contains('|')
            || content.contains('#')
            || RE_BULLET_LIST.is_match(content)
            || RE_BLOCKQUOTE.is_match(content)
            || RE_DIVIDER.is_match(content)
    }

    fn truncate_if_needed(&mut self) {
        if self.history.len() <= self.max_history_length {
            return;
        }

        let truncate_amount = self.history.len() - self.max_history_length;
        self.history.drain(0..truncate_amount);

        for positions in self.chunk_stats.values_mut() {
            positions.retain_mut(|pos| {
                if *pos >= truncate_amount {
                    *pos -= truncate_amount;
                    true
                } else {
                    false
                }
            });
        }

        self.last_index = self.last_index.saturating_sub(truncate_amount);
    }

    fn chunk_at(&self, start: usize) -> String {
        self.history[start..start + self.chunk_size]
            .iter()
            .collect()
    }

    fn hash_chunk(chunk: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(chunk.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    fn check_chunk_loop(&mut self, position: usize, chunk: &str, hash: &str) -> bool {
        let first_pos = self
            .chunk_stats
            .get(hash)
            .and_then(|positions| positions.first().copied());

        if let Some(first_pos) = first_pos {
            if self.chunk_at(first_pos) != chunk {
                return false;
            }
        }

        let positions = self.chunk_stats.entry(hash.to_string()).or_default();
        positions.push(position);

        let occurrences = positions.len();
        if occurrences < self.loop_threshold {
            return false;
        }

        let start_index = positions.len().saturating_sub(self.loop_threshold);
        let recent = &positions[start_index..];
        let total_distance =
            recent.last().unwrap_or(&position) - recent.first().unwrap_or(&position);
        let avg_distance = total_distance / (self.loop_threshold - 1);
        let max_distance = self.chunk_size * self.max_distance_multiplier;

        let is_loop = avg_distance <= max_distance;

        if occurrences >= self.loop_threshold {
            debug!(
                hash_prefix = &hash[..8.min(hash.len())],
                occurrences,
                threshold = self.loop_threshold,
                avg_distance,
                max_distance,
                is_loop,
                "content_detector: chunk threshold check"
            );
        }

        is_loop
    }
}

#[cfg(test)]
mod tests {
    use super::ContentLoopDetector;

    #[test]
    fn detects_repetition() {
        let mut detector = ContentLoopDetector::new(10, 4, 200);
        let chunk = "repeat repeat ";
        let mut detected = false;
        for _ in 0..10 {
            if detector.check(chunk) {
                detected = true;
                break;
            }
        }
        assert!(detected);
    }

    #[test]
    fn skips_code_blocks() {
        let mut detector = ContentLoopDetector::new(10, 4, 200);
        let content = "```fn test() {}```";
        for _ in 0..5 {
            assert!(!detector.check(content));
        }
        assert_eq!(detector.history_len(), 0);
    }

    #[test]
    fn truncates_history() {
        let mut detector = ContentLoopDetector::new(5, 3, 50);
        let long_text = "a".repeat(200);
        detector.check(&long_text);
        assert!(detector.history_len() <= 50);
    }

    #[test]
    fn ignores_tables_and_lists() {
        let mut detector = ContentLoopDetector::new(10, 4, 200);
        assert!(!detector.check("| a | b |"));
        assert!(!detector.check("- item"));
        assert_eq!(detector.history_len(), 0);
    }
}
