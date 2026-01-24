//! Core skill types.

use crate::agent::skills::{SkillError, SkillResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tiktoken_rs::cl100k_base;

/// Skill load priority and selection weight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SkillWeight {
    /// Always load this skill.
    Always,
    /// High-priority skill.
    High,
    /// Medium-priority skill.
    #[default]
    Medium,
    /// Load on demand when matched.
    OnDemand,
}

impl SkillWeight {
    /// Priority for sorting (higher is more important).
    #[must_use]
    pub const fn priority(self) -> u8 {
        match self {
            SkillWeight::Always => 3,
            SkillWeight::High => 2,
            SkillWeight::Medium => 1,
            SkillWeight::OnDemand => 0,
        }
    }
}

/// Activation mode for selecting skills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ActivationMode {
    /// Hybrid matching (keywords + semantic).
    #[default]
    Hybrid,
    /// Tool-only activation.
    ToolOnly,
}

/// Metadata parsed from frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMetadata {
    /// Unique skill identifier.
    pub name: String,
    /// Description used for semantic matching.
    pub description: String,
    #[serde(default)]
    /// Keyword triggers for fast matching.
    pub triggers: Vec<String>,
    #[serde(default)]
    /// Tools associated with this skill.
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    /// Selection weight for matching.
    pub weight: SkillWeight,
    #[serde(default)]
    /// Supporting file references for progressive disclosure.
    pub references: Vec<PathBuf>,
    #[serde(default)]
    /// Activation mode for matching.
    pub activation: ActivationMode,
    #[serde(skip)]
    /// Cached embedding vector (optional).
    pub embedding: Option<Vec<f32>>,
}

/// Lazily loaded supporting content for progressive disclosure.
#[derive(Debug, Clone)]
pub struct LazyContent {
    path: PathBuf,
    content: Option<String>,
}

impl LazyContent {
    /// Create a lazy loader for the given path.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            content: None,
        }
    }

    /// Load the referenced content on demand.
    pub fn load(&mut self) -> SkillResult<&str> {
        if self.content.is_none() {
            let content = std::fs::read_to_string(&self.path).map_err(|source| {
                SkillError::ReadSkillFile {
                    path: self.path.clone(),
                    source,
                }
            })?;
            self.content = Some(content);
        }

        self.content
            .as_deref()
            .ok_or_else(|| SkillError::ReadSkillFile {
                path: self.path.clone(),
                source: std::io::Error::other("Lazy content missing after load"),
            })
    }
}

/// Fully loaded skill content.
#[derive(Debug, Clone)]
pub struct Skill {
    /// Parsed metadata from frontmatter.
    pub metadata: SkillMetadata,
    /// Markdown content body.
    pub content: String,
    /// Supporting files loaded on demand.
    pub supporting_files: HashMap<PathBuf, LazyContent>,
    /// Timestamp when the skill was loaded.
    pub loaded_at: DateTime<Utc>,
    /// Token count of the content.
    pub token_count: usize,
}

/// Context about a loaded skill (for logging/debugging).
#[derive(Debug, Clone)]
pub struct SkillContext {
    /// Skill name.
    pub name: String,
    /// Skill weight.
    pub weight: SkillWeight,
    /// Whether a keyword trigger matched.
    pub trigger_match: bool,
    /// Semantic similarity score, if available.
    pub semantic_score: Option<f32>,
    /// Token count for the skill content.
    pub token_count: usize,
}

/// Count tokens in a string using cl100k tokenizer (GPT-4/Claude compatible).
#[must_use]
pub fn count_tokens(text: &str) -> usize {
    cl100k_base().map_or(text.len() / 4, |bpe| {
        bpe.encode_with_special_tokens(text).len()
    })
}
