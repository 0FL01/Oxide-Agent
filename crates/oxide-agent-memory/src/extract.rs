//! Conservative reusable-memory extraction from finalized episodes.

use crate::types::{EpisodeOutcome, EpisodeRecord, MemoryRecord, MemoryType};
use uuid::Uuid;

const MEMORY_CONTENT_MAX_CHARS: usize = 240;
const MEMORY_TITLE_MAX_CHARS: usize = 96;
const MEMORY_SHORT_DESCRIPTION_MAX_CHARS: usize = 160;
const MIN_SIGNAL_CHARS: usize = 24;
const MIN_SIGNAL_WORDS: usize = 4;
const MIN_EPISODE_IMPORTANCE: f32 = 0.7;
const MAX_DECISIONS: usize = 2;
const MAX_CONSTRAINTS: usize = 2;
const MAX_DISCOVERIES: usize = 1;
const MAX_TOTAL_MEMORIES: usize = 4;

/// Structured high-signal fields available for conservative reusable-memory extraction.
#[derive(Debug, Clone, Default)]
pub struct EpisodeMemorySignals {
    /// Decisions captured by structured compaction summaries.
    pub decisions: Vec<String>,
    /// Constraints captured by structured compaction summaries.
    pub constraints: Vec<String>,
    /// Discoveries captured by structured compaction summaries.
    pub discoveries: Vec<String>,
}

/// Conservative extractor that turns high-signal episode facts into reusable memories.
#[derive(Debug, Clone)]
pub struct ReusableMemoryExtractor {
    enabled: bool,
}

impl Default for ReusableMemoryExtractor {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl ReusableMemoryExtractor {
    /// Create an enabled conservative extractor.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a disabled extractor.
    #[must_use]
    pub fn disabled() -> Self {
        Self { enabled: false }
    }

    /// Extract reusable memories from a finalized episode.
    #[must_use]
    pub fn extract(
        &self,
        episode: &EpisodeRecord,
        signals: &EpisodeMemorySignals,
    ) -> Vec<MemoryRecord> {
        if !self.enabled || !episode_is_eligible(episode) {
            return Vec::new();
        }

        let mut memories = Vec::new();
        self.collect_fixed_type(
            episode,
            &signals.decisions,
            MemoryType::Decision,
            MAX_DECISIONS,
            &mut memories,
        );
        self.collect_fixed_type(
            episode,
            &signals.constraints,
            MemoryType::Constraint,
            MAX_CONSTRAINTS,
            &mut memories,
        );
        self.collect_discoveries(episode, &signals.discoveries, &mut memories);
        memories.truncate(MAX_TOTAL_MEMORIES);
        memories
    }

    fn collect_fixed_type(
        &self,
        episode: &EpisodeRecord,
        items: &[String],
        memory_type: MemoryType,
        limit: usize,
        memories: &mut Vec<MemoryRecord>,
    ) {
        for item in items {
            if memories.len() >= MAX_TOTAL_MEMORIES {
                break;
            }

            let Some(content) = normalize_candidate(item) else {
                continue;
            };
            if !matches_expected_type(memory_type, &content) {
                continue;
            }

            push_memory(
                memories,
                build_memory_record(episode, memory_type, &content),
            );
            if count_memories_of_type(memories, memory_type) == limit {
                break;
            }
        }
    }

    fn collect_discoveries(
        &self,
        episode: &EpisodeRecord,
        items: &[String],
        memories: &mut Vec<MemoryRecord>,
    ) {
        let mut collected = 0;
        for item in items {
            if memories.len() >= MAX_TOTAL_MEMORIES {
                break;
            }

            let Some(content) = normalize_candidate(item) else {
                continue;
            };
            let Some(memory_type) = classify_discovery(&content) else {
                continue;
            };

            push_memory(
                memories,
                build_memory_record(episode, memory_type, &content),
            );
            collected += 1;
            if collected == MAX_DISCOVERIES {
                break;
            }
        }
    }
}

fn episode_is_eligible(episode: &EpisodeRecord) -> bool {
    episode.importance >= MIN_EPISODE_IMPORTANCE
        && matches!(
            episode.outcome,
            EpisodeOutcome::Success | EpisodeOutcome::Partial
        )
}

fn count_memories_of_type(memories: &[MemoryRecord], memory_type: MemoryType) -> usize {
    memories
        .iter()
        .filter(|memory| memory.memory_type == memory_type)
        .count()
}

fn push_memory(memories: &mut Vec<MemoryRecord>, candidate: MemoryRecord) {
    if memories
        .iter()
        .any(|existing| existing.memory_id == candidate.memory_id)
    {
        return;
    }
    memories.push(candidate);
}

fn build_memory_record(
    episode: &EpisodeRecord,
    memory_type: MemoryType,
    content: &str,
) -> MemoryRecord {
    let title_prefix = title_prefix(memory_type);
    let title = truncate_chars(
        &format!("{title_prefix}: {content}"),
        MEMORY_TITLE_MAX_CHARS,
    );
    MemoryRecord {
        memory_id: memory_id_for(&episode.episode_id, memory_type, content),
        context_key: episode.context_key.clone(),
        source_episode_id: Some(episode.episode_id.clone()),
        memory_type,
        title,
        content: content.to_string(),
        short_description: truncate_chars(content, MEMORY_SHORT_DESCRIPTION_MAX_CHARS),
        importance: base_importance(memory_type)
            .max(episode.importance)
            .min(1.0),
        confidence: base_confidence(memory_type),
        source: Some("post_run_extract".to_string()),
        reason: Some("conservative reusable-memory extraction from finalized episode".to_string()),
        tags: vec![
            "episode".to_string(),
            memory_type_tag(memory_type).to_string(),
        ],
        created_at: episode.created_at,
        updated_at: episode.created_at,
    }
}

fn normalize_candidate(value: &str) -> Option<String> {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let normalized = truncate_chars(&normalized, MEMORY_CONTENT_MAX_CHARS);
    let word_count = normalized.split_whitespace().count();
    if normalized.len() < MIN_SIGNAL_CHARS
        || word_count < MIN_SIGNAL_WORDS
        || is_noisy_signal(&normalized)
    {
        return None;
    }
    Some(normalized)
}

fn matches_expected_type(memory_type: MemoryType, content: &str) -> bool {
    match memory_type {
        MemoryType::Decision => looks_like_decision(content),
        MemoryType::Constraint => looks_like_constraint(content),
        MemoryType::Fact => looks_like_fact(content),
        MemoryType::Procedure => looks_like_procedure(content),
        MemoryType::Preference => false,
    }
}

fn classify_discovery(content: &str) -> Option<MemoryType> {
    if looks_like_procedure(content) {
        return Some(MemoryType::Procedure);
    }
    looks_like_fact(content).then_some(MemoryType::Fact)
}

fn looks_like_decision(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    has_domain_anchor(&lower)
        && contains_any(
            &lower,
            &[
                "use ",
                "keep ",
                "switch to",
                "prefer ",
                "standardize",
                "persist ",
                "route ",
                "store ",
                "finalize",
                "wire ",
                "separate ",
                "attach ",
            ],
        )
}

fn looks_like_constraint(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    has_domain_anchor(&lower)
        && contains_any(
            &lower,
            &[
                "must",
                "must not",
                "never",
                "do not",
                "don't",
                "cannot",
                "can't",
                "only",
                "required",
                "without ",
                "blocked",
                "forbid",
                "top-level",
                "sub-agent",
                "scoped",
            ],
        )
}

fn looks_like_procedure(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    has_domain_anchor(&lower)
        && starts_with_any(
            &lower,
            &[
                "run ", "use ", "call ", "read ", "write ", "persist ", "attach ", "wire ",
                "create ", "store ", "fetch ", "inspect ", "update ", "reindex ",
            ],
        )
}

fn looks_like_fact(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    has_domain_anchor(&lower)
        && contains_any(
            &lower,
            &[
                " is ",
                " are ",
                " lives in ",
                " stored in ",
                " handled in ",
                " implemented in ",
                " uses ",
                " supports ",
                " contains ",
                " returns ",
                " available in ",
                " exists in ",
            ],
        )
        && !looks_like_decision(content)
        && !looks_like_constraint(content)
        && !looks_like_procedure(content)
}

fn has_domain_anchor(lower: &str) -> bool {
    contains_any(
        lower,
        &[
            ".rs",
            ".toml",
            "::",
            "/",
            "cargo ",
            "r2",
            "memory",
            "storage",
            "thread",
            "session",
            "episode",
            "context",
            "provider",
            "transport",
            "hook",
            "runner",
            "postrun",
            "sub-agent",
            "scope",
            "prompt",
            "sandbox",
            "approval",
            "compaction",
            "tool",
        ],
    ) || lower.split_whitespace().any(|token| {
        token.contains('_') || token.contains('-') || token.chars().any(char::is_uppercase)
    })
}

fn is_noisy_signal(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    lower.contains('?')
        || contains_any(
            &lower,
            &[
                "todo",
                "follow up",
                "remaining work",
                "next step",
                "investigate",
                "need to",
                "needs ",
                "should ",
                "maybe",
                "probably",
                "consider ",
                "open question",
                "waiting for",
                "user input",
                "risk:",
                "warning:",
                "later",
            ],
        )
}

fn memory_id_for(source_episode_id: &str, memory_type: MemoryType, content: &str) -> String {
    let seed = format!(
        "memory:{source_episode_id}:{}:{content}",
        memory_type_tag(memory_type)
    );
    format!(
        "memory-{}",
        Uuid::new_v5(&Uuid::NAMESPACE_URL, seed.as_bytes())
    )
}

fn base_importance(memory_type: MemoryType) -> f32 {
    match memory_type {
        MemoryType::Decision => 0.82,
        MemoryType::Constraint => 0.84,
        MemoryType::Procedure => 0.78,
        MemoryType::Fact => 0.74,
        MemoryType::Preference => 0.7,
    }
}

fn base_confidence(memory_type: MemoryType) -> f32 {
    match memory_type {
        MemoryType::Decision => 0.84,
        MemoryType::Constraint => 0.88,
        MemoryType::Procedure => 0.76,
        MemoryType::Fact => 0.72,
        MemoryType::Preference => 0.7,
    }
}

fn title_prefix(memory_type: MemoryType) -> &'static str {
    match memory_type {
        MemoryType::Decision => "Decision",
        MemoryType::Constraint => "Constraint",
        MemoryType::Procedure => "Procedure",
        MemoryType::Fact => "Fact",
        MemoryType::Preference => "Preference",
    }
}

fn memory_type_tag(memory_type: MemoryType) -> &'static str {
    match memory_type {
        MemoryType::Decision => "decision",
        MemoryType::Constraint => "constraint",
        MemoryType::Procedure => "procedure",
        MemoryType::Fact => "fact",
        MemoryType::Preference => "preference",
    }
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

fn starts_with_any(value: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|prefix| value.starts_with(prefix))
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.trim().chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::{EpisodeMemorySignals, ReusableMemoryExtractor};
    use crate::types::{EpisodeOutcome, EpisodeRecord, MemoryType};
    use chrono::{TimeZone, Utc};

    fn ts() -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000, 0)
            .single()
            .expect("valid timestamp")
    }

    fn episode(importance: f32) -> EpisodeRecord {
        EpisodeRecord {
            episode_id: "episode-1".to_string(),
            thread_id: "thread-1".to_string(),
            context_key: "topic-a".to_string(),
            goal: "Implement Stage 5".to_string(),
            summary: "summary".to_string(),
            outcome: EpisodeOutcome::Success,
            tools_used: vec!["read_file".to_string()],
            artifacts: Vec::new(),
            failures: Vec::new(),
            importance,
            created_at: ts(),
        }
    }

    #[test]
    fn extract_keeps_only_high_signal_records() {
        let extractor = ReusableMemoryExtractor::new();
        let memories = extractor.extract(
            &episode(0.9),
            &EpisodeMemorySignals {
                decisions: vec![
                    "Use StorageMemoryRepository for transport-backed persistent memory writes"
                        .to_string(),
                ],
                constraints: vec![
                    "Sub-agent runs must never persist durable memory records".to_string(),
                ],
                discoveries: vec![
                    "PostRun persistence is handled in crates/oxide-agent-core/src/agent/runner/responses.rs"
                        .to_string(),
                    "Investigate follow up cleanup strategy later".to_string(),
                ],
            },
        );

        assert_eq!(memories.len(), 3);
        assert!(memories
            .iter()
            .any(|memory| memory.memory_type == MemoryType::Decision));
        assert!(memories
            .iter()
            .any(|memory| memory.memory_type == MemoryType::Constraint));
        assert!(memories
            .iter()
            .any(|memory| memory.memory_type == MemoryType::Fact));
        assert!(memories
            .iter()
            .all(|memory| memory.source_episode_id.as_deref() == Some("episode-1")));
    }

    #[test]
    fn extract_supports_conservative_procedure_detection() {
        let extractor = ReusableMemoryExtractor::new();
        let memories = extractor.extract(
            &episode(0.85),
            &EpisodeMemorySignals {
                discoveries: vec![
                    "Run cargo clippy --workspace --all-targets -- -D warnings before finishing the task"
                        .to_string(),
                ],
                ..EpisodeMemorySignals::default()
            },
        );

        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].memory_type, MemoryType::Procedure);
    }

    #[test]
    fn extract_skips_low_importance_episodes() {
        let extractor = ReusableMemoryExtractor::new();
        let memories = extractor.extract(
            &episode(0.55),
            &EpisodeMemorySignals {
                decisions: vec!["Use R2 storage for archive blobs".to_string()],
                ..EpisodeMemorySignals::default()
            },
        );

        assert!(memories.is_empty());
    }
}
