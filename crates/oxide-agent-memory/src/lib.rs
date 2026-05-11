//! oxide-agent-memory
//!
//! Typed long-term memory model, repository abstractions, and retrieval
//! primitives for persistent agent memory (hybrid RAG).

pub mod archive;
pub mod consolidation;
pub mod extract;
pub mod finalize;
pub mod in_memory;
pub mod pg;
pub mod repository;
pub mod types;

pub use archive::ArchiveBlobStore;
pub use consolidation::{
    stable_memory_content_hash, ConsolidatedContext, ConsolidationPolicy, ContextConsolidator,
};
pub use extract::{EpisodeMemorySignals, ReusableMemoryExtractor};
pub use finalize::{EpisodeFinalizationInput, EpisodeFinalizationPlan, EpisodeFinalizer};
pub use in_memory::{InMemoryArchiveBlobStore, InMemoryMemoryRepository};
pub use repository::{MemoryRepository, RepositoryError};
pub use types::*;
