use uuid::Uuid;

/// Returns the R2 key for a user's configuration file.
#[must_use]
pub fn user_config_key(user_id: i64) -> String {
    format!("users/{user_id}/config.json")
}

/// Returns the R2 key for a user's chat history file.
#[must_use]
pub fn user_history_key(user_id: i64) -> String {
    format!("users/{user_id}/history.json")
}

/// Returns the R2 key for a user's chat history file scoped by chat UUID.
#[must_use]
pub fn user_chat_history_key(user_id: i64, chat_uuid: &str) -> String {
    format!("users/{user_id}/chats/{chat_uuid}/history.json")
}

/// Returns the R2 prefix for all chat histories under a transport context.
#[must_use]
pub fn user_context_chat_history_prefix(user_id: i64, context_key: &str) -> String {
    format!("users/{user_id}/chats/{context_key}/")
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

/// Returns the R2 key for an agent profile record.
#[must_use]
pub fn agent_profile_key(user_id: i64, agent_id: &str) -> String {
    format!("users/{user_id}/control_plane/agent_profiles/{agent_id}.json")
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

/// Generates a new random chat UUID (v4).
#[must_use]
pub fn generate_chat_uuid() -> String {
    Uuid::new_v4().to_string()
}
