//! Scope-aware persistent-memory read tools.

use crate::agent::persistent_memory::{
    DurableMemoryRetrievalDiagnostics, DurableMemoryRetriever, DurableMemorySearchItem,
    DurableMemorySearchOutcome, DurableMemorySearchRequest, MemoryEmbeddingGenerator,
    PersistentMemoryStore,
};
use crate::agent::provider::ToolProvider;
use crate::agent::session::AgentMemoryScope;
use crate::llm::ToolDefinition;
use crate::storage::{StorageMemoryRepository, StorageProvider};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use oxide_agent_memory::{
    EmbeddingOwnerType, EpisodeListFilter, MemoryListFilter, MemoryType, TimeRange,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::info;
use uuid::Uuid;

const TOOL_MEMORY_SEARCH: &str = "memory_search";
const TOOL_MEMORY_READ_EPISODE: &str = "memory_read_episode";
const TOOL_MEMORY_READ_THREAD_SUMMARY: &str = "memory_read_thread_summary";
const TOOL_MEMORY_READ_THREAD_WINDOW: &str = "memory_read_thread_window";
const TOOL_MEMORY_DIAGNOSTICS: &str = "memory_diagnostics";
const DEFAULT_SEARCH_LIMIT: usize = 8;
const MAX_SEARCH_LIMIT: usize = 20;
const DEFAULT_THREAD_EPISODE_LIMIT: usize = 6;
const DEFAULT_WINDOW_LIMIT: usize = 20;
const MAX_WINDOW_LIMIT: usize = 50;
const DEFAULT_DIAGNOSTICS_LIMIT: usize = 12;
const MAX_DIAGNOSTICS_LIMIT: usize = 50;
const ARCHIVE_MESSAGE_MAX_CHARS: usize = 500;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum SearchSourceType {
    Episode,
    Memory,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum SearchMemoryTypeArg {
    Fact,
    Preference,
    Procedure,
    Decision,
    Constraint,
}

impl From<SearchMemoryTypeArg> for MemoryType {
    fn from(value: SearchMemoryTypeArg) -> Self {
        match value {
            SearchMemoryTypeArg::Fact => MemoryType::Fact,
            SearchMemoryTypeArg::Preference => MemoryType::Preference,
            SearchMemoryTypeArg::Procedure => MemoryType::Procedure,
            SearchMemoryTypeArg::Decision => MemoryType::Decision,
            SearchMemoryTypeArg::Constraint => MemoryType::Constraint,
        }
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct TimeRangeArgs {
    since: Option<String>,
    until: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemorySearchArgs {
    query: String,
    #[serde(default)]
    types: Vec<SearchSourceType>,
    context_key: Option<String>,
    memory_type: Option<SearchMemoryTypeArg>,
    time_range: Option<TimeRangeArgs>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryReadEpisodeArgs {
    episode_id: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct MemoryReadThreadSummaryArgs {
    thread_id: Option<String>,
    episode_limit: Option<usize>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct MemoryReadThreadWindowArgs {
    thread_id: Option<String>,
    offset: Option<usize>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct MemoryDiagnosticsArgs {
    context_key: Option<String>,
    thread_id: Option<String>,
    limit: Option<usize>,
}

/// Tool names exposed by the persistent-memory read provider.
pub fn memory_tool_names() -> Vec<String> {
    vec![
        TOOL_MEMORY_SEARCH.to_string(),
        TOOL_MEMORY_READ_EPISODE.to_string(),
        TOOL_MEMORY_READ_THREAD_SUMMARY.to_string(),
        TOOL_MEMORY_READ_THREAD_WINDOW.to_string(),
        TOOL_MEMORY_DIAGNOSTICS.to_string(),
    ]
}

/// Provider for scope-aware persistent-memory read tools.
pub struct MemoryProvider {
    store: Arc<dyn PersistentMemoryStore>,
    artifact_storage: Option<Arc<dyn StorageProvider>>,
    scope: AgentMemoryScope,
    query_embedding_generator: Option<Arc<dyn MemoryEmbeddingGenerator>>,
    query_embedding_model_id: Option<String>,
}

impl MemoryProvider {
    /// Create a provider bound to the current persistent-memory scope.
    #[must_use]
    pub fn new(storage: Arc<dyn StorageProvider>, scope: AgentMemoryScope) -> Self {
        let store: Arc<dyn PersistentMemoryStore> =
            Arc::new(StorageMemoryRepository::new(Arc::clone(&storage)));
        Self::new_with_store(store, Some(storage), scope)
    }

    /// Create a provider from an explicit typed-memory backend.
    #[must_use]
    pub fn new_with_store(
        store: Arc<dyn PersistentMemoryStore>,
        artifact_storage: Option<Arc<dyn StorageProvider>>,
        scope: AgentMemoryScope,
    ) -> Self {
        Self {
            store,
            artifact_storage,
            scope,
            query_embedding_generator: None,
            query_embedding_model_id: None,
        }
    }

    /// Attach an optional embedding generator so `memory_search` can run hybrid retrieval.
    #[must_use]
    pub fn with_query_embedding_generator(
        mut self,
        generator: Arc<dyn MemoryEmbeddingGenerator>,
    ) -> Self {
        self.query_embedding_generator = Some(generator);
        self
    }

    /// Attach the active embedding profile identifier for vector-search isolation.
    #[must_use]
    pub fn with_query_embedding_model_id(mut self, model_id: impl Into<String>) -> Self {
        self.query_embedding_model_id = Some(model_id.into());
        self
    }

    fn durable_memory_retriever(&self) -> DurableMemoryRetriever {
        let mut retriever = DurableMemoryRetriever::new_with_store(Arc::clone(&self.store));
        if let Some(generator) = self.query_embedding_generator.as_ref() {
            retriever = retriever.with_query_embedding_generator(Arc::clone(generator));
        }
        if let Some(model_id) = self.query_embedding_model_id.as_ref() {
            retriever = retriever.with_query_embedding_model_id(model_id.clone());
        }
        retriever
    }

    fn tools_definitions() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: TOOL_MEMORY_SEARCH.to_string(),
                description: "Search scoped durable memories and episodes for relevant prior work"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Search query"},
                        "types": {
                            "type": "array",
                            "items": {"type": "string", "enum": ["episode", "memory"]},
                            "description": "Restrict result kinds; defaults to both"
                        },
                        "context_key": {
                            "type": "string",
                            "description": "Optional explicit context; must match the current context"
                        },
                        "memory_type": {
                            "type": "string",
                            "enum": ["fact", "preference", "procedure", "decision", "constraint"],
                            "description": "Restrict reusable memory kind"
                        },
                        "time_range": {
                            "type": "object",
                            "properties": {
                                "since": {"type": "string", "description": "Inclusive RFC3339 lower bound"},
                                "until": {"type": "string", "description": "Inclusive RFC3339 upper bound"}
                            },
                            "additionalProperties": false
                        },
                        "limit": {"type": "integer", "minimum": 1, "maximum": 20}
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }),
            },
            ToolDefinition {
                name: TOOL_MEMORY_READ_EPISODE.to_string(),
                description: "Read one scoped durable episode record including archived artifact refs"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "episode_id": {"type": "string", "description": "Episode identifier to read"}
                    },
                    "required": ["episode_id"],
                    "additionalProperties": false
                }),
            },
            ToolDefinition {
                name: TOOL_MEMORY_READ_THREAD_SUMMARY.to_string(),
                description: "Read the scoped durable thread summary with recent episodes"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "thread_id": {"type": "string", "description": "Optional explicit thread id; defaults to current scope thread"},
                        "episode_limit": {"type": "integer", "minimum": 1, "maximum": 20}
                    },
                    "additionalProperties": false
                }),
            },
            ToolDefinition {
                name: TOOL_MEMORY_READ_THREAD_WINDOW.to_string(),
                description: "Best-effort read of archived compacted history messages for the scoped durable thread"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "thread_id": {"type": "string", "description": "Optional explicit thread id; defaults to current scope thread"},
                        "offset": {"type": "integer", "minimum": 0},
                        "limit": {"type": "integer", "minimum": 1, "maximum": 50}
                    },
                    "additionalProperties": false
                }),
            },
            ToolDefinition {
                name: TOOL_MEMORY_DIAGNOSTICS.to_string(),
                description: "Inspect scoped durable-memory records, deletions, and embedding health"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "context_key": {
                            "type": "string",
                            "description": "Optional explicit context; must match the current context"
                        },
                        "thread_id": {
                            "type": "string",
                            "description": "Optional explicit visible thread id for episode diagnostics"
                        },
                        "limit": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": MAX_DIAGNOSTICS_LIMIT,
                            "description": "How many recent memories and episodes to inspect"
                        }
                    },
                    "additionalProperties": false
                }),
            },
        ]
    }

    fn parse_args<T: for<'de> Deserialize<'de>>(arguments: &str, tool_name: &str) -> Result<T> {
        serde_json::from_str(arguments)
            .map_err(|error| anyhow!("invalid arguments for {tool_name}: {error}"))
    }

    fn resolve_context_key(&self, requested: Option<&str>) -> Result<String> {
        match requested {
            Some(context_key) if context_key != self.scope.context_key => Err(anyhow!(
                "memory tools are context-scoped; requested context_key '{}' does not match current context '{}'",
                context_key,
                self.scope.context_key
            )),
            Some(context_key) => Ok(context_key.to_string()),
            None => Ok(self.scope.context_key.clone()),
        }
    }

    fn scoped_thread_id(&self) -> String {
        scoped_thread_id(&self.scope)
    }

    async fn execute_search(&self, arguments: &str) -> Result<String> {
        let args: MemorySearchArgs = Self::parse_args(arguments, TOOL_MEMORY_SEARCH)?;
        let limit = normalize_limit(args.limit, DEFAULT_SEARCH_LIMIT, MAX_SEARCH_LIMIT);
        let context_key = self.resolve_context_key(args.context_key.as_deref())?;
        let time_range = parse_time_range(args.time_range)?;
        let include_episodes =
            args.types.is_empty() || args.types.contains(&SearchSourceType::Episode);
        let include_memories =
            args.types.is_empty() || args.types.contains(&SearchSourceType::Memory);

        let outcome = self
            .durable_memory_retriever()
            .search_with_diagnostics(
                &AgentMemoryScope::new(self.scope.user_id, &context_key, &self.scope.flow_id),
                DurableMemorySearchRequest {
                    query: args.query.clone(),
                    search_episodes: include_episodes,
                    search_memories: include_memories,
                    memory_type: args.memory_type.map(Into::into),
                    time_range,
                    min_importance: None,
                    limit,
                    candidate_limit: Some(limit.max(DEFAULT_SEARCH_LIMIT)),
                    allow_full_thread_read: true,
                },
            )
            .await
            .map_err(|error| anyhow!("failed to search durable memory: {error}"))?;
        let diagnostics = retrieval_diagnostics(&outcome);
        let results = outcome
            .items
            .into_iter()
            .map(search_result)
            .collect::<Vec<_>>();

        Ok(json!({
            "ok": true,
            "query": args.query,
            "context_key": context_key,
            "result_count": results.len(),
            "diagnostics": diagnostics,
            "results": results,
        })
        .to_string())
    }

    async fn execute_diagnostics(&self, arguments: &str) -> Result<String> {
        let args: MemoryDiagnosticsArgs = Self::parse_args(arguments, TOOL_MEMORY_DIAGNOSTICS)?;
        let limit = normalize_limit(args.limit, DEFAULT_DIAGNOSTICS_LIMIT, MAX_DIAGNOSTICS_LIMIT);
        let context_key = self.resolve_context_key(args.context_key.as_deref())?;
        let diagnostics_scope =
            AgentMemoryScope::new(self.scope.user_id, &context_key, &self.scope.flow_id);
        let thread_id = args
            .thread_id
            .unwrap_or_else(|| scoped_thread_id(&diagnostics_scope));

        let mut memories = self
            .store
            .list_memories(
                &context_key,
                &MemoryListFilter {
                    include_deleted: true,
                    limit: Some(limit),
                    ..MemoryListFilter::default()
                },
            )
            .await
            .map_err(|error| anyhow!("failed to list memory diagnostics records: {error}"))?;
        memories.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.memory_id.cmp(&right.memory_id))
        });

        let mut embedding_counts = EmbeddingDiagnosticsCounts::default();
        let mut recent_memories = Vec::with_capacity(memories.len());
        for memory in memories {
            let embedding = self
                .store
                .get_embedding(EmbeddingOwnerType::Memory, &memory.memory_id)
                .await
                .map_err(|error| {
                    anyhow!(
                        "failed to inspect memory embedding {}: {error}",
                        memory.memory_id
                    )
                })?;
            embedding_counts.observe(embedding.as_ref());
            recent_memories.push(json!({
                "memory": memory,
                "embedding": embedding,
                "embedding_status": embedding_status_label(embedding.as_ref()),
            }));
        }

        let thread =
            self.store.get_thread(&thread_id).await.map_err(|error| {
                anyhow!("failed to read diagnostics thread {thread_id}: {error}")
            })?;
        let visible_thread = thread
            .as_ref()
            .filter(|thread| thread_is_visible(thread, &diagnostics_scope));

        let mut recent_episodes = Vec::new();
        if let Some(thread) = visible_thread {
            let mut episodes = self
                .store
                .list_episodes_for_thread(
                    &thread.thread_id,
                    &EpisodeListFilter {
                        limit: Some(limit),
                        ..EpisodeListFilter::default()
                    },
                )
                .await
                .map_err(|error| {
                    anyhow!("failed to list thread episodes for diagnostics: {error}")
                })?;
            episodes.sort_by(|left, right| {
                right
                    .created_at
                    .cmp(&left.created_at)
                    .then_with(|| left.episode_id.cmp(&right.episode_id))
            });
            for episode in episodes {
                let embedding = self
                    .store
                    .get_embedding(EmbeddingOwnerType::Episode, &episode.episode_id)
                    .await
                    .map_err(|error| {
                        anyhow!(
                            "failed to inspect episode embedding {}: {error}",
                            episode.episode_id
                        )
                    })?;
                embedding_counts.observe(embedding.as_ref());
                recent_episodes.push(json!({
                    "episode": episode,
                    "embedding": embedding,
                    "embedding_status": embedding_status_label(embedding.as_ref()),
                }));
            }
        }

        info!(
            context_key = %context_key,
            inspected_memory_count = recent_memories.len(),
            inspected_episode_count = recent_episodes.len(),
            embedding_ready_count = embedding_counts.ready,
            embedding_pending_count = embedding_counts.pending,
            embedding_failed_count = embedding_counts.failed,
            embedding_missing_count = embedding_counts.missing,
            "Durable memory diagnostics snapshot"
        );

        Ok(json!({
            "ok": true,
            "context_key": context_key,
            "thread": {
                "thread_id": thread_id,
                "found": visible_thread.is_some(),
                "title": visible_thread.map(|thread| thread.title.clone()),
                "short_summary": visible_thread.map(|thread| thread.short_summary.clone()),
            },
            "recent_memories": recent_memories,
            "recent_episodes": recent_episodes,
            "embedding_status_counts": embedding_counts.as_json(),
            "notes": [
                "recent_memories spans the scoped context and includes soft-deleted records",
                "recent_episodes only covers the inspected visible thread",
                "embedding_status = missing means no embedding row exists for that owner"
            ],
        })
        .to_string())
    }

    async fn execute_read_episode(&self, arguments: &str) -> Result<String> {
        let args: MemoryReadEpisodeArgs = Self::parse_args(arguments, TOOL_MEMORY_READ_EPISODE)?;
        let Some(episode) = self
            .store
            .get_episode(&args.episode_id)
            .await
            .map_err(|error| anyhow!("failed to read memory episode: {error}"))?
        else {
            return Ok(
                json!({"ok": true, "found": false, "episode_id": args.episode_id}).to_string(),
            );
        };

        let Some(thread) = self
            .store
            .get_thread(&episode.thread_id)
            .await
            .map_err(|error| anyhow!("failed to read episode thread: {error}"))?
        else {
            return Ok(
                json!({"ok": true, "found": false, "episode_id": args.episode_id}).to_string(),
            );
        };

        if !thread_is_visible(&thread, &self.scope) {
            return Ok(
                json!({"ok": true, "found": false, "episode_id": args.episode_id}).to_string(),
            );
        }

        Ok(json!({
            "ok": true,
            "found": true,
            "episode": episode,
            "thread": {
                "thread_id": thread.thread_id,
                "title": thread.title,
                "short_summary": thread.short_summary,
            }
        })
        .to_string())
    }

    async fn execute_read_thread_summary(&self, arguments: &str) -> Result<String> {
        let args: MemoryReadThreadSummaryArgs =
            Self::parse_args(arguments, TOOL_MEMORY_READ_THREAD_SUMMARY)?;
        let thread_id = args.thread_id.unwrap_or_else(|| self.scoped_thread_id());
        let Some(thread) = self
            .store
            .get_thread(&thread_id)
            .await
            .map_err(|error| anyhow!("failed to read memory thread: {error}"))?
        else {
            return Ok(json!({"ok": true, "found": false, "thread_id": thread_id}).to_string());
        };

        if !thread_is_visible(&thread, &self.scope) {
            return Ok(json!({"ok": true, "found": false, "thread_id": thread_id}).to_string());
        }

        let episode_limit = normalize_limit(args.episode_limit, DEFAULT_THREAD_EPISODE_LIMIT, 20);
        let recent_episodes = self
            .store
            .list_episodes_for_thread(
                &thread_id,
                &EpisodeListFilter {
                    min_importance: None,
                    outcome: None,
                    limit: Some(episode_limit),
                },
            )
            .await
            .map_err(|error| anyhow!("failed to list thread episodes: {error}"))?;

        Ok(json!({
            "ok": true,
            "found": true,
            "thread": thread,
            "recent_episodes": recent_episodes.into_iter().map(|episode| json!({
                "episode_id": episode.episode_id,
                "goal": episode.goal,
                "summary": truncate_chars(&episode.summary, 220),
                "outcome": episode.outcome,
                "importance": episode.importance,
                "created_at": episode.created_at,
                "artifact_count": episode.artifacts.len(),
            })).collect::<Vec<_>>(),
        })
        .to_string())
    }

    async fn execute_read_thread_window(&self, arguments: &str) -> Result<String> {
        let args: MemoryReadThreadWindowArgs =
            Self::parse_args(arguments, TOOL_MEMORY_READ_THREAD_WINDOW)?;
        let thread_id = args.thread_id.unwrap_or_else(|| self.scoped_thread_id());
        let Some(thread) = self
            .store
            .get_thread(&thread_id)
            .await
            .map_err(|error| anyhow!("failed to read memory thread: {error}"))?
        else {
            return Ok(json!({"ok": true, "found": false, "thread_id": thread_id}).to_string());
        };

        if !thread_is_visible(&thread, &self.scope) {
            return Ok(json!({"ok": true, "found": false, "thread_id": thread_id}).to_string());
        }

        let offset = args.offset.unwrap_or(0);
        let limit = normalize_limit(args.limit, DEFAULT_WINDOW_LIMIT, MAX_WINDOW_LIMIT);
        let Some(artifact_storage) = self.artifact_storage.as_ref() else {
            return Err(anyhow!(
                "memory thread window reads require artifact storage to be configured"
            ));
        };
        let mut episodes = self
            .store
            .list_episodes_for_thread(
                &thread_id,
                &EpisodeListFilter {
                    min_importance: None,
                    outcome: None,
                    limit: Some(100),
                },
            )
            .await
            .map_err(|error| anyhow!("failed to list thread episodes for window read: {error}"))?;
        episodes.sort_by(|left, right| left.created_at.cmp(&right.created_at));

        let mut sources = Vec::new();
        let mut messages = Vec::new();
        for episode in &episodes {
            let mut artifacts = episode.artifacts.clone();
            artifacts.sort_by(|left, right| left.created_at.cmp(&right.created_at));
            for artifact in artifacts
                .into_iter()
                .filter(|artifact| artifact.storage_key.starts_with("archive/"))
            {
                let payload = self
                    .artifact_storage
                    .as_ref()
                    .unwrap_or(artifact_storage)
                    .load_text_artifact(artifact.storage_key.clone())
                    .await
                    .map_err(|error| {
                        anyhow!("failed to load archived thread window artifact: {error}")
                    })?;
                let Some(payload) = payload else {
                    continue;
                };
                sources.push(json!({
                    "episode_id": episode.episode_id,
                    "storage_key": artifact.storage_key,
                    "title": artifact.description,
                    "created_at": artifact.created_at,
                }));
                messages.extend(extract_archive_messages(
                    &payload,
                    &episode.episode_id,
                    &artifact.storage_key,
                    &artifact.description,
                ));
            }
        }

        let total_messages = messages.len();
        let window = messages
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect::<Vec<_>>();

        Ok(json!({
            "ok": true,
            "found": true,
            "thread_id": thread_id,
            "thread_title": thread.title,
            "offset": offset,
            "limit": limit,
            "available_messages": total_messages,
            "returned_messages": window.len(),
            "archive_source_count": sources.len(),
            "sources": sources,
            "messages": window,
        })
        .to_string())
    }
}

#[async_trait]
impl ToolProvider for MemoryProvider {
    fn name(&self) -> &'static str {
        "memory"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        Self::tools_definitions()
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        matches!(
            tool_name,
            TOOL_MEMORY_SEARCH
                | TOOL_MEMORY_READ_EPISODE
                | TOOL_MEMORY_READ_THREAD_SUMMARY
                | TOOL_MEMORY_READ_THREAD_WINDOW
                | TOOL_MEMORY_DIAGNOSTICS
        )
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        _progress_tx: Option<&tokio::sync::mpsc::Sender<crate::agent::progress::AgentEvent>>,
        _cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        match tool_name {
            TOOL_MEMORY_SEARCH => self.execute_search(arguments).await,
            TOOL_MEMORY_READ_EPISODE => self.execute_read_episode(arguments).await,
            TOOL_MEMORY_READ_THREAD_SUMMARY => self.execute_read_thread_summary(arguments).await,
            TOOL_MEMORY_READ_THREAD_WINDOW => self.execute_read_thread_window(arguments).await,
            TOOL_MEMORY_DIAGNOSTICS => self.execute_diagnostics(arguments).await,
            _ => Err(anyhow!("unknown memory tool: {tool_name}")),
        }
    }
}

fn scoped_thread_id(scope: &AgentMemoryScope) -> String {
    let scoped = format!(
        "thread:{}:{}:{}",
        scope.user_id, scope.context_key, scope.flow_id
    );
    format!(
        "thread-{}",
        Uuid::new_v5(&Uuid::NAMESPACE_URL, scoped.as_bytes())
    )
}

fn normalize_limit(value: Option<usize>, default: usize, max: usize) -> usize {
    value.unwrap_or(default).clamp(1, max)
}

fn parse_time_range(input: Option<TimeRangeArgs>) -> Result<TimeRange> {
    let Some(input) = input else {
        return Ok(TimeRange::default());
    };

    Ok(TimeRange {
        since: parse_optional_datetime(input.since.as_deref())?,
        until: parse_optional_datetime(input.until.as_deref())?,
    })
}

fn parse_optional_datetime(value: Option<&str>) -> Result<Option<DateTime<Utc>>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let parsed = DateTime::parse_from_rfc3339(value)
        .map_err(|error| anyhow!("invalid RFC3339 timestamp '{value}': {error}"))?;
    Ok(Some(parsed.with_timezone(&Utc)))
}

fn thread_is_visible(thread: &oxide_agent_memory::ThreadRecord, scope: &AgentMemoryScope) -> bool {
    thread.user_id == scope.user_id && thread.context_key == scope.context_key
}

#[derive(Default)]
struct EmbeddingDiagnosticsCounts {
    ready: usize,
    pending: usize,
    failed: usize,
    missing: usize,
}

impl EmbeddingDiagnosticsCounts {
    fn observe(&mut self, embedding: Option<&oxide_agent_memory::EmbeddingRecord>) {
        match embedding.map(|embedding| embedding.status) {
            Some(oxide_agent_memory::EmbeddingStatus::Ready) => self.ready += 1,
            Some(oxide_agent_memory::EmbeddingStatus::Pending) => self.pending += 1,
            Some(oxide_agent_memory::EmbeddingStatus::Failed) => self.failed += 1,
            None => self.missing += 1,
        }
    }

    fn as_json(&self) -> Value {
        json!({
            "ready": self.ready,
            "pending": self.pending,
            "failed": self.failed,
            "missing": self.missing,
        })
    }
}

fn embedding_status_label(embedding: Option<&oxide_agent_memory::EmbeddingRecord>) -> &'static str {
    match embedding.map(|embedding| embedding.status) {
        Some(oxide_agent_memory::EmbeddingStatus::Ready) => "ready",
        Some(oxide_agent_memory::EmbeddingStatus::Pending) => "pending",
        Some(oxide_agent_memory::EmbeddingStatus::Failed) => "failed",
        None => "missing",
    }
}

fn retrieval_diagnostics(outcome: &DurableMemorySearchOutcome) -> Value {
    retrieval_diagnostics_from_inner(&outcome.diagnostics)
}

fn retrieval_diagnostics_from_inner(diagnostics: &DurableMemoryRetrievalDiagnostics) -> Value {
    json!({
        "hit": diagnostics.hit(),
        "empty_reason": diagnostics.empty_reason,
        "requested_sources": {
            "episodes": diagnostics.search_episodes,
            "memories": diagnostics.search_memories,
        },
        "candidate_limit": diagnostics.candidate_limit,
        "lexical_hits": {
            "episodes": diagnostics.episode_lexical_hits,
            "memories": diagnostics.memory_lexical_hits,
        },
        "vector_hits": {
            "episodes": diagnostics.episode_vector_hits,
            "memories": diagnostics.memory_vector_hits,
        },
        "vector_status": {
            "episodes": diagnostics.episode_vector_status.as_str(),
            "memories": diagnostics.memory_vector_status.as_str(),
        },
        "fused_candidate_count": diagnostics.fused_candidate_count,
        "injected_item_count": diagnostics.injected_item_count,
        "final_contribution": {
            "lexical_only": diagnostics.lexical_only_items,
            "vector_only": diagnostics.vector_only_items,
            "hybrid": diagnostics.hybrid_items,
        },
        "filtered_out": {
            "low_score": diagnostics.filtered_low_score,
            "duplicate_snippet": diagnostics.filtered_duplicate_snippet,
            "covered_by_episode": diagnostics.filtered_covered_episode,
        },
    })
}

fn search_result(item: DurableMemorySearchItem) -> Value {
    match item {
        DurableMemorySearchItem::Episode {
            record,
            snippet,
            score,
            lexical_score,
            vector_score,
        } => json!({
            "kind": "episode",
            "score": score,
            "lexical_score": lexical_score,
            "vector_score": vector_score,
            "snippet": snippet,
            "episode_id": record.episode_id,
            "thread_id": record.thread_id,
            "goal": record.goal,
            "outcome": record.outcome,
            "importance": record.importance,
            "created_at": record.created_at,
        }),
        DurableMemorySearchItem::Memory {
            record,
            snippet,
            score,
            lexical_score,
            vector_score,
        } => json!({
            "kind": "memory",
            "score": score,
            "lexical_score": lexical_score,
            "vector_score": vector_score,
            "snippet": snippet,
            "memory_id": record.memory_id,
            "source_episode_id": record.source_episode_id,
            "memory_type": record.memory_type,
            "title": record.title,
            "short_description": record.short_description,
            "importance": record.importance,
            "confidence": record.confidence,
            "source": record.source,
            "reason": record.reason,
            "tags": record.tags,
            "updated_at": record.updated_at,
        }),
    }
}

fn extract_archive_messages(
    payload: &str,
    episode_id: &str,
    storage_key: &str,
    title: &str,
) -> Vec<Value> {
    let parsed = serde_json::from_str::<Value>(payload).ok();
    let Some(messages) = parsed
        .as_ref()
        .and_then(|value| value.get("messages"))
        .and_then(Value::as_array)
    else {
        return vec![json!({
            "episode_id": episode_id,
            "role": "system",
            "kind": "archive_blob",
            "content": truncate_chars(payload, ARCHIVE_MESSAGE_MAX_CHARS),
            "source_storage_key": storage_key,
            "source_title": title,
        })];
    };

    messages
        .iter()
        .map(|message| {
            json!({
                "episode_id": episode_id,
                "role": message.get("role").and_then(Value::as_str).unwrap_or("unknown"),
                "kind": message.get("kind").and_then(Value::as_str).unwrap_or("legacy"),
                "tool_name": message.get("tool_name").and_then(Value::as_str),
                "content": truncate_chars(
                    message.get("content").and_then(Value::as_str).unwrap_or_default(),
                    ARCHIVE_MESSAGE_MAX_CHARS,
                ),
                "source_storage_key": storage_key,
                "source_title": title,
            })
        })
        .collect()
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut truncated = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        truncated.push('…');
    }
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MockStorageProvider;
    use chrono::TimeZone;
    use mockall::predicate::{eq, function};
    use oxide_agent_memory::{
        stable_memory_content_hash, ArtifactRef, EmbeddingFailureUpdate, EmbeddingOwnerType,
        EmbeddingPendingUpdate, EmbeddingReadyUpdate, EmbeddingUpdateBase, EpisodeOutcome,
        EpisodeRecord, EpisodeSearchFilter, EpisodeSearchHit, InMemoryMemoryRepository,
        MemoryRecord, MemorySearchFilter, MemorySearchHit, ThreadRecord,
    };

    struct FakeEmbeddingGenerator;

    #[async_trait]
    impl MemoryEmbeddingGenerator for FakeEmbeddingGenerator {
        async fn embed_document(
            &self,
            _text: &str,
            _title: Option<&str>,
        ) -> anyhow::Result<Vec<f32>> {
            Ok(vec![1.0, 0.0])
        }

        async fn embed_query(&self, _text: &str) -> anyhow::Result<Vec<f32>> {
            Ok(vec![1.0, 0.0])
        }
    }

    fn scope() -> AgentMemoryScope {
        AgentMemoryScope::new(7, "topic-a", "flow-a")
    }

    fn ts(seconds: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(seconds, 0)
            .single()
            .expect("valid timestamp")
    }

    fn thread_record() -> ThreadRecord {
        ThreadRecord {
            thread_id: scoped_thread_id(&scope()),
            user_id: 7,
            context_key: "topic-a".to_string(),
            title: "Stage work".to_string(),
            short_summary: "Summary".to_string(),
            created_at: ts(10),
            updated_at: ts(20),
            last_activity_at: ts(20),
        }
    }

    fn episode_record() -> EpisodeRecord {
        EpisodeRecord {
            episode_id: "ep-1".to_string(),
            thread_id: scoped_thread_id(&scope()),
            context_key: "topic-a".to_string(),
            goal: "Investigate memory search".to_string(),
            summary: "Confirmed lexical retrieval works.".to_string(),
            outcome: EpisodeOutcome::Success,
            tools_used: vec!["memory_search".to_string()],
            artifacts: vec![ArtifactRef {
                storage_key: "archive/topic-a/flow-a/history-1.json".to_string(),
                description: "Compacted history".to_string(),
                content_type: Some("application/json".to_string()),
                source: Some("test".to_string()),
                reason: None,
                tags: vec!["archive".to_string()],
                created_at: ts(30),
            }],
            failures: vec![],
            importance: 0.9,
            created_at: ts(30),
        }
    }

    fn memory_record() -> MemoryRecord {
        MemoryRecord {
            memory_id: "mem-1".to_string(),
            context_key: "topic-a".to_string(),
            source_episode_id: Some("ep-1".to_string()),
            memory_type: MemoryType::Fact,
            title: "R2_REGION exact lookup".to_string(),
            content: "Keep exact env var matching in lexical search".to_string(),
            short_description: "env var lookup".to_string(),
            importance: 0.95,
            confidence: 0.9,
            source: Some("test".to_string()),
            content_hash: Some(stable_memory_content_hash(
                MemoryType::Fact,
                "Keep exact env var matching in lexical search",
            )),
            reason: Some("fixture".to_string()),
            tags: vec!["search".to_string()],
            created_at: ts(31),
            updated_at: ts(32),
            deleted_at: None,
        }
    }

    #[tokio::test]
    async fn memory_search_merges_episode_and_memory_hits() {
        let mut storage = MockStorageProvider::new();
        storage
            .expect_search_memory_episodes_lexical()
            .with(
                eq("R2_REGION".to_string()),
                function(|filter: &EpisodeSearchFilter| {
                    filter.context_key.as_deref() == Some("topic-a")
                        && filter.user_id == Some(7)
                        && filter.limit == Some(8)
                }),
            )
            .returning(|_, _| {
                Ok(vec![EpisodeSearchHit {
                    record: episode_record(),
                    score: 0.7,
                    snippet: "episode hit".to_string(),
                }])
            });
        storage
            .expect_search_memory_episodes_vector()
            .with(
                function(|embedding: &Vec<f32>| embedding == &vec![1.0, 0.0]),
                eq("test-profile".to_string()),
                function(|filter: &EpisodeSearchFilter| {
                    filter.context_key.as_deref() == Some("topic-a")
                        && filter.user_id == Some(7)
                        && filter.limit == Some(8)
                }),
            )
            .returning(|_, _, _| Ok(Vec::new()));
        storage
            .expect_search_memory_records_lexical()
            .with(
                eq("R2_REGION".to_string()),
                function(|filter: &MemorySearchFilter| {
                    filter.context_key.as_deref() == Some("topic-a")
                        && filter.user_id == Some(7)
                        && filter.limit == Some(8)
                        && filter.memory_type == Some(MemoryType::Fact)
                }),
            )
            .returning(|_, _| {
                Ok(vec![MemorySearchHit {
                    record: memory_record(),
                    score: 0.9,
                    snippet: "memory hit".to_string(),
                }])
            });
        storage
            .expect_search_memory_records_vector()
            .with(
                function(|embedding: &Vec<f32>| embedding == &vec![1.0, 0.0]),
                eq("test-profile".to_string()),
                function(|filter: &MemorySearchFilter| {
                    filter.context_key.as_deref() == Some("topic-a")
                        && filter.user_id == Some(7)
                        && filter.limit == Some(8)
                        && filter.memory_type == Some(MemoryType::Fact)
                }),
            )
            .returning(|_, _, _| {
                Ok(vec![MemorySearchHit {
                    record: memory_record(),
                    score: 0.95,
                    snippet: "semantic memory hit".to_string(),
                }])
            });

        let provider = MemoryProvider::new(Arc::new(storage), scope())
            .with_query_embedding_generator(Arc::new(FakeEmbeddingGenerator))
            .with_query_embedding_model_id("test-profile");
        let result = provider
            .execute(
                TOOL_MEMORY_SEARCH,
                r#"{"query":"R2_REGION","memory_type":"fact","limit":5}"#,
                None,
                None,
            )
            .await
            .expect("search must succeed");

        let parsed: Value = serde_json::from_str(&result).expect("valid json");
        assert_eq!(parsed["result_count"], 2);
        assert_eq!(parsed["results"][0]["kind"], "memory");
        assert_eq!(parsed["results"][1]["kind"], "episode");
        assert_eq!(parsed["diagnostics"]["hit"], true);
        assert_eq!(parsed["diagnostics"]["vector_status"]["memories"], "hit");
        assert_eq!(parsed["diagnostics"]["final_contribution"]["hybrid"], 1);
        assert_eq!(
            parsed["diagnostics"]["final_contribution"]["lexical_only"],
            1
        );
    }

    #[tokio::test]
    async fn memory_diagnostics_reports_deleted_records_and_embedding_statuses() {
        let repository = Arc::new(InMemoryMemoryRepository::new());
        repository
            .upsert_thread(thread_record())
            .await
            .expect("thread should persist");
        repository
            .create_episode(episode_record())
            .await
            .expect("episode should persist");

        let mut active_memory = memory_record();
        active_memory.memory_id = "mem-active".to_string();
        active_memory.title = "Active memory".to_string();
        let mut deleted_memory = memory_record();
        deleted_memory.memory_id = "mem-deleted".to_string();
        deleted_memory.title = "Deleted memory".to_string();
        deleted_memory.content = "Delete this memory after consolidation".to_string();
        deleted_memory.content_hash = Some(stable_memory_content_hash(
            deleted_memory.memory_type,
            &deleted_memory.content,
        ));

        repository
            .create_memory(active_memory.clone())
            .await
            .expect("active memory should persist");
        repository
            .create_memory(deleted_memory.clone())
            .await
            .expect("deleted memory should persist");
        repository
            .delete_memory(&deleted_memory.memory_id)
            .await
            .expect("deleted memory should soft-delete");

        repository
            .upsert_embedding_ready(EmbeddingReadyUpdate {
                base: EmbeddingUpdateBase {
                    owner_id: episode_record().episode_id,
                    owner_type: EmbeddingOwnerType::Episode,
                    model_id: "test-embed".to_string(),
                    content_hash: "ep-hash".to_string(),
                },
                embedding: vec![0.1, 0.2],
                indexed_at: ts(40),
            })
            .await
            .expect("episode embedding should persist");
        repository
            .upsert_embedding_pending(EmbeddingPendingUpdate {
                base: EmbeddingUpdateBase {
                    owner_id: active_memory.memory_id.clone(),
                    owner_type: EmbeddingOwnerType::Memory,
                    model_id: "test-embed".to_string(),
                    content_hash: "mem-active-hash".to_string(),
                },
                requested_at: ts(41),
            })
            .await
            .expect("pending embedding should persist");
        repository
            .upsert_embedding_failure(EmbeddingFailureUpdate {
                base: EmbeddingUpdateBase {
                    owner_id: deleted_memory.memory_id.clone(),
                    owner_type: EmbeddingOwnerType::Memory,
                    model_id: "test-embed".to_string(),
                    content_hash: "mem-deleted-hash".to_string(),
                },
                error: "embedding provider timeout".to_string(),
                failed_at: ts(42),
            })
            .await
            .expect("failed embedding should persist");

        let repository_for_provider: Arc<dyn PersistentMemoryStore> = repository;
        let provider = MemoryProvider::new_with_store(repository_for_provider, None, scope());
        let result = provider
            .execute(TOOL_MEMORY_DIAGNOSTICS, r#"{"limit":10}"#, None, None)
            .await
            .expect("memory diagnostics should succeed");

        let parsed: Value = serde_json::from_str(&result).expect("valid json");
        assert_eq!(parsed["thread"]["found"], true);
        assert_eq!(parsed["embedding_status_counts"]["ready"], 1);
        assert_eq!(parsed["embedding_status_counts"]["pending"], 1);
        assert_eq!(parsed["embedding_status_counts"]["failed"], 1);
        assert_eq!(parsed["embedding_status_counts"]["missing"], 0);
        assert_eq!(parsed["recent_memories"].as_array().map(Vec::len), Some(2));
        assert!(parsed["recent_memories"]
            .as_array()
            .expect("memories array")
            .iter()
            .any(|entry| entry["memory"]["deleted_at"].is_string()));
    }

    #[tokio::test]
    async fn read_episode_returns_scoped_episode() {
        let mut storage = MockStorageProvider::new();
        storage
            .expect_get_memory_episode()
            .with(eq("ep-1".to_string()))
            .returning(|_| Ok(Some(episode_record())));
        storage
            .expect_get_memory_thread()
            .with(eq(scoped_thread_id(&scope())))
            .returning(|_| Ok(Some(thread_record())));

        let provider = MemoryProvider::new(Arc::new(storage), scope());
        let result = provider
            .execute(
                TOOL_MEMORY_READ_EPISODE,
                r#"{"episode_id":"ep-1"}"#,
                None,
                None,
            )
            .await
            .expect("read episode must succeed");

        let parsed: Value = serde_json::from_str(&result).expect("valid json");
        assert_eq!(parsed["found"], true);
        assert_eq!(parsed["episode"]["episode_id"], "ep-1");
    }

    #[tokio::test]
    async fn thread_summary_defaults_to_scoped_thread() {
        let mut storage = MockStorageProvider::new();
        let thread_id = scoped_thread_id(&scope());
        storage
            .expect_get_memory_thread()
            .with(eq(thread_id.clone()))
            .returning(|_| Ok(Some(thread_record())));
        storage
            .expect_list_memory_episodes_for_thread()
            .with(
                eq(thread_id),
                function(|filter: &EpisodeListFilter| filter.limit == Some(6)),
            )
            .returning(|_, _| Ok(vec![episode_record()]));

        let provider = MemoryProvider::new(Arc::new(storage), scope());
        let result = provider
            .execute(TOOL_MEMORY_READ_THREAD_SUMMARY, "{}", None, None)
            .await
            .expect("thread summary must succeed");

        let parsed: Value = serde_json::from_str(&result).expect("valid json");
        assert_eq!(parsed["found"], true);
        assert_eq!(parsed["recent_episodes"][0]["episode_id"], "ep-1");
    }

    #[tokio::test]
    async fn thread_window_reads_archived_messages() {
        let mut storage = MockStorageProvider::new();
        let thread_id = scoped_thread_id(&scope());
        storage
            .expect_get_memory_thread()
            .with(eq(thread_id.clone()))
            .returning(|_| Ok(Some(thread_record())));
        storage
            .expect_list_memory_episodes_for_thread()
            .with(
                eq(thread_id),
                function(|filter: &EpisodeListFilter| filter.limit == Some(100)),
            )
            .returning(|_, _| Ok(vec![episode_record()]));
        storage
            .expect_load_text_artifact()
            .with(eq("archive/topic-a/flow-a/history-1.json".to_string()))
            .returning(|_| {
                Ok(Some(
                    json!({
                        "messages": [
                            {"role": "user", "kind": "user_turn", "content": "Need a durable memory read tool"},
                            {"role": "assistant", "kind": "assistant_response", "content": "I will implement Stage 9."}
                        ]
                    })
                    .to_string(),
                ))
            });

        let provider = MemoryProvider::new(Arc::new(storage), scope());
        let result = provider
            .execute(
                TOOL_MEMORY_READ_THREAD_WINDOW,
                r#"{"limit":10}"#,
                None,
                None,
            )
            .await
            .expect("thread window must succeed");

        let parsed: Value = serde_json::from_str(&result).expect("valid json");
        assert_eq!(parsed["found"], true);
        assert_eq!(parsed["returned_messages"], 2);
        assert_eq!(parsed["messages"][0]["role"], "user");
    }

    #[test]
    fn memory_tools_expose_read_only_surface() {
        let tool_names = memory_tool_names();
        assert_eq!(
            tool_names,
            vec![
                TOOL_MEMORY_SEARCH.to_string(),
                TOOL_MEMORY_READ_EPISODE.to_string(),
                TOOL_MEMORY_READ_THREAD_SUMMARY.to_string(),
                TOOL_MEMORY_READ_THREAD_WINDOW.to_string(),
                TOOL_MEMORY_DIAGNOSTICS.to_string(),
            ]
        );
    }
}
