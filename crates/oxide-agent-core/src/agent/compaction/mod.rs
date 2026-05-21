//! Agent Mode context compaction building blocks.
//!
//! The active runtime path is `CompactionController` plus `LocalLlmSummary`.
//! Older staged compaction modules (`service`, `classifier`, `externalize`,
//! `prune`, `rebuild`, `summarizer`) are retained as compatibility and
//! regression-test surface for old persisted sessions. They must not be wired
//! as a production runtime fallback.

pub mod archive;
pub mod budget;
pub mod classifier;
pub mod controller;
pub mod dedup_superseded;
pub mod error_retry_collapse;
pub mod externalize;
pub mod history;
pub mod local_llm_summary;
pub mod prompt;
pub mod prune;
pub mod rebuild;
/// Compatibility-only legacy staged compaction service.
pub mod service;
pub mod summarizer;
mod summary;
pub mod task;
pub mod types;

pub use archive::{
    persist_compacted_history_chunk, ArchiveChunk, ArchiveRecord, ArchiveRef, ArchiveSink,
    NoopArchiveSink,
};
pub use budget::{count_tokens_cached, estimate_request_budget};
pub use classifier::{classify_hot_memory, classify_hot_memory_with_policy};
pub use controller::{
    CompactRequestContext, CompactRunOutcome, CompactionController, CompactionControllerError,
};
pub use dedup_superseded::{dedup_superseded_tool_results, DedupSupersededContract};
pub use error_retry_collapse::collapse_error_retries;
pub use externalize::{
    externalize_hot_memory, ExternalizedPayloadRecord, NoopPayloadSink, PayloadSink,
};
pub use history::{
    build_compacted_history, extract_previous_compacted_summary, is_any_compaction_summary_message,
    is_current_compacted_summary_message, BuildCompactedHistoryRequest, CompactedHistoryBuildError,
    PreviousCompactedSummary,
};
pub use local_llm_summary::LocalLlmSummary;
pub use prompt::{build_compaction_user_message, compaction_system_prompt};
pub use prune::prune_hot_memory;
pub use rebuild::{rebuild_hot_context, truncate_to_working_set};
pub use service::CompactionService;
pub use summarizer::{CompactionSummarizer, CompactionSummarizerConfig};
pub use task::{
    CompactSummaryBackend, CompactSummaryError, CompactSummaryRequest, CompactSummaryResult,
};
pub use types::{
    AgentMessageKind, ArchivePersistenceOutcome, BreadcrumbCard, BudgetEstimate, BudgetState,
    ClassifiedMemoryEntry, CompactedSummaryMetadata, CompactionBackend, CompactionClassSummary,
    CompactionOutcome, CompactionPhase, CompactionPolicy, CompactionReason, CompactionRequest,
    CompactionRetention, CompactionScope, CompactionSnapshot, CompactionSummary, CompactionTrigger,
    DedupSupersededOutcome, ErrorRetryCollapseOutcome, ExternalizationOutcome, HotContextLimits,
    HotMemoryBudget, PruneOutcome, RebuildOutcome, RecentRawWindow, SummaryGenerationOutcome,
    LEGACY_BREADCRUMB_PREFIX, LEGACY_COMPACTION_SUMMARY_PREFIX, OXIDE_COMPACTED_SUMMARY_PREFIX,
};

#[cfg(test)]
mod tests;
