//! Task-local memory behavior signals used as input for LLM Wiki updates.

use crate::agent::session::AgentMemoryScope;
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

const MEMORY_BEHAVIOR_MAX_DRAFTS: usize = 8;

/// Scope-aware policy for task-local memory behavior signals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicMemoryPolicy {
    /// Human-readable label used in advisory cards.
    pub context_label: String,
    /// Whether procedural memories may be extracted from tool activity.
    pub allow_procedure_capture: bool,
    /// Whether failure memories may be extracted from tool activity.
    pub allow_failure_capture: bool,
    /// Whether preference extraction from repeated patterns is allowed.
    pub allow_preference_capture: bool,
    /// Whether a retrieval advisor may suggest durable-memory reads.
    pub allow_manual_read_advice: bool,
    /// Whether history-card guidance should be shown.
    pub allow_history_cards: bool,
}

impl TopicMemoryPolicy {
    /// Build the memory-signal policy for the provided agent memory scope.
    #[must_use]
    pub fn from_scope(scope: Option<&AgentMemoryScope>) -> Self {
        let synthetic = scope
            .map(|scope| scope.context_key.starts_with("session:"))
            .unwrap_or(true);
        let context_label = scope
            .map(|scope| {
                if synthetic {
                    "this conversation".to_string()
                } else {
                    format!("topic '{}'", scope.context_key)
                }
            })
            .unwrap_or_else(|| "this conversation".to_string());

        Self {
            context_label,
            allow_procedure_capture: true,
            allow_failure_capture: true,
            allow_preference_capture: !synthetic,
            allow_manual_read_advice: true,
            allow_history_cards: !synthetic,
        }
    }
}

/// Coarse kind for tool-derived wiki-memory update candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolDerivedMemoryKind {
    /// A stable fact or observation.
    Fact,
    /// A user or topic preference inferred from repeated behavior.
    Preference,
    /// A reusable procedure or workflow.
    Procedure,
}

/// Tool-derived wiki-memory update candidate captured during the live agent run.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDerivedMemoryDraft {
    /// Coarse candidate kind.
    pub kind: ToolDerivedMemoryKind,
    /// Short candidate title.
    pub title: String,
    /// Candidate body content.
    pub content: String,
    /// One-line human-readable summary.
    pub short_description: String,
    /// Candidate importance in the range expected by the signal producer.
    pub importance: f32,
    /// Candidate confidence in the range expected by the signal producer.
    pub confidence: f32,
    /// Source hook or tool that produced the candidate.
    pub source: String,
    /// Reason why the candidate was captured.
    pub reason: String,
    /// Lightweight tags for later wiki patch planning.
    pub tags: Vec<String>,
    /// Compact evidence lines that explain why this candidate was captured.
    pub evidence: Vec<String>,
    /// Capture timestamp.
    pub captured_at: DateTime<Utc>,
}

#[derive(Debug, Default)]
struct MemoryBehaviorState {
    drafts: Vec<ToolDerivedMemoryDraft>,
    pattern_counts: HashMap<String, usize>,
    emitted_patterns: HashSet<String>,
}

/// Task-local runtime used by hooks to capture bounded wiki-memory signals.
#[derive(Debug, Default)]
pub struct MemoryBehaviorRuntime {
    state: Mutex<MemoryBehaviorState>,
}

impl MemoryBehaviorRuntime {
    /// Create an empty memory behavior runtime.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear all accumulated task-local signals.
    pub fn reset(&self) {
        if let Ok(mut state) = self.state.lock() {
            *state = MemoryBehaviorState::default();
        }
    }

    /// Record a bounded tool-derived memory candidate.
    pub fn record_draft(&self, draft: ToolDerivedMemoryDraft) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state
            .drafts
            .iter()
            .any(|existing| existing.kind == draft.kind && existing.content == draft.content)
        {
            return;
        }
        if state.drafts.len() >= MEMORY_BEHAVIOR_MAX_DRAFTS {
            return;
        }
        state.drafts.push(draft);
    }

    /// Observe a repeated pattern and return true when its threshold is reached once.
    #[must_use]
    pub fn observe_pattern(&self, pattern: &str, threshold: usize) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        let count = state.pattern_counts.entry(pattern.to_string()).or_insert(0);
        *count = count.saturating_add(1);
        *count >= threshold && state.emitted_patterns.insert(pattern.to_string())
    }

    /// Return the captured candidates for downstream wiki patch planning.
    #[must_use]
    pub fn snapshot(&self) -> Vec<ToolDerivedMemoryDraft> {
        self.state
            .lock()
            .map(|state| state.drafts.clone())
            .unwrap_or_default()
    }
}
