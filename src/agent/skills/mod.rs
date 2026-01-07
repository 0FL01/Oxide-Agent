//! Skill system for modular agent context.
//!
//! Loads skill definitions from markdown files with YAML frontmatter
//! and selects relevant modules per user request.

pub mod cache;
pub mod embeddings;
pub mod loader;
pub mod matcher;
pub mod registry;
pub mod types;

pub use cache::SkillCache;
pub use embeddings::EmbeddingService;
pub use loader::SkillLoader;
pub use matcher::{SkillMatch, SkillMatcher, SkillMatcherInput};
pub use registry::{SkillPrompt, SkillRegistry};
pub use types::{ActivationMode, LazyContent, Skill, SkillContext, SkillMetadata, SkillWeight};

use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;

/// Errors produced by the skills subsystem.
#[derive(Debug, Error)]
pub enum SkillError {
    /// Skills directory is missing or invalid.
    #[error("Skills directory missing: {0}")]
    SkillsDirMissing(String),
    /// Skills directory path is not a directory.
    #[error("Skills path is not a directory: {0}")]
    SkillsDirInvalid(String),
    /// Failed to read a skill file.
    #[error("Failed to read skill file {path}: {source}")]
    ReadSkillFile {
        /// Path to the skill file.
        path: PathBuf,
        /// Underlying IO error.
        source: std::io::Error,
    },
    /// Frontmatter is missing or malformed.
    #[error("Missing or invalid frontmatter in {path}")]
    MissingFrontmatter {
        /// Skill file path.
        path: PathBuf,
    },
    /// YAML frontmatter parsing failed.
    #[error("Invalid YAML frontmatter in {path}: {source}")]
    FrontmatterYaml {
        /// Skill file path.
        path: PathBuf,
        /// YAML parsing error.
        source: serde_yaml::Error,
    },
    /// Skill definition is missing a name.
    #[error("Skill name missing in {path}")]
    MissingSkillName {
        /// Skill file path.
        path: PathBuf,
    },
    /// A requested skill could not be found.
    #[error("Skill not found: {0}")]
    SkillNotFound(String),
    /// Failed to read or write the embedding cache.
    #[error("Embedding cache error: {0}")]
    EmbeddingCache(String),
    /// Embedding dimension mismatch.
    #[error("Embedding dimension mismatch: expected {expected}, got {actual}")]
    EmbeddingDimensionMismatch {
        /// Expected embedding size.
        expected: usize,
        /// Actual embedding size.
        actual: usize,
    },
    /// Embedding generation is unavailable (missing config).
    #[error("Embedding generation unavailable: {0}")]
    EmbeddingUnavailable(String),
    /// Embedding request failed.
    #[error("Embedding request failed: {0}")]
    EmbeddingRequest(String),
}

/// Result type for skills operations.
pub type SkillResult<T> = Result<T, SkillError>;

/// Runtime configuration for the skills subsystem.
#[derive(Debug, Clone)]
pub struct SkillConfig {
    /// Directory containing skill markdown files.
    pub skills_dir: PathBuf,
    /// Directory used for embedding cache files.
    pub embedding_cache_dir: PathBuf,
    /// Maximum token budget for selected skills.
    pub token_budget: usize,
    /// Similarity threshold for semantic matching.
    pub semantic_threshold: f32,
    /// Maximum number of non-core skills to select.
    pub max_selected: usize,
    /// Time-to-live for metadata cache.
    pub cache_ttl: Duration,
    /// Embedding model name.
    pub embedding_model: String,
    /// Embedding vector dimension.
    pub embedding_dimension: usize,
    /// Maximum number of loaded skills cached in memory.
    pub max_loaded_skills: usize,
}

impl SkillConfig {
    /// Build configuration from environment variables and defaults.
    #[must_use]
    pub fn from_env() -> Self {
        let token_budget = crate::config::get_skill_token_budget();
        let max_selected = crate::config::get_skill_max_selected();
        let cache_ttl = Duration::from_secs(crate::config::get_skill_cache_ttl_secs());

        let max_loaded_skills = max_selected.saturating_add(5);

        Self {
            skills_dir: PathBuf::from(crate::config::get_skills_dir()),
            embedding_cache_dir: PathBuf::from(crate::config::get_embedding_cache_dir()),
            token_budget,
            semantic_threshold: crate::config::get_skill_semantic_threshold(),
            max_selected,
            cache_ttl,
            embedding_model: crate::config::get_mistral_embed_model(),
            embedding_dimension: crate::config::EMBEDDING_DIMENSION,
            max_loaded_skills,
        }
    }
}
