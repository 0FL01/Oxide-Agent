//! Topic-level transaction helpers: FOR UPDATE locks and dedup guard.

use sqlx_core::{query::query, transaction::Transaction};
use sqlx_postgres::Postgres;

use super::helpers::{db_error, row_value};
use super::rows::{
    row_to_agent_flow, row_to_agent_profile, row_to_topic_agents_md, row_to_topic_binding,
    row_to_topic_context, row_to_topic_infra_config,
};
use super::{
    AgentFlowRecord, AgentProfileRecord, StorageError, TopicAgentsMdRecord, TopicBindingRecord,
    TopicContextRecord, TopicInfraConfigRecord,
};
use crate::storage::control_plane::normalize_topic_prompt_payload;

pub(super) async fn get_agent_flow_record_for_update(
    tx: &mut Transaction<'_, Postgres>,
    user_id: i64,
    context_key: &str,
    flow_id: &str,
) -> Result<Option<AgentFlowRecord>, StorageError> {
    let row = query::<Postgres>(
        r#"
        SELECT user_id, context_key, flow_id, schema_version, created_at, updated_at
        FROM agent_flows
        WHERE user_id = $1 AND context_key = $2 AND flow_id = $3
        FOR UPDATE
        "#,
    )
    .bind(user_id)
    .bind(context_key)
    .bind(flow_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(db_error)?;

    row.map(|row| row_to_agent_flow(&row)).transpose()
}

pub(super) async fn get_agent_profile_for_update(
    tx: &mut Transaction<'_, Postgres>,
    user_id: i64,
    agent_id: &str,
) -> Result<Option<AgentProfileRecord>, StorageError> {
    let row = query::<Postgres>(
        r#"
        SELECT user_id, agent_id, profile, version, schema_version, created_at, updated_at
        FROM agent_profiles
        WHERE user_id = $1 AND agent_id = $2
        FOR UPDATE
        "#,
    )
    .bind(user_id)
    .bind(agent_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(db_error)?;

    row.map(|row| row_to_agent_profile(&row)).transpose()
}

pub(super) async fn get_topic_context_for_update(
    tx: &mut Transaction<'_, Postgres>,
    user_id: i64,
    topic_id: &str,
) -> Result<Option<TopicContextRecord>, StorageError> {
    let row = query::<Postgres>(
        r#"
        SELECT user_id, topic_id, context, version, schema_version, created_at, updated_at
        FROM topic_contexts
        WHERE user_id = $1 AND topic_id = $2
        FOR UPDATE
        "#,
    )
    .bind(user_id)
    .bind(topic_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(db_error)?;

    row.map(|row| row_to_topic_context(&row)).transpose()
}

pub(super) async fn get_topic_agents_md_for_update(
    tx: &mut Transaction<'_, Postgres>,
    user_id: i64,
    topic_id: &str,
) -> Result<Option<TopicAgentsMdRecord>, StorageError> {
    let row = query::<Postgres>(
        r#"
        SELECT user_id, topic_id, agents_md, version, schema_version, created_at, updated_at
        FROM topic_agents_md
        WHERE user_id = $1 AND topic_id = $2
        FOR UPDATE
        "#,
    )
    .bind(user_id)
    .bind(topic_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(db_error)?;

    row.map(|row| row_to_topic_agents_md(&row)).transpose()
}

pub(super) async fn get_topic_infra_config_for_update(
    tx: &mut Transaction<'_, Postgres>,
    user_id: i64,
    topic_id: &str,
) -> Result<Option<TopicInfraConfigRecord>, StorageError> {
    let row = query::<Postgres>(
        r#"
        SELECT user_id, topic_id, target_name, host, port, remote_user, auth_mode,
               secret_ref, sudo_secret_ref, environment, tags, allowed_tool_modes,
               version, schema_version, created_at, updated_at
        FROM topic_infra_configs
        WHERE user_id = $1 AND topic_id = $2
        FOR UPDATE
        "#,
    )
    .bind(user_id)
    .bind(topic_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(db_error)?;

    row.map(|row| row_to_topic_infra_config(&row)).transpose()
}

pub(super) async fn get_topic_binding_for_update(
    tx: &mut Transaction<'_, Postgres>,
    user_id: i64,
    topic_id: &str,
) -> Result<Option<TopicBindingRecord>, StorageError> {
    let row = query::<Postgres>(
        r#"
        SELECT user_id, topic_id, agent_id, binding_kind, chat_id, thread_id,
               expires_at, last_activity_at, version, schema_version, created_at, updated_at
        FROM topic_bindings
        WHERE user_id = $1 AND topic_id = $2
        FOR UPDATE
        "#,
    )
    .bind(user_id)
    .bind(topic_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(db_error)?;

    row.map(|row| row_to_topic_binding(&row)).transpose()
}

#[derive(Clone, Copy)]
pub(super) enum TopicPromptStoreKind {
    Context,
    AgentsMd,
}

impl TopicPromptStoreKind {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Context => "topic_context",
            Self::AgentsMd => "topic_agents_md",
        }
    }
}

pub(super) async fn ensure_topic_prompt_not_duplicated_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: i64,
    topic_id: &str,
    attempted_kind: TopicPromptStoreKind,
    candidate: &str,
) -> Result<(), StorageError> {
    let normalized_candidate = normalize_topic_prompt_payload(candidate);
    let (existing_kind, row) = match attempted_kind {
        TopicPromptStoreKind::Context => {
            let row = query::<Postgres>(
                r#"
                SELECT agents_md AS content
                FROM topic_agents_md
                WHERE user_id = $1 AND topic_id = $2
                FOR UPDATE
                "#,
            )
            .bind(user_id)
            .bind(topic_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(db_error)?;
            (TopicPromptStoreKind::AgentsMd, row)
        }
        TopicPromptStoreKind::AgentsMd => {
            let row = query::<Postgres>(
                r#"
                SELECT context AS content
                FROM topic_contexts
                WHERE user_id = $1 AND topic_id = $2
                FOR UPDATE
                "#,
            )
            .bind(user_id)
            .bind(topic_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(db_error)?;
            (TopicPromptStoreKind::Context, row)
        }
    };

    let existing_content = row
        .map(|row| row_value::<String>(&row, "content"))
        .transpose()?;
    if let Some(existing_content) = existing_content
        && normalize_topic_prompt_payload(&existing_content) == normalized_candidate
    {
        return Err(StorageError::DuplicateTopicPromptContent {
            topic_id: topic_id.to_string(),
            existing_kind: existing_kind.as_str().to_string(),
            attempted_kind: attempted_kind.as_str().to_string(),
        });
    }

    Ok(())
}
