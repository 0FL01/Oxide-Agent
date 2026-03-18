//! Agent Mode context compaction building blocks.

pub mod archive;
pub mod budget;
pub mod classifier;
pub mod service;
pub mod types;

pub use archive::{ArchiveRecord, ArchiveRef, ArchiveSink, NoopArchiveSink};
pub use budget::estimate_request_budget;
pub use classifier::classify_hot_memory;
pub use service::CompactionService;
pub use types::{
    AgentMessageKind, BudgetEstimate, BudgetState, ClassifiedMemoryEntry, CompactionClassSummary,
    CompactionOutcome, CompactionPolicy, CompactionRequest, CompactionRetention,
    CompactionSnapshot, CompactionTrigger, HotMemoryBudget, RecentRawWindow,
};
