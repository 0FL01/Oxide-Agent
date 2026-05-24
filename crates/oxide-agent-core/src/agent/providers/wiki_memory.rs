//! Lightweight tools for listing, reading, and deleting scoped durable wiki memory.

use crate::agent::provider::ToolProvider;
use crate::agent::tool_runtime::{
    OutputNormalizer, ToolExecutor, ToolInvocation, ToolName, ToolOutput, ToolRuntimeConfig,
    ToolRuntimeError,
};
use crate::agent::wiki_memory::{wiki_context_id, WikiPage, WikiStore};
use crate::llm::ToolDefinition;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

const TOOL_WIKI_MEMORY_LIST: &str = "wiki_memory_list";
const TOOL_WIKI_MEMORY_READ: &str = "wiki_memory_read";
const TOOL_WIKI_MEMORY_DELETE: &str = "wiki_memory_delete";
const DEFAULT_LIST_LIMIT: usize = 20;
const MAX_LIST_LIMIT: usize = 100;
const CORE_FILES: &[(&str, &str)] = &[
    ("overview", "overview.md"),
    ("decisions", "decisions.md"),
    ("constraints", "constraints.md"),
    ("procedures", "procedures.md"),
    ("open-questions", "open-questions.md"),
];

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
enum WikiMemoryListKind {
    #[default]
    All,
    Page,
    Inbox,
    Core,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WikiMemoryListArgs {
    #[serde(default)]
    kind: WikiMemoryListKind,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WikiMemoryReadArgs {
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WikiMemoryDeleteArgs {
    id: String,
    #[serde(default)]
    expected_hash: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WikiEntryKind {
    Page,
    Inbox,
    Core,
}

impl WikiEntryKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Inbox => "inbox",
            Self::Core => "core",
        }
    }

    const fn matches_filter(self, filter: WikiMemoryListKind) -> bool {
        match filter {
            WikiMemoryListKind::All => true,
            WikiMemoryListKind::Page => matches!(self, Self::Page),
            WikiMemoryListKind::Inbox => matches!(self, Self::Inbox),
            WikiMemoryListKind::Core => matches!(self, Self::Core),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WikiIndexEntry {
    id: String,
    kind: WikiEntryKind,
    slug: String,
    path: String,
    title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WikiMemoryTarget {
    Page { slug: String, path: String },
    Inbox { slug: String, path: String },
    Core { slug: String, file: &'static str },
}

impl WikiMemoryTarget {
    fn parse(id: &str) -> Result<Self> {
        let trimmed = id.trim();
        let (kind, value) = trimmed
            .split_once(':')
            .ok_or_else(|| anyhow!("wiki memory id must look like kind:value"))?;
        let value = value.trim();
        if value.is_empty() {
            return Err(anyhow!("wiki memory id value must not be empty"));
        }

        match kind.trim() {
            "page" => Ok(Self::Page {
                slug: value.to_string(),
                path: format!("pages/{value}.md"),
            }),
            "inbox" => Ok(Self::Inbox {
                slug: value.to_string(),
                path: format!("inbox/{value}.md"),
            }),
            "core" => {
                let (_, file) = CORE_FILES
                    .iter()
                    .find(|(slug, _)| *slug == value)
                    .ok_or_else(|| anyhow!("unsupported core wiki memory id: {value}"))?;
                Ok(Self::Core {
                    slug: value.to_string(),
                    file,
                })
            }
            _ => Err(anyhow!("unsupported wiki memory kind: {kind}")),
        }
    }

    fn id(&self) -> String {
        match self {
            Self::Page { slug, .. } => format!("page:{slug}"),
            Self::Inbox { slug, .. } => format!("inbox:{slug}"),
            Self::Core { slug, .. } => format!("core:{slug}"),
        }
    }

    const fn kind(&self) -> WikiEntryKind {
        match self {
            Self::Page { .. } => WikiEntryKind::Page,
            Self::Inbox { .. } => WikiEntryKind::Inbox,
            Self::Core { .. } => WikiEntryKind::Core,
        }
    }

    fn path(&self) -> &str {
        match self {
            Self::Page { path, .. } | Self::Inbox { path, .. } => path,
            Self::Core { file, .. } => file,
        }
    }
}

/// Tool provider that exposes scoped durable wiki memory management.
pub struct WikiMemoryProvider {
    store: WikiStore,
    user_id: i64,
    context_key: String,
}

impl WikiMemoryProvider {
    /// Create a provider bound to the current user and durable wiki context.
    #[must_use]
    pub fn new(store: WikiStore, user_id: i64, context_key: String) -> Self {
        Self {
            store,
            user_id,
            context_key,
        }
    }

    /// Build native typed runtime executors for wiki memory tools.
    #[must_use]
    pub fn tool_runtime_executors(self: &Arc<Self>) -> Vec<Arc<dyn ToolExecutor>> {
        let execution_lock = Arc::new(Mutex::new(()));
        Self::tool_definitions()
            .into_iter()
            .map(|spec| {
                Arc::new(WikiMemoryToolExecutor {
                    provider: Arc::clone(self),
                    name: ToolName::from(spec.name.clone()),
                    spec,
                    execution_lock: Arc::clone(&execution_lock),
                }) as Arc<dyn ToolExecutor>
            })
            .collect()
    }

    fn tool_definitions() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: TOOL_WIKI_MEMORY_LIST.to_string(),
                description: "List scoped durable wiki memory items from the current context"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "kind": {
                            "type": "string",
                            "enum": ["all", "page", "inbox", "core"],
                            "description": "Optional filter by wiki memory item kind"
                        },
                        "query": {
                            "type": "string",
                            "description": "Optional case-insensitive substring filter against id, title, slug, and path"
                        },
                        "limit": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": MAX_LIST_LIMIT,
                            "description": "Maximum number of items to return"
                        }
                    },
                    "additionalProperties": false
                }),
            },
            ToolDefinition {
                name: TOOL_WIKI_MEMORY_READ.to_string(),
                description:
                    "Read one scoped durable wiki memory item by id such as page:slug or inbox:slug"
                        .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "Wiki memory id. Supported forms: page:<slug>, inbox:<slug>, core:<name>"
                        }
                    },
                    "required": ["id"],
                    "additionalProperties": false
                }),
            },
            ToolDefinition {
                name: TOOL_WIKI_MEMORY_DELETE.to_string(),
                description: "Delete one scoped durable wiki memory page or inbox item by id"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "Wiki memory id. Supported forms: page:<slug> or inbox:<slug>"
                        },
                        "expected_hash": {
                            "type": "string",
                            "description": "Optional optimistic concurrency hash from a previous read"
                        },
                        "dry_run": {
                            "type": "boolean",
                            "description": "Preview deletion without persisting"
                        }
                    },
                    "required": ["id"],
                    "additionalProperties": false
                }),
            },
        ]
    }

    fn parse_args<T: for<'de> Deserialize<'de>>(arguments: &str, tool_name: &str) -> Result<T> {
        serde_json::from_str(arguments)
            .map_err(|error| anyhow!("invalid arguments for {tool_name}: {error}"))
    }

    fn context_id(&self) -> String {
        wiki_context_id(self.user_id, &self.context_key)
    }

    async fn load_index_entries(&self, context_id: &str) -> Result<Vec<WikiIndexEntry>> {
        let Some(index) = self.store.read_context_file(context_id, "index.md").await? else {
            return Ok(Vec::new());
        };

        let mut entries = Vec::new();
        for line in index.content.lines() {
            let Some(path) = markdown_link_path(line) else {
                continue;
            };
            let Some(title) = markdown_link_text(line) else {
                continue;
            };
            let path = path.to_string();
            let title = title.trim().to_string();
            if let Some(slug) = path
                .strip_prefix("pages/")
                .and_then(|value| value.strip_suffix(".md"))
            {
                entries.push(WikiIndexEntry {
                    id: format!("page:{slug}"),
                    kind: WikiEntryKind::Page,
                    slug: slug.to_string(),
                    path,
                    title,
                });
                continue;
            }
            if let Some(slug) = path
                .strip_prefix("inbox/")
                .and_then(|value| value.strip_suffix(".md"))
            {
                entries.push(WikiIndexEntry {
                    id: format!("inbox:{slug}"),
                    kind: WikiEntryKind::Inbox,
                    slug: slug.to_string(),
                    path,
                    title,
                });
                continue;
            }
            if let Some((slug, _)) = CORE_FILES.iter().find(|(_, file)| *file == path) {
                entries.push(WikiIndexEntry {
                    id: format!("core:{slug}"),
                    kind: WikiEntryKind::Core,
                    slug: (*slug).to_string(),
                    path,
                    title,
                });
            }
        }
        Ok(entries)
    }

    async fn read_target(
        &self,
        context_id: &str,
        target: &WikiMemoryTarget,
    ) -> Result<Option<WikiPage>> {
        match target {
            WikiMemoryTarget::Page { slug, .. } => {
                Ok(self.store.read_context_page(context_id, slug).await?)
            }
            WikiMemoryTarget::Inbox { slug, .. } => {
                Ok(self.store.read_context_inbox_item(context_id, slug).await?)
            }
            WikiMemoryTarget::Core { file, .. } => {
                Ok(self.store.read_context_file(context_id, file).await?)
            }
        }
    }

    async fn execute_list(&self, arguments: &str) -> Result<String> {
        let args: WikiMemoryListArgs = Self::parse_args(arguments, TOOL_WIKI_MEMORY_LIST)?;
        let context_id = self.context_id();
        let query = args
            .query
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let query = query.map(str::to_lowercase);
        let limit = args
            .limit
            .unwrap_or(DEFAULT_LIST_LIMIT)
            .clamp(1, MAX_LIST_LIMIT);
        let entries = self.load_index_entries(&context_id).await?;

        let items = entries
            .into_iter()
            .filter(|entry| entry.kind.matches_filter(args.kind))
            .filter(|entry| match query.as_ref() {
                Some(query) => {
                    let haystack = format!(
                        "{}\n{}\n{}\n{}",
                        entry.id, entry.title, entry.slug, entry.path
                    )
                    .to_lowercase();
                    haystack.contains(query)
                }
                None => true,
            })
            .take(limit)
            .map(|entry| {
                json!({
                    "id": entry.id,
                    "kind": entry.kind.as_str(),
                    "slug": entry.slug,
                    "path": entry.path,
                    "title": entry.title,
                })
            })
            .collect::<Vec<_>>();

        Ok(json!({
            "ok": true,
            "context_id": context_id,
            "count": items.len(),
            "items": items,
        })
        .to_string())
    }

    async fn execute_read(&self, arguments: &str) -> Result<String> {
        let args: WikiMemoryReadArgs = Self::parse_args(arguments, TOOL_WIKI_MEMORY_READ)?;
        let context_id = self.context_id();
        let target = WikiMemoryTarget::parse(&args.id)?;
        let page = self.read_target(&context_id, &target).await?;

        Ok(match page {
            Some(page) => json!({
                "ok": true,
                "found": true,
                "context_id": context_id,
                "id": target.id(),
                "kind": target.kind().as_str(),
                "path": target.path(),
                "key": page.key,
                "content_hash": page.content_hash,
                "content": page.content,
            }),
            None => json!({
                "ok": true,
                "found": false,
                "context_id": context_id,
                "id": target.id(),
                "kind": target.kind().as_str(),
                "path": target.path(),
            }),
        }
        .to_string())
    }

    async fn execute_delete(&self, arguments: &str) -> Result<String> {
        let args: WikiMemoryDeleteArgs = Self::parse_args(arguments, TOOL_WIKI_MEMORY_DELETE)?;
        let context_id = self.context_id();
        let target = WikiMemoryTarget::parse(&args.id)?;

        let (deleted, previous_hash, key, metadata_path) = match &target {
            WikiMemoryTarget::Page { slug, path } => {
                let existing = self.store.read_context_page(&context_id, slug).await?;
                let previous_hash = existing.as_ref().map(|page| page.content_hash.clone());
                if let (Some(expected), Some(actual)) =
                    (args.expected_hash.as_deref(), previous_hash.as_deref())
                {
                    if expected != actual {
                        return Err(anyhow!(
                            "wiki memory hash mismatch for {}: expected {}, actual {}",
                            target.id(),
                            expected,
                            actual
                        ));
                    }
                }
                let key = existing
                    .as_ref()
                    .map(|page| page.key.clone())
                    .or_else(|| self.store.context_page_key(&context_id, slug).ok());
                if !args.dry_run && existing.is_some() {
                    self.store.delete_context_page(&context_id, slug).await?;
                }
                (existing.is_some(), previous_hash, key, path.clone())
            }
            WikiMemoryTarget::Inbox { slug, path } => {
                let existing = self
                    .store
                    .read_context_inbox_item(&context_id, slug)
                    .await?;
                let previous_hash = existing.as_ref().map(|page| page.content_hash.clone());
                if let (Some(expected), Some(actual)) =
                    (args.expected_hash.as_deref(), previous_hash.as_deref())
                {
                    if expected != actual {
                        return Err(anyhow!(
                            "wiki memory hash mismatch for {}: expected {}, actual {}",
                            target.id(),
                            expected,
                            actual
                        ));
                    }
                }
                let key = existing
                    .as_ref()
                    .map(|page| page.key.clone())
                    .or_else(|| self.store.context_inbox_key(&context_id, slug).ok());
                if !args.dry_run && existing.is_some() {
                    self.store
                        .delete_context_inbox_item(&context_id, slug)
                        .await?;
                }
                (existing.is_some(), previous_hash, key, path.clone())
            }
            WikiMemoryTarget::Core { .. } => {
                return Err(anyhow!(
                    "core wiki files are read-only; delete page:* or inbox:* items instead"
                ));
            }
        };

        let index_page = self
            .store
            .read_context_file(&context_id, "index.md")
            .await?;
        let cleaned_index = index_page
            .as_ref()
            .map(|page| remove_index_entry(&page.content, &metadata_path))
            .filter(|updated| {
                index_page
                    .as_ref()
                    .map(|page| updated != &page.content)
                    .unwrap_or(false)
            });

        if !args.dry_run {
            if let Some(updated_index) = cleaned_index.as_ref() {
                self.store
                    .put_context_file(&context_id, "index.md", updated_index)
                    .await?;
            }
            if deleted || cleaned_index.is_some() {
                let existing_log = self.store.read_context_file(&context_id, "log.md").await?;
                let updated_log = append_delete_log_entry(
                    existing_log.as_ref().map(|page| page.content.as_str()),
                    &context_id,
                    &metadata_path,
                );
                self.store
                    .put_context_file(&context_id, "log.md", &updated_log)
                    .await?;
            }
        }

        Ok(json!({
            "ok": true,
            "dry_run": args.dry_run,
            "context_id": context_id,
            "id": target.id(),
            "kind": target.kind().as_str(),
            "path": target.path(),
            "key": key,
            "found": deleted || cleaned_index.is_some(),
            "deleted": deleted,
            "previous_hash": previous_hash,
            "cleaned_index": cleaned_index.is_some(),
        })
        .to_string())
    }

    async fn execute_tool(&self, tool_name: &str, arguments: &str) -> Result<String> {
        match tool_name {
            TOOL_WIKI_MEMORY_LIST => self.execute_list(arguments).await,
            TOOL_WIKI_MEMORY_READ => self.execute_read(arguments).await,
            TOOL_WIKI_MEMORY_DELETE => self.execute_delete(arguments).await,
            _ => Err(anyhow!("unknown wiki memory tool: {tool_name}")),
        }
    }
}

#[async_trait]
impl ToolProvider for WikiMemoryProvider {
    fn name(&self) -> &'static str {
        "wiki_memory"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        Self::tool_definitions()
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        matches!(
            tool_name,
            TOOL_WIKI_MEMORY_LIST | TOOL_WIKI_MEMORY_READ | TOOL_WIKI_MEMORY_DELETE
        )
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        _progress_tx: Option<&tokio::sync::mpsc::Sender<crate::agent::progress::AgentEvent>>,
        _cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        self.execute_tool(tool_name, arguments).await
    }
}

struct WikiMemoryToolExecutor {
    provider: Arc<WikiMemoryProvider>,
    name: ToolName,
    spec: ToolDefinition,
    execution_lock: Arc<Mutex<()>>,
}

#[async_trait]
impl ToolExecutor for WikiMemoryToolExecutor {
    fn name(&self) -> ToolName {
        self.name.clone()
    }

    fn spec(&self) -> ToolDefinition {
        self.spec.clone()
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        let _guard = self.execution_lock.lock().await;
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig {
            timeout: invocation.timeout.clone(),
            artifact_dir: invocation.execution_context.artifact_dir.clone(),
            ..ToolRuntimeConfig::default()
        });
        self.provider
            .execute_tool(self.name.as_str(), &invocation.raw_arguments)
            .await
            .map(|output| normalizer.success(&invocation, &output, ""))
            .map_err(|error| ToolRuntimeError::Failure(error.to_string()))
    }
}

fn markdown_link_text(line: &str) -> Option<&str> {
    let start = line.find('[')? + 1;
    let end = line[start..].find(']')? + start;
    Some(&line[start..end])
}

fn markdown_link_path(line: &str) -> Option<&str> {
    let start = line.find("](")? + 2;
    let rest = &line[start..];
    let end = rest.find(')')?;
    Some(&rest[..end])
}

fn remove_index_entry(existing: &str, path: &str) -> String {
    let mut lines = Vec::new();
    let mut skipping_indented = false;
    for line in existing.lines() {
        if line.contains(&format!("]({path})")) {
            skipping_indented = true;
            continue;
        }
        if skipping_indented {
            if line.starts_with("  ") || line.starts_with('\t') {
                continue;
            }
            skipping_indented = false;
        }
        lines.push(line);
    }

    let mut content = lines.join("\n");
    if !content.ends_with('\n') {
        content.push('\n');
    }
    content
}

fn append_delete_log_entry(existing: Option<&str>, context_id: &str, path: &str) -> String {
    let base = existing
        .map(str::trim_end)
        .filter(|value| !value.is_empty())
        .map_or_else(
            || format!("# Context Wiki Log\n\nContext ID: {context_id}"),
            ToOwned::to_owned,
        );
    format!(
        "{base}\n- {} source=tool:wiki_memory_delete reason=\"deleted wiki memory item\" changed={}\n",
        Utc::now().to_rfc3339(),
        path,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::identity::SessionId;
    use crate::agent::tool_runtime::{
        ModelMetadata, ProviderMetadata, ToolBatchId, ToolCallId, ToolExecutionContext,
        ToolOutputStatus, ToolTimeoutConfig, TurnId,
    };
    use crate::agent::wiki_memory::WikiObjectBackend;
    use crate::llm::InvocationId;
    use crate::storage::StorageError;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    #[derive(Default)]
    struct InMemoryWikiBackend {
        objects: Mutex<HashMap<String, String>>,
        delete_keys: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl WikiObjectBackend for InMemoryWikiBackend {
        async fn get_text(&self, key: &str) -> Result<Option<String>, StorageError> {
            Ok(self.objects.lock().await.get(key).cloned())
        }

        async fn put_text(&self, key: &str, content: &str) -> Result<(), StorageError> {
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

    fn provider(backend: Arc<InMemoryWikiBackend>) -> WikiMemoryProvider {
        WikiMemoryProvider::new(WikiStore::new(backend, "prod"), 7, "topic-a".to_string())
    }

    fn context_id() -> String {
        wiki_context_id(7, "topic-a")
    }

    fn runtime_invocation(tool_name: &str, raw_arguments: &str) -> ToolInvocation {
        let now = Utc::now();
        ToolInvocation {
            session_id: SessionId::from(7),
            turn_id: TurnId::from("turn-wiki-memory"),
            batch_id: ToolBatchId::from("batch-wiki-memory"),
            batch_index: 0,
            invocation_id: InvocationId::from(format!("invoke-{tool_name}")),
            tool_call_id: ToolCallId::from(format!("call-{tool_name}")),
            provider_tool_call_id: None,
            tool_name: ToolName::from(tool_name),
            raw_provider_payload: json!({}),
            raw_arguments: raw_arguments.to_string(),
            normalized_arguments: serde_json::Value::Null,
            cancellation_token: CancellationToken::new(),
            timeout: ToolTimeoutConfig::default(),
            execution_context: ToolExecutionContext::new(std::env::temp_dir()),
            provider_metadata: ProviderMetadata {
                provider: "test".to_string(),
                protocol: "chat_like".to_string(),
            },
            model_metadata: ModelMetadata {
                model: "test-model".to_string(),
            },
            working_directory: None,
            environment_metadata: None,
            created_at: now,
            started_at: Some(now),
        }
    }

    #[tokio::test]
    async fn list_returns_indexed_items() {
        let backend = Arc::new(InMemoryWikiBackend::default());
        let context_id = context_id();
        backend.objects.lock().await.insert(
            format!("prod/wiki/v1/contexts/{context_id}/index.md"),
            "# Wiki Index\n\n## Core pages\n\n- [overview](overview.md) - current project overview\n\n## Topic pages\n\n- [btc price](pages/btc-price.md) - captured wiki update candidate\n\n## Inbox\n\n- [btc reminder](inbox/btc-reminder.md) - captured wiki update candidate\n"
                .to_string(),
        );
        let provider = provider(backend);

        let result = provider
            .execute(TOOL_WIKI_MEMORY_LIST, r#"{"kind":"all"}"#, None, None)
            .await
            .expect("list should succeed");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("valid json");

        assert_eq!(parsed["count"], 3);
        assert_eq!(parsed["items"][0]["id"], "core:overview");
        assert_eq!(parsed["items"][1]["id"], "page:btc-price");
        assert_eq!(parsed["items"][2]["id"], "inbox:btc-reminder");
    }

    #[tokio::test]
    async fn read_returns_page_content_and_hash() {
        let backend = Arc::new(InMemoryWikiBackend::default());
        let context_id = context_id();
        backend.objects.lock().await.insert(
            format!("prod/wiki/v1/contexts/{context_id}/pages/btc-price.md"),
            "---\ntitle: BTC price\ntype: note\nupdated_at: 2026-05-19T00:00:00Z\nconfidence: high\ntags: [btc]\nsources:\n  - run:test\n---\n\n76 840.69"
                .to_string(),
        );
        let provider = provider(backend);

        let result = provider
            .execute(
                TOOL_WIKI_MEMORY_READ,
                r#"{"id":"page:btc-price"}"#,
                None,
                None,
            )
            .await
            .expect("read should succeed");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("valid json");

        assert_eq!(parsed["found"], true);
        assert_eq!(parsed["id"], "page:btc-price");
        assert!(parsed["content"]
            .as_str()
            .expect("content string")
            .contains("76 840.69"));
        assert!(parsed["content_hash"].as_str().is_some());
    }

    #[tokio::test]
    async fn typed_runtime_executor_reads_page_content() {
        let backend = Arc::new(InMemoryWikiBackend::default());
        let context_id = context_id();
        backend.objects.lock().await.insert(
            format!("prod/wiki/v1/contexts/{context_id}/pages/btc-price.md"),
            "# BTC price\n\n76 840.69".to_string(),
        );
        let provider = Arc::new(provider(backend));
        let executor = provider
            .tool_runtime_executors()
            .into_iter()
            .find(|executor| executor.name().as_str() == TOOL_WIKI_MEMORY_READ)
            .expect("typed wiki memory read executor registered");

        let output = executor
            .execute(runtime_invocation(
                TOOL_WIKI_MEMORY_READ,
                r#"{"id":"page:btc-price"}"#,
            ))
            .await
            .expect("typed wiki memory read succeeds");

        assert_eq!(output.status, ToolOutputStatus::Success);
        assert!(output
            .stdout
            .text
            .as_deref()
            .expect("stdout text")
            .contains("76 840.69"));
    }

    #[tokio::test]
    async fn delete_removes_page_and_cleans_index_and_logs() {
        let backend = Arc::new(InMemoryWikiBackend::default());
        let context_id = context_id();
        let page_key = format!("prod/wiki/v1/contexts/{context_id}/pages/btc-price.md");
        backend
            .objects
            .lock()
            .await
            .insert(page_key.clone(), "# BTC price\n\n76 840.69".to_string());
        backend.objects.lock().await.insert(
            format!("prod/wiki/v1/contexts/{context_id}/index.md"),
            "# Wiki Index\n\n## Topic pages\n\n- [btc price](pages/btc-price.md) - captured wiki update candidate\n"
                .to_string(),
        );
        let provider = provider(Arc::clone(&backend));

        let result = provider
            .execute(
                TOOL_WIKI_MEMORY_DELETE,
                r#"{"id":"page:btc-price"}"#,
                None,
                None,
            )
            .await
            .expect("delete should succeed");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("valid json");
        let objects = backend.objects.lock().await;
        let index = objects
            .get(&format!("prod/wiki/v1/contexts/{context_id}/index.md"))
            .expect("index present");
        let log = objects
            .get(&format!("prod/wiki/v1/contexts/{context_id}/log.md"))
            .expect("log present");

        assert_eq!(parsed["deleted"], true);
        assert_eq!(parsed["cleaned_index"], true);
        assert!(!objects.contains_key(&page_key));
        assert!(!index.contains("pages/btc-price.md"));
        assert!(log.contains("source=tool:wiki_memory_delete"));
        drop(objects);
        assert_eq!(backend.delete_keys.lock().await.as_slice(), [page_key]);
    }

    #[tokio::test]
    async fn delete_rejects_core_files() {
        let backend = Arc::new(InMemoryWikiBackend::default());
        let provider = provider(backend);

        let error = provider
            .execute(
                TOOL_WIKI_MEMORY_DELETE,
                r#"{"id":"core:overview"}"#,
                None,
                None,
            )
            .await
            .expect_err("core delete must fail");

        assert!(error.to_string().contains("read-only"));
    }
}
