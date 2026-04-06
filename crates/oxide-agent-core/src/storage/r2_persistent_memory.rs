use super::{
    keys::{
        persistent_memory_episode_key, persistent_memory_record_key,
        persistent_memory_session_state_key, persistent_memory_thread_key,
    },
    r2::R2Storage,
    StorageError,
};
use oxide_agent_memory::{
    ArtifactRef, EpisodeListFilter, EpisodeRecord, EpisodeSearchFilter, EpisodeSearchHit,
    MemoryListFilter, MemoryRecord, MemorySearchFilter, MemorySearchHit, SessionStateRecord,
    ThreadRecord,
};

const EPISODE_SNIPPET_LEN: usize = 160;
const MEMORY_SNIPPET_LEN: usize = 160;
const MEMORY_THREADS_PREFIX: &str = "persistent_memory/threads/";
const MEMORY_CONTEXTS_PREFIX: &str = "persistent_memory/contexts/";

impl R2Storage {
    pub(super) async fn upsert_memory_thread_inner(
        &self,
        record: ThreadRecord,
    ) -> Result<ThreadRecord, StorageError> {
        let key = persistent_memory_thread_key(&record.thread_id);
        let stored = if let Some(existing) = self.load_json::<ThreadRecord>(&key).await? {
            ThreadRecord {
                created_at: existing.created_at,
                ..record
            }
        } else {
            record
        };
        self.save_json(&key, &stored).await?;
        Ok(stored)
    }

    pub(super) async fn create_memory_episode_inner(
        &self,
        record: EpisodeRecord,
    ) -> Result<EpisodeRecord, StorageError> {
        let key = persistent_memory_episode_key(&record.thread_id, &record.episode_id);
        if self.load_json::<EpisodeRecord>(&key).await?.is_some() {
            return Err(StorageError::InvalidInput(format!(
                "persistent episode {} already exists",
                record.episode_id
            )));
        }
        self.save_json(&key, &record).await?;
        Ok(record)
    }

    pub(super) async fn upsert_memory_session_state_inner(
        &self,
        record: SessionStateRecord,
    ) -> Result<SessionStateRecord, StorageError> {
        let key = persistent_memory_session_state_key(&record.session_id);
        self.save_json(&key, &record).await?;
        Ok(record)
    }

    pub(super) async fn create_memory_record_inner(
        &self,
        record: MemoryRecord,
    ) -> Result<MemoryRecord, StorageError> {
        let key = persistent_memory_record_key(&record.context_key, &record.memory_id);
        if self.load_json::<MemoryRecord>(&key).await?.is_some() {
            return Err(StorageError::InvalidInput(format!(
                "persistent memory {} already exists",
                record.memory_id
            )));
        }
        self.save_json(&key, &record).await?;
        Ok(record)
    }

    pub(super) async fn link_memory_episode_artifact_inner(
        &self,
        episode_id: String,
        artifact: ArtifactRef,
    ) -> Result<Option<EpisodeRecord>, StorageError> {
        let Some(mut episode) = self.get_memory_episode_inner(episode_id).await? else {
            return Ok(None);
        };

        if let Some(existing) = episode
            .artifacts
            .iter_mut()
            .find(|existing| existing.storage_key == artifact.storage_key)
        {
            merge_artifact_ref(existing, artifact);
        } else {
            episode.artifacts.push(artifact);
        }

        let key = persistent_memory_episode_key(&episode.thread_id, &episode.episode_id);
        self.save_json(&key, &episode).await?;
        Ok(Some(episode))
    }

    pub(super) async fn get_memory_thread_inner(
        &self,
        thread_id: String,
    ) -> Result<Option<ThreadRecord>, StorageError> {
        self.load_json(&persistent_memory_thread_key(&thread_id))
            .await
    }

    pub(super) async fn list_memory_threads_inner(
        &self,
    ) -> Result<Vec<ThreadRecord>, StorageError> {
        let mut threads = Vec::new();
        for key in self.list_keys_under_prefix(MEMORY_THREADS_PREFIX).await? {
            if !key.ends_with(".json") || key.contains("/episodes/") {
                continue;
            }
            if let Some(record) = self.load_json::<ThreadRecord>(&key).await? {
                threads.push(record);
            }
        }

        Ok(threads)
    }

    pub(super) async fn get_memory_episode_inner(
        &self,
        episode_id: String,
    ) -> Result<Option<EpisodeRecord>, StorageError> {
        let needle = format!("/episodes/{episode_id}.json");
        for key in self.list_keys_under_prefix(MEMORY_THREADS_PREFIX).await? {
            if !key.ends_with(&needle) {
                continue;
            }
            return self.load_json::<EpisodeRecord>(&key).await;
        }

        Ok(None)
    }

    pub(super) async fn list_memory_episodes_for_thread_inner(
        &self,
        thread_id: String,
        filter: EpisodeListFilter,
    ) -> Result<Vec<EpisodeRecord>, StorageError> {
        let prefix = format!("persistent_memory/threads/{thread_id}/episodes/");
        let mut episodes = self
            .list_json_under_prefix::<EpisodeRecord>(&prefix)
            .await?
            .into_iter()
            .filter(|episode| match filter.min_importance {
                Some(min_importance) => episode.importance >= min_importance,
                None => true,
            })
            .filter(|episode| match filter.outcome {
                Some(outcome) => episode.outcome == outcome,
                None => true,
            })
            .collect::<Vec<_>>();

        episodes.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| left.episode_id.cmp(&right.episode_id))
        });

        if let Some(limit) = filter.limit {
            episodes.truncate(limit);
        }

        Ok(episodes)
    }

    pub(super) async fn get_memory_record_inner(
        &self,
        memory_id: String,
    ) -> Result<Option<MemoryRecord>, StorageError> {
        for record in self
            .list_json_under_prefix::<MemoryRecord>(MEMORY_CONTEXTS_PREFIX)
            .await?
        {
            if record.memory_id == memory_id {
                return Ok(Some(record));
            }
        }

        Ok(None)
    }

    pub(super) async fn list_memory_records_inner(
        &self,
        context_key: String,
        filter: MemoryListFilter,
    ) -> Result<Vec<MemoryRecord>, StorageError> {
        let prefix = format!("persistent_memory/contexts/{context_key}/memories/");
        let mut memories = self
            .list_json_under_prefix::<MemoryRecord>(&prefix)
            .await?
            .into_iter()
            .filter(|memory| match filter.memory_type {
                Some(memory_type) => memory.memory_type == memory_type,
                None => true,
            })
            .filter(|memory| match filter.min_importance {
                Some(min_importance) => memory.importance >= min_importance,
                None => true,
            })
            .filter(|memory| filter.tags.iter().all(|tag| memory.tags.contains(tag)))
            .collect::<Vec<_>>();

        memories.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.memory_id.cmp(&right.memory_id))
        });

        if let Some(limit) = filter.limit {
            memories.truncate(limit);
        }

        Ok(memories)
    }

    pub(super) async fn search_memory_episodes_lexical_inner(
        &self,
        query: String,
        filter: EpisodeSearchFilter,
    ) -> Result<Vec<EpisodeSearchHit>, StorageError> {
        let terms = lexical_terms(&query);
        if terms.is_empty() {
            return Ok(Vec::new());
        }

        let threads = self.list_memory_threads_inner().await?;
        let filtered_threads = threads
            .into_iter()
            .filter(|thread| match &filter.context_key {
                Some(context_key) => &thread.context_key == context_key,
                None => true,
            })
            .filter(|thread| match filter.user_id {
                Some(user_id) => thread.user_id == user_id,
                None => true,
            })
            .collect::<Vec<_>>();

        let mut hits = Vec::new();
        for thread in filtered_threads {
            let episodes = self
                .list_memory_episodes_for_thread_inner(
                    thread.thread_id,
                    EpisodeListFilter::default(),
                )
                .await?;
            hits.extend(
                episodes
                    .into_iter()
                    .filter(|episode| matches_episode_search(episode, &filter))
                    .filter_map(|episode| {
                        let tools = episode.tools_used.join(" ");
                        let failures = episode.failures.join(" ");
                        let score = lexical_score(
                            &[
                                (&episode.goal, 3.0),
                                (&episode.summary, 2.0),
                                (&tools, 1.5),
                                (&failures, 1.5),
                            ],
                            &terms,
                        );
                        (score > 0.0).then(|| EpisodeSearchHit {
                            record: episode.clone(),
                            score,
                            snippet: snippet_for(
                                &[&episode.goal, &episode.summary],
                                &terms,
                                EPISODE_SNIPPET_LEN,
                            ),
                        })
                    }),
            );
        }

        hits.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| right.record.importance.total_cmp(&left.record.importance))
                .then_with(|| right.record.created_at.cmp(&left.record.created_at))
                .then_with(|| left.record.episode_id.cmp(&right.record.episode_id))
        });

        if let Some(limit) = filter.limit {
            hits.truncate(limit);
        }

        Ok(hits)
    }

    pub(super) async fn search_memory_records_lexical_inner(
        &self,
        query: String,
        filter: MemorySearchFilter,
    ) -> Result<Vec<MemorySearchHit>, StorageError> {
        let terms = lexical_terms(&query);
        if terms.is_empty() {
            return Ok(Vec::new());
        }

        let threads = self
            .list_memory_threads_inner()
            .await?
            .into_iter()
            .map(|thread| (thread.thread_id.clone(), thread))
            .collect::<std::collections::HashMap<_, _>>();

        let mut episodes = std::collections::HashMap::new();
        if filter.user_id.is_some() {
            for thread_id in threads.keys() {
                for episode in self
                    .list_memory_episodes_for_thread_inner(
                        thread_id.clone(),
                        EpisodeListFilter::default(),
                    )
                    .await?
                {
                    episodes.insert(episode.episode_id.clone(), episode.thread_id.clone());
                }
            }
        }

        let memories = if let Some(context_key) = &filter.context_key {
            self.list_memory_records_inner(context_key.clone(), MemoryListFilter::default())
                .await?
        } else {
            self.list_json_under_prefix::<MemoryRecord>(MEMORY_CONTEXTS_PREFIX)
                .await?
        };

        let mut hits = memories
            .into_iter()
            .filter(|memory| matches_memory_search(memory, &filter, &episodes, &threads))
            .filter_map(|memory| {
                let tags = memory.tags.join(" ");
                let score = lexical_score(
                    &[
                        (&memory.title, 3.0),
                        (&memory.short_description, 2.0),
                        (&memory.content, 2.0),
                        (&tags, 1.0),
                    ],
                    &terms,
                );
                (score > 0.0).then(|| MemorySearchHit {
                    record: memory.clone(),
                    score,
                    snippet: snippet_for(
                        &[&memory.title, &memory.short_description, &memory.content],
                        &terms,
                        MEMORY_SNIPPET_LEN,
                    ),
                })
            })
            .collect::<Vec<_>>();

        hits.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| right.record.importance.total_cmp(&left.record.importance))
                .then_with(|| right.record.confidence.total_cmp(&left.record.confidence))
                .then_with(|| right.record.updated_at.cmp(&left.record.updated_at))
                .then_with(|| left.record.memory_id.cmp(&right.record.memory_id))
        });

        if let Some(limit) = filter.limit {
            hits.truncate(limit);
        }

        Ok(hits)
    }
}

fn lexical_terms(query: &str) -> Vec<String> {
    query
        .split(|character: char| {
            !(character.is_alphanumeric() || character == '_' || character == '-')
        })
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn field_matches(field: &str, term: &str) -> bool {
    field.to_ascii_lowercase().contains(term)
}

fn lexical_score(fields: &[(&str, f32)], terms: &[String]) -> f32 {
    terms
        .iter()
        .map(|term| {
            fields
                .iter()
                .filter_map(|(field, weight)| field_matches(field, term).then_some(*weight))
                .sum::<f32>()
        })
        .sum()
}

fn snippet_for(fields: &[&str], terms: &[String], max_chars: usize) -> String {
    let source = fields
        .iter()
        .copied()
        .find(|field| !field.is_empty() && terms.iter().any(|term| field_matches(field, term)))
        .or_else(|| fields.iter().copied().find(|field| !field.is_empty()))
        .unwrap_or_default();

    truncate_snippet(source, max_chars)
}

fn truncate_snippet(value: &str, max_chars: usize) -> String {
    let mut truncated = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        truncated.push('…');
    }
    truncated
}

fn merge_artifact_ref(existing: &mut ArtifactRef, incoming: ArtifactRef) {
    if existing.description.trim().is_empty() && !incoming.description.trim().is_empty() {
        existing.description = incoming.description;
    }
    if existing.content_type.is_none() {
        existing.content_type = incoming.content_type;
    }
    if existing.source.is_none() {
        existing.source = incoming.source;
    }
    if existing.reason.is_none() {
        existing.reason = incoming.reason;
    }
    merge_tags(&mut existing.tags, incoming.tags);
}

fn merge_tags(existing: &mut Vec<String>, incoming: Vec<String>) {
    for tag in incoming {
        if !existing.iter().any(|current| current == &tag) {
            existing.push(tag);
        }
    }
}

fn matches_episode_search(episode: &EpisodeRecord, filter: &EpisodeSearchFilter) -> bool {
    match filter.outcome {
        Some(outcome) if episode.outcome != outcome => return false,
        _ => {}
    }
    match filter.min_importance {
        Some(min_importance) if episode.importance < min_importance => return false,
        _ => {}
    }
    match filter.time_range.since {
        Some(since) if episode.created_at < since => return false,
        _ => {}
    }
    match filter.time_range.until {
        Some(until) if episode.created_at > until => return false,
        _ => {}
    }

    true
}

fn matches_memory_search(
    memory: &MemoryRecord,
    filter: &MemorySearchFilter,
    episodes: &std::collections::HashMap<String, String>,
    threads: &std::collections::HashMap<String, ThreadRecord>,
) -> bool {
    if let Some(context_key) = &filter.context_key {
        if &memory.context_key != context_key {
            return false;
        }
    }
    if let Some(user_id) = filter.user_id {
        let Some(source_episode_id) = memory.source_episode_id.as_ref() else {
            return false;
        };
        let Some(thread_id) = episodes.get(source_episode_id) else {
            return false;
        };
        let Some(thread) = threads.get(thread_id) else {
            return false;
        };
        if thread.user_id != user_id {
            return false;
        }
    }
    if let Some(memory_type) = filter.memory_type {
        if memory.memory_type != memory_type {
            return false;
        }
    }
    if let Some(min_importance) = filter.min_importance {
        if memory.importance < min_importance {
            return false;
        }
    }
    if !filter.tags.iter().all(|tag| memory.tags.contains(tag)) {
        return false;
    }
    if let Some(since) = filter.time_range.since {
        if memory.updated_at < since {
            return false;
        }
    }
    if let Some(until) = filter.time_range.until {
        if memory.updated_at > until {
            return false;
        }
    }

    true
}
