//! oxide-agent-memory
//!
//! Typed long-term memory model, repository abstractions, and retrieval
//! primitives for persistent agent memory (hybrid RAG).

pub mod archive;
pub mod in_memory;
pub mod repository;
pub mod types;

pub use archive::ArchiveBlobStore;
pub use in_memory::{InMemoryArchiveBlobStore, InMemoryMemoryRepository};
pub use repository::{MemoryRepository, RepositoryError};
pub use types::*;
