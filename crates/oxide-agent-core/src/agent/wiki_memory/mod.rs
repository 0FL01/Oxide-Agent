//! Durable LLM Wiki memory primitives.
//!
//! This module is intentionally separate from `agent::memory`, which remains
//! hot/session context for active agent runs.

/// Per-run read-through cache for deterministic wiki pages.
pub mod cache;
/// Configuration and size limits for durable wiki memory.
pub mod config;
/// Bounded prompt context assembly from cached wiki pages.
pub mod context;
/// Patch model and validation for constrained wiki writes.
pub mod patch;
/// Conservative post-run patch planning for wiki update candidates.
pub mod planner;
/// Scope and slug helpers for deterministic wiki namespaces.
pub mod scope;
/// Bounded signal buffer for post-run wiki update planning.
pub mod signals;
/// Text-object store for deterministic wiki page reads and writes.
pub mod store;

pub use cache::{CachedWikiPage, WikiCacheMetrics, WikiFlushResult, WikiSessionCache};
pub use config::WikiMemoryConfig;
pub use context::{WikiContextAssembler, WikiContextAssemblerConfig, WikiRenderedContext};
pub use patch::{
    ValidatedWikiPatch, ValidatedWikiPatchOperation, WikiPatchOperation, WikiPatchSet,
    WikiPatchValidator, WikiPatchValidatorConfig,
};
pub use planner::{WikiPatchPlanner, WikiPatchPlannerConfig};
pub use scope::{wiki_context_id, wiki_slug};
pub use signals::{WikiSignal, WikiSignalBuffer, WikiSignalBufferConfig, WikiSignalKind};
pub use store::{WikiObjectBackend, WikiPage, WikiStore, wiki_content_hash};
