use super::store::wiki_content_hash;
use std::collections::HashSet;

/// Maximum defaults for one run's wiki signal collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WikiSignalBufferConfig {
    /// Maximum candidate signals retained for one run.
    pub max_candidates: usize,
    /// Maximum UTF-8 bytes retained for one run.
    pub max_bytes: usize,
}

impl Default for WikiSignalBufferConfig {
    fn default() -> Self {
        Self {
            max_candidates: 16,
            max_bytes: 32 * 1024,
        }
    }
}

/// Durable-memory signal category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WikiSignalKind {
    /// Explicit user request to remember durable information.
    ExplicitRemember,
    /// Durable decision or rationale.
    Decision,
    /// Reusable procedure or runbook.
    Procedure,
    /// Stable constraint or policy.
    Constraint,
    /// User preference.
    Preference,
    /// Low-confidence fact that should usually route to inbox.
    LowConfidence,
}

/// Candidate durable-memory signal collected during a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WikiSignal {
    /// Signal category.
    pub kind: WikiSignalKind,
    /// Human-readable signal content.
    pub content: String,
    /// Compact source refs such as `run:2026-05-19:task-abc123`.
    pub source_refs: Vec<String>,
    /// Whether the signal came from explicit user memory intent.
    pub explicit: bool,
}

/// Bounded per-run signal buffer used before patch planning.
pub struct WikiSignalBuffer {
    config: WikiSignalBufferConfig,
    signals: Vec<WikiSignal>,
    seen_hashes: HashSet<String>,
    bytes: usize,
}

impl WikiSignalBuffer {
    /// Create an empty bounded signal buffer.
    #[must_use]
    pub fn new(config: WikiSignalBufferConfig) -> Self {
        Self {
            config,
            signals: Vec::new(),
            seen_hashes: HashSet::new(),
            bytes: 0,
        }
    }

    /// Attempt to add a signal, returning whether it was retained.
    pub fn push(&mut self, signal: WikiSignal) -> bool {
        let content = signal.content.trim();
        if content.is_empty() || self.signals.len() >= self.config.max_candidates {
            return false;
        }

        let signal_bytes = content.len();
        if self.bytes + signal_bytes > self.config.max_bytes {
            return false;
        }

        let hash = wiki_content_hash(content);
        if !self.seen_hashes.insert(hash) {
            return false;
        }

        self.bytes += signal_bytes;
        self.signals.push(WikiSignal {
            content: content.to_string(),
            ..signal
        });
        true
    }

    /// Return retained signals.
    #[must_use]
    pub fn signals(&self) -> &[WikiSignal] {
        &self.signals
    }

    /// Return retained signal bytes.
    #[must_use]
    pub const fn bytes(&self) -> usize {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signal(content: &str) -> WikiSignal {
        WikiSignal {
            kind: WikiSignalKind::Decision,
            content: content.to_string(),
            source_refs: vec!["run:2026-05-19:test".to_string()],
            explicit: false,
        }
    }

    #[test]
    fn signal_buffer_deduplicates_by_content_hash() {
        let mut buffer = WikiSignalBuffer::new(WikiSignalBufferConfig::default());
        assert!(buffer.push(signal("Remember deploy decision")));
        assert!(!buffer.push(signal("Remember deploy decision")));
        assert_eq!(buffer.signals().len(), 1);
    }

    #[test]
    fn signal_buffer_enforces_byte_limit() {
        let mut buffer = WikiSignalBuffer::new(WikiSignalBufferConfig {
            max_candidates: 16,
            max_bytes: 8,
        });
        assert!(!buffer.push(signal("too large for limit")));
        assert!(buffer.signals().is_empty());
    }
}
