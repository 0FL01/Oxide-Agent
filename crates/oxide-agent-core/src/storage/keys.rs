use uuid::Uuid;

fn normalize_object_prefix(prefix: &str) -> String {
    prefix.trim_matches('/').to_string()
}

fn key_with_optional_prefix(prefix: &str, suffix: &str) -> String {
    let normalized = normalize_object_prefix(prefix);
    if normalized.is_empty() {
        suffix.to_string()
    } else {
        format!("{normalized}/{suffix}")
    }
}

/// Returns the deterministic storage key for a global LLM Wiki file.
#[must_use]
pub fn wiki_global_key(prefix: &str, file: &str) -> String {
    key_with_optional_prefix(prefix, &format!("wiki/v1/global/{file}"))
}

/// Returns the deterministic storage key for a context-scoped LLM Wiki file.
#[must_use]
pub fn wiki_context_key(prefix: &str, context_id: &str, file: &str) -> String {
    key_with_optional_prefix(prefix, &format!("wiki/v1/contexts/{context_id}/{file}"))
}

/// Returns the deterministic key prefix that covers all wiki rows/objects for a context.
#[must_use]
pub fn wiki_context_prefix(prefix: &str, context_id: &str) -> String {
    key_with_optional_prefix(prefix, &format!("wiki/v1/contexts/{context_id}/"))
}

/// Returns the deterministic storage key for a context-scoped LLM Wiki topic page.
#[must_use]
pub fn wiki_context_page_key(prefix: &str, context_id: &str, slug: &str) -> String {
    key_with_optional_prefix(
        prefix,
        &format!("wiki/v1/contexts/{context_id}/pages/{slug}.md"),
    )
}

/// Returns the deterministic storage key for a context-scoped LLM Wiki inbox item.
#[must_use]
pub fn wiki_context_inbox_key(prefix: &str, context_id: &str, item_slug: &str) -> String {
    key_with_optional_prefix(
        prefix,
        &format!("wiki/v1/contexts/{context_id}/inbox/{item_slug}.md"),
    )
}

/// Returns the deterministic storage key for a context-scoped immutable LLM Wiki raw archive item.
#[must_use]
pub fn wiki_context_raw_key(prefix: &str, context_id: &str, yyyy_mm: &str, run_id: &str) -> String {
    key_with_optional_prefix(
        prefix,
        &format!("wiki/v1/contexts/{context_id}/raw/{yyyy_mm}/{run_id}.md"),
    )
}

/// Generates a new random flow UUID (v4).
#[must_use]
pub fn generate_flow_id() -> String {
    Uuid::new_v4().to_string()
}
