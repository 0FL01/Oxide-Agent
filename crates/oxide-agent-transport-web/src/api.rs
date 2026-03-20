//! API types shared across the crate.
//!
//! Re-exports the key types from `oxide-agent-core` that are needed
//! for the HTTP layer to build correct types.

pub use async_trait::async_trait;
pub use oxide_agent_core::agent::progress::ProgressState;
pub use oxide_agent_core::agent::AgentMemory;
pub use oxide_agent_core::storage::StorageProvider;
