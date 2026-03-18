//! Agent Mode context compaction building blocks.

pub mod archive;
pub mod budget;
pub mod classifier;
pub mod externalize;
pub mod service;
pub mod types;

pub use archive::{ArchiveRecord, ArchiveRef, ArchiveSink, NoopArchiveSink};
pub use budget::estimate_request_budget;
pub use classifier::classify_hot_memory;
pub use externalize::{
    externalize_hot_memory, ExternalizedPayloadRecord, NoopPayloadSink, PayloadSink,
};
pub use service::CompactionService;
pub use types::{
    AgentMessageKind, BudgetEstimate, BudgetState, ClassifiedMemoryEntry, CompactionClassSummary,
    CompactionOutcome, CompactionPolicy, CompactionRequest, CompactionRetention, CompactionScope,
    CompactionSnapshot, CompactionTrigger, ExternalizationOutcome, HotMemoryBudget,
    RecentRawWindow,
};
