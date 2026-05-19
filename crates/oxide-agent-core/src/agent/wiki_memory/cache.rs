use super::patch::ValidatedWikiPatch;
use super::store::{wiki_content_hash, WikiPage, WikiStore};
use crate::storage::StorageError;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use tokio::sync::Mutex;

/// Cached wiki page plus original hash metadata for future dirty-page tracking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedWikiPage {
    /// Deterministic object key for the page.
    pub key: String,
    /// UTF-8 Markdown content.
    pub content: String,
    /// Current SHA-256 content hash.
    pub content_hash: String,
    /// Hash observed when the page first entered this session cache.
    pub original_content_hash: Option<String>,
    /// Whether the page has local unflushed changes.
    pub dirty: bool,
}

impl CachedWikiPage {
    fn clean(page: WikiPage) -> Self {
        Self {
            key: page.key,
            original_content_hash: Some(page.content_hash.clone()),
            content_hash: page.content_hash,
            content: page.content,
            dirty: false,
        }
    }

    fn bootstrap(key: String, content: String) -> Self {
        Self {
            key,
            content_hash: wiki_content_hash(&content),
            content,
            original_content_hash: None,
            dirty: false,
        }
    }
}

/// Read-path metrics for one wiki session cache.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WikiCacheMetrics {
    /// Pages served from the in-memory cache.
    pub cache_hits: usize,
    /// Pages not found in the in-memory cache.
    pub cache_misses: usize,
    /// Deterministic backend GET attempts.
    pub backend_gets: usize,
    /// Missing backend objects converted to in-memory bootstrap pages.
    pub bootstrap_pages: usize,
    /// Pages marked dirty by validated patches.
    pub dirty_pages: usize,
    /// Deterministic backend PUT attempts.
    pub backend_puts: usize,
    /// Dirty pages skipped because content hash did not change.
    pub skipped_puts: usize,
    /// Dirty pages successfully flushed.
    pub flushed_pages: usize,
}

/// Result of flushing dirty wiki pages.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WikiFlushResult {
    /// Dirty pages considered for flush.
    pub considered_pages: usize,
    /// Pages written to the backend.
    pub written_pages: usize,
    /// Pages skipped because hash did not change.
    pub skipped_unchanged_pages: usize,
}

#[derive(Default)]
struct WikiSessionCacheState {
    pages: HashMap<String, CachedWikiPage>,
    metrics: WikiCacheMetrics,
}

/// Per-run read-through cache for deterministic wiki pages.
pub struct WikiSessionCache {
    store: WikiStore,
    state: Mutex<WikiSessionCacheState>,
}

impl WikiSessionCache {
    /// Create an empty read-through cache over a wiki store.
    #[must_use]
    pub fn new(store: WikiStore) -> Self {
        Self {
            store,
            state: Mutex::new(WikiSessionCacheState::default()),
        }
    }

    /// Return a snapshot of cache metrics.
    pub async fn metrics(&self) -> WikiCacheMetrics {
        self.state.lock().await.metrics
    }

    /// Load global `index.md`, bootstrapping it in memory when absent.
    pub async fn load_global_index(&self) -> Result<CachedWikiPage, StorageError> {
        let key = self.store.global_file_key("index.md")?;
        self.load_or_bootstrap(
            key,
            || self.store.read_global_file("index.md"),
            bootstrap_global_index,
        )
        .await
    }

    /// Load context `index.md`, bootstrapping it in memory when absent.
    pub async fn load_context_index(
        &self,
        context_id: &str,
    ) -> Result<CachedWikiPage, StorageError> {
        let key = self.store.context_file_key(context_id, "index.md")?;
        self.load_or_bootstrap(
            key,
            || self.store.read_context_file(context_id, "index.md"),
            || bootstrap_context_index(context_id),
        )
        .await
    }

    /// Load a context core file, returning `None` when the object is absent.
    pub async fn load_context_file(
        &self,
        context_id: &str,
        file: &str,
    ) -> Result<Option<CachedWikiPage>, StorageError> {
        let key = self.store.context_file_key(context_id, file)?;
        self.load_optional(key, || self.store.read_context_file(context_id, file))
            .await
    }

    /// Load a global file, returning `None` when the object is absent.
    pub async fn load_global_file(
        &self,
        file: &str,
    ) -> Result<Option<CachedWikiPage>, StorageError> {
        let key = self.store.global_file_key(file)?;
        self.load_optional(key, || self.store.read_global_file(file))
            .await
    }

    /// Load a context topic page, returning `None` when the object is absent.
    pub async fn load_context_page(
        &self,
        context_id: &str,
        slug: &str,
    ) -> Result<Option<CachedWikiPage>, StorageError> {
        let key = self.store.context_page_key(context_id, slug)?;
        self.load_optional(key, || self.store.read_context_page(context_id, slug))
            .await
    }

    /// Apply validated patch operations to the local dirty-page cache.
    pub async fn apply_validated_patch(
        &self,
        patch: &ValidatedWikiPatch,
    ) -> Result<usize, StorageError> {
        let mut applied = 0;
        let mut state = self.state.lock().await;

        for operation in &patch.operations {
            if let Some(expected) = operation.expected_hash.as_ref() {
                let Some(existing) = state.pages.get(&operation.key) else {
                    return Err(StorageError::InvalidInput(format!(
                        "wiki patch expected hash for uncached page {}",
                        operation.key
                    )));
                };
                if &existing.content_hash != expected {
                    return Err(StorageError::InvalidInput(format!(
                        "wiki patch expected hash mismatch for {}",
                        operation.key
                    )));
                }
            }

            let content_hash = wiki_content_hash(&operation.content);
            let original_content_hash = state
                .pages
                .get(&operation.key)
                .and_then(|page| page.original_content_hash.clone())
                .or_else(|| {
                    state
                        .pages
                        .get(&operation.key)
                        .map(|page| page.content_hash.clone())
                });

            state.pages.insert(
                operation.key.clone(),
                CachedWikiPage {
                    key: operation.key.clone(),
                    content: operation.content.clone(),
                    content_hash,
                    original_content_hash,
                    dirty: true,
                },
            );
            applied += 1;
        }

        state.metrics.dirty_pages += applied;
        Ok(applied)
    }

    /// Flush dirty pages to the backend, skipping unchanged content hashes.
    pub async fn flush_dirty_pages(&self) -> Result<WikiFlushResult, StorageError> {
        let dirty_pages = {
            let state = self.state.lock().await;
            let mut pages = state
                .pages
                .values()
                .filter(|page| page.dirty)
                .cloned()
                .collect::<Vec<_>>();
            pages.sort_by_key(|page| (flush_order_rank(&page.key), page.key.clone()));
            pages
        };

        let mut result = WikiFlushResult {
            considered_pages: dirty_pages.len(),
            ..WikiFlushResult::default()
        };

        for page in dirty_pages {
            if page.original_content_hash.as_deref() == Some(page.content_hash.as_str()) {
                self.mark_clean(&page.key, false).await;
                result.skipped_unchanged_pages += 1;
                continue;
            }

            self.store
                .put_validated_key(&page.key, &page.content)
                .await?;
            self.mark_clean(&page.key, true).await;
            result.written_pages += 1;
        }

        Ok(result)
    }

    /// Reconcile runtime-owned protected metadata files for a validated patch.
    ///
    /// Planner patches cannot directly edit `index.md` or `log.md`; the runtime
    /// updates them after validation so pages are discoverable without S3 LIST
    /// and each patch cycle has one compact chronological entry.
    pub async fn reconcile_context_patch_metadata(
        &self,
        context_id: &str,
        patch: &ValidatedWikiPatch,
        timestamp: DateTime<Utc>,
    ) -> Result<usize, StorageError> {
        let entries = patch
            .operations
            .iter()
            .filter_map(|operation| metadata_entry_from_key(context_id, &operation.key))
            .collect::<Vec<_>>();
        if entries.is_empty() {
            return Ok(0);
        }

        let mut staged = 0;
        let index = self.load_context_index(context_id).await?;
        let updated_index = update_context_index(index.content.as_str(), &entries);
        if updated_index != index.content {
            self.stage_dirty_page(index.key, updated_index).await;
            staged += 1;
        }

        let log_key = self.store.context_file_key(context_id, "log.md")?;
        let log = match self.load_context_file(context_id, "log.md").await? {
            Some(page) => page,
            None => CachedWikiPage::bootstrap(log_key, bootstrap_context_log(context_id)),
        };
        let updated_log =
            append_context_log_entry(log.content.as_str(), patch, &entries, timestamp);
        if updated_log != log.content {
            self.stage_dirty_page(log.key, updated_log).await;
            staged += 1;
        }

        Ok(staged)
    }

    async fn load_optional<F, Fut>(
        &self,
        key: String,
        load: F,
    ) -> Result<Option<CachedWikiPage>, StorageError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Option<WikiPage>, StorageError>>,
    {
        if let Some(page) = self.cached_page(&key).await {
            return Ok(Some(page));
        }

        self.record_backend_get(&key).await;
        let loaded = load().await?;
        match loaded {
            Some(page) => {
                let cached = CachedWikiPage::clean(page);
                self.insert_page(key, cached.clone()).await;
                Ok(Some(cached))
            }
            None => Ok(None),
        }
    }

    async fn load_or_bootstrap<F, Fut, B>(
        &self,
        key: String,
        load: F,
        bootstrap: B,
    ) -> Result<CachedWikiPage, StorageError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Option<WikiPage>, StorageError>>,
        B: FnOnce() -> String,
    {
        if let Some(page) = self.cached_page(&key).await {
            return Ok(page);
        }

        self.record_backend_get(&key).await;
        let page = match load().await? {
            Some(page) => CachedWikiPage::clean(page),
            None => {
                let page = CachedWikiPage::bootstrap(key.clone(), bootstrap());
                self.state.lock().await.metrics.bootstrap_pages += 1;
                page
            }
        };
        self.insert_page(key, page.clone()).await;
        Ok(page)
    }

    async fn cached_page(&self, key: &str) -> Option<CachedWikiPage> {
        let mut state = self.state.lock().await;
        match state.pages.get(key).cloned() {
            Some(page) => {
                state.metrics.cache_hits += 1;
                Some(page)
            }
            None => {
                state.metrics.cache_misses += 1;
                None
            }
        }
    }

    async fn insert_page(&self, key: String, page: CachedWikiPage) {
        self.state.lock().await.pages.insert(key, page);
    }

    async fn stage_dirty_page(&self, key: String, content: String) {
        let mut state = self.state.lock().await;
        let content_hash = wiki_content_hash(&content);
        let original_content_hash = state
            .pages
            .get(&key)
            .and_then(|page| page.original_content_hash.clone())
            .or_else(|| state.pages.get(&key).map(|page| page.content_hash.clone()));
        state.pages.insert(
            key.clone(),
            CachedWikiPage {
                key,
                content,
                content_hash,
                original_content_hash,
                dirty: true,
            },
        );
        state.metrics.dirty_pages += 1;
    }

    async fn record_backend_get(&self, _key: &str) {
        self.state.lock().await.metrics.backend_gets += 1;
    }

    async fn mark_clean(&self, key: &str, wrote_backend: bool) {
        let mut state = self.state.lock().await;
        if let Some(page) = state.pages.get_mut(key) {
            page.dirty = false;
            page.original_content_hash = Some(page.content_hash.clone());
        }
        if wrote_backend {
            state.metrics.backend_puts += 1;
            state.metrics.flushed_pages += 1;
        } else {
            state.metrics.skipped_puts += 1;
        }
    }
}

fn flush_order_rank(key: &str) -> u8 {
    if key.ends_with("/index.md") {
        2
    } else if key.ends_with("/log.md") {
        3
    } else if key.contains("/inbox/") || key.contains("/raw/") {
        1
    } else {
        0
    }
}

fn bootstrap_global_index() -> String {
    "# Wiki Index\n\nUpdated: bootstrap\nScope: global\n\n## Core pages\n\n## Maintenance\n\n- page_count: 0\n- inbox_count: 0\n- raw_archive_enabled: false\n".to_string()
}

fn bootstrap_context_index(context_id: &str) -> String {
    format!(
        "# Wiki Index\n\nUpdated: bootstrap\nScope: context\nContext ID: {context_id}\n\n## Core pages\n\n- [overview](overview.md) - current project overview, active goals, key facts\n\n## Topic pages\n\n## Inbox\n\n## Maintenance\n\n- page_count: 1\n- inbox_count: 0\n- raw_archive_enabled: false\n"
    )
}

fn bootstrap_context_log(context_id: &str) -> String {
    format!("# Context Wiki Log\n\nContext ID: {context_id}\n")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PatchMetadataEntry {
    relative_path: String,
    title: String,
    inbox: bool,
}

fn metadata_entry_from_key(context_id: &str, key: &str) -> Option<PatchMetadataEntry> {
    let marker = format!("contexts/{context_id}/");
    let relative = key.split_once(&marker)?.1;
    let inbox = relative.starts_with("inbox/");
    let supported = inbox
        || relative.starts_with("pages/")
        || matches!(
            relative,
            "overview.md"
                | "decisions.md"
                | "constraints.md"
                | "procedures.md"
                | "open-questions.md"
        );
    if !supported || !relative.ends_with(".md") {
        return None;
    }

    let title = relative
        .trim_end_matches(".md")
        .rsplit('/')
        .next()
        .unwrap_or(relative)
        .replace('-', " ");
    Some(PatchMetadataEntry {
        relative_path: relative.to_string(),
        title,
        inbox,
    })
}

fn update_context_index(existing: &str, entries: &[PatchMetadataEntry]) -> String {
    let mut content = existing.trim_end().to_string();
    let mut page_lines = Vec::new();
    let mut inbox_lines = Vec::new();

    for entry in entries {
        if existing.contains(&format!("]({})", entry.relative_path)) {
            continue;
        }
        let line = format!(
            "- [{}]({}) - captured wiki update candidate",
            entry.title, entry.relative_path
        );
        if entry.inbox {
            inbox_lines.push(line);
        } else {
            page_lines.push(line);
        }
    }

    if !page_lines.is_empty() {
        append_index_section(&mut content, "Topic pages", &page_lines);
    }
    if !inbox_lines.is_empty() {
        append_index_section(&mut content, "Inbox", &inbox_lines);
    }

    content.push('\n');
    content
}

fn append_index_section(content: &mut String, section: &str, lines: &[String]) {
    if !content.contains(&format!("## {section}")) {
        content.push_str("\n\n## ");
        content.push_str(section);
        content.push('\n');
    }
    for line in lines {
        content.push('\n');
        content.push_str(line);
    }
}

fn append_context_log_entry(
    existing: &str,
    patch: &ValidatedWikiPatch,
    entries: &[PatchMetadataEntry],
    timestamp: DateTime<Utc>,
) -> String {
    let mut content = existing.trim_end().to_string();
    let changed = entries
        .iter()
        .map(|entry| entry.relative_path.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let source = patch
        .source_refs
        .first()
        .map(String::as_str)
        .unwrap_or("run:unknown");
    let reason = patch.reason.replace('"', "'");
    content.push_str(&format!(
        "\n- {} source={} reason=\"{}\" changed={}\n",
        timestamp.to_rfc3339(),
        source,
        reason,
        changed
    ));
    content
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::wiki_memory::{
        ValidatedWikiPatch, ValidatedWikiPatchOperation, WikiObjectBackend,
    };
    use async_trait::async_trait;
    use chrono::TimeZone;
    use std::sync::Arc;

    #[derive(Default)]
    struct InMemoryWikiBackend {
        objects: Mutex<HashMap<String, String>>,
        put_keys: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl WikiObjectBackend for InMemoryWikiBackend {
        async fn get_text(&self, key: &str) -> Result<Option<String>, StorageError> {
            Ok(self.objects.lock().await.get(key).cloned())
        }

        async fn put_text(&self, key: &str, content: &str) -> Result<(), StorageError> {
            self.put_keys.lock().await.push(key.to_string());
            self.objects
                .lock()
                .await
                .insert(key.to_string(), content.to_string());
            Ok(())
        }
    }

    fn validated_patch(key: &str, content: &str) -> ValidatedWikiPatch {
        ValidatedWikiPatch {
            reason: "test".to_string(),
            source_refs: vec!["run:test".to_string()],
            operations: vec![ValidatedWikiPatchOperation {
                key: key.to_string(),
                content: content.to_string(),
                expected_hash: None,
                inbox: false,
            }],
        }
    }

    #[tokio::test]
    async fn apply_patch_marks_page_dirty_and_flush_writes_once() {
        let backend = Arc::new(InMemoryWikiBackend::default());
        let store_backend = Arc::clone(&backend);
        let cache = WikiSessionCache::new(WikiStore::new(store_backend, "prod"));
        let key = "prod/wiki/v1/contexts/ctx-12345678/procedures.md";

        let applied = cache
            .apply_validated_patch(&validated_patch(key, "# Procedure"))
            .await
            .expect("patch should apply");
        let flush = cache
            .flush_dirty_pages()
            .await
            .expect("flush should succeed");

        assert_eq!(applied, 1);
        assert_eq!(flush.considered_pages, 1);
        assert_eq!(flush.written_pages, 1);
        assert_eq!(backend.put_keys.lock().await.as_slice(), [key]);
        assert_eq!(
            backend.objects.lock().await.get(key).map(String::as_str),
            Some("# Procedure")
        );
    }

    #[tokio::test]
    async fn flush_skips_unchanged_hash() {
        let backend = Arc::new(InMemoryWikiBackend::default());
        let key = "prod/wiki/v1/contexts/ctx-12345678/overview.md";
        backend
            .objects
            .lock()
            .await
            .insert(key.to_string(), "# Overview".to_string());
        let store_backend = Arc::clone(&backend);
        let cache = WikiSessionCache::new(WikiStore::new(store_backend, "prod"));

        cache
            .load_context_file("ctx-12345678", "overview.md")
            .await
            .expect("load should succeed");
        cache
            .apply_validated_patch(&validated_patch(key, "# Overview"))
            .await
            .expect("patch should apply");
        let flush = cache
            .flush_dirty_pages()
            .await
            .expect("flush should succeed");

        assert_eq!(flush.considered_pages, 1);
        assert_eq!(flush.written_pages, 0);
        assert_eq!(flush.skipped_unchanged_pages, 1);
        assert!(backend.put_keys.lock().await.is_empty());
    }

    #[tokio::test]
    async fn apply_patch_rejects_expected_hash_mismatch() {
        let backend = Arc::new(InMemoryWikiBackend::default());
        let key = "prod/wiki/v1/contexts/ctx-12345678/overview.md";
        backend
            .objects
            .lock()
            .await
            .insert(key.to_string(), "# Overview".to_string());
        let cache = WikiSessionCache::new(WikiStore::new(backend, "prod"));

        cache
            .load_context_file("ctx-12345678", "overview.md")
            .await
            .expect("load should succeed");
        let patch = ValidatedWikiPatch {
            reason: "test".to_string(),
            source_refs: vec!["run:test".to_string()],
            operations: vec![ValidatedWikiPatchOperation {
                key: key.to_string(),
                content: "# Changed".to_string(),
                expected_hash: Some("wrong".to_string()),
                inbox: false,
            }],
        };

        let result = cache.apply_validated_patch(&patch).await;

        assert!(matches!(result, Err(StorageError::InvalidInput(_))));
    }

    #[tokio::test]
    async fn reconcile_context_patch_metadata_updates_index_and_log() {
        let backend = Arc::new(InMemoryWikiBackend::default());
        let store_backend = Arc::clone(&backend);
        let cache = WikiSessionCache::new(WikiStore::new(store_backend, "prod"));
        let key = "prod/wiki/v1/contexts/ctx-12345678/pages/deploy-workflow.md";
        let patch = validated_patch(key, "# Deploy workflow");

        cache
            .apply_validated_patch(&patch)
            .await
            .expect("patch should apply");
        let metadata_pages = cache
            .reconcile_context_patch_metadata(
                "ctx-12345678",
                &patch,
                Utc.with_ymd_and_hms(2026, 5, 19, 10, 0, 0)
                    .single()
                    .expect("valid time"),
            )
            .await
            .expect("metadata reconciliation should succeed");
        let flush = cache
            .flush_dirty_pages()
            .await
            .expect("flush should succeed");

        assert_eq!(metadata_pages, 2);
        assert_eq!(flush.written_pages, 3);
        let objects = backend.objects.lock().await;
        assert!(objects
            .get("prod/wiki/v1/contexts/ctx-12345678/index.md")
            .expect("index should be written")
            .contains("pages/deploy-workflow.md"));
        assert!(objects
            .get("prod/wiki/v1/contexts/ctx-12345678/log.md")
            .expect("log should be written")
            .contains("changed=pages/deploy-workflow.md"));
    }
}
