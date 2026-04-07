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
    stable_memory_content_hash, ArtifactRef, EmbeddingOwnerType, EpisodeListFilter, EpisodeRecord,
    MemoryListFilter, MemoryRecord, MemoryType, TimeRange,
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
const TOOL_MEMORY_WRITE_FACT: &str = "memory_write_fact";
const TOOL_MEMORY_WRITE_PROCEDURE: &str = "memory_write_procedure";
const TOOL_MEMORY_LINK_ARTIFACT: &str = "memory_link_artifact";

const DEFAULT_SEARCH_LIMIT: usize = 8;
const MAX_SEARCH_LIMIT: usize = 20;
const DEFAULT_THREAD_EPISODE_LIMIT: usize = 6;
const DEFAULT_WINDOW_LIMIT: usize = 20;
const MAX_WINDOW_LIMIT: usize = 50;
const DEFAULT_DIAGNOSTICS_LIMIT: usize = 12;
const MAX_DIAGNOSTICS_LIMIT: usize = 50;
const ARCHIVE_MESSAGE_MAX_CHARS: usize = 500;
const MEMORY_TITLE_MAX_CHARS: usize = 96;
const MEMORY_SHORT_DESCRIPTION_MAX_CHARS: usize = 160;
const MEMORY_CONTENT_MAX_CHARS: usize = 600;
const MEMORY_REASON_MAX_CHARS: usize = 240;
const MEMORY_SOURCE_MAX_CHARS: usize = 64;
const ARTIFACT_DESCRIPTION_MAX_CHARS: usize = 160;
const TAG_MAX_CHARS: usize = 32;
const MAX_TAGS: usize = 12;

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

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum WriteMemoryTypeArg {
    Fact,
    Preference,
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

impl From<WriteMemoryTypeArg> for MemoryType {
    fn from(value: WriteMemoryTypeArg) -> Self {
        match value {
            WriteMemoryTypeArg::Fact => MemoryType::Fact,
            WriteMemoryTypeArg::Preference => MemoryType::Preference,
            WriteMemoryTypeArg::Constraint => MemoryType::Constraint,
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryWriteFactArgs {
    content: String,
    title: Option<String>,
    short_description: Option<String>,
    memory_type: Option<WriteMemoryTypeArg>,
    source_episode_id: Option<String>,
    source: Option<String>,
    reason: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryWriteProcedureArgs {
    content: String,
    title: Option<String>,
    short_description: Option<String>,
    source_episode_id: Option<String>,
    source: Option<String>,
    reason: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryLinkArtifactArgs {
    episode_id: String,
    storage_key: String,
    description: String,
    content_type: Option<String>,
    source: Option<String>,
    reason: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

struct ExplicitMemoryDraft<'a> {
    title: Option<&'a str>,
    short_description: Option<&'a str>,
    content: &'a str,
    source: Option<&'a str>,
    reason: Option<&'a str>,
    user_tags: Vec<String>,
}

/// Tool names exposed by the persistent-memory read provider.
pub fn memory_tool_names() -> Vec<String> {
    vec![
        TOOL_MEMORY_SEARCH.to_string(),
        TOOL_MEMORY_READ_EPISODE.to_string(),
        TOOL_MEMORY_READ_THREAD_SUMMARY.to_string(),
        TOOL_MEMORY_READ_THREAD_WINDOW.to_string(),
        TOOL_MEMORY_DIAGNOSTICS.to_string(),
        TOOL_MEMORY_WRITE_FACT.to_string(),
        TOOL_MEMORY_WRITE_PROCEDURE.to_string(),
        TOOL_MEMORY_LINK_ARTIFACT.to_string(),
    ]
}

/// Provider for scope-aware persistent-memory read tools.
pub struct MemoryProvider {
    store: Arc<dyn PersistentMemoryStore>,
    artifact_storage: Option<Arc<dyn StorageProvider>>,
    scope: AgentMemoryScope,
    query_embedding_generator: Option<Arc<dyn MemoryEmbeddingGenerator>>,
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

    fn durable_memory_retriever(&self) -> DurableMemoryRetriever {
        let mut retriever = DurableMemoryRetriever::new_with_store(Arc::clone(&self.store));
        if let Some(generator) = self.query_embedding_generator.as_ref() {
            retriever = retriever.with_query_embedding_generator(Arc::clone(generator));
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
            ToolDefinition {
                name: TOOL_MEMORY_WRITE_FACT.to_string(),
                description: "Write a scoped durable fact, preference, or constraint with duplicate guard"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "content": {"type": "string", "description": "Durable fact/preference/constraint content"},
                        "title": {"type": "string", "description": "Optional short human title"},
                        "short_description": {"type": "string", "description": "Optional retrieval preview"},
                        "memory_type": {
                            "type": "string",
                            "enum": ["fact", "preference", "constraint"],
                            "description": "Defaults to fact"
                        },
                        "source_episode_id": {"type": "string", "description": "Optional visible source episode"},
                        "source": {"type": "string", "description": "Optional audit source, e.g. user_request or explicit_tool"},
                        "reason": {"type": "string", "description": "Optional audit reason for storing this memory"},
                        "tags": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Optional tags for retrieval and audit"
                        }
                    },
                    "required": ["content"],
                    "additionalProperties": false
                }),
            },
            ToolDefinition {
                name: TOOL_MEMORY_WRITE_PROCEDURE.to_string(),
                description: "Write a scoped durable reusable procedure or playbook with duplicate guard"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "content": {"type": "string", "description": "Reusable procedure or playbook content"},
                        "title": {"type": "string", "description": "Optional short human title"},
                        "short_description": {"type": "string", "description": "Optional retrieval preview"},
                        "source_episode_id": {"type": "string", "description": "Optional visible source episode"},
                        "source": {"type": "string", "description": "Optional audit source, e.g. user_request or explicit_tool"},
                        "reason": {"type": "string", "description": "Optional audit reason for storing this procedure"},
                        "tags": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Optional tags for retrieval and audit"
                        }
                    },
                    "required": ["content"],
                    "additionalProperties": false
                }),
            },
            ToolDefinition {
                name: TOOL_MEMORY_LINK_ARTIFACT.to_string(),
                description: "Link one artifact to a visible durable episode with duplicate guard"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "episode_id": {"type": "string", "description": "Visible episode identifier to attach the artifact to"},
                        "storage_key": {"type": "string", "description": "Artifact storage key or stable path"},
                        "description": {"type": "string", "description": "Human description of the artifact"},
                        "content_type": {"type": "string", "description": "Optional MIME type or format hint"},
                        "source": {"type": "string", "description": "Optional audit source, e.g. sandbox or explicit_tool"},
                        "reason": {"type": "string", "description": "Optional audit reason for linking the artifact"},
                        "tags": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Optional tags for retrieval and audit"
                        }
                    },
                    "required": ["episode_id", "storage_key", "description"],
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

    async fn resolve_visible_episode(&self, episode_id: &str) -> Result<Option<EpisodeRecord>> {
        let Some(episode) = self
            .store
            .get_episode(episode_id)
            .await
            .map_err(|error| anyhow!("failed to read memory episode: {error}"))?
        else {
            return Ok(None);
        };

        let Some(thread) = self
            .store
            .get_thread(&episode.thread_id)
            .await
            .map_err(|error| anyhow!("failed to read episode thread: {error}"))?
        else {
            return Ok(None);
        };

        if !thread_is_visible(&thread, &self.scope) {
            return Ok(None);
        }

        Ok(Some(episode))
    }

    async fn require_visible_source_episode(
        &self,
        source_episode_id: Option<&str>,
    ) -> Result<Option<EpisodeRecord>> {
        let Some(source_episode_id) = source_episode_id else {
            return Ok(None);
        };
        self.resolve_visible_episode(source_episode_id)
            .await?
            .ok_or_else(|| {
                anyhow!(
                    "source_episode_id '{}' is not visible in the current scoped memory context",
                    source_episode_id
                )
            })
            .map(Some)
    }

    async fn create_memory_with_duplicate_guard(
        &self,
        record: MemoryRecord,
    ) -> Result<(bool, MemoryRecord)> {
        match self.store.create_memory(record.clone()).await {
            Ok(stored) => Ok((false, stored)),
            Err(error) if is_duplicate_repository_write_error(&error) => {
                let existing = self
                    .store
                    .get_memory(&record.memory_id)
                    .await
                    .map_err(|fetch_error| {
                        anyhow!(
                            "memory write reported duplicate but existing record could not be loaded: {fetch_error}"
                        )
                    })?
                    .ok_or_else(|| {
                        anyhow!(
                            "memory write reported duplicate but record {} is missing",
                            record.memory_id
                        )
                    })?;
                Ok((true, existing))
            }
            Err(error) => Err(anyhow!("failed to persist reusable memory: {error}")),
        }
    }

    async fn execute_write_fact(&self, arguments: &str) -> Result<String> {
        let args: MemoryWriteFactArgs = Self::parse_args(arguments, TOOL_MEMORY_WRITE_FACT)?;
        let source_episode = self
            .require_visible_source_episode(args.source_episode_id.as_deref())
            .await?;
        let memory_type = args.memory_type.unwrap_or(WriteMemoryTypeArg::Fact).into();
        let record = build_explicit_memory_record(
            &self.scope,
            source_episode.as_ref(),
            memory_type,
            ExplicitMemoryDraft {
                title: args.title.as_deref(),
                short_description: args.short_description.as_deref(),
                content: &args.content,
                source: args.source.as_deref(),
                reason: args.reason.as_deref(),
                user_tags: args.tags,
            },
        )?;
        let (duplicate, stored) = self.create_memory_with_duplicate_guard(record).await?;

        info!(
            memory_write_source = "tool",
            context_key = %stored.context_key,
            memory_id = %stored.memory_id,
            memory_type = memory_type_tag(stored.memory_type),
            duplicate,
            source_episode_id = stored.source_episode_id.as_deref().unwrap_or("none"),
            "Explicit memory write telemetry"
        );

        Ok(json!({
            "ok": true,
            "duplicate": duplicate,
            "memory": stored,
        })
        .to_string())
    }

    async fn execute_write_procedure(&self, arguments: &str) -> Result<String> {
        let args: MemoryWriteProcedureArgs =
            Self::parse_args(arguments, TOOL_MEMORY_WRITE_PROCEDURE)?;
        let source_episode = self
            .require_visible_source_episode(args.source_episode_id.as_deref())
            .await?;
        let record = build_explicit_memory_record(
            &self.scope,
            source_episode.as_ref(),
            MemoryType::Procedure,
            ExplicitMemoryDraft {
                title: args.title.as_deref(),
                short_description: args.short_description.as_deref(),
                content: &args.content,
                source: args.source.as_deref(),
                reason: args.reason.as_deref(),
                user_tags: args.tags,
            },
        )?;
        let (duplicate, stored) = self.create_memory_with_duplicate_guard(record).await?;

        info!(
            memory_write_source = "tool",
            context_key = %stored.context_key,
            memory_id = %stored.memory_id,
            memory_type = memory_type_tag(stored.memory_type),
            duplicate,
            source_episode_id = stored.source_episode_id.as_deref().unwrap_or("none"),
            "Explicit memory write telemetry"
        );

        Ok(json!({
            "ok": true,
            "duplicate": duplicate,
            "memory": stored,
        })
        .to_string())
    }

    async fn execute_link_artifact(&self, arguments: &str) -> Result<String> {
        let args: MemoryLinkArtifactArgs = Self::parse_args(arguments, TOOL_MEMORY_LINK_ARTIFACT)?;
        let Some(existing_episode) = self.resolve_visible_episode(&args.episode_id).await? else {
            return Ok(
                json!({"ok": true, "found": false, "episode_id": args.episode_id}).to_string(),
            );
        };
        let duplicate = existing_episode
            .artifacts
            .iter()
            .any(|artifact| artifact.storage_key == args.storage_key);
        let artifact = build_artifact_ref(
            &args.storage_key,
            &args.description,
            args.content_type.as_deref(),
            args.source.as_deref(),
            args.reason.as_deref(),
            args.tags,
        )?;

        let Some(updated_episode) = self
            .store
            .link_episode_artifact(&args.episode_id, artifact.clone())
            .await
            .map_err(|error| anyhow!("failed to link episode artifact: {error}"))?
        else {
            return Ok(
                json!({"ok": true, "found": false, "episode_id": args.episode_id}).to_string(),
            );
        };
        let stored_artifact = updated_episode
            .artifacts
            .iter()
            .find(|candidate| candidate.storage_key == artifact.storage_key)
            .cloned()
            .unwrap_or(artifact);

        Ok(json!({
            "ok": true,
            "found": true,
            "duplicate": duplicate,
            "episode_id": updated_episode.episode_id,
            "artifact": stored_artifact,
            "artifact_count": updated_episode.artifacts.len(),
        })
        .to_string())
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
                | TOOL_MEMORY_WRITE_FACT
                | TOOL_MEMORY_WRITE_PROCEDURE
                | TOOL_MEMORY_LINK_ARTIFACT
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
            TOOL_MEMORY_WRITE_FACT => self.execute_write_fact(arguments).await,
            TOOL_MEMORY_WRITE_PROCEDURE => self.execute_write_procedure(arguments).await,
            TOOL_MEMORY_LINK_ARTIFACT => self.execute_link_artifact(arguments).await,
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

fn build_explicit_memory_record(
    scope: &AgentMemoryScope,
    source_episode: Option<&EpisodeRecord>,
    memory_type: MemoryType,
    draft: ExplicitMemoryDraft<'_>,
) -> Result<MemoryRecord> {
    let content = normalize_required_text(draft.content, "content", MEMORY_CONTENT_MAX_CHARS)?;
    let default_title = format!("{}: {}", memory_type_title(memory_type), content);
    let title = normalize_optional_text(draft.title, MEMORY_TITLE_MAX_CHARS)
        .unwrap_or_else(|| truncate_chars(&default_title, MEMORY_TITLE_MAX_CHARS));
    let short_description =
        normalize_optional_text(draft.short_description, MEMORY_SHORT_DESCRIPTION_MAX_CHARS)
            .unwrap_or_else(|| truncate_chars(&content, MEMORY_SHORT_DESCRIPTION_MAX_CHARS));
    let source = normalize_optional_text(draft.source, MEMORY_SOURCE_MAX_CHARS)
        .or_else(|| Some("explicit_tool".to_string()));
    let reason = normalize_optional_text(draft.reason, MEMORY_REASON_MAX_CHARS);
    let mut system_tags = vec!["explicit", memory_type_tag(memory_type)];
    if source_episode.is_some() {
        system_tags.push("episode_linked");
    }
    let tags = normalize_tags(draft.user_tags, &system_tags);
    let now = Utc::now();
    let content_hash = stable_memory_content_hash(memory_type, &content);

    Ok(MemoryRecord {
        memory_id: explicit_memory_id(&scope.context_key, memory_type, &content),
        context_key: scope.context_key.clone(),
        source_episode_id: source_episode.map(|episode| episode.episode_id.clone()),
        memory_type,
        title,
        content,
        short_description,
        importance: explicit_memory_importance(memory_type),
        confidence: explicit_memory_confidence(memory_type),
        source,
        content_hash: Some(content_hash),
        reason,
        tags,
        created_at: now,
        updated_at: now,
        deleted_at: None,
    })
}

fn build_artifact_ref(
    storage_key: &str,
    description: &str,
    content_type: Option<&str>,
    source: Option<&str>,
    reason: Option<&str>,
    user_tags: Vec<String>,
) -> Result<ArtifactRef> {
    let storage_key =
        normalize_required_text(storage_key, "storage_key", MEMORY_CONTENT_MAX_CHARS)?;
    let description =
        normalize_required_text(description, "description", ARTIFACT_DESCRIPTION_MAX_CHARS)?;

    Ok(ArtifactRef {
        storage_key,
        description,
        content_type: normalize_optional_text(content_type, MEMORY_SOURCE_MAX_CHARS),
        source: normalize_optional_text(source, MEMORY_SOURCE_MAX_CHARS)
            .or_else(|| Some("explicit_tool".to_string())),
        reason: normalize_optional_text(reason, MEMORY_REASON_MAX_CHARS),
        tags: normalize_tags(user_tags, &["explicit", "artifact"]),
        created_at: Utc::now(),
    })
}

fn normalize_required_text(value: &str, field: &str, max_chars: usize) -> Result<String> {
    normalize_optional_text(Some(value), max_chars)
        .ok_or_else(|| anyhow!("{field} must not be empty"))
}

fn normalize_optional_text(value: Option<&str>, max_chars: usize) -> Option<String> {
    let value = value?;
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return None;
    }
    Some(truncate_chars(&normalized, max_chars))
}

fn normalize_tags(tags: Vec<String>, system_tags: &[&str]) -> Vec<String> {
    let mut normalized = Vec::new();
    for tag in system_tags.iter().map(|tag| (*tag).to_string()).chain(tags) {
        let tag = tag.trim().to_ascii_lowercase();
        if tag.is_empty() {
            continue;
        }
        let tag = truncate_chars(&tag, TAG_MAX_CHARS);
        if normalized.iter().any(|existing| existing == &tag) {
            continue;
        }
        normalized.push(tag);
        if normalized.len() == MAX_TAGS {
            break;
        }
    }
    normalized
}

fn explicit_memory_id(context_key: &str, memory_type: MemoryType, content: &str) -> String {
    let seed = format!(
        "explicit-memory:{context_key}:{}:{content}",
        memory_type_tag(memory_type)
    );
    format!(
        "memory-{}",
        Uuid::new_v5(&Uuid::NAMESPACE_URL, seed.as_bytes())
    )
}

fn explicit_memory_importance(memory_type: MemoryType) -> f32 {
    match memory_type {
        MemoryType::Constraint => 0.92,
        MemoryType::Procedure => 0.88,
        MemoryType::Preference => 0.84,
        MemoryType::Fact => 0.82,
        MemoryType::Decision => 0.9,
    }
}

fn explicit_memory_confidence(memory_type: MemoryType) -> f32 {
    match memory_type {
        MemoryType::Constraint => 0.94,
        MemoryType::Procedure => 0.9,
        MemoryType::Preference => 0.86,
        MemoryType::Fact => 0.88,
        MemoryType::Decision => 0.9,
    }
}

fn memory_type_title(memory_type: MemoryType) -> &'static str {
    match memory_type {
        MemoryType::Fact => "Fact",
        MemoryType::Preference => "Preference",
        MemoryType::Procedure => "Procedure",
        MemoryType::Decision => "Decision",
        MemoryType::Constraint => "Constraint",
    }
}

fn memory_type_tag(memory_type: MemoryType) -> &'static str {
    match memory_type {
        MemoryType::Fact => "fact",
        MemoryType::Preference => "preference",
        MemoryType::Procedure => "procedure",
        MemoryType::Decision => "decision",
        MemoryType::Constraint => "constraint",
    }
}

fn is_duplicate_repository_write_error(error: &oxide_agent_memory::RepositoryError) -> bool {
    matches!(
        error,
        oxide_agent_memory::RepositoryError::Conflict(message)
            | oxide_agent_memory::RepositoryError::Storage(message)
                if message.contains("already exists")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{MockStorageProvider, StorageError};
    use chrono::TimeZone;
    use mockall::predicate::{eq, function};
    use oxide_agent_memory::{
        ArtifactRef, EmbeddingFailureUpdate, EmbeddingOwnerType, EmbeddingPendingUpdate,
        EmbeddingReadyUpdate, EmbeddingUpdateBase, EpisodeOutcome, EpisodeRecord,
        EpisodeSearchFilter, EpisodeSearchHit, InMemoryMemoryRepository, MemoryRecord,
        MemorySearchFilter, MemorySearchHit, ThreadRecord,
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
                function(|filter: &EpisodeSearchFilter| {
                    filter.context_key.as_deref() == Some("topic-a")
                        && filter.user_id == Some(7)
                        && filter.limit == Some(8)
                }),
            )
            .returning(|_, _| Ok(Vec::new()));
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
                    score: 0.95,
                    snippet: "semantic memory hit".to_string(),
                }])
            });

        let provider = MemoryProvider::new(Arc::new(storage), scope())
            .with_query_embedding_generator(Arc::new(FakeEmbeddingGenerator));
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

    #[tokio::test]
    async fn write_fact_persists_scoped_explicit_memory() {
        let mut storage = MockStorageProvider::new();
        storage
            .expect_get_memory_episode()
            .with(eq("ep-1".to_string()))
            .returning(|_| Ok(Some(episode_record())));
        storage
            .expect_get_memory_thread()
            .with(eq(scoped_thread_id(&scope())))
            .returning(|_| Ok(Some(thread_record())));
        storage
            .expect_create_memory_record()
            .with(function(|record: &MemoryRecord| {
                record.context_key == "topic-a"
                    && record.source_episode_id.as_deref() == Some("ep-1")
                    && record.memory_type == MemoryType::Constraint
                    && record.source.as_deref() == Some("user_request")
                    && record.reason.as_deref() == Some("remember team policy")
                    && record.tags.iter().any(|tag| tag == "explicit")
                    && record.tags.iter().any(|tag| tag == "constraint")
                    && record.tags.iter().any(|tag| tag == "policy")
            }))
            .returning(Ok);

        let provider = MemoryProvider::new(Arc::new(storage), scope());
        let result = provider
            .execute(
                TOOL_MEMORY_WRITE_FACT,
                r#"{"content":"Sub-agents must not write persistent memory directly","memory_type":"constraint","source_episode_id":"ep-1","source":"user_request","reason":"remember team policy","tags":["policy"]}"#,
                None,
                None,
            )
            .await
            .expect("write fact must succeed");

        let parsed: Value = serde_json::from_str(&result).expect("valid json");
        assert_eq!(parsed["duplicate"], false);
        assert_eq!(parsed["memory"]["memory_type"], "constraint");
        assert_eq!(parsed["memory"]["source"], "user_request");
    }

    #[tokio::test]
    async fn write_fact_returns_existing_record_on_duplicate() {
        let mut storage = MockStorageProvider::new();
        storage.expect_create_memory_record().returning(|_| {
            Err(StorageError::InvalidInput(
                "memory already exists".to_string(),
            ))
        });
        storage
            .expect_get_memory_record()
            .with(function(|memory_id: &String| {
                memory_id.starts_with("memory-")
            }))
            .returning(|_| Ok(Some(memory_record())));

        let provider = MemoryProvider::new(Arc::new(storage), scope());
        let result = provider
            .execute(
                TOOL_MEMORY_WRITE_FACT,
                r#"{"content":"Keep exact env var matching in lexical search","source":"explicit_tool"}"#,
                None,
                None,
            )
            .await
            .expect("duplicate write fact must succeed");

        let parsed: Value = serde_json::from_str(&result).expect("valid json");
        assert_eq!(parsed["duplicate"], true);
        assert_eq!(parsed["memory"]["memory_id"], "mem-1");
    }

    #[tokio::test]
    async fn write_procedure_rejects_out_of_scope_source_episode() {
        let mut storage = MockStorageProvider::new();
        storage
            .expect_get_memory_episode()
            .with(eq("ep-1".to_string()))
            .returning(|_| Ok(Some(episode_record())));
        storage
            .expect_get_memory_thread()
            .with(eq(scoped_thread_id(&scope())))
            .returning(|_| {
                Ok(Some(ThreadRecord {
                    context_key: "topic-b".to_string(),
                    ..thread_record()
                }))
            });

        let provider = MemoryProvider::new(Arc::new(storage), scope());
        let error = provider
            .execute(
                TOOL_MEMORY_WRITE_PROCEDURE,
                r#"{"content":"Run cargo fmt before cargo clippy","source_episode_id":"ep-1"}"#,
                None,
                None,
            )
            .await
            .expect_err("out-of-scope source episode must fail");

        assert!(error
            .to_string()
            .contains("source_episode_id 'ep-1' is not visible"));
    }

    #[tokio::test]
    async fn link_artifact_reports_duplicate_storage_key() {
        let mut storage = MockStorageProvider::new();
        storage
            .expect_get_memory_episode()
            .with(eq("ep-1".to_string()))
            .returning(|_| Ok(Some(episode_record())));
        storage
            .expect_get_memory_thread()
            .with(eq(scoped_thread_id(&scope())))
            .returning(|_| Ok(Some(thread_record())));
        storage
            .expect_link_memory_episode_artifact()
            .with(
                eq("ep-1".to_string()),
                function(|artifact: &ArtifactRef| {
                    artifact.storage_key == "archive/topic-a/flow-a/history-1.json"
                        && artifact.source.as_deref() == Some("sandbox")
                        && artifact.tags.iter().any(|tag| tag == "artifact")
                }),
            )
            .returning(|_, _| Ok(Some(episode_record())));

        let provider = MemoryProvider::new(Arc::new(storage), scope());
        let result = provider
            .execute(
                TOOL_MEMORY_LINK_ARTIFACT,
                r#"{"episode_id":"ep-1","storage_key":"archive/topic-a/flow-a/history-1.json","description":"Compacted history","source":"sandbox","tags":["report"]}"#,
                None,
                None,
            )
            .await
            .expect("artifact link must succeed");

        let parsed: Value = serde_json::from_str(&result).expect("valid json");
        assert_eq!(parsed["found"], true);
        assert_eq!(parsed["duplicate"], true);
        assert_eq!(
            parsed["artifact"]["storage_key"],
            "archive/topic-a/flow-a/history-1.json"
        );
    }
}
