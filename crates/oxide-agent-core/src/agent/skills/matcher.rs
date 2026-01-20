//! Skill matching logic (keyword + semantic).

use crate::agent::skills::types::{SkillMetadata, SkillWeight};
use std::cmp::Ordering;
use std::collections::HashMap;

/// Input for skill selection.
pub struct SkillMatcherInput<'a> {
    /// User message used for matching.
    pub user_message: &'a str,
    /// Skill metadata list.
    pub metadata: &'a [SkillMetadata],
    /// Precomputed semantic scores per skill.
    pub semantic_scores: Option<&'a HashMap<String, f32>>,
    /// Whether embeddings are available for matching.
    pub embeddings_available: bool,
}

/// Match result for a skill.
#[derive(Debug, Clone)]
pub struct SkillMatch {
    /// Skill name.
    pub name: String,
    /// Skill weight.
    pub weight: SkillWeight,
    /// Whether a keyword trigger matched.
    pub trigger_match: bool,
    /// Semantic similarity score, if available.
    pub semantic_score: Option<f32>,
    /// Combined score used for ranking.
    pub combined_score: f32,
}

/// Selects skills using hybrid matching.
#[derive(Debug, Clone)]
pub struct SkillMatcher {
    semantic_threshold: f32,
    max_selected: usize,
}

impl SkillMatcher {
    /// Create a matcher with thresholds and selection limits.
    #[must_use]
    pub fn new(semantic_threshold: f32, max_selected: usize) -> Self {
        Self {
            semantic_threshold,
            max_selected,
        }
    }

    /// Select relevant skills for the given input.
    #[must_use]
    pub fn select_skills(&self, input: SkillMatcherInput<'_>) -> Vec<SkillMatch> {
        let user_message = input.user_message.to_lowercase();
        let mut matches = Vec::new();

        for meta in input.metadata {
            let trigger_match = meta
                .triggers
                .iter()
                .any(|trigger| user_message.contains(&trigger.to_lowercase()));

            let semantic_score = input
                .semantic_scores
                .and_then(|scores| scores.get(&meta.name).copied());
            let semantic_pass = semantic_score
                .map(|score| score >= self.semantic_threshold)
                .unwrap_or(false);

            let qualifies = match meta.weight {
                SkillWeight::Always => true,
                SkillWeight::High => trigger_match || semantic_pass || !input.embeddings_available,
                SkillWeight::Medium | SkillWeight::OnDemand => trigger_match || semantic_pass,
            };

            if !qualifies {
                continue;
            }

            let combined_score =
                0.7 * semantic_score.unwrap_or(0.0) + if trigger_match { 0.3 } else { 0.0 };

            matches.push(SkillMatch {
                name: meta.name.clone(),
                weight: meta.weight,
                trigger_match,
                semantic_score,
                combined_score,
            });
        }

        let mut deduped = Vec::new();
        let mut seen = HashMap::<String, usize>::new();

        for skill in matches {
            if let Some(index) = seen.get(skill.name.as_str()).copied() {
                if is_better_match(&skill, &deduped[index]) {
                    deduped[index] = skill;
                }
            } else {
                seen.insert(skill.name.clone(), deduped.len());
                deduped.push(skill);
            }
        }

        let mut always = Vec::new();
        let mut candidates = Vec::new();

        for skill in deduped {
            if skill.weight == SkillWeight::Always {
                always.push(skill);
            } else {
                candidates.push(skill);
            }
        }

        candidates.sort_by(|a, b| {
            b.weight
                .priority()
                .cmp(&a.weight.priority())
                .then_with(|| {
                    b.combined_score
                        .partial_cmp(&a.combined_score)
                        .unwrap_or(Ordering::Equal)
                })
                .then_with(|| a.name.cmp(&b.name))
        });

        let mut selected = always;
        selected.extend(candidates.into_iter().take(self.max_selected));
        selected
    }
}

fn is_better_match(candidate: &SkillMatch, current: &SkillMatch) -> bool {
    let weight_cmp = candidate.weight.priority().cmp(&current.weight.priority());
    if weight_cmp != Ordering::Equal {
        return weight_cmp == Ordering::Greater;
    }

    let score_cmp = candidate.combined_score.total_cmp(&current.combined_score);
    if score_cmp != Ordering::Equal {
        return score_cmp == Ordering::Greater;
    }

    let trigger_cmp = candidate.trigger_match.cmp(&current.trigger_match);
    if trigger_cmp != Ordering::Equal {
        return trigger_cmp == Ordering::Greater;
    }

    candidate
        .semantic_score
        .unwrap_or(0.0)
        .total_cmp(&current.semantic_score.unwrap_or(0.0))
        == Ordering::Greater
}
