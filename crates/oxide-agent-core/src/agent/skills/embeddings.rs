//! Embedding generation and caching for skills.

use crate::agent::skills::types::SkillMetadata;
use crate::agent::skills::{SkillConfig, SkillError, SkillResult};
use crate::llm::{LlmClient, LlmError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::{info, warn};

#[derive(Debug, Serialize, Deserialize)]
struct EmbeddingCacheEntry {
    embedding: Vec<f32>,
}

/// Embedding service for skill descriptions.
pub struct EmbeddingService {
    llm_client: Arc<LlmClient>,
    cache_dir: PathBuf,
    dimension: Arc<Mutex<Option<usize>>>,
    in_memory: HashMap<String, Vec<f32>>,
}

impl EmbeddingService {
    /// Create a new embedding service from config.
    #[must_use]
    pub fn new(llm_client: Arc<LlmClient>, config: &SkillConfig) -> Self {
        if !llm_client.is_embedding_available() {
            warn!("Embeddings disabled, using keyword matching only");
        }

        Self {
            llm_client,
            cache_dir: config.embedding_cache_dir.clone(),
            dimension: Arc::new(Mutex::new(None)),
            in_memory: HashMap::new(),
        }
    }

    /// Clear in-memory embeddings cache.
    pub fn clear_cache(&mut self) {
        self.in_memory.clear();
    }

    /// Get or determine embedding dimension.
    async fn get_dimension(&self) -> SkillResult<usize> {
        {
            let dim = self
                .dimension
                .lock()
                .map_err(|e| SkillError::EmbeddingRequest(format!("Mutex poisoned: {}", e)))?;
            if let Some(d) = *dim {
                return Ok(d);
            }
        }

        let detected = self
            .llm_client
            .probe_embedding_dimension()
            .await
            .unwrap_or(1536);
        info!(dimension = detected, "Auto-detected embedding dimension");

        let mut dim = self
            .dimension
            .lock()
            .map_err(|e| SkillError::EmbeddingRequest(format!("Mutex poisoned: {}", e)))?;
        if dim.is_none() {
            *dim = Some(detected);
        }
        Ok(detected)
    }

    /// Compute semantic similarity scores for all skills.
    pub async fn semantic_scores(
        &mut self,
        user_message: &str,
        metadata: &mut [SkillMetadata],
    ) -> SkillResult<Option<HashMap<String, f32>>> {
        let user_embedding = match self.generate_embedding(user_message).await {
            Ok(embedding) => embedding,
            Err(err) => {
                warn!(error = %err, "Embedding unavailable, falling back to keyword matching");
                return Ok(None);
            }
        };

        let mut scores = HashMap::new();

        for meta in metadata.iter_mut() {
            let embedding = match self.embedding_for_skill(meta).await {
                Ok(Some(vec)) => vec,
                Ok(None) => {
                    warn!(skill = %meta.name, "Embedding not available for skill, skipping semantic match");
                    continue;
                }
                Err(err) => {
                    warn!(skill = %meta.name, error = %err, "Failed to load skill embedding");
                    continue;
                }
            };

            if let Some(similarity) = cosine_similarity(&user_embedding, &embedding) {
                scores.insert(meta.name.clone(), similarity);
            }
        }

        Ok(Some(scores))
    }

    async fn embedding_for_skill(
        &mut self,
        meta: &mut SkillMetadata,
    ) -> SkillResult<Option<Vec<f32>>> {
        if let Some(embedding) = meta.embedding.clone() {
            return Ok(Some(embedding));
        }

        if let Some(embedding) = self.load_cached_embedding(&meta.name)? {
            meta.embedding = Some(embedding.clone());
            return Ok(Some(embedding));
        }

        let embedding = self.generate_embedding(&meta.description).await?;
        self.save_cached_embedding(&meta.name, &embedding).await?;
        meta.embedding = Some(embedding.clone());
        Ok(Some(embedding))
    }

    async fn generate_embedding(&self, text: &str) -> SkillResult<Vec<f32>> {
        use std::time::Duration;
        use tokio::time::timeout;

        const EMBEDDING_TIMEOUT_SECS: u64 = 30;

        let embedding_future = self.llm_client.generate_embedding(text);

        let result = timeout(
            Duration::from_secs(EMBEDDING_TIMEOUT_SECS),
            embedding_future,
        )
        .await
        .map_err(|_| {
            SkillError::EmbeddingRequest(format!(
                "Embedding generation timeout after {EMBEDDING_TIMEOUT_SECS}s"
            ))
        })?
        .map_err(|err| match err {
            LlmError::MissingConfig(msg) => SkillError::EmbeddingUnavailable(msg),
            _ => SkillError::EmbeddingRequest(err.to_string()),
        })?;

        let expected_dim = self.get_dimension().await?;
        if result.len() != expected_dim {
            let mut dim = self
                .dimension
                .lock()
                .map_err(|e| SkillError::EmbeddingRequest(format!("Mutex poisoned: {}", e)))?;
            *dim = Some(result.len());
            info!(
                old_dimension = expected_dim,
                new_dimension = result.len(),
                "Updated embedding dimension based on API response"
            );
        }

        Ok(result)
    }

    fn cache_path(&self, skill_name: &str) -> PathBuf {
        let file_name = format!("{skill_name}.json");
        self.cache_dir.join(file_name)
    }

    async fn ensure_dimension_match(&self, embedding: &[f32]) -> SkillResult<()> {
        let expected_dim = self.get_dimension().await?;
        if embedding.len() != expected_dim {
            return Err(SkillError::EmbeddingDimensionMismatch {
                expected: expected_dim,
                actual: embedding.len(),
            });
        }
        Ok(())
    }

    fn load_cached_embedding(&mut self, skill_name: &str) -> SkillResult<Option<Vec<f32>>> {
        if let Some(embedding) = self.in_memory.get(skill_name) {
            return Ok(Some(embedding.clone()));
        }

        let path = self.cache_path(skill_name);
        if !path.exists() {
            return Ok(None);
        }

        let data = std::fs::read(&path).map_err(|err| {
            SkillError::EmbeddingCache(format!("failed to read {}: {err}", path.display()))
        })?;

        let entry: EmbeddingCacheEntry = serde_json::from_slice(&data).map_err(|err| {
            SkillError::EmbeddingCache(format!("failed to parse {}: {err}", path.display()))
        })?;

        let current_dim = self
            .dimension
            .lock()
            .map_err(|e| SkillError::EmbeddingRequest(format!("Mutex poisoned: {}", e)))?;
        if let Some(d) = *current_dim {
            if entry.embedding.len() != d {
                warn!(
                    skill = %skill_name,
                    cached_dim = entry.embedding.len(),
                    current_dim = d,
                    "Cached embedding dimension mismatch, ignoring cache"
                );
                return Ok(None);
            }
        }

        self.in_memory
            .insert(skill_name.to_string(), entry.embedding.clone());

        Ok(Some(entry.embedding))
    }

    async fn save_cached_embedding(
        &mut self,
        skill_name: &str,
        embedding: &[f32],
    ) -> SkillResult<()> {
        self.ensure_dimension_match(embedding).await?;
        std::fs::create_dir_all(&self.cache_dir).map_err(|err| {
            SkillError::EmbeddingCache(format!(
                "failed to create {}: {err}",
                self.cache_dir.display()
            ))
        })?;

        let path = self.cache_path(skill_name);
        let entry = EmbeddingCacheEntry {
            embedding: embedding.to_vec(),
        };
        let encoded = serde_json::to_vec(&entry)
            .map_err(|err| SkillError::EmbeddingCache(err.to_string()))?;
        std::fs::write(&path, encoded).map_err(|err| {
            SkillError::EmbeddingCache(format!("failed to write {}: {err}", path.display()))
        })?;

        self.in_memory
            .insert(skill_name.to_string(), embedding.to_vec());

        info!(skill = %skill_name, "Saved embedding to cache");
        Ok(())
    }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> Option<f32> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }

    let mut dot = 0.0;
    let mut norm_a = 0.0;
    let mut norm_b = 0.0;

    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }

    if norm_a == 0.0 || norm_b == 0.0 {
        return None;
    }

    Some(dot / (norm_a.sqrt() * norm_b.sqrt()))
}
