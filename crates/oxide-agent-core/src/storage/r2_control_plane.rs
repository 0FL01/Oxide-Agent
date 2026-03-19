use super::control_plane::{validate_topic_agents_md_content, validate_topic_context_content};
use super::keys::topic_prompt_guard_key;
use super::{
    builders::{
        build_agent_profile_record, build_audit_event_record, build_topic_agents_md_record,
        build_topic_binding_record, build_topic_context_record, build_topic_infra_config_record,
    },
    keys::{
        agent_profile_key, audit_events_key, private_secret_key, topic_agents_md_key,
        topic_binding_key, topic_context_key, topic_infra_config_key,
    },
    r2::R2Storage,
    utils::{
        current_timestamp_unix_secs, select_audit_events_page, should_retry_control_plane_rmw,
        CONTROL_PLANE_RMW_MAX_RETRIES, CONTROL_PLANE_RMW_RETRY_BACKOFF_MS,
    },
    AgentProfileRecord, AppendAuditEventOptions, AuditEventRecord, StorageError,
    TopicAgentsMdRecord, TopicBindingRecord, TopicContextRecord, TopicInfraConfigRecord,
    UpsertAgentProfileOptions, UpsertTopicAgentsMdOptions, UpsertTopicBindingOptions,
    UpsertTopicContextOptions, UpsertTopicInfraConfigOptions,
};
use crate::storage::r2_base::TopicPromptStoreKind;
use std::time::Duration;
use tokio::time::sleep;
use tracing::warn;
use uuid::Uuid;

impl R2Storage {
    pub(super) async fn get_agent_profile_inner(
        &self,
        user_id: i64,
        agent_id: String,
    ) -> Result<Option<AgentProfileRecord>, StorageError> {
        self.load_json(&agent_profile_key(user_id, &agent_id)).await
    }

    pub(super) async fn upsert_agent_profile_inner(
        &self,
        options: UpsertAgentProfileOptions,
    ) -> Result<AgentProfileRecord, StorageError> {
        let key = agent_profile_key(options.user_id, &options.agent_id);
        let _lock_guard = self.control_plane_locks.acquire(key.clone()).await;

        for attempt in 1..=CONTROL_PLANE_RMW_MAX_RETRIES {
            let (existing, etag) = self.load_json_with_etag::<AgentProfileRecord>(&key).await?;
            let now = current_timestamp_unix_secs();
            let record = build_agent_profile_record(options.clone(), existing, now);

            if self
                .save_json_conditionally(&key, &record, etag.as_deref())
                .await?
            {
                return Ok(record);
            }

            if should_retry_control_plane_rmw(attempt) {
                warn!(
                    key = %key,
                    attempt,
                    "agent profile optimistic concurrency conflict, retrying"
                );
                sleep(Duration::from_millis(
                    CONTROL_PLANE_RMW_RETRY_BACKOFF_MS * attempt as u64,
                ))
                .await;
            }
        }

        Err(StorageError::ConcurrencyConflict {
            key,
            attempts: CONTROL_PLANE_RMW_MAX_RETRIES,
        })
    }

    pub(super) async fn delete_agent_profile_inner(
        &self,
        user_id: i64,
        agent_id: String,
    ) -> Result<(), StorageError> {
        self.delete_object(&agent_profile_key(user_id, &agent_id))
            .await
    }

    pub(super) async fn get_topic_context_inner(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicContextRecord>, StorageError> {
        self.load_json(&topic_context_key(user_id, &topic_id)).await
    }

    pub(super) async fn upsert_topic_context_inner(
        &self,
        options: UpsertTopicContextOptions,
    ) -> Result<TopicContextRecord, StorageError> {
        let context = validate_topic_context_content(&options.context)?;
        let topic_prompt_guard_key = topic_prompt_guard_key(options.user_id, &options.topic_id);
        let _topic_prompt_guard = self
            .control_plane_locks
            .acquire(topic_prompt_guard_key)
            .await;
        let key = topic_context_key(options.user_id, &options.topic_id);
        let _lock_guard = self.control_plane_locks.acquire(key.clone()).await;
        self.ensure_topic_prompt_not_duplicated(
            options.user_id,
            &options.topic_id,
            TopicPromptStoreKind::Context,
            &context,
        )
        .await?;
        let options = UpsertTopicContextOptions { context, ..options };

        for attempt in 1..=CONTROL_PLANE_RMW_MAX_RETRIES {
            let (existing, etag) = self.load_json_with_etag::<TopicContextRecord>(&key).await?;
            let now = current_timestamp_unix_secs();
            let record = build_topic_context_record(options.clone(), existing, now);

            if self
                .save_json_conditionally(&key, &record, etag.as_deref())
                .await?
            {
                return Ok(record);
            }

            if should_retry_control_plane_rmw(attempt) {
                warn!(
                    key = %key,
                    attempt,
                    "topic context optimistic concurrency conflict, retrying"
                );
                sleep(Duration::from_millis(
                    CONTROL_PLANE_RMW_RETRY_BACKOFF_MS * attempt as u64,
                ))
                .await;
            }
        }

        Err(StorageError::ConcurrencyConflict {
            key,
            attempts: CONTROL_PLANE_RMW_MAX_RETRIES,
        })
    }

    pub(super) async fn delete_topic_context_inner(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError> {
        self.delete_object(&topic_context_key(user_id, &topic_id))
            .await
    }

    pub(super) async fn get_topic_agents_md_inner(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicAgentsMdRecord>, StorageError> {
        self.load_json(&topic_agents_md_key(user_id, &topic_id))
            .await
    }

    pub(super) async fn upsert_topic_agents_md_inner(
        &self,
        options: UpsertTopicAgentsMdOptions,
    ) -> Result<TopicAgentsMdRecord, StorageError> {
        let agents_md = validate_topic_agents_md_content(&options.agents_md)?;
        let topic_prompt_guard_key = topic_prompt_guard_key(options.user_id, &options.topic_id);
        let _topic_prompt_guard = self
            .control_plane_locks
            .acquire(topic_prompt_guard_key)
            .await;
        let key = topic_agents_md_key(options.user_id, &options.topic_id);
        let _lock_guard = self.control_plane_locks.acquire(key.clone()).await;
        self.ensure_topic_prompt_not_duplicated(
            options.user_id,
            &options.topic_id,
            TopicPromptStoreKind::AgentsMd,
            &agents_md,
        )
        .await?;
        let options = UpsertTopicAgentsMdOptions {
            agents_md,
            ..options
        };

        for attempt in 1..=CONTROL_PLANE_RMW_MAX_RETRIES {
            let (existing, etag) = self
                .load_json_with_etag::<TopicAgentsMdRecord>(&key)
                .await?;
            let now = current_timestamp_unix_secs();
            let record = build_topic_agents_md_record(options.clone(), existing, now);

            if self
                .save_json_conditionally(&key, &record, etag.as_deref())
                .await?
            {
                return Ok(record);
            }

            if should_retry_control_plane_rmw(attempt) {
                warn!(
                    key = %key,
                    attempt,
                    "topic AGENTS.md optimistic concurrency conflict, retrying"
                );
                sleep(Duration::from_millis(
                    CONTROL_PLANE_RMW_RETRY_BACKOFF_MS * attempt as u64,
                ))
                .await;
            }
        }

        Err(StorageError::ConcurrencyConflict {
            key,
            attempts: CONTROL_PLANE_RMW_MAX_RETRIES,
        })
    }

    pub(super) async fn delete_topic_agents_md_inner(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError> {
        self.delete_object(&topic_agents_md_key(user_id, &topic_id))
            .await
    }

    pub(super) async fn get_topic_infra_config_inner(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicInfraConfigRecord>, StorageError> {
        self.load_json(&topic_infra_config_key(user_id, &topic_id))
            .await
    }

    pub(super) async fn upsert_topic_infra_config_inner(
        &self,
        options: UpsertTopicInfraConfigOptions,
    ) -> Result<TopicInfraConfigRecord, StorageError> {
        let key = topic_infra_config_key(options.user_id, &options.topic_id);
        let _lock_guard = self.control_plane_locks.acquire(key.clone()).await;

        for attempt in 1..=CONTROL_PLANE_RMW_MAX_RETRIES {
            let (existing, etag) = self
                .load_json_with_etag::<TopicInfraConfigRecord>(&key)
                .await?;
            let now = current_timestamp_unix_secs();
            let record = build_topic_infra_config_record(options.clone(), existing, now);

            if self
                .save_json_conditionally(&key, &record, etag.as_deref())
                .await?
            {
                return Ok(record);
            }

            if should_retry_control_plane_rmw(attempt) {
                warn!(
                    key = %key,
                    attempt,
                    "topic infra config optimistic concurrency conflict, retrying"
                );
                sleep(Duration::from_millis(
                    CONTROL_PLANE_RMW_RETRY_BACKOFF_MS * attempt as u64,
                ))
                .await;
            }
        }

        Err(StorageError::ConcurrencyConflict {
            key,
            attempts: CONTROL_PLANE_RMW_MAX_RETRIES,
        })
    }

    pub(super) async fn delete_topic_infra_config_inner(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError> {
        self.delete_object(&topic_infra_config_key(user_id, &topic_id))
            .await
    }

    pub(super) async fn get_secret_value_inner(
        &self,
        user_id: i64,
        secret_ref: String,
    ) -> Result<Option<String>, StorageError> {
        self.load_text(&private_secret_key(user_id, &secret_ref))
            .await
    }

    pub(super) async fn put_secret_value_inner(
        &self,
        user_id: i64,
        secret_ref: String,
        value: String,
    ) -> Result<(), StorageError> {
        self.save_text(&private_secret_key(user_id, &secret_ref), &value)
            .await
    }

    pub(super) async fn delete_secret_value_inner(
        &self,
        user_id: i64,
        secret_ref: String,
    ) -> Result<(), StorageError> {
        self.delete_object(&private_secret_key(user_id, &secret_ref))
            .await
    }

    pub(super) async fn get_topic_binding_inner(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicBindingRecord>, StorageError> {
        self.load_json(&topic_binding_key(user_id, &topic_id)).await
    }

    pub(super) async fn upsert_topic_binding_inner(
        &self,
        options: UpsertTopicBindingOptions,
    ) -> Result<TopicBindingRecord, StorageError> {
        let key = topic_binding_key(options.user_id, &options.topic_id);
        let _lock_guard = self.control_plane_locks.acquire(key.clone()).await;

        for attempt in 1..=CONTROL_PLANE_RMW_MAX_RETRIES {
            let (existing, etag) = self.load_json_with_etag::<TopicBindingRecord>(&key).await?;
            let now = current_timestamp_unix_secs();
            let record = build_topic_binding_record(options.clone(), existing, now);

            if self
                .save_json_conditionally(&key, &record, etag.as_deref())
                .await?
            {
                return Ok(record);
            }

            if should_retry_control_plane_rmw(attempt) {
                warn!(
                    key = %key,
                    attempt,
                    "topic binding optimistic concurrency conflict, retrying"
                );
                sleep(Duration::from_millis(
                    CONTROL_PLANE_RMW_RETRY_BACKOFF_MS * attempt as u64,
                ))
                .await;
            }
        }

        Err(StorageError::ConcurrencyConflict {
            key,
            attempts: CONTROL_PLANE_RMW_MAX_RETRIES,
        })
    }

    pub(super) async fn delete_topic_binding_inner(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError> {
        self.delete_object(&topic_binding_key(user_id, &topic_id))
            .await
    }

    pub(super) async fn append_audit_event_inner(
        &self,
        options: AppendAuditEventOptions,
    ) -> Result<AuditEventRecord, StorageError> {
        let key = audit_events_key(options.user_id);
        let _lock_guard = self.control_plane_locks.acquire(key.clone()).await;

        for attempt in 1..=CONTROL_PLANE_RMW_MAX_RETRIES {
            let (current_events, etag) = self
                .load_json_with_etag::<Vec<AuditEventRecord>>(&key)
                .await?;
            let mut events = current_events.unwrap_or_default();
            let now = current_timestamp_unix_secs();
            let record = build_audit_event_record(
                options.clone(),
                events.last().map(|event| event.version),
                now,
                Uuid::new_v4().to_string(),
            );

            events.push(record.clone());
            if self
                .save_json_conditionally(&key, &events, etag.as_deref())
                .await?
            {
                return Ok(record);
            }

            if should_retry_control_plane_rmw(attempt) {
                warn!(
                    key = %key,
                    attempt,
                    "audit stream optimistic concurrency conflict, retrying"
                );
                sleep(Duration::from_millis(
                    CONTROL_PLANE_RMW_RETRY_BACKOFF_MS * attempt as u64,
                ))
                .await;
            }
        }

        Err(StorageError::ConcurrencyConflict {
            key,
            attempts: CONTROL_PLANE_RMW_MAX_RETRIES,
        })
    }

    pub(super) async fn list_audit_events_inner(
        &self,
        user_id: i64,
        limit: usize,
    ) -> Result<Vec<AuditEventRecord>, StorageError> {
        let events: Vec<AuditEventRecord> = self
            .load_json(&audit_events_key(user_id))
            .await?
            .unwrap_or_default();
        let start = events.len().saturating_sub(limit);
        Ok(events[start..].to_vec())
    }

    pub(super) async fn list_audit_events_page_inner(
        &self,
        user_id: i64,
        before_version: Option<u64>,
        limit: usize,
    ) -> Result<Vec<AuditEventRecord>, StorageError> {
        let events: Vec<AuditEventRecord> = self
            .load_json(&audit_events_key(user_id))
            .await?
            .unwrap_or_default();
        Ok(select_audit_events_page(events, before_version, limit))
    }
}
