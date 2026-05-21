//! Agent Mode context compaction building blocks.
//!
//! The runtime path is `CompactionController` plus `LocalLlmSummary`. Legacy
//! persisted data is handled by serde-compatible message/type fields and the
//! deterministic history migration helpers.

pub mod archive;
pub mod budget;
pub mod controller;
pub mod history;
pub mod local_llm_summary;
pub mod prompt;
pub mod task;
pub mod types;

pub use archive::{ArchiveChunk, ArchiveRecord, ArchiveRef};
pub use budget::{count_tokens_cached, estimate_request_budget};
pub use controller::{
    CompactRequestContext, CompactRunOutcome, CompactionController, CompactionControllerError,
};
pub use history::{
    build_compacted_history, extract_previous_compacted_summary, is_any_compaction_summary_message,
    is_current_compacted_summary_message, BuildCompactedHistoryRequest, CompactedHistoryBuildError,
    PreviousCompactedSummary,
};
pub use local_llm_summary::LocalLlmSummary;
pub use task::{
    CompactSummaryBackend, CompactSummaryError, CompactSummaryRequest, CompactSummaryResult,
};
pub use types::{
    AgentMessageKind, BreadcrumbCard, BudgetEstimate, BudgetState, CompactedSummaryMetadata,
    CompactionBackend, CompactionPhase, CompactionPolicy, CompactionReason, CompactionRequest,
    CompactionRetention, CompactionScope, CompactionSummary, CompactionTrigger, HotContextLimits,
    HotMemoryBudget, LEGACY_BREADCRUMB_PREFIX, LEGACY_COMPACTION_SUMMARY_PREFIX,
    OXIDE_COMPACTED_SUMMARY_PREFIX,
};
