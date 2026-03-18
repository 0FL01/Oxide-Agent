//! Agent Mode context compaction foundation.
//!
//! Stage 1 introduces a dedicated module boundary for compaction-related
//! orchestration without changing the external Agent Mode behavior yet.

pub mod archive;
pub mod service;
pub mod types;

pub use archive::{ArchiveRecord, ArchiveRef, ArchiveSink, NoopArchiveSink};
pub use service::CompactionService;
pub use types::{
    AgentMessageKind, CompactionOutcome, CompactionPolicy, CompactionRequest, CompactionRetention,
    CompactionTrigger,
};
