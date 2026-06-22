//! Agent Mode context compaction building blocks.
//!
//! The runtime path is `CompactionController` plus `LocalLlmSummary`, which
//! writes one current `OXIDE_COMPACTED_SUMMARY_V1` handoff message.
//!
//! Documentation: `docs/context-window-tracking.md`

pub mod archive;
pub mod block;
pub mod budget;
pub mod controller;
pub mod engine;
pub mod history;
pub mod local_llm_summary;
pub mod prompt;
pub mod refs;
pub mod renderer;
pub mod state;
pub mod task;
pub mod types;

pub use archive::ArchiveRef;
pub use block::{CompressionBlock, CompressionSelection, SummaryPart};
pub use budget::{count_tokens_cached, estimate_request_budget};
pub use controller::{
    CompactRequestContext, CompactRunOutcome, CompactionController, CompactionControllerError,
};
pub use engine::{CompactionEngine, CompactionError};
pub use history::{
    BuildCompactedHistoryRequest, CompactedHistoryBuildError, PreviousCompactedSummary,
    build_compacted_history, extract_previous_compacted_summary, is_any_compaction_summary_message,
    is_current_compacted_summary_message,
};
pub use local_llm_summary::LocalLlmSummary;
pub use refs::{BlockRef, MessageRef};
pub use renderer::CompactionRenderer;
pub use state::CompactionState;
pub use task::{
    CompactSummaryBackend, CompactSummaryError, CompactSummaryRequest, CompactSummaryResult,
};
pub use types::{
    AgentMessageKind, BudgetEstimate, BudgetState, CompactedSummaryMetadata, CompactionBackend,
    CompactionPhase, CompactionPolicy, CompactionReason, CompactionRequest, CompactionRetention,
    CompactionScope, CompactionTrigger, HotMemoryBudget, OXIDE_COMPACTED_SUMMARY_PREFIX,
    wiki_memory_lookup_available,
};
