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

/// Returns the R2 key for a user's configuration file.
#[must_use]
pub fn user_config_key(user_id: i64) -> String {
    format!("users/{user_id}/config.json")
}

/// Returns the R2 key for a user's agent memory file.
#[must_use]
pub fn user_agent_memory_key(user_id: i64) -> String {
    format!("users/{user_id}/agent_memory.json")
}

/// Returns the R2 key for a user's agent memory file scoped by transport context.
#[must_use]
pub fn user_context_agent_memory_key(user_id: i64, context_key: &str) -> String {
    format!("users/{user_id}/topics/{context_key}/agent_memory.json")
}

/// Returns the R2 prefix for all topic-scoped agent flows in a transport context.
#[must_use]
pub fn user_context_agent_flows_prefix(user_id: i64, context_key: &str) -> String {
    format!("users/{user_id}/topics/{context_key}/flows/")
}

/// Returns the R2 prefix for a specific topic-scoped agent flow.
#[must_use]
pub fn user_context_agent_flow_prefix(user_id: i64, context_key: &str, flow_id: &str) -> String {
    format!("users/{user_id}/topics/{context_key}/flows/{flow_id}/")
}

/// Returns the R2 key for a topic-scoped agent flow metadata record.
#[must_use]
pub fn user_context_agent_flow_key(user_id: i64, context_key: &str, flow_id: &str) -> String {
    format!("users/{user_id}/topics/{context_key}/flows/{flow_id}/meta.json")
}

/// Returns the R2 key for a topic-scoped agent flow memory file.
#[must_use]
pub fn user_context_agent_flow_memory_key(
    user_id: i64,
    context_key: &str,
    flow_id: &str,
) -> String {
    format!("users/{user_id}/topics/{context_key}/flows/{flow_id}/memory.json")
}

/// Returns the S3 key for a global LLM Wiki file.
#[must_use]
pub fn wiki_global_key(prefix: &str, file: &str) -> String {
    key_with_optional_prefix(prefix, &format!("wiki/v1/global/{file}"))
}

/// Returns the S3 key for a context-scoped LLM Wiki file.
#[must_use]
pub fn wiki_context_key(prefix: &str, context_id: &str, file: &str) -> String {
    key_with_optional_prefix(prefix, &format!("wiki/v1/contexts/{context_id}/{file}"))
}

/// Returns the S3 prefix that covers all objects for a wiki context (pages, inbox, raw, core files).
#[must_use]
pub fn wiki_context_prefix(prefix: &str, context_id: &str) -> String {
    key_with_optional_prefix(prefix, &format!("wiki/v1/contexts/{context_id}/"))
}

/// Returns the S3 key for a context-scoped LLM Wiki topic page.
#[must_use]
pub fn wiki_context_page_key(prefix: &str, context_id: &str, slug: &str) -> String {
    key_with_optional_prefix(
        prefix,
        &format!("wiki/v1/contexts/{context_id}/pages/{slug}.md"),
    )
}

/// Returns the S3 key for a context-scoped LLM Wiki inbox item.
#[must_use]
pub fn wiki_context_inbox_key(prefix: &str, context_id: &str, item_slug: &str) -> String {
    key_with_optional_prefix(
        prefix,
        &format!("wiki/v1/contexts/{context_id}/inbox/{item_slug}.md"),
    )
}

/// Returns the S3 key for a context-scoped immutable LLM Wiki raw archive item.
#[must_use]
pub fn wiki_context_raw_key(prefix: &str, context_id: &str, yyyy_mm: &str, run_id: &str) -> String {
    key_with_optional_prefix(
        prefix,
        &format!("wiki/v1/contexts/{context_id}/raw/{yyyy_mm}/{run_id}.md"),
    )
}

/// Returns the R2 key for an agent profile record.
#[must_use]
pub fn agent_profile_key(user_id: i64, agent_id: &str) -> String {
    format!("users/{user_id}/control_plane/agent_profiles/{agent_id}.json")
}

/// Returns the R2 prefix for all agent profile records of a user.
#[must_use]
pub fn agent_profiles_prefix(user_id: i64) -> String {
    format!("users/{user_id}/control_plane/agent_profiles/")
}

/// Returns the R2 key for a topic context record.
#[must_use]
pub fn topic_context_key(user_id: i64, topic_id: &str) -> String {
    format!("users/{user_id}/control_plane/topic_contexts/{topic_id}.json")
}

/// Returns the R2 key for a topic-scoped `AGENTS.md` record.
#[must_use]
pub fn topic_agents_md_key(user_id: i64, topic_id: &str) -> String {
    format!("users/{user_id}/control_plane/topic_agents_md/{topic_id}.json")
}

#[must_use]
#[cfg(feature = "storage-s3-r2")]
pub(crate) fn topic_prompt_guard_key(user_id: i64, topic_id: &str) -> String {
    format!("users/{user_id}/control_plane/topic_prompts/{topic_id}")
}

/// Returns the R2 key for a topic infrastructure configuration record.
#[must_use]
pub fn topic_infra_config_key(user_id: i64, topic_id: &str) -> String {
    format!("users/{user_id}/control_plane/topic_infra/{topic_id}.json")
}

/// Returns the R2 key for a topic binding record.
#[must_use]
pub fn topic_binding_key(user_id: i64, topic_id: &str) -> String {
    format!("users/{user_id}/control_plane/topic_bindings/{topic_id}.json")
}

/// Returns the R2 prefix for reminder job records.
#[must_use]
pub fn reminder_jobs_prefix(user_id: i64) -> String {
    format!("users/{user_id}/control_plane/reminders/")
}

/// Returns the R2 key for a reminder job record.
#[must_use]
pub fn reminder_job_key(user_id: i64, reminder_id: &str) -> String {
    format!("users/{user_id}/control_plane/reminders/{reminder_id}.json")
}

/// Returns the R2 key for private secret material.
#[must_use]
pub fn private_secret_key(user_id: i64, secret_ref: &str) -> String {
    format!("users/{user_id}/private/secrets/{secret_ref}")
}

/// Returns the R2 key for a user audit events stream.
#[must_use]
pub fn audit_events_key(user_id: i64) -> String {
    format!("users/{user_id}/control_plane/audit/events.json")
}

/// Generates a new random flow UUID (v4).
#[must_use]
pub fn generate_flow_id() -> String {
    Uuid::new_v4().to_string()
}
