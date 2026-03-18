//! Agent Mode context compaction building blocks.

pub mod archive;
pub mod budget;
pub mod classifier;
pub mod externalize;
pub mod prompt;
pub mod prune;
pub mod service;
pub mod summarizer;
pub mod types;

pub use archive::{ArchiveRecord, ArchiveRef, ArchiveSink, NoopArchiveSink};
pub use budget::estimate_request_budget;
pub use classifier::classify_hot_memory;
pub use externalize::{
    externalize_hot_memory, ExternalizedPayloadRecord, NoopPayloadSink, PayloadSink,
};
pub use prompt::{build_compaction_user_message, compaction_system_prompt};
pub use prune::prune_hot_memory;
pub use service::CompactionService;
pub use summarizer::{CompactionSummarizer, CompactionSummarizerConfig};
pub use types::{
    AgentMessageKind, BudgetEstimate, BudgetState, ClassifiedMemoryEntry, CompactionClassSummary,
    CompactionOutcome, CompactionPolicy, CompactionRequest, CompactionRetention, CompactionScope,
    CompactionSnapshot, CompactionSummary, CompactionTrigger, ExternalizationOutcome,
    HotMemoryBudget, PruneOutcome, RecentRawWindow, SummaryGenerationOutcome,
};
