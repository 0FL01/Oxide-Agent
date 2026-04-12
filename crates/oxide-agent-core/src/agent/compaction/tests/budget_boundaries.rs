use crate::agent::compaction::{
    estimate_request_budget, BudgetState, CompactionPolicy, CompactionRequest,
};
use crate::agent::{AgentContext, EphemeralSession};

fn boundary_policy() -> CompactionPolicy {
    CompactionPolicy {
        warning_threshold_percent: 20,
        prune_threshold_percent: 40,
        compact_threshold_percent: 60,
        over_limit_threshold_percent: 80,
        hard_reserve_tokens: 0,
        ..CompactionPolicy::default()
    }
}

fn request_for_budget() -> CompactionRequest<'static> {
    CompactionRequest::new(
        crate::agent::compaction::CompactionTrigger::PreRun,
        "Inspect budget boundaries",
        "",
        &[],
        "demo-model",
        0,
        false,
    )
}

fn estimate_with_skill_tokens(skill_tokens: usize) -> crate::agent::compaction::BudgetEstimate {
    let mut session = EphemeralSession::new(1_000);
    if skill_tokens > 0 {
        assert!(session.register_loaded_skill("release", skill_tokens));
    }
    estimate_request_budget(&boundary_policy(), &request_for_budget(), &session)
}

#[test]
fn budget_thresholds_transition_at_expected_percent_boundaries() {
    assert_eq!(estimate_with_skill_tokens(199).state, BudgetState::Healthy);
    assert_eq!(estimate_with_skill_tokens(200).state, BudgetState::Warning);
    assert_eq!(estimate_with_skill_tokens(399).state, BudgetState::Warning);
    assert_eq!(
        estimate_with_skill_tokens(400).state,
        BudgetState::ShouldPrune
    );
    assert_eq!(
        estimate_with_skill_tokens(599).state,
        BudgetState::ShouldPrune
    );
    assert_eq!(
        estimate_with_skill_tokens(600).state,
        BudgetState::ShouldCompact
    );
    assert_eq!(
        estimate_with_skill_tokens(799).state,
        BudgetState::ShouldCompact
    );
    assert_eq!(
        estimate_with_skill_tokens(800).state,
        BudgetState::OverLimit
    );
}

#[test]
fn budget_includes_loaded_skill_tokens_in_projected_total() {
    let estimate = estimate_with_skill_tokens(500);

    assert_eq!(estimate.loaded_skill_tokens, 500);
    assert_eq!(estimate.total_input_tokens, 500);
    assert_eq!(estimate.projected_total_tokens, 500);
    assert_eq!(estimate.state, BudgetState::ShouldPrune);
}

#[test]
fn budget_threshold_metadata_matches_context_window_percentages() {
    let estimate = estimate_with_skill_tokens(0);

    assert_eq!(estimate.context_window_tokens, 1_000);
    assert_eq!(estimate.warning_threshold_tokens, 200);
    assert_eq!(estimate.prune_threshold_tokens, 400);
    assert_eq!(estimate.compact_threshold_tokens, 600);
    assert_eq!(estimate.over_limit_threshold_tokens, 800);
}
