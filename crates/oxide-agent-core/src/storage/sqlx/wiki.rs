use super::StorageError;

pub(crate) const WIKI_SCHEMA_VERSION: i32 = 1;
pub(crate) const WIKI_DEFAULT_MAX_BYTES: usize = 64 * 1024;
pub(crate) const WIKI_INBOX_MAX_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WikiScopeKind {
    Global,
    Context,
}

impl WikiScopeKind {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Context => "context",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WikiItemKind {
    Global,
    Core,
    Page,
    Inbox,
    Raw,
}

impl WikiItemKind {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Core => "core",
            Self::Page => "page",
            Self::Inbox => "inbox",
            Self::Raw => "raw",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WikiAddress {
    pub(super) storage_prefix: String,
    pub(super) scope_kind: WikiScopeKind,
    pub(super) context_id: String,
    pub(super) item_kind: WikiItemKind,
    pub(super) path: String,
}

pub(super) fn parse_wiki_storage_key(storage_key: &str) -> Result<WikiAddress, StorageError> {
    let key = storage_key.trim_matches('/');
    let marker = "wiki/v1/";
    let marker_start = key.find(marker).ok_or_else(|| {
        StorageError::InvalidInput(format!(
            "wiki storage key `{storage_key}` does not contain `{marker}`"
        ))
    })?;
    if marker_start > 0 && !key[..marker_start].ends_with('/') {
        return Err(StorageError::InvalidInput(format!(
            "wiki storage key `{storage_key}` has invalid prefix before `{marker}`"
        )));
    }

    let storage_prefix = key[..marker_start].trim_end_matches('/').to_string();
    let logical = &key[marker_start + marker.len()..];
    let parts = logical.split('/').collect::<Vec<_>>();
    if parts
        .iter()
        .any(|part| part.is_empty() || *part == "." || *part == "..")
    {
        return Err(StorageError::InvalidInput(format!(
            "wiki storage key `{storage_key}` contains an unsafe path segment"
        )));
    }

    match parts.as_slice() {
        ["global", file] => {
            validate_wiki_markdown_leaf(file, storage_key)?;
            Ok(WikiAddress {
                storage_prefix,
                scope_kind: WikiScopeKind::Global,
                context_id: String::new(),
                item_kind: WikiItemKind::Global,
                path: (*file).to_string(),
            })
        }
        ["contexts", context_id, file] => {
            validate_wiki_context_id(context_id, storage_key)?;
            validate_wiki_markdown_leaf(file, storage_key)?;
            Ok(WikiAddress {
                storage_prefix,
                scope_kind: WikiScopeKind::Context,
                context_id: (*context_id).to_string(),
                item_kind: WikiItemKind::Core,
                path: (*file).to_string(),
            })
        }
        ["contexts", context_id, "pages", file] => {
            validate_wiki_context_id(context_id, storage_key)?;
            validate_wiki_markdown_leaf(file, storage_key)?;
            Ok(WikiAddress {
                storage_prefix,
                scope_kind: WikiScopeKind::Context,
                context_id: (*context_id).to_string(),
                item_kind: WikiItemKind::Page,
                path: format!("pages/{file}"),
            })
        }
        ["contexts", context_id, "inbox", file] => {
            validate_wiki_context_id(context_id, storage_key)?;
            validate_wiki_markdown_leaf(file, storage_key)?;
            Ok(WikiAddress {
                storage_prefix,
                scope_kind: WikiScopeKind::Context,
                context_id: (*context_id).to_string(),
                item_kind: WikiItemKind::Inbox,
                path: format!("inbox/{file}"),
            })
        }
        ["contexts", context_id, "raw", yyyy_mm, file] => {
            validate_wiki_context_id(context_id, storage_key)?;
            validate_wiki_year_month(yyyy_mm, storage_key)?;
            validate_wiki_markdown_leaf(file, storage_key)?;
            Ok(WikiAddress {
                storage_prefix,
                scope_kind: WikiScopeKind::Context,
                context_id: (*context_id).to_string(),
                item_kind: WikiItemKind::Raw,
                path: format!("raw/{yyyy_mm}/{file}"),
            })
        }
        _ => Err(StorageError::InvalidInput(format!(
            "wiki storage key `{storage_key}` does not match a supported wiki path"
        ))),
    }
}

fn validate_wiki_context_id(context_id: &str, storage_key: &str) -> Result<(), StorageError> {
    if context_id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        Ok(())
    } else {
        Err(StorageError::InvalidInput(format!(
            "wiki storage key `{storage_key}` contains invalid context id `{context_id}`"
        )))
    }
}

fn validate_wiki_markdown_leaf(file: &str, storage_key: &str) -> Result<(), StorageError> {
    if file.ends_with(".md") && !file.contains('/') && !file.contains('\\') {
        Ok(())
    } else {
        Err(StorageError::InvalidInput(format!(
            "wiki storage key `{storage_key}` contains invalid markdown file `{file}`"
        )))
    }
}

fn validate_wiki_year_month(yyyy_mm: &str, storage_key: &str) -> Result<(), StorageError> {
    let valid = yyyy_mm.len() == 7
        && yyyy_mm.as_bytes()[4] == b'-'
        && yyyy_mm[..4].chars().all(|ch| ch.is_ascii_digit())
        && yyyy_mm[5..].chars().all(|ch| ch.is_ascii_digit());
    if valid {
        Ok(())
    } else {
        Err(StorageError::InvalidInput(format!(
            "wiki storage key `{storage_key}` contains invalid raw archive month `{yyyy_mm}`"
        )))
    }
}

pub(super) fn validate_wiki_content_size(
    address: &WikiAddress,
    content: &str,
) -> Result<(), StorageError> {
    let max_bytes = match address.item_kind {
        WikiItemKind::Inbox => WIKI_INBOX_MAX_BYTES,
        WikiItemKind::Global | WikiItemKind::Core | WikiItemKind::Page | WikiItemKind::Raw => {
            WIKI_DEFAULT_MAX_BYTES
        }
    };
    let content_bytes = content.len();
    if content_bytes <= max_bytes {
        Ok(())
    } else {
        Err(StorageError::InvalidInput(format!(
            "wiki {} `{}` content size {content_bytes} exceeds {max_bytes} bytes",
            address.item_kind.as_str(),
            address.path
        )))
    }
}
