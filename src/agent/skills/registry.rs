//! Skill registry and selection.

use crate::agent::skills::embeddings::EmbeddingService;
use crate::agent::skills::loader::SkillLoader;
use crate::agent::skills::matcher::{SkillMatch, SkillMatcher, SkillMatcherInput};
use crate::agent::skills::types::{Skill, SkillContext, SkillWeight};
use crate::agent::skills::{SkillCache, SkillConfig, SkillError, SkillResult};
use crate::llm::LlmClient;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, warn};

/// Resulting prompt composed from selected skills.
#[derive(Debug, Clone)]
pub struct SkillPrompt {
    /// Combined prompt content for selected skills.
    pub content: String,
    /// Selected skills with context metadata.
    pub skills: Vec<SkillContext>,
    /// Token count of the combined prompt.
    pub token_count: usize,
    /// Skills skipped due to token budget.
    pub skipped: Vec<String>,
}

/// Central registry for skill metadata and content.
pub struct SkillRegistry {
    config: SkillConfig,
    loader: SkillLoader,
    matcher: SkillMatcher,
    embeddings: EmbeddingService,
    cache: SkillCache,
    metadata: Vec<crate::agent::skills::types::SkillMetadata>,
    last_loaded: Instant,
}

impl SkillRegistry {
    /// Initialize the registry if the skills directory exists.
    pub fn from_env(llm_client: Arc<LlmClient>) -> SkillResult<Option<Self>> {
        let config = SkillConfig::from_env();
        let skills_dir = config.skills_dir.clone();

        match std::fs::metadata(&skills_dir) {
            Ok(meta) => {
                if !meta.is_dir() {
                    return Err(SkillError::SkillsDirInvalid(
                        skills_dir.display().to_string(),
                    ));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                warn!(
                    skills_dir = %skills_dir.display(),
                    "Skills directory not found, skill system will be inactive"
                );
                return Ok(None);
            }
            Err(err) => {
                return Err(SkillError::SkillsDirMissing(format!(
                    "{}: {err}",
                    skills_dir.display()
                )));
            }
        }

        let loader = SkillLoader::new(skills_dir.clone());
        let metadata = loader.load_all_metadata()?;
        if metadata.is_empty() {
            warn!(
                skills_dir = %skills_dir.display(),
                "Skills directory exists but contains no valid skill definitions"
            );
            return Ok(None);
        }

        info!(
            skills_count = metadata.len(),
            skills_dir = %skills_dir.display(),
            skills = ?metadata.iter().map(|m| &m.name).collect::<Vec<_>>(),
            "Skill registry initialized"
        );

        let matcher = SkillMatcher::new(config.semantic_threshold, config.max_selected);
        let embeddings = EmbeddingService::new(llm_client, &config);
        let cache = SkillCache::new(config.max_loaded_skills);

        Ok(Some(Self {
            config,
            loader,
            matcher,
            embeddings,
            cache,
            metadata,
            last_loaded: Instant::now(),
        }))
    }

    /// Build a system prompt for the given user message.
    pub async fn build_prompt(&mut self, user_message: &str) -> SkillResult<SkillPrompt> {
        self.refresh_if_stale()?;

        let semantic_scores = self
            .embeddings
            .semantic_scores(user_message, &mut self.metadata)
            .await?;

        let matches = self.matcher.select_skills(SkillMatcherInput {
            user_message,
            metadata: &self.metadata,
            semantic_scores: semantic_scores.as_ref(),
            embeddings_available: semantic_scores.is_some(),
        });

        let mut prompt_parts = Vec::new();
        let mut contexts = Vec::new();
        let mut total_tokens = 0usize;
        let mut skipped = Vec::new();

        for skill_match in matches {
            let skill = match self.get_or_load_skill(&skill_match.name) {
                Ok(skill) => skill,
                Err(err) => {
                    warn!(skill = %skill_match.name, error = %err, "Failed to load skill");
                    continue;
                }
            };

            let is_always = skill_match.weight == SkillWeight::Always;
            if !is_always
                && total_tokens.saturating_add(skill.token_count) > self.config.token_budget
            {
                skipped.push(skill.metadata.name.clone());
                continue;
            }

            total_tokens = total_tokens.saturating_add(skill.token_count);
            prompt_parts.push(skill.content.clone());
            contexts.push(skill_context(&skill, &skill_match));
        }

        let content = prompt_parts.join("\n\n");

        if content.is_empty() {
            warn!("Skill prompt is empty, fallback may be required");
        }

        Ok(SkillPrompt {
            content,
            skills: contexts,
            token_count: total_tokens,
            skipped,
        })
    }

    /// Load a skill by tool name for dynamic injection.
    pub async fn load_skill_for_tool(
        &mut self,
        tool_name: &str,
    ) -> SkillResult<Option<Arc<Skill>>> {
        self.refresh_if_stale()?;

        let skill_name = self
            .metadata
            .iter()
            .find(|meta| meta.allowed_tools.iter().any(|tool| tool == tool_name))
            .map(|meta| meta.name.clone());

        let Some(skill_name) = skill_name else {
            return Ok(None);
        };

        let skill = self.get_or_load_skill(&skill_name)?;
        Ok(Some(skill))
    }

    #[must_use]
    /// Get the configured skills directory.
    pub fn skills_dir(&self) -> &Path {
        self.config.skills_dir.as_path()
    }

    fn refresh_if_stale(&mut self) -> SkillResult<()> {
        if self.last_loaded.elapsed() < self.config.cache_ttl {
            return Ok(());
        }

        info!("Refreshing skill metadata (cache TTL expired)");
        self.metadata = self.loader.load_all_metadata()?;
        self.cache.clear();
        self.embeddings.clear_cache();
        self.last_loaded = Instant::now();

        Ok(())
    }

    fn get_or_load_skill(&mut self, name: &str) -> SkillResult<Arc<Skill>> {
        if let Some(skill) = self.cache.get(name) {
            return Ok(skill);
        }

        let skill = self.loader.load_skill_content(name)?;
        Ok(self.cache.insert(skill))
    }
}

fn skill_context(skill: &Skill, skill_match: &SkillMatch) -> SkillContext {
    SkillContext {
        name: skill.metadata.name.clone(),
        weight: skill_match.weight,
        trigger_match: skill_match.trigger_match,
        semantic_score: skill_match.semantic_score,
        token_count: skill.token_count,
    }
}
