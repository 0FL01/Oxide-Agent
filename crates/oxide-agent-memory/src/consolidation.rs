use crate::types::{CleanupStatus, MemoryRecord, MemoryType, SessionStateRecord};
use chrono::{DateTime, Duration, Utc};
use sha2::Digest;
use std::collections::{HashMap, HashSet};

const MIN_TOKEN_LEN: usize = 3;

#[derive(Debug, Clone)]
pub struct ConsolidationPolicy {
    pub min_similarity: f32,
    pub expiry_importance_threshold: f32,
    pub stale_session_after: Duration,
    pub fact_ttl: Duration,
    pub preference_ttl: Duration,
    pub procedure_ttl: Duration,
    pub decision_ttl: Duration,
    pub constraint_ttl: Duration,
    pub fact_decay_per_day: f32,
    pub preference_decay_per_day: f32,
    pub procedure_decay_per_day: f32,
    pub decision_decay_per_day: f32,
    pub constraint_decay_per_day: f32,
}

impl Default for ConsolidationPolicy {
    fn default() -> Self {
        Self {
            min_similarity: 0.72,
            expiry_importance_threshold: 0.35,
            stale_session_after: Duration::hours(6),
            fact_ttl: Duration::days(90),
            preference_ttl: Duration::days(90),
            procedure_ttl: Duration::days(120),
            decision_ttl: Duration::days(240),
            constraint_ttl: Duration::days(365),
            fact_decay_per_day: 0.004,
            preference_decay_per_day: 0.006,
            procedure_decay_per_day: 0.0035,
            decision_decay_per_day: 0.002,
            constraint_decay_per_day: 0.0015,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ConsolidatedContext {
    pub upserts: Vec<MemoryRecord>,
    pub deletions: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ContextConsolidator {
    policy: ConsolidationPolicy,
}

impl ContextConsolidator {
    #[must_use]
    pub fn new(policy: ConsolidationPolicy) -> Self {
        Self { policy }
    }

    #[must_use]
    pub fn consolidate(
        &self,
        memories: &[MemoryRecord],
        now: DateTime<Utc>,
    ) -> ConsolidatedContext {
        let mut working = memories
            .iter()
            .filter(|memory| memory.deleted_at.is_none())
            .cloned()
            .map(|mut memory| {
                memory.content_hash = Some(memory.content_hash.clone().unwrap_or_else(|| {
                    stable_memory_content_hash(memory.memory_type, &memory.content)
                }));
                let rescored = self.rescored_importance(&memory, now);
                if (memory.importance - rescored).abs() > 0.001 {
                    memory.importance = rescored;
                }
                memory
            })
            .collect::<Vec<_>>();

        let mut upserts = HashMap::<String, MemoryRecord>::new();
        let mut deletions = HashSet::<String>::new();

        let mut exact_groups = HashMap::<(MemoryType, String), Vec<usize>>::new();
        for (index, memory) in working.iter().enumerate() {
            let hash = memory
                .content_hash
                .clone()
                .unwrap_or_else(|| stable_memory_content_hash(memory.memory_type, &memory.content));
            exact_groups
                .entry((memory.memory_type, hash))
                .or_default()
                .push(index);
        }

        for indexes in exact_groups.into_values() {
            if indexes.len() < 2 {
                continue;
            }
            let winner = select_winner(&working, &indexes);
            let mut merged = working[winner].clone();
            for index in indexes {
                if index == winner {
                    continue;
                }
                merged = merge_memories(merged, &working[index], now);
                deletions.insert(working[index].memory_id.clone());
            }
            upserts.insert(merged.memory_id.clone(), merged.clone());
            working[winner] = merged;
        }

        let mut consumed = HashSet::<usize>::new();
        for left in 0..working.len() {
            if consumed.contains(&left) || deletions.contains(&working[left].memory_id) {
                continue;
            }
            for right in (left + 1)..working.len() {
                if consumed.contains(&right)
                    || deletions.contains(&working[right].memory_id)
                    || working[left].memory_type != working[right].memory_type
                {
                    continue;
                }
                if similarity(&working[left], &working[right]) < self.policy.min_similarity {
                    continue;
                }
                let winner = select_winner(&working, &[left, right]);
                let loser = if winner == left { right } else { left };
                let merged = merge_memories(working[winner].clone(), &working[loser], now);
                deletions.insert(working[loser].memory_id.clone());
                upserts.insert(merged.memory_id.clone(), merged.clone());
                working[winner] = merged;
                consumed.insert(loser);
            }
        }

        for memory in working {
            if deletions.contains(&memory.memory_id) {
                continue;
            }
            if self.should_expire(&memory, now) {
                deletions.insert(memory.memory_id.clone());
            } else if upserts.contains_key(&memory.memory_id) || memory.content_hash.is_some() {
                upserts.entry(memory.memory_id.clone()).or_insert(memory);
            }
        }

        ConsolidatedContext {
            upserts: upserts.into_values().collect(),
            deletions: deletions.into_iter().collect(),
        }
    }

    #[must_use]
    pub fn stale_sessions<'a>(
        &self,
        states: impl IntoIterator<Item = &'a SessionStateRecord>,
        now: DateTime<Utc>,
    ) -> Vec<SessionStateRecord> {
        let cutoff = now - self.policy.stale_session_after;
        let mut stale = states
            .into_iter()
            .filter(|state| {
                matches!(
                    state.cleanup_status,
                    CleanupStatus::Idle | CleanupStatus::Cleaning
                ) && state.updated_at <= cutoff
            })
            .cloned()
            .collect::<Vec<_>>();
        stale.sort_by(|left, right| left.updated_at.cmp(&right.updated_at));
        stale
    }

    fn rescored_importance(&self, memory: &MemoryRecord, now: DateTime<Utc>) -> f32 {
        let age_days = (now - memory.updated_at).num_days().max(0) as f32;
        let decay = match memory.memory_type {
            MemoryType::Fact => self.policy.fact_decay_per_day,
            MemoryType::Preference => self.policy.preference_decay_per_day,
            MemoryType::Procedure => self.policy.procedure_decay_per_day,
            MemoryType::Decision => self.policy.decision_decay_per_day,
            MemoryType::Constraint => self.policy.constraint_decay_per_day,
        };
        (memory.importance - decay * age_days).clamp(0.0, 1.0)
    }

    fn should_expire(&self, memory: &MemoryRecord, now: DateTime<Utc>) -> bool {
        memory.importance <= self.policy.expiry_importance_threshold
            && now - memory.updated_at >= ttl_for(memory.memory_type, &self.policy)
    }
}

#[must_use]
pub fn stable_memory_content_hash(memory_type: MemoryType, content: &str) -> String {
    let kind = match memory_type {
        MemoryType::Fact => "fact",
        MemoryType::Preference => "preference",
        MemoryType::Procedure => "procedure",
        MemoryType::Decision => "decision",
        MemoryType::Constraint => "constraint",
    };
    let normalized = normalize_text(content);
    let digest = sha2::Sha256::digest(format!("{kind}:{normalized}").as_bytes());
    format!("{:x}", digest)
}

fn ttl_for(memory_type: MemoryType, policy: &ConsolidationPolicy) -> Duration {
    match memory_type {
        MemoryType::Fact => policy.fact_ttl,
        MemoryType::Preference => policy.preference_ttl,
        MemoryType::Procedure => policy.procedure_ttl,
        MemoryType::Decision => policy.decision_ttl,
        MemoryType::Constraint => policy.constraint_ttl,
    }
}

fn select_winner(memories: &[MemoryRecord], indexes: &[usize]) -> usize {
    *indexes
        .iter()
        .max_by(|left, right| compare_memory_priority(&memories[**left], &memories[**right]))
        .expect("winner selection requires at least one memory")
}

fn compare_memory_priority(left: &MemoryRecord, right: &MemoryRecord) -> std::cmp::Ordering {
    left.importance
        .total_cmp(&right.importance)
        .then_with(|| left.confidence.total_cmp(&right.confidence))
        .then_with(|| left.updated_at.cmp(&right.updated_at))
        .then_with(|| right.memory_id.cmp(&left.memory_id))
}

fn merge_memories(
    mut canonical: MemoryRecord,
    duplicate: &MemoryRecord,
    now: DateTime<Utc>,
) -> MemoryRecord {
    canonical.importance = canonical.importance.max(duplicate.importance);
    canonical.confidence = canonical.confidence.max(duplicate.confidence);
    canonical.updated_at = now.max(canonical.updated_at.max(duplicate.updated_at));
    if canonical.source.is_none() {
        canonical.source = duplicate.source.clone();
    }
    if canonical.reason.is_none() {
        canonical.reason = duplicate.reason.clone();
    }
    if canonical.source_episode_id.is_none() {
        canonical.source_episode_id = duplicate.source_episode_id.clone();
    }
    for tag in &duplicate.tags {
        if !canonical.tags.iter().any(|existing| existing == tag) {
            canonical.tags.push(tag.clone());
        }
    }
    if canonical.content_hash.is_none() {
        canonical.content_hash = duplicate.content_hash.clone();
    }
    canonical
}

fn similarity(left: &MemoryRecord, right: &MemoryRecord) -> f32 {
    let left_tokens = token_set(left);
    let right_tokens = token_set(right);
    if left_tokens.is_empty() || right_tokens.is_empty() {
        return 0.0;
    }
    let overlap = left_tokens.intersection(&right_tokens).count() as f32;
    let union = left_tokens.union(&right_tokens).count() as f32;
    if union == 0.0 {
        0.0
    } else {
        overlap / union
    }
}

fn token_set(memory: &MemoryRecord) -> HashSet<String> {
    normalize_text(&format!(
        "{} {} {}",
        memory.title, memory.short_description, memory.content
    ))
    .split_whitespace()
    .filter(|token| token.len() >= MIN_TOKEN_LEN)
    .map(str::to_string)
    .collect()
}

fn normalize_text(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character == '_' || character == '-' {
                character.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::{stable_memory_content_hash, ConsolidationPolicy, ContextConsolidator};
    use crate::types::{CleanupStatus, MemoryRecord, MemoryType, SessionStateRecord};
    use chrono::{DateTime, TimeZone, Utc};

    fn ts(seconds: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(seconds, 0)
            .single()
            .expect("valid timestamp")
    }

    fn memory(
        memory_id: &str,
        memory_type: MemoryType,
        content: &str,
        updated_at: i64,
    ) -> MemoryRecord {
        MemoryRecord {
            memory_id: memory_id.to_string(),
            context_key: "topic-a".to_string(),
            source_episode_id: Some("ep-1".to_string()),
            memory_type,
            title: format!("{memory_id} title"),
            content: content.to_string(),
            short_description: content.to_string(),
            importance: 0.7,
            confidence: 0.8,
            source: Some("test".to_string()),
            content_hash: Some(stable_memory_content_hash(memory_type, content)),
            reason: None,
            tags: vec!["fixture".to_string()],
            created_at: ts(updated_at - 10),
            updated_at: ts(updated_at),
            deleted_at: None,
        }
    }

    #[test]
    fn stable_hash_is_type_sensitive() {
        assert_ne!(
            stable_memory_content_hash(MemoryType::Fact, "same content"),
            stable_memory_content_hash(MemoryType::Decision, "same content")
        );
    }

    #[test]
    fn consolidator_deduplicates_exact_memories() {
        let consolidator = ContextConsolidator::new(ConsolidationPolicy::default());
        let result = consolidator.consolidate(
            &[
                memory(
                    "mem-1",
                    MemoryType::Fact,
                    "Use cargo check before build",
                    100,
                ),
                memory(
                    "mem-2",
                    MemoryType::Fact,
                    "Use cargo check before build",
                    120,
                ),
            ],
            ts(200),
        );
        assert_eq!(result.deletions.len(), 1);
        assert_eq!(result.upserts.len(), 1);
    }

    #[test]
    fn consolidator_expires_old_low_signal_memories() {
        let consolidator = ContextConsolidator::new(ConsolidationPolicy::default());
        let mut record = memory("mem-1", MemoryType::Preference, "Prefer terse answers", 0);
        record.importance = 0.2;
        let result = consolidator.consolidate(&[record], ts(200 * 24 * 3600));
        assert_eq!(result.deletions, vec!["mem-1".to_string()]);
    }

    #[test]
    fn stale_sessions_pick_idle_old_entries() {
        let consolidator = ContextConsolidator::new(ConsolidationPolicy::default());
        let states = vec![
            SessionStateRecord {
                session_id: "sess-1".to_string(),
                context_key: "topic-a".to_string(),
                hot_token_estimate: 10,
                last_compacted_at: None,
                last_finalized_at: None,
                cleanup_status: CleanupStatus::Idle,
                pending_episode_id: None,
                updated_at: ts(0),
            },
            SessionStateRecord {
                session_id: "sess-2".to_string(),
                context_key: "topic-a".to_string(),
                hot_token_estimate: 10,
                last_compacted_at: None,
                last_finalized_at: None,
                cleanup_status: CleanupStatus::Active,
                pending_episode_id: None,
                updated_at: ts(0),
            },
        ];
        let stale = consolidator.stale_sessions(&states, ts(24 * 3600));
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].session_id, "sess-1");
    }
}
