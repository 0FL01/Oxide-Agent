//! Lightweight tools for listing, reading, and deleting scoped durable wiki memory.

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
const TOOL_WIKI_MEMORY_SEARCH: &str = "wiki_memory_search";
const TOOL_WIKI_MEMORY_DELETE: &str = "wiki_memory_delete";
const DEFAULT_LIST_LIMIT: usize = 20;
const MAX_LIST_LIMIT: usize = 100;
const DEFAULT_READ_MAX_BYTES: usize = 4096;
const MAX_READ_MAX_BYTES: usize = 32 * 1024;
const DEFAULT_EXCERPT_BYTES: usize = 360;
const MAX_SEARCH_LIMIT: usize = 20;
const DEFAULT_SEARCH_LIMIT: usize = 8;
const DEFAULT_SEARCH_SCAN_LIMIT: usize = 40;
const MAX_SEARCH_SCAN_LIMIT: usize = 100;
const DEFAULT_SEARCH_SNIPPET_BYTES: usize = 260;
const MAX_SEARCH_SNIPPET_BYTES: usize = 800;
const DEFAULT_SEARCH_MATCHES_PER_ITEM: usize = 2;
const MAX_SEARCH_MATCHES_PER_ITEM: usize = 5;
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
    #[serde(default)]
    full: bool,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    max_bytes: Option<usize>,
    #[serde(default)]
    heading: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WikiMemorySearchArgs {
    query: String,
    #[serde(default)]
    kind: WikiMemoryListKind,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    scan_limit: Option<usize>,
    #[serde(default)]
    max_snippet_bytes: Option<usize>,
    #[serde(default)]
    max_matches_per_item: Option<usize>,
    #[serde(default = "default_search_body")]
    search_body: bool,
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct WikiPageMetadata {
    title: Option<String>,
    page_type: Option<String>,
    updated_at: Option<String>,
    confidence: Option<String>,
    tags: Vec<String>,
    summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WikiContentWindow {
    content: String,
    offset: usize,
    returned_bytes: usize,
    total_bytes: usize,
    truncated: bool,
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
                description:
                    "List scoped durable wiki memory items with bounded metadata and excerpts"
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
                            "description": "Optional case-insensitive substring filter against id, title, slug, path, metadata, and excerpt"
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
                description: "Read a bounded window of one scoped durable wiki memory item by id"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "Wiki memory id. Supported forms: page:<slug>, inbox:<slug>, core:<name>"
                        },
                        "full": {
                            "type": "boolean",
                            "description": "Return the full page content. Default false; prefer bounded reads unless the page is known to be small."
                        },
                        "offset": {
                            "type": "integer",
                            "minimum": 0,
                            "description": "Byte offset for a bounded content window"
                        },
                        "max_bytes": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": MAX_READ_MAX_BYTES,
                            "description": "Maximum content bytes to return when full is false"
                        },
                        "heading": {
                            "type": "string",
                            "description": "Optional Markdown heading text or prefix to read a section instead of the whole page"
                        }
                    },
                    "required": ["id"],
                    "additionalProperties": false
                }),
            },
            ToolDefinition {
                name: TOOL_WIKI_MEMORY_SEARCH.to_string(),
                description: "Search scoped durable wiki memory with bounded lexical snippets"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Case-insensitive lexical query"
                        },
                        "kind": {
                            "type": "string",
                            "enum": ["all", "page", "inbox", "core"],
                            "description": "Optional filter by wiki memory item kind"
                        },
                        "limit": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": MAX_SEARCH_LIMIT,
                            "description": "Maximum number of matched items to return"
                        },
                        "scan_limit": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": MAX_SEARCH_SCAN_LIMIT,
                            "description": "Maximum indexed items whose bodies may be loaded"
                        },
                        "search_body": {
                            "type": "boolean",
                            "description": "Whether to load bounded page bodies for snippet search. Default true."
                        },
                        "max_snippet_bytes": {
                            "type": "integer",
                            "minimum": 80,
                            "maximum": MAX_SEARCH_SNIPPET_BYTES,
                            "description": "Maximum bytes per returned snippet"
                        },
                        "max_matches_per_item": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": MAX_SEARCH_MATCHES_PER_ITEM,
                            "description": "Maximum snippets returned for one item"
                        }
                    },
                    "required": ["query"],
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

        let mut items = Vec::new();
        for entry in entries
            .into_iter()
            .filter(|entry| entry.kind.matches_filter(args.kind))
        {
            let item = self.enriched_index_entry(&context_id, &entry).await?;
            if let Some(query) = query.as_ref() {
                let haystack = wiki_item_search_text(&item);
                if !haystack.contains(query) {
                    continue;
                }
            }
            items.push(item);
            if items.len() >= limit {
                break;
            }
        }

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
            Some(page) => {
                let metadata = parse_page_metadata(&page.content);
                let (source_content, section_found) = match args
                    .heading
                    .as_deref()
                    .map(str::trim)
                    .filter(|heading| !heading.is_empty())
                {
                    Some(heading) => match markdown_section(&page.content, heading) {
                        Some(section) => (section.to_string(), true),
                        None => (String::new(), false),
                    },
                    None => (page.content.clone(), true),
                };
                let window = if args.full {
                    WikiContentWindow {
                        content: source_content.clone(),
                        offset: 0,
                        returned_bytes: source_content.len(),
                        total_bytes: source_content.len(),
                        truncated: false,
                    }
                } else {
                    content_window(
                        &source_content,
                        args.offset.unwrap_or(0),
                        args.max_bytes
                            .unwrap_or(DEFAULT_READ_MAX_BYTES)
                            .clamp(1, MAX_READ_MAX_BYTES),
                    )
                };
                json!({
                    "ok": true,
                    "found": true,
                    "section_found": section_found,
                    "context_id": context_id,
                    "id": target.id(),
                    "kind": target.kind().as_str(),
                    "path": target.path(),
                    "key": page.key,
                    "content_hash": page.content_hash,
                    "size_bytes": page.content.len(),
                    "metadata": metadata_json(&metadata),
                    "range": {
                        "offset": window.offset,
                        "returned_bytes": window.returned_bytes,
                        "total_bytes": window.total_bytes,
                        "truncated": window.truncated,
                    },
                    "content": window.content,
                })
            }
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

    async fn execute_search(&self, arguments: &str) -> Result<String> {
        let args: WikiMemorySearchArgs = Self::parse_args(arguments, TOOL_WIKI_MEMORY_SEARCH)?;
        let query = args.query.trim().to_lowercase();
        if query.is_empty() {
            return Err(anyhow!("wiki memory search query must not be empty"));
        }
        let context_id = self.context_id();
        let limit = args
            .limit
            .unwrap_or(DEFAULT_SEARCH_LIMIT)
            .clamp(1, MAX_SEARCH_LIMIT);
        let scan_limit = args
            .scan_limit
            .unwrap_or(DEFAULT_SEARCH_SCAN_LIMIT)
            .clamp(1, MAX_SEARCH_SCAN_LIMIT);
        let max_snippet_bytes = args
            .max_snippet_bytes
            .unwrap_or(DEFAULT_SEARCH_SNIPPET_BYTES)
            .clamp(80, MAX_SEARCH_SNIPPET_BYTES);
        let max_matches_per_item = args
            .max_matches_per_item
            .unwrap_or(DEFAULT_SEARCH_MATCHES_PER_ITEM)
            .clamp(1, MAX_SEARCH_MATCHES_PER_ITEM);
        let entries = self.load_index_entries(&context_id).await?;

        let mut scanned = 0;
        let mut matches = Vec::new();
        for entry in entries
            .into_iter()
            .filter(|entry| entry.kind.matches_filter(args.kind))
        {
            if scanned >= scan_limit || matches.len() >= limit {
                break;
            }
            scanned += 1;

            let page = if args.search_body {
                self.read_target(&context_id, &WikiMemoryTarget::parse(&entry.id)?)
                    .await?
            } else {
                None
            };
            let metadata = page
                .as_ref()
                .map(|page| parse_page_metadata(&page.content))
                .unwrap_or_default();
            let index_text = format!(
                "{}\n{}\n{}\n{}\n{}",
                entry.id,
                entry.title,
                entry.slug,
                entry.path,
                metadata_search_text(&metadata)
            )
            .to_lowercase();
            let mut snippets = page
                .as_ref()
                .map(|page| {
                    search_snippets(
                        &page.content,
                        &query,
                        max_matches_per_item,
                        max_snippet_bytes,
                    )
                })
                .unwrap_or_default();

            if snippets.is_empty() && index_text.contains(&query) {
                let text = metadata
                    .summary
                    .as_deref()
                    .filter(|summary| !summary.is_empty())
                    .unwrap_or(&entry.title);
                snippets.push(snippet_around(text, 0, max_snippet_bytes));
            }
            if snippets.is_empty() {
                continue;
            }

            matches.push(json!({
                "id": entry.id,
                "kind": entry.kind.as_str(),
                "slug": entry.slug,
                "path": entry.path,
                "title": metadata.title.as_deref().unwrap_or(&entry.title),
                "metadata": metadata_json(&metadata),
                "content_hash": page.as_ref().map(|page| page.content_hash.clone()),
                "size_bytes": page.as_ref().map(|page| page.content.len()),
                "snippets": snippets,
            }));
        }

        Ok(json!({
            "ok": true,
            "context_id": context_id,
            "query": query,
            "scanned": scanned,
            "count": matches.len(),
            "matches": matches,
        })
        .to_string())
    }

    async fn enriched_index_entry(
        &self,
        context_id: &str,
        entry: &WikiIndexEntry,
    ) -> Result<serde_json::Value> {
        let page = self
            .read_target(context_id, &WikiMemoryTarget::parse(&entry.id)?)
            .await?;
        let metadata = page
            .as_ref()
            .map(|page| parse_page_metadata(&page.content))
            .unwrap_or_default();
        let excerpt = page.as_ref().and_then(|page| {
            first_content_excerpt(&page.content, DEFAULT_EXCERPT_BYTES)
                .filter(|excerpt| !excerpt.is_empty())
        });

        Ok(json!({
            "id": entry.id,
            "kind": entry.kind.as_str(),
            "slug": entry.slug,
            "path": entry.path,
            "title": metadata.title.as_deref().unwrap_or(&entry.title),
            "index_title": entry.title,
            "found": page.is_some(),
            "content_hash": page.as_ref().map(|page| page.content_hash.clone()),
            "size_bytes": page.as_ref().map(|page| page.content.len()),
            "metadata": metadata_json(&metadata),
            "excerpt": excerpt,
        }))
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
                    && expected != actual {
                        return Err(anyhow!(
                            "wiki memory hash mismatch for {}: expected {}, actual {}",
                            target.id(),
                            expected,
                            actual
                        ));
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
                    && expected != actual {
                        return Err(anyhow!(
                            "wiki memory hash mismatch for {}: expected {}, actual {}",
                            target.id(),
                            expected,
                            actual
                        ));
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
            TOOL_WIKI_MEMORY_SEARCH => self.execute_search(arguments).await,
            TOOL_WIKI_MEMORY_DELETE => self.execute_delete(arguments).await,
            _ => Err(anyhow!("unknown wiki memory tool: {tool_name}")),
        }
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

const fn default_search_body() -> bool {
    true
}

fn metadata_json(metadata: &WikiPageMetadata) -> serde_json::Value {
    json!({
        "title": metadata.title,
        "type": metadata.page_type,
        "updated_at": metadata.updated_at,
        "confidence": metadata.confidence,
        "tags": metadata.tags,
        "summary": metadata.summary,
    })
}

fn metadata_search_text(metadata: &WikiPageMetadata) -> String {
    [
        metadata.title.as_deref().unwrap_or_default(),
        metadata.page_type.as_deref().unwrap_or_default(),
        metadata.updated_at.as_deref().unwrap_or_default(),
        metadata.confidence.as_deref().unwrap_or_default(),
        metadata.summary.as_deref().unwrap_or_default(),
        &metadata.tags.join(" "),
    ]
    .join("\n")
}

fn wiki_item_search_text(item: &serde_json::Value) -> String {
    item.to_string().to_lowercase()
}

fn parse_page_metadata(content: &str) -> WikiPageMetadata {
    let mut metadata = WikiPageMetadata {
        summary: first_content_excerpt(content, DEFAULT_EXCERPT_BYTES),
        ..WikiPageMetadata::default()
    };
    let Some(frontmatter) = frontmatter_block(content) else {
        if metadata.title.is_none() {
            metadata.title = first_markdown_heading(content).map(ToOwned::to_owned);
        }
        return metadata;
    };

    let mut in_tags = false;
    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if in_tags {
            if let Some(tag) = trimmed.strip_prefix("- ") {
                metadata.tags.push(clean_yaml_scalar(tag));
                continue;
            }
            in_tags = false;
        }

        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "title" => metadata.title = non_empty_yaml_scalar(value),
            "type" => metadata.page_type = non_empty_yaml_scalar(value),
            "updated_at" => metadata.updated_at = non_empty_yaml_scalar(value),
            "confidence" => metadata.confidence = non_empty_yaml_scalar(value),
            "tags" => {
                if value.is_empty() {
                    in_tags = true;
                } else {
                    metadata.tags.extend(parse_inline_yaml_list(value));
                }
            }
            _ => {}
        }
    }

    if metadata.title.is_none() {
        metadata.title = first_markdown_heading(content).map(ToOwned::to_owned);
    }
    metadata.tags.sort();
    metadata.tags.dedup();
    metadata
}

fn frontmatter_block(content: &str) -> Option<&str> {
    let rest = content.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    Some(&rest[..end])
}

fn body_without_frontmatter(content: &str) -> &str {
    let Some(rest) = content.strip_prefix("---\n") else {
        return content;
    };
    let Some(end) = rest.find("\n---") else {
        return content;
    };
    rest[end + 4..].trim_start_matches('\n')
}

fn first_markdown_heading(content: &str) -> Option<&str> {
    body_without_frontmatter(content).lines().find_map(|line| {
        let trimmed = line.trim();
        let heading = trimmed.trim_start_matches('#').trim();
        (trimmed.starts_with('#') && !heading.is_empty()).then_some(heading)
    })
}

fn first_content_excerpt(content: &str, max_bytes: usize) -> Option<String> {
    let body = body_without_frontmatter(content);
    let mut lines = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed.starts_with("- Kind:")
            || trimmed.starts_with("- Reason:")
            || trimmed.starts_with("- Source:")
        {
            continue;
        }
        lines.push(trimmed);
        if lines.join(" ").len() >= max_bytes {
            break;
        }
    }
    let compact = lines.join(" ");
    let compact = compact.trim();
    (!compact.is_empty()).then(|| truncate_to_bytes(compact, max_bytes))
}

fn clean_yaml_scalar(value: &str) -> String {
    value
        .trim()
        .trim_matches(',')
        .trim_matches('"')
        .trim_matches('\'')
        .trim()
        .to_string()
}

fn non_empty_yaml_scalar(value: &str) -> Option<String> {
    let cleaned = clean_yaml_scalar(value);
    (!cleaned.is_empty()).then_some(cleaned)
}

fn parse_inline_yaml_list(value: &str) -> Vec<String> {
    let trimmed = value.trim();
    let Some(inner) = trimmed
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
    else {
        return non_empty_yaml_scalar(trimmed).into_iter().collect();
    };
    inner.split(',').filter_map(non_empty_yaml_scalar).collect()
}

fn content_window(content: &str, offset: usize, max_bytes: usize) -> WikiContentWindow {
    let total_bytes = content.len();
    let start = char_boundary_at_or_before(content, offset.min(total_bytes));
    let end = char_boundary_at_or_before(content, start.saturating_add(max_bytes).min(total_bytes));
    let window = content[start..end].to_string();
    WikiContentWindow {
        returned_bytes: window.len(),
        content: window,
        offset: start,
        total_bytes,
        truncated: start > 0 || end < total_bytes,
    }
}

fn char_boundary_at_or_before(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn truncate_to_bytes(value: &str, max_bytes: usize) -> String {
    let end = char_boundary_at_or_before(value, max_bytes.min(value.len()));
    value[..end].to_string()
}

fn markdown_section<'a>(content: &'a str, heading_query: &str) -> Option<&'a str> {
    let query = heading_query.trim().to_lowercase();
    let mut start = None;
    let mut start_level = 0usize;
    let mut cursor = 0usize;

    for line in content.split_inclusive('\n') {
        let line_start = cursor;
        cursor += line.len();
        let trimmed = line.trim_start();
        let Some(level) = markdown_heading_level(trimmed) else {
            continue;
        };
        let heading = trimmed[level..].trim().to_lowercase();
        if let Some(section_start) = start {
            if level <= start_level {
                return Some(content[section_start..line_start].trim_end());
            }
            continue;
        }
        if heading == query || heading.starts_with(&query) {
            start = Some(line_start);
            start_level = level;
        }
    }

    start.map(|section_start| content[section_start..].trim_end())
}

fn markdown_heading_level(line: &str) -> Option<usize> {
    let level = line.chars().take_while(|ch| *ch == '#').count();
    (level > 0 && line.chars().nth(level).is_some_and(char::is_whitespace)).then_some(level)
}

fn search_snippets(
    content: &str,
    query: &str,
    max_matches: usize,
    max_snippet_bytes: usize,
) -> Vec<String> {
    let mut snippets = Vec::new();
    for line in body_without_frontmatter(content).lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.to_lowercase().contains(query) {
            snippets.push(snippet_around(trimmed, 0, max_snippet_bytes));
            if snippets.len() >= max_matches {
                break;
            }
        }
    }
    snippets
}

fn snippet_around(value: &str, offset: usize, max_bytes: usize) -> String {
    let half = max_bytes / 2;
    let start = char_boundary_at_or_before(value, offset.saturating_sub(half).min(value.len()));
    let end = char_boundary_at_or_before(value, start.saturating_add(max_bytes).min(value.len()));
    let mut snippet = value[start..end].trim().to_string();
    if start > 0 {
        snippet.insert_str(0, "...");
    }
    if end < value.len() {
        snippet.push_str("...");
    }
    snippet
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

    fn typed_executor(
        provider: &Arc<WikiMemoryProvider>,
        tool_name: &str,
    ) -> Arc<dyn ToolExecutor> {
        provider
            .tool_runtime_executors()
            .into_iter()
            .find(|executor| executor.name().as_str() == tool_name)
            .expect("typed wiki memory executor registered")
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
        let provider = Arc::new(provider(backend));

        let output = typed_executor(&provider, TOOL_WIKI_MEMORY_LIST)
            .execute(runtime_invocation(
                TOOL_WIKI_MEMORY_LIST,
                r#"{"kind":"all"}"#,
            ))
            .await
            .expect("list should succeed");
        assert_eq!(output.status, ToolOutputStatus::Success);
        let parsed: serde_json::Value =
            serde_json::from_str(output.stdout.text.as_deref().expect("stdout text"))
                .expect("valid json");

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
        let provider = Arc::new(provider(backend));

        let output = typed_executor(&provider, TOOL_WIKI_MEMORY_READ)
            .execute(runtime_invocation(
                TOOL_WIKI_MEMORY_READ,
                r#"{"id":"page:btc-price"}"#,
            ))
            .await
            .expect("read should succeed");
        assert_eq!(output.status, ToolOutputStatus::Success);
        let parsed: serde_json::Value =
            serde_json::from_str(output.stdout.text.as_deref().expect("stdout text"))
                .expect("valid json");

        assert_eq!(parsed["found"], true);
        assert_eq!(parsed["id"], "page:btc-price");
        assert!(parsed["content"]
            .as_str()
            .expect("content string")
            .contains("76 840.69"));
        assert!(parsed["content_hash"].as_str().is_some());
    }

    #[tokio::test]
    async fn read_returns_bounded_window_by_default() {
        let backend = Arc::new(InMemoryWikiBackend::default());
        let context_id = context_id();
        backend.objects.lock().await.insert(
            format!("prod/wiki/v1/contexts/{context_id}/pages/runbook.md"),
            format!(
                "---\ntitle: Runbook\ntype: procedure\nupdated_at: 2026-05-19T00:00:00Z\nconfidence: high\ntags:\n  - deploy\nsources:\n  - run:test\n---\n\n# Runbook\n\n{}",
                "deploy step ".repeat(600)
            ),
        );
        let provider = Arc::new(provider(backend));

        let output = typed_executor(&provider, TOOL_WIKI_MEMORY_READ)
            .execute(runtime_invocation(
                TOOL_WIKI_MEMORY_READ,
                r#"{"id":"page:runbook","max_bytes":128}"#,
            ))
            .await
            .expect("read should succeed");
        let parsed: serde_json::Value =
            serde_json::from_str(output.stdout.text.as_deref().expect("stdout text"))
                .expect("valid json");

        assert_eq!(parsed["found"], true);
        assert_eq!(parsed["range"]["truncated"], true);
        assert!(parsed["content"].as_str().expect("content").len() <= 128);
        assert_eq!(parsed["metadata"]["type"], "procedure");
        assert_eq!(parsed["metadata"]["tags"][0], "deploy");
    }

    #[tokio::test]
    async fn read_can_target_markdown_heading() {
        let backend = Arc::new(InMemoryWikiBackend::default());
        let context_id = context_id();
        backend.objects.lock().await.insert(
            format!("prod/wiki/v1/contexts/{context_id}/procedures.md"),
            "# Procedures\n\nIntro\n\n## Deploy\n\nRun smoke tests.\n\n## Rollback\n\nUse previous image.\n"
                .to_string(),
        );
        let provider = Arc::new(provider(backend));

        let output = typed_executor(&provider, TOOL_WIKI_MEMORY_READ)
            .execute(runtime_invocation(
                TOOL_WIKI_MEMORY_READ,
                r#"{"id":"core:procedures","heading":"Deploy"}"#,
            ))
            .await
            .expect("read should succeed");
        let parsed: serde_json::Value =
            serde_json::from_str(output.stdout.text.as_deref().expect("stdout text"))
                .expect("valid json");

        assert_eq!(parsed["section_found"], true);
        assert!(parsed["content"]
            .as_str()
            .expect("content")
            .contains("Run smoke tests"));
        assert!(!parsed["content"]
            .as_str()
            .expect("content")
            .contains("previous image"));
    }

    #[tokio::test]
    async fn search_returns_bounded_snippets() {
        let backend = Arc::new(InMemoryWikiBackend::default());
        let context_id = context_id();
        backend.objects.lock().await.insert(
            format!("prod/wiki/v1/contexts/{context_id}/index.md"),
            "# Wiki Index\n\n## Core pages\n\n- [procedures](procedures.md) - current procedures\n\n## Topic pages\n\n- [deploy notes](pages/deploy-notes.md) - captured wiki update candidate\n"
                .to_string(),
        );
        backend.objects.lock().await.insert(
            format!("prod/wiki/v1/contexts/{context_id}/pages/deploy-notes.md"),
            "---\ntitle: Deploy notes\ntype: procedure\nupdated_at: 2026-05-19T00:00:00Z\nconfidence: high\ntags: [deploy]\nsources:\n  - run:test\n---\n\n# Deploy notes\n\nBefore deploy, run smoke tests and verify rollback notes.\n"
                .to_string(),
        );
        let provider = Arc::new(provider(backend));

        let output = typed_executor(&provider, TOOL_WIKI_MEMORY_SEARCH)
            .execute(runtime_invocation(
                TOOL_WIKI_MEMORY_SEARCH,
                r#"{"query":"rollback","limit":5,"max_snippet_bytes":120}"#,
            ))
            .await
            .expect("search should succeed");
        let parsed: serde_json::Value =
            serde_json::from_str(output.stdout.text.as_deref().expect("stdout text"))
                .expect("valid json");

        assert_eq!(parsed["count"], 1);
        assert_eq!(parsed["matches"][0]["id"], "page:deploy-notes");
        assert!(parsed["matches"][0]["snippets"][0]
            .as_str()
            .expect("snippet")
            .contains("rollback"));
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
        let output = typed_executor(&provider, TOOL_WIKI_MEMORY_READ)
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
        let provider = Arc::new(provider(Arc::clone(&backend)));

        let output = typed_executor(&provider, TOOL_WIKI_MEMORY_DELETE)
            .execute(runtime_invocation(
                TOOL_WIKI_MEMORY_DELETE,
                r#"{"id":"page:btc-price"}"#,
            ))
            .await
            .expect("delete should succeed");
        assert_eq!(output.status, ToolOutputStatus::Success);
        let parsed: serde_json::Value =
            serde_json::from_str(output.stdout.text.as_deref().expect("stdout text"))
                .expect("valid json");
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
        let provider = Arc::new(provider(backend));

        let error = typed_executor(&provider, TOOL_WIKI_MEMORY_DELETE)
            .execute(runtime_invocation(
                TOOL_WIKI_MEMORY_DELETE,
                r#"{"id":"core:overview"}"#,
            ))
            .await
            .expect_err("core delete must fail");

        assert!(error.to_string().contains("read-only"));
    }
}
