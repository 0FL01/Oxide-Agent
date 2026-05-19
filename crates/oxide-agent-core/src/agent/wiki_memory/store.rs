use crate::storage::{
    wiki_context_inbox_key, wiki_context_key, wiki_context_page_key, wiki_context_raw_key,
    wiki_global_key, R2Storage, StorageError, StorageProvider,
};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// Minimal text object backend used by LLM Wiki memory.
#[async_trait]
pub trait WikiObjectBackend: Send + Sync {
    /// Fetch one deterministic text object by key.
    async fn get_text(&self, key: &str) -> Result<Option<String>, StorageError>;
    /// Store one deterministic text object by key.
    async fn put_text(&self, key: &str, content: &str) -> Result<(), StorageError>;
    /// Delete one deterministic text object by key.
    async fn delete_text(&self, key: &str) -> Result<(), StorageError>;
}

#[async_trait]
impl WikiObjectBackend for R2Storage {
    async fn get_text(&self, key: &str) -> Result<Option<String>, StorageError> {
        self.load_text(key).await
    }

    async fn put_text(&self, key: &str, content: &str) -> Result<(), StorageError> {
        self.save_text(key, content).await
    }

    async fn delete_text(&self, key: &str) -> Result<(), StorageError> {
        self.delete_object(key).await
    }
}

/// Loaded wiki page content with its deterministic object key and content hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WikiPage {
    /// S3/R2 object key used for the page.
    pub key: String,
    /// UTF-8 Markdown page content.
    pub content: String,
    /// SHA-256 hash of `content`.
    pub content_hash: String,
}

/// Deterministic store for LLM Wiki Markdown pages.
#[derive(Clone)]
pub struct WikiStore {
    backend: Arc<dyn WikiObjectBackend>,
    prefix: String,
}

struct StorageProviderWikiBackend {
    storage: Arc<dyn StorageProvider>,
}

#[async_trait]
impl WikiObjectBackend for StorageProviderWikiBackend {
    async fn get_text(&self, key: &str) -> Result<Option<String>, StorageError> {
        self.storage.load_wiki_text(key.to_string()).await
    }

    async fn put_text(&self, key: &str, content: &str) -> Result<(), StorageError> {
        self.storage
            .save_wiki_text(key.to_string(), content.to_string())
            .await
    }

    async fn delete_text(&self, key: &str) -> Result<(), StorageError> {
        self.storage.delete_wiki_text(key.to_string()).await
    }
}

impl WikiStore {
    /// Create a wiki store over a text object backend and optional storage prefix.
    #[must_use]
    pub fn new(backend: Arc<dyn WikiObjectBackend>, prefix: impl Into<String>) -> Self {
        Self {
            backend,
            prefix: prefix.into(),
        }
    }

    /// Create a wiki store backed by the repository storage facade.
    #[must_use]
    pub fn from_storage_provider(
        storage: Arc<dyn StorageProvider>,
        prefix: impl Into<String>,
    ) -> Self {
        Self::new(Arc::new(StorageProviderWikiBackend { storage }), prefix)
    }

    /// Return the deterministic object key for a global wiki file.
    pub fn global_file_key(&self, file: &str) -> Result<String, StorageError> {
        validate_markdown_file_name(file, "global wiki file")?;
        Ok(wiki_global_key(&self.prefix, file))
    }

    /// Return the deterministic object key for a context wiki core file.
    pub fn context_file_key(&self, context_id: &str, file: &str) -> Result<String, StorageError> {
        validate_context_id(context_id)?;
        validate_markdown_file_name(file, "context wiki file")?;
        Ok(wiki_context_key(&self.prefix, context_id, file))
    }

    /// Return the deterministic object key for a context wiki topic page.
    pub fn context_page_key(&self, context_id: &str, slug: &str) -> Result<String, StorageError> {
        validate_context_id(context_id)?;
        validate_slug(slug, "context wiki page slug")?;
        Ok(wiki_context_page_key(&self.prefix, context_id, slug))
    }

    /// Return the deterministic object key for a context wiki inbox item.
    pub fn context_inbox_key(
        &self,
        context_id: &str,
        item_slug: &str,
    ) -> Result<String, StorageError> {
        validate_context_id(context_id)?;
        validate_slug(item_slug, "context wiki inbox slug")?;
        Ok(wiki_context_inbox_key(&self.prefix, context_id, item_slug))
    }

    /// Return the deterministic object key for a context wiki raw archive item.
    pub fn context_raw_key(
        &self,
        context_id: &str,
        yyyy_mm: &str,
        run_id: &str,
    ) -> Result<String, StorageError> {
        validate_context_id(context_id)?;
        validate_year_month(yyyy_mm)?;
        validate_slug(run_id, "context wiki raw run id")?;
        Ok(wiki_context_raw_key(
            &self.prefix,
            context_id,
            yyyy_mm,
            run_id,
        ))
    }

    /// Read a file from the per-user global wiki namespace.
    pub async fn read_global_file(&self, file: &str) -> Result<Option<WikiPage>, StorageError> {
        self.read_key(self.global_file_key(file)?).await
    }

    /// Write a file to the per-user global wiki namespace.
    pub async fn put_global_file(&self, file: &str, content: &str) -> Result<(), StorageError> {
        self.put_key(&self.global_file_key(file)?, content).await
    }

    /// Read a core file from a context wiki namespace.
    pub async fn read_context_file(
        &self,
        context_id: &str,
        file: &str,
    ) -> Result<Option<WikiPage>, StorageError> {
        self.read_key(self.context_file_key(context_id, file)?)
            .await
    }

    /// Write a core file to a context wiki namespace.
    pub async fn put_context_file(
        &self,
        context_id: &str,
        file: &str,
        content: &str,
    ) -> Result<(), StorageError> {
        self.put_key(&self.context_file_key(context_id, file)?, content)
            .await
    }

    /// Read a topic page from a context wiki namespace.
    pub async fn read_context_page(
        &self,
        context_id: &str,
        slug: &str,
    ) -> Result<Option<WikiPage>, StorageError> {
        self.read_key(self.context_page_key(context_id, slug)?)
            .await
    }

    /// Write a topic page to a context wiki namespace.
    pub async fn put_context_page(
        &self,
        context_id: &str,
        slug: &str,
        content: &str,
    ) -> Result<(), StorageError> {
        self.put_key(&self.context_page_key(context_id, slug)?, content)
            .await
    }

    /// Delete a topic page from a context wiki namespace.
    pub async fn delete_context_page(
        &self,
        context_id: &str,
        slug: &str,
    ) -> Result<(), StorageError> {
        self.delete_key(&self.context_page_key(context_id, slug)?)
            .await
    }

    /// Read an inbox item from a context wiki namespace.
    pub async fn read_context_inbox_item(
        &self,
        context_id: &str,
        item_slug: &str,
    ) -> Result<Option<WikiPage>, StorageError> {
        self.read_key(self.context_inbox_key(context_id, item_slug)?)
            .await
    }

    /// Write an inbox item to a context wiki namespace.
    pub async fn put_context_inbox_item(
        &self,
        context_id: &str,
        item_slug: &str,
        content: &str,
    ) -> Result<(), StorageError> {
        self.put_key(&self.context_inbox_key(context_id, item_slug)?, content)
            .await
    }

    /// Delete an inbox item from a context wiki namespace.
    pub async fn delete_context_inbox_item(
        &self,
        context_id: &str,
        item_slug: &str,
    ) -> Result<(), StorageError> {
        self.delete_key(&self.context_inbox_key(context_id, item_slug)?)
            .await
    }

    /// Read an optional immutable raw archive item from a context wiki namespace.
    pub async fn read_context_raw_item(
        &self,
        context_id: &str,
        yyyy_mm: &str,
        run_id: &str,
    ) -> Result<Option<WikiPage>, StorageError> {
        self.read_key(self.context_raw_key(context_id, yyyy_mm, run_id)?)
            .await
    }

    /// Write an optional immutable raw archive item to a context wiki namespace.
    pub async fn put_context_raw_item(
        &self,
        context_id: &str,
        yyyy_mm: &str,
        run_id: &str,
        content: &str,
    ) -> Result<(), StorageError> {
        self.put_key(&self.context_raw_key(context_id, yyyy_mm, run_id)?, content)
            .await
    }

    async fn read_key(&self, key: String) -> Result<Option<WikiPage>, StorageError> {
        self.backend.get_text(&key).await.map(|content| {
            content.map(|content| WikiPage {
                key,
                content_hash: wiki_content_hash(&content),
                content,
            })
        })
    }

    async fn put_key(&self, key: &str, content: &str) -> Result<(), StorageError> {
        self.backend.put_text(key, content).await
    }

    async fn delete_key(&self, key: &str) -> Result<(), StorageError> {
        self.backend.delete_text(key).await
    }

    pub(crate) async fn put_validated_key(
        &self,
        key: &str,
        content: &str,
    ) -> Result<(), StorageError> {
        self.put_key(key, content).await
    }
}

fn validate_markdown_file_name(value: &str, label: &str) -> Result<(), StorageError> {
    validate_safe_segment(value, label)?;
    if !value.ends_with(".md") {
        return Err(StorageError::InvalidInput(format!(
            "{label} must be a Markdown file"
        )));
    }
    Ok(())
}

fn validate_context_id(value: &str) -> Result<(), StorageError> {
    validate_safe_segment(value, "wiki context id")
}

fn validate_slug(value: &str, label: &str) -> Result<(), StorageError> {
    validate_safe_segment(value, label)?;
    if value.ends_with(".md") {
        return Err(StorageError::InvalidInput(format!(
            "{label} must not include the .md extension"
        )));
    }
    Ok(())
}

fn validate_year_month(value: &str) -> Result<(), StorageError> {
    let valid = value.len() == 7
        && value.as_bytes().get(4) == Some(&b'-')
        && value
            .bytes()
            .enumerate()
            .all(|(idx, byte)| idx == 4 || byte.is_ascii_digit());

    if valid {
        Ok(())
    } else {
        Err(StorageError::InvalidInput(
            "context wiki raw archive month must use yyyy-mm".to_string(),
        ))
    }
}

fn validate_safe_segment(value: &str, label: &str) -> Result<(), StorageError> {
    if value.is_empty()
        || value.contains("..")
        || value.contains('/')
        || value.contains('\\')
        || value.contains(':')
        || value.chars().any(char::is_control)
    {
        return Err(StorageError::InvalidInput(format!(
            "{label} contains unsafe path characters"
        )));
    }

    Ok(())
}

/// Compute the stable SHA-256 content hash for a wiki page.
#[must_use]
pub fn wiki_content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tokio::sync::Mutex;

    #[derive(Default)]
    struct InMemoryWikiBackend {
        objects: Mutex<HashMap<String, String>>,
        get_keys: Mutex<Vec<String>>,
        put_keys: Mutex<Vec<String>>,
        delete_keys: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl WikiObjectBackend for InMemoryWikiBackend {
        async fn get_text(&self, key: &str) -> Result<Option<String>, StorageError> {
            self.get_keys.lock().await.push(key.to_string());
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

        async fn delete_text(&self, key: &str) -> Result<(), StorageError> {
            self.delete_keys.lock().await.push(key.to_string());
            self.objects.lock().await.remove(key);
            Ok(())
        }
    }

    #[tokio::test]
    async fn wiki_store_reads_deterministic_global_key() {
        let backend = Arc::new(InMemoryWikiBackend::default());
        backend.objects.lock().await.insert(
            "prod/wiki/v1/global/index.md".to_string(),
            "# Index".to_string(),
        );
        let store_backend = Arc::clone(&backend);
        let store = WikiStore::new(store_backend, "prod");

        let page = store
            .read_global_file("index.md")
            .await
            .expect("read should succeed")
            .expect("page should exist");

        assert_eq!(page.key, "prod/wiki/v1/global/index.md");
        assert_eq!(page.content, "# Index");
        assert_eq!(page.content_hash, wiki_content_hash("# Index"));
        assert_eq!(
            *backend.get_keys.lock().await,
            vec!["prod/wiki/v1/global/index.md".to_string()]
        );
    }

    #[tokio::test]
    async fn wiki_store_writes_deterministic_context_page_key() {
        let backend = Arc::new(InMemoryWikiBackend::default());
        let store_backend = Arc::clone(&backend);
        let store = WikiStore::new(store_backend, "prod");

        store
            .put_context_page("ctx-12345678", "deploy-runbook", "# Deploy")
            .await
            .expect("write should succeed");

        assert_eq!(
            backend
                .objects
                .lock()
                .await
                .get("prod/wiki/v1/contexts/ctx-12345678/pages/deploy-runbook.md")
                .cloned(),
            Some("# Deploy".to_string())
        );
        assert_eq!(
            *backend.put_keys.lock().await,
            vec!["prod/wiki/v1/contexts/ctx-12345678/pages/deploy-runbook.md".to_string()]
        );
    }

    #[tokio::test]
    async fn wiki_store_missing_page_returns_none_without_discovery() {
        let backend = Arc::new(InMemoryWikiBackend::default());
        let store_backend = Arc::clone(&backend);
        let store = WikiStore::new(store_backend, "prod");

        let page = store
            .read_context_file("ctx-12345678", "overview.md")
            .await
            .expect("read should succeed");

        assert!(page.is_none());
        assert_eq!(
            *backend.get_keys.lock().await,
            vec!["prod/wiki/v1/contexts/ctx-12345678/overview.md".to_string()]
        );
    }

    #[tokio::test]
    async fn wiki_store_rejects_traversal_before_backend_read() {
        let backend = Arc::new(InMemoryWikiBackend::default());
        let store_backend = Arc::clone(&backend);
        let store = WikiStore::new(store_backend, "prod");

        let result = store.read_context_page("ctx-12345678", "../secrets").await;

        assert!(matches!(result, Err(StorageError::InvalidInput(_))));
        assert!(backend.get_keys.lock().await.is_empty());
    }

    #[tokio::test]
    async fn wiki_store_rejects_page_slug_with_extension() {
        let backend = Arc::new(InMemoryWikiBackend::default());
        let store_backend = Arc::clone(&backend);
        let store = WikiStore::new(store_backend, "prod");

        let result = store
            .put_context_page("ctx-12345678", "deploy-runbook.md", "# Deploy")
            .await;

        assert!(matches!(result, Err(StorageError::InvalidInput(_))));
        assert!(backend.put_keys.lock().await.is_empty());
    }

    #[tokio::test]
    async fn wiki_store_deletes_deterministic_context_inbox_key() {
        let backend = Arc::new(InMemoryWikiBackend::default());
        backend.objects.lock().await.insert(
            "prod/wiki/v1/contexts/ctx-12345678/inbox/price-note.md".to_string(),
            "# Price note".to_string(),
        );
        let store_backend = Arc::clone(&backend);
        let store = WikiStore::new(store_backend, "prod");

        store
            .delete_context_inbox_item("ctx-12345678", "price-note")
            .await
            .expect("delete should succeed");

        assert_eq!(
            *backend.delete_keys.lock().await,
            vec!["prod/wiki/v1/contexts/ctx-12345678/inbox/price-note.md".to_string()]
        );
        assert!(!backend
            .objects
            .lock()
            .await
            .contains_key("prod/wiki/v1/contexts/ctx-12345678/inbox/price-note.md"));
    }
}
