//! Agent Mode context compaction building blocks.

pub mod archive;
pub mod budget;
pub mod classifier;
pub mod dedup_superseded;
pub mod error_retry_collapse;
pub mod externalize;
pub mod prompt;
pub mod prune;
pub mod rebuild;
pub mod service;
pub mod summarizer;
mod summary;
pub mod types;

pub use archive::{
    persist_compacted_history_chunk, ArchiveChunk, ArchiveRecord, ArchiveRef, ArchiveSink,
    NoopArchiveSink,
};
pub use budget::{count_tokens_cached, estimate_request_budget};
pub use classifier::{classify_hot_memory, classify_hot_memory_with_policy};
pub use dedup_superseded::{
    dedup_superseded_tool_results, DedupSupersededContract, DedupSupersededOutcome,
};
pub use error_retry_collapse::collapse_error_retries;
pub use externalize::{
    externalize_hot_memory, ExternalizedPayloadRecord, NoopPayloadSink, PayloadSink,
};
pub use prompt::{build_compaction_user_message, compaction_system_prompt};
pub use prune::prune_hot_memory;
pub use rebuild::rebuild_hot_context;
pub use service::CompactionService;
pub use summarizer::{CompactionSummarizer, CompactionSummarizerConfig};
pub use types::{
    AgentMessageKind, ArchivePersistenceOutcome, BudgetEstimate, BudgetState,
    ClassifiedMemoryEntry, CompactionClassSummary, CompactionOutcome, CompactionPolicy,
    CompactionRequest, CompactionRetention, CompactionScope, CompactionSnapshot, CompactionSummary,
    CompactionTrigger, ErrorRetryCollapseOutcome, ExternalizationOutcome, HotMemoryBudget,
    PruneOutcome, RebuildOutcome, RecentRawWindow, SummaryGenerationOutcome,
};

#[cfg(test)]
mod tests;
