//! Stage-0 contract for superseded tool-result deduplication.

use super::types::{AgentMessageKind, CompactionSnapshot, DedupSupersededOutcome};
use crate::agent::memory::AgentMessage;

/// Placeholder written into older tool results once a later identical output supersedes them.
pub const SUPERSEDED_DEDUP_PLACEHOLDER: &str =
    "[deduplicated tool result: superseded by later identical output]";

/// Stage-0 scope contract for deterministic superseded-result deduplication.
///
/// This contract is intentionally conservative and documents the constraints for
/// later implementation stages while keeping runtime behavior unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DedupSupersededContract {
    /// Stage-level toggle. Disabled in Stage 0.
    pub enabled: bool,
    /// Read-only deterministic tools that are eligible for dedup.
    pub tool_allowlist: Vec<String>,
    /// Dedup applies only to old history prior to the latest summary boundary.
    pub only_before_latest_summary_boundary: bool,
    /// Dedup never touches the protected recent tool window.
    pub preserve_protected_recent_tool_window: bool,
    /// Dedup requires canonical tool-name and args equality.
    pub require_canonical_tool_and_args_match: bool,
    /// Dedup requires output fingerprint equality.
    pub require_output_fingerprint_match: bool,
    /// Dedup is blocked when mutating actions occurred between candidate reads.
    pub block_on_intermediate_mutating_actions: bool,
    /// Placeholder message for superseded entries.
    pub superseded_placeholder: String,
}

impl Default for DedupSupersededContract {
    fn default() -> Self {
        Self {
            enabled: false,
            tool_allowlist: vec!["read_file".to_string()],
            only_before_latest_summary_boundary: true,
            preserve_protected_recent_tool_window: true,
            require_canonical_tool_and_args_match: true,
            require_output_fingerprint_match: true,
            block_on_intermediate_mutating_actions: true,
            superseded_placeholder: SUPERSEDED_DEDUP_PLACEHOLDER.to_string(),
        }
    }
}

/// Stage-0 no-op deterministic stage for superseded tool-result deduplication.
///
/// The function is wired into compaction orchestration so future stages can add
/// dedup behavior without changing the execution topology.
#[must_use]
pub fn dedup_superseded_tool_results(
    _contract: &DedupSupersededContract,
    snapshot: &CompactionSnapshot,
    messages: &[AgentMessage],
) -> (Vec<AgentMessage>, DedupSupersededOutcome) {
    let has_tool_history = snapshot.entries.iter().any(|entry| {
        matches!(
            entry.kind,
            AgentMessageKind::AssistantToolCall | AgentMessageKind::ToolResult
        )
    });
    if !has_tool_history {
        return (messages.to_vec(), DedupSupersededOutcome::default());
    }

    (messages.to_vec(), DedupSupersededOutcome::default())
}

#[cfg(test)]
mod tests {
    use super::DedupSupersededContract;

    #[test]
    fn stage_zero_contract_is_conservative_and_disabled() {
        let contract = DedupSupersededContract::default();
        assert!(!contract.enabled);
        assert_eq!(contract.tool_allowlist, vec!["read_file"]);
        assert!(contract.only_before_latest_summary_boundary);
        assert!(contract.preserve_protected_recent_tool_window);
        assert!(contract.require_canonical_tool_and_args_match);
        assert!(contract.require_output_fingerprint_match);
        assert!(contract.block_on_intermediate_mutating_actions);
        assert!(contract
            .superseded_placeholder
            .contains("superseded by later identical output"));
    }
}
