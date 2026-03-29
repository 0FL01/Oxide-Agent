//! Shared helpers for bounded structured compaction summaries.

use super::types::{AgentMessageKind, CompactionSnapshot, CompactionSummary};
use crate::agent::memory::AgentMessage;

const SUMMARY_GOAL_MAX_CHARS: usize = 240;
const SUMMARY_LIST_ITEM_MAX_CHARS: usize = 240;
const SUMMARY_CONSTRAINT_LIMIT: usize = 8;
const SUMMARY_DECISION_LIMIT: usize = 8;
const SUMMARY_DISCOVERY_LIMIT: usize = 8;
const SUMMARY_RELEVANT_ENTITY_LIMIT: usize = 10;
const SUMMARY_REMAINING_WORK_LIMIT: usize = 8;
const SUMMARY_RISK_LIMIT: usize = 8;

#[must_use]
pub(super) fn collect_existing_structured_summaries(
    snapshot: &CompactionSnapshot,
    messages: &[AgentMessage],
) -> Vec<CompactionSummary> {
    snapshot
        .entries
        .iter()
        .filter(|entry| entry.kind == AgentMessageKind::Summary)
        .filter_map(|entry| messages.get(entry.index))
        .filter_map(|message| message.summary_payload().cloned())
        .collect()
}

#[must_use]
pub(super) fn normalize_summary(summary: CompactionSummary) -> Option<CompactionSummary> {
    let normalized = CompactionSummary {
        goal: normalize_scalar(summary.goal, SUMMARY_GOAL_MAX_CHARS),
        constraints: normalize_items(
            summary.constraints,
            SUMMARY_CONSTRAINT_LIMIT,
            SUMMARY_LIST_ITEM_MAX_CHARS,
        ),
        decisions: normalize_items(
            summary.decisions,
            SUMMARY_DECISION_LIMIT,
            SUMMARY_LIST_ITEM_MAX_CHARS,
        ),
        discoveries: normalize_items(
            summary.discoveries,
            SUMMARY_DISCOVERY_LIMIT,
            SUMMARY_LIST_ITEM_MAX_CHARS,
        ),
        relevant_files_entities: normalize_items(
            summary.relevant_files_entities,
            SUMMARY_RELEVANT_ENTITY_LIMIT,
            SUMMARY_LIST_ITEM_MAX_CHARS,
        ),
        remaining_work: normalize_items(
            summary.remaining_work,
            SUMMARY_REMAINING_WORK_LIMIT,
            SUMMARY_LIST_ITEM_MAX_CHARS,
        ),
        risks: normalize_items(
            summary.risks,
            SUMMARY_RISK_LIMIT,
            SUMMARY_LIST_ITEM_MAX_CHARS,
        ),
    };

    summary_has_signal(&normalized).then_some(normalized)
}

#[must_use]
pub(super) fn merge_summaries_bounded(
    existing_summaries: impl IntoIterator<Item = CompactionSummary>,
    new_summary: Option<CompactionSummary>,
) -> Option<CompactionSummary> {
    let mut summaries: Vec<CompactionSummary> = existing_summaries
        .into_iter()
        .filter_map(normalize_summary)
        .collect();
    if let Some(summary) = new_summary.and_then(normalize_summary) {
        summaries.push(summary);
    }
    if summaries.is_empty() {
        return None;
    }

    let mut merged = CompactionSummary {
        goal: summaries
            .iter()
            .rev()
            .find_map(|summary| {
                let goal = summary.goal.trim();
                (!goal.is_empty()).then(|| goal.to_string())
            })
            .unwrap_or_default(),
        ..CompactionSummary::default()
    };
    for summary in summaries {
        push_unique(&mut merged.constraints, summary.constraints);
        push_unique(&mut merged.decisions, summary.decisions);
        push_unique(&mut merged.discoveries, summary.discoveries);
        push_unique(
            &mut merged.relevant_files_entities,
            summary.relevant_files_entities,
        );
        push_unique(&mut merged.remaining_work, summary.remaining_work);
        push_unique(&mut merged.risks, summary.risks);
    }

    normalize_summary(merged)
}

#[must_use]
pub(super) fn summary_has_signal(summary: &CompactionSummary) -> bool {
    !summary.goal.trim().is_empty()
        || !summary.constraints.is_empty()
        || !summary.decisions.is_empty()
        || !summary.discoveries.is_empty()
        || !summary.relevant_files_entities.is_empty()
        || !summary.remaining_work.is_empty()
        || !summary.risks.is_empty()
}

fn push_unique(target: &mut Vec<String>, items: Vec<String>) {
    for item in items {
        let trimmed = item.trim();
        if trimmed.is_empty() || target.iter().any(|existing| existing == trimmed) {
            continue;
        }
        target.push(trimmed.to_string());
    }
}

fn normalize_items(items: Vec<String>, limit: usize, max_chars: usize) -> Vec<String> {
    dedupe_limit(
        items
            .into_iter()
            .map(|item| normalize_scalar(item, max_chars))
            .collect(),
        limit,
    )
}

fn dedupe_limit(items: Vec<String>, limit: usize) -> Vec<String> {
    let mut deduped = Vec::new();
    for item in items {
        let trimmed = item.trim();
        if trimmed.is_empty() || deduped.iter().any(|existing: &String| existing == trimmed) {
            continue;
        }
        deduped.push(trimmed.to_string());
        if deduped.len() == limit {
            break;
        }
    }
    deduped
}

fn normalize_scalar(value: String, max_chars: usize) -> String {
    value.trim().chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::merge_summaries_bounded;
    use crate::agent::compaction::CompactionSummary;

    #[test]
    fn merge_summaries_bounded_preserves_latest_goal_and_caps_lists() {
        let existing = CompactionSummary {
            goal: "Older goal".to_string(),
            decisions: (0..8)
                .map(|index| format!("existing decision {index}"))
                .collect(),
            ..CompactionSummary::default()
        };
        let updated = CompactionSummary {
            goal: "Latest goal".to_string(),
            decisions: (0..8)
                .map(|index| format!("updated decision {index}"))
                .collect(),
            ..CompactionSummary::default()
        };

        let merged = merge_summaries_bounded(vec![existing], Some(updated)).expect("summary");

        assert_eq!(merged.goal, "Latest goal");
        assert_eq!(merged.decisions.len(), 8);
        assert!(merged
            .decisions
            .iter()
            .any(|item| item == "existing decision 0"));
        assert!(merged
            .decisions
            .iter()
            .any(|item| item == "existing decision 7"));
        assert!(!merged
            .decisions
            .iter()
            .any(|item| item == "updated decision 7"));
    }
}
