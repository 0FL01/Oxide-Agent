use super::*;

const MEMORY_BEHAVIOR_MAX_DRAFTS: usize = 8;

/// Scope-aware policy for topic-native memory behavior.
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

/// Tool-derived reusable-memory draft captured during the live agent run.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDerivedMemoryDraft {
    pub memory_type: MemoryType,
    pub title: String,
    pub content: String,
    pub short_description: String,
    pub importance: f32,
    pub confidence: f32,
    pub source: String,
    pub reason: String,
    pub tags: Vec<String>,
    pub captured_at: DateTime<Utc>,
}

#[derive(Debug, Default)]
struct MemoryBehaviorState {
    drafts: Vec<ToolDerivedMemoryDraft>,
    pattern_counts: HashMap<String, usize>,
    emitted_patterns: HashSet<String>,
}

/// Task-local runtime used by Stage-14 hooks to capture memory behavior signals.
#[derive(Debug, Default)]
pub struct MemoryBehaviorRuntime {
    state: Mutex<MemoryBehaviorState>,
}

impl MemoryBehaviorRuntime {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&self) {
        if let Ok(mut state) = self.state.lock() {
            *state = MemoryBehaviorState::default();
        }
    }

    pub fn record_draft(&self, draft: ToolDerivedMemoryDraft) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.drafts.iter().any(|existing| {
            existing.memory_type == draft.memory_type && existing.content == draft.content
        }) {
            return;
        }
        if state.drafts.len() >= MEMORY_BEHAVIOR_MAX_DRAFTS {
            return;
        }
        state.drafts.push(draft);
    }

    #[must_use]
    pub fn observe_pattern(&self, pattern: &str, threshold: usize) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        let count = state.pattern_counts.entry(pattern.to_string()).or_insert(0);
        *count = count.saturating_add(1);
        *count >= threshold && state.emitted_patterns.insert(pattern.to_string())
    }

    #[must_use]
    pub fn snapshot(&self) -> Vec<ToolDerivedMemoryDraft> {
        self.state
            .lock()
            .map(|state| state.drafts.clone())
            .unwrap_or_default()
    }
}
