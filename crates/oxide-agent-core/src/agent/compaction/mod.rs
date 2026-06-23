//! Agent Mode context compaction building blocks.
//!
//! The unified compaction architecture preserves the raw transcript in
//! `AgentMemory` and produces compacted model-facing context through a
//! renderer overlay driven by `CompactionState`. The `CompactionEngine` is
//! the sole mutation authority for compaction state. All triggers
//! (pre-sampling, context-limit, model-downshift, manual, agent compress)
//! go through `compact_via_engine` → `CompactionEngine::apply_compression`.

pub mod admission;
pub mod archive;
pub mod auto_select;
pub mod block;
pub mod budget;
pub mod controller;
pub mod engine;
pub mod local_llm_summary;
pub mod prompt;
pub mod refs;
pub mod renderer;
pub mod state;
pub mod strategy;
pub mod task;
pub mod types;

pub use admission::{
    AdmissionBlocker, AdmissionBudget, AdmissionDecision, ChunkSummaryResult, ContextAdmission,
    EmergencySummarizer, ManifestSpec, PayloadDescriptor, PayloadKind, SummarizeError,
    split_into_chunks, summarize_in_chunks,
};
pub use archive::ArchiveRef;
pub use block::{CompressionBlock, CompressionSelection, SummaryPart};
pub use budget::{count_tokens_cached, estimate_request_budget};
pub use controller::{
    CompactionController, CompactionControllerError, EngineCompactionOutcome,
    EngineCompactionResult, EngineCompactionSkipped,
};
pub use engine::{CompactionEngine, CompactionError};
pub use local_llm_summary::LocalLlmSummary;
pub use refs::{BlockRef, MessageRef};
pub use renderer::CompactionRenderer;
pub use state::CompactionState;
pub use strategy::RenderPolicy;
pub use task::{
    CompactSummaryBackend, CompactSummaryError, CompactSummaryRequest, CompactSummaryResult,
};
pub use types::{
    AgentMessageKind, BudgetEstimate, BudgetState, CompactionBackend, CompactionPhase,
    CompactionPolicy, CompactionReason, CompactionRequest, CompactionRetention, CompactionScope,
    CompactionTrigger, HotMemoryBudget, wiki_memory_lookup_available,
};
