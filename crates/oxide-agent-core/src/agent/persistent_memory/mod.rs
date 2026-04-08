use crate::agent::memory::AgentMessage;
use crate::agent::session::AgentMemoryScope;
use crate::config::ModelInfo;
use crate::llm::{EmbeddingTaskType, LlmClient, LlmError};
use crate::storage::{StorageMemoryRepository, StorageProvider};
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use lazy_regex::lazy_regex;
use oxide_agent_memory::{
    stable_memory_content_hash, ArtifactRef, ConsolidationPolicy, ContextConsolidator,
    EmbeddingBackfillRequest, EmbeddingFailureUpdate, EmbeddingOwnerType, EmbeddingPendingUpdate,
    EmbeddingReadyUpdate, EmbeddingRecord, EmbeddingUpdateBase, EpisodeEmbeddingCandidate,
    EpisodeFinalizationInput, EpisodeFinalizer, EpisodeListFilter, EpisodeOutcome, EpisodeRecord,
    EpisodeSearchFilter, EpisodeSearchHit, MemoryEmbeddingCandidate, MemoryListFilter,
    MemoryRecord, MemoryRepository, MemorySearchFilter, MemorySearchHit, MemoryType,
    RepositoryError, SessionStateListFilter, SessionStateRecord, ThreadRecord, TimeRange,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::{sleep, timeout};
use tracing::{info, warn};

mod behavior;
mod coordinator;
mod embeddings;
mod post_run;
mod retrieval;
mod store;

#[cfg(test)]
mod tests;

pub use behavior::{MemoryBehaviorRuntime, ToolDerivedMemoryDraft, TopicMemoryPolicy};
pub use coordinator::PersistentMemoryCoordinator;
pub use embeddings::{
    LlmMemoryEmbeddingGenerator, MemoryEmbeddingGenerator, PersistentMemoryEmbeddingIndexer,
};
pub use retrieval::{DurableMemoryRetrievalOptions, DurableMemoryRetriever};
pub use store::{connect_postgres_memory_store, PersistentMemoryStore};

pub(crate) use post_run::{
    LlmPostRunMemoryWriter, PersistentRunContext, PersistentRunPhase, PostRunMemoryWriter,
    PostRunMemoryWriterConfig, PostRunMemoryWriterInput,
};
pub(crate) use retrieval::{
    DurableMemoryRetrievalDiagnostics, DurableMemorySearchItem, DurableMemorySearchOutcome,
    DurableMemorySearchRequest,
};

#[cfg(test)]
pub(crate) use post_run::{ValidatedPostRunEpisode, ValidatedPostRunMemoryWrite};

#[cfg(test)]
pub(crate) use retrieval::HybridCandidate;

fn outcome_label(outcome: EpisodeOutcome) -> &'static str {
    match outcome {
        EpisodeOutcome::Success => "success",
        EpisodeOutcome::Failure => "failure",
        EpisodeOutcome::Partial => "partial",
        EpisodeOutcome::Cancelled => "cancelled",
    }
}

fn memory_type_label(memory_type: MemoryType) -> &'static str {
    match memory_type {
        MemoryType::Fact => "fact",
        MemoryType::Preference => "preference",
        MemoryType::Procedure => "procedure",
        MemoryType::Decision => "decision",
        MemoryType::Constraint => "constraint",
    }
}
