//! Manager control-plane provider.
//!
//! Exposes user-scoped CRUD tools for topic bindings and agent profiles.

use crate::agent::provider::ToolProvider;
use crate::llm::ToolDefinition;
use crate::storage::{
    AgentProfileRecord, AppendAuditEventOptions, OptionalMetadataPatch, StorageProvider,
    TopicBindingKind, TopicBindingRecord, UpsertAgentProfileOptions, UpsertTopicBindingOptions,
};
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::json;
use std::sync::Arc;

const TOOL_TOPIC_BINDING_SET: &str = "topic_binding_set";
const TOOL_TOPIC_BINDING_GET: &str = "topic_binding_get";
const TOOL_TOPIC_BINDING_DELETE: &str = "topic_binding_delete";
const TOOL_TOPIC_BINDING_ROLLBACK: &str = "topic_binding_rollback";
const TOOL_AGENT_PROFILE_UPSERT: &str = "agent_profile_upsert";
const TOOL_AGENT_PROFILE_GET: &str = "agent_profile_get";
const TOOL_AGENT_PROFILE_DELETE: &str = "agent_profile_delete";
const TOOL_AGENT_PROFILE_ROLLBACK: &str = "agent_profile_rollback";
const ROLLBACK_AUDIT_PAGE_SIZE: usize = 200;

enum AuditStatus {
    Written,
    WriteFailed(String),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicBindingSetArgs {
    topic_id: String,
    agent_id: String,
    #[serde(default)]
    binding_kind: Option<TopicBindingKind>,
    #[serde(default)]
    chat_id: OptionalMetadataPatch<i64>,
    #[serde(default)]
    thread_id: OptionalMetadataPatch<i64>,
    #[serde(default)]
    expires_at: OptionalMetadataPatch<i64>,
    #[serde(default)]
    last_activity_at: Option<i64>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicBindingGetArgs {
    topic_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicBindingDeleteArgs {
    topic_id: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicBindingRollbackArgs {
    topic_id: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentProfileUpsertArgs {
    agent_id: String,
    profile: serde_json::Value,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentProfileGetArgs {
    agent_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentProfileDeleteArgs {
    agent_id: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentProfileRollbackArgs {
    agent_id: String,
    #[serde(default)]
    dry_run: bool,
}

/// Tool provider that manages user-scoped control-plane records.
pub struct ManagerControlPlaneProvider {
    storage: Arc<dyn StorageProvider>,
    user_id: i64,
}

impl ManagerControlPlaneProvider {
    /// Creates a manager control-plane provider bound to a specific user.
    #[must_use]
    pub const fn new(storage: Arc<dyn StorageProvider>, user_id: i64) -> Self {
        Self { storage, user_id }
    }

    fn topic_binding_set_parameters() -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "topic_id": { "type": "string", "description": "Stable topic identifier" },
                "agent_id": { "type": "string", "description": "Target agent identifier" },
                "binding_kind": { "type": "string", "enum": ["manual", "runtime"], "description": "Binding source kind" },
                "chat_id": { "type": ["integer", "null"], "description": "Optional transport chat identifier; null clears stored value" },
                "thread_id": { "type": ["integer", "null"], "description": "Optional transport thread identifier; null clears stored value" },
                "expires_at": { "type": ["integer", "null"], "description": "Optional expiry unix timestamp; null clears stored value" },
                "last_activity_at": { "type": "integer", "description": "Optional last activity unix timestamp" },
                "dry_run": { "type": "boolean", "description": "Validate and preview without persisting" }
            },
            "required": ["topic_id", "agent_id"]
        })
    }

    fn tools_definitions() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: TOOL_TOPIC_BINDING_SET.to_string(),
                description: "Set or update topic-to-agent binding for current user".to_string(),
                parameters: Self::topic_binding_set_parameters(),
            },
            ToolDefinition {
                name: TOOL_TOPIC_BINDING_GET.to_string(),
                description: "Get topic-to-agent binding for current user".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier" }
                    },
                    "required": ["topic_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_BINDING_ROLLBACK.to_string(),
                description: "Rollback last topic binding mutation for current user".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier" },
                        "dry_run": { "type": "boolean", "description": "Preview rollback without persisting" }
                    },
                    "required": ["topic_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_BINDING_DELETE.to_string(),
                description: "Delete topic-to-agent binding for current user".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without persisting" }
                    },
                    "required": ["topic_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_AGENT_PROFILE_UPSERT.to_string(),
                description: "Create or update agent profile for current user".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "agent_id": { "type": "string", "description": "Stable agent identifier" },
                        "profile": { "type": "object", "description": "Arbitrary JSON profile payload" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without persisting" }
                    },
                    "required": ["agent_id", "profile"]
                }),
            },
            ToolDefinition {
                name: TOOL_AGENT_PROFILE_GET.to_string(),
                description: "Get agent profile for current user".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "agent_id": { "type": "string", "description": "Stable agent identifier" }
                    },
                    "required": ["agent_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_AGENT_PROFILE_DELETE.to_string(),
                description: "Delete agent profile for current user".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "agent_id": { "type": "string", "description": "Stable agent identifier" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without persisting" }
                    },
                    "required": ["agent_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_AGENT_PROFILE_ROLLBACK.to_string(),
                description: "Rollback last agent profile mutation for current user".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "agent_id": { "type": "string", "description": "Stable agent identifier" },
                        "dry_run": { "type": "boolean", "description": "Preview rollback without persisting" }
                    },
                    "required": ["agent_id"]
                }),
            },
        ]
    }

    fn validate_non_empty(value: String, field_name: &str) -> Result<String> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            bail!("{field_name} must not be empty");
        }
        Ok(trimmed.to_string())
    }

    fn validate_profile_object(profile: serde_json::Value) -> Result<serde_json::Value> {
        if !profile.is_object() {
            bail!("profile must be a JSON object");
        }
        Ok(profile)
    }

    fn to_json_string(value: serde_json::Value) -> Result<String> {
        serde_json::to_string(&value)
            .map_err(|err| anyhow!("failed to serialize tool response: {err}"))
    }

    fn parse_args<T: for<'de> Deserialize<'de>>(arguments: &str, tool_name: &str) -> Result<T> {
        serde_json::from_str(arguments).map_err(|err| anyhow!("invalid {tool_name} args: {err}"))
    }

    fn dry_run_outcome(dry_run: bool) -> &'static str {
        if dry_run {
            "dry_run"
        } else {
            "applied"
        }
    }

    fn optional_metadata_payload_value(value: OptionalMetadataPatch<i64>) -> Option<i64> {
        match value {
            OptionalMetadataPatch::Set(inner) => Some(inner),
            OptionalMetadataPatch::Keep | OptionalMetadataPatch::Clear => None,
        }
    }

    fn restore_metadata_patch(value: Option<i64>) -> OptionalMetadataPatch<i64> {
        value
            .map(OptionalMetadataPatch::Set)
            .unwrap_or(OptionalMetadataPatch::Clear)
    }

    fn previous_from_payload<T: DeserializeOwned>(
        payload: &serde_json::Value,
    ) -> Result<Option<T>> {
        let Some(previous) = payload.get("previous") else {
            return Ok(None);
        };

        if previous.is_null() {
            return Ok(None);
        }

        serde_json::from_value(previous.clone())
            .map(Some)
            .map_err(|err| anyhow!("invalid previous snapshot in audit payload: {err}"))
    }

    fn is_applied_mutation_event(event: &crate::storage::AuditEventRecord) -> bool {
        event.payload.get("outcome") != Some(&json!("dry_run"))
    }

    fn action_matches(action: &str, candidates: &[&str]) -> bool {
        candidates.contains(&action)
    }

    async fn append_audit_with_status(&self, options: AppendAuditEventOptions) -> AuditStatus {
        match self.storage.append_audit_event(options).await {
            Ok(_) => AuditStatus::Written,
            Err(err) => AuditStatus::WriteFailed(err.to_string()),
        }
    }

    fn attach_audit_status(
        mut response: serde_json::Value,
        status: AuditStatus,
    ) -> serde_json::Value {
        if let Some(response_object) = response.as_object_mut() {
            match status {
                AuditStatus::Written => {
                    response_object.insert("audit_status".to_string(), json!("written"));
                }
                AuditStatus::WriteFailed(error) => {
                    response_object.insert("audit_status".to_string(), json!("write_failed"));
                    response_object.insert("audit_error".to_string(), json!(error));
                }
            }
        }

        response
    }

    async fn find_latest_applied_mutation<F>(
        &self,
        mut predicate: F,
    ) -> Result<Option<crate::storage::AuditEventRecord>>
    where
        F: FnMut(&crate::storage::AuditEventRecord) -> bool,
    {
        let mut cursor = None;

        loop {
            let events = self
                .storage
                .list_audit_events_page(self.user_id, cursor, ROLLBACK_AUDIT_PAGE_SIZE)
                .await
                .map_err(|err| anyhow!("failed to list audit events: {err}"))?;

            if events.is_empty() {
                return Ok(None);
            }

            if let Some(event) = events
                .iter()
                .find(|event| Self::is_applied_mutation_event(event) && predicate(event))
            {
                return Ok(Some(event.clone()));
            }

            cursor = events.last().map(|event| event.version);
            if cursor.is_none() {
                return Ok(None);
            }
        }
    }

    async fn last_topic_binding_mutation(
        &self,
        topic_id: &str,
    ) -> Result<Option<crate::storage::AuditEventRecord>> {
        self.find_latest_applied_mutation(|event| {
            event.topic_id.as_deref() == Some(topic_id)
                && Self::action_matches(
                    event.action.as_str(),
                    &[
                        TOOL_TOPIC_BINDING_SET,
                        TOOL_TOPIC_BINDING_DELETE,
                        TOOL_TOPIC_BINDING_ROLLBACK,
                    ],
                )
        })
        .await
    }

    async fn last_agent_profile_mutation(
        &self,
        agent_id: &str,
    ) -> Result<Option<crate::storage::AuditEventRecord>> {
        self.find_latest_applied_mutation(|event| {
            event.agent_id.as_deref() == Some(agent_id)
                && Self::action_matches(
                    event.action.as_str(),
                    &[
                        TOOL_AGENT_PROFILE_UPSERT,
                        TOOL_AGENT_PROFILE_DELETE,
                        TOOL_AGENT_PROFILE_ROLLBACK,
                    ],
                )
        })
        .await
    }

    async fn execute_topic_binding_set(&self, arguments: &str) -> Result<String> {
        let args: TopicBindingSetArgs = Self::parse_args(arguments, TOOL_TOPIC_BINDING_SET)?;
        let topic_id = Self::validate_non_empty(args.topic_id, "topic_id")?;
        let agent_id = Self::validate_non_empty(args.agent_id, "agent_id")?;
        let binding_kind = args.binding_kind;
        let chat_id = args.chat_id;
        let thread_id = args.thread_id;
        let expires_at = args.expires_at;
        let chat_id_payload = Self::optional_metadata_payload_value(chat_id);
        let thread_id_payload = Self::optional_metadata_payload_value(thread_id);
        let expires_at_payload = Self::optional_metadata_payload_value(expires_at);
        let last_activity_at = args.last_activity_at;
        let previous = self
            .storage
            .get_topic_binding(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current topic binding: {err}"))?;

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: Some(topic_id.clone()),
                    agent_id: Some(agent_id.clone()),
                    action: TOOL_TOPIC_BINDING_SET.to_string(),
                    payload: json!({
                        "topic_id": topic_id,
                        "agent_id": agent_id,
                        "binding_kind": binding_kind,
                        "chat_id": chat_id_payload,
                        "thread_id": thread_id_payload,
                        "expires_at": expires_at_payload,
                        "last_activity_at": last_activity_at,
                        "previous": previous,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            let response = Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": "upsert",
                        "topic_id": topic_id,
                        "agent_id": agent_id,
                        "binding_kind": binding_kind,
                        "chat_id": chat_id_payload,
                        "thread_id": thread_id_payload,
                        "expires_at": expires_at_payload,
                        "last_activity_at": last_activity_at
                    },
                    "previous": previous
                }),
                audit_status,
            );

            return Self::to_json_string(response);
        }

        let record = self
            .storage
            .upsert_topic_binding(UpsertTopicBindingOptions {
                user_id: self.user_id,
                topic_id: topic_id.clone(),
                agent_id: agent_id.clone(),
                binding_kind,
                chat_id,
                thread_id,
                expires_at,
                last_activity_at,
            })
            .await
            .map_err(|err| anyhow!("failed to upsert topic binding: {err}"))?;

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id),
                agent_id: Some(agent_id),
                action: TOOL_TOPIC_BINDING_SET.to_string(),
                payload: json!({
                    "topic_id": record.topic_id,
                    "agent_id": record.agent_id,
                    "version": record.version,
                    "previous": previous,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        let response =
            Self::attach_audit_status(json!({ "ok": true, "binding": record }), audit_status);
        Self::to_json_string(response)
    }

    async fn execute_topic_binding_get(&self, arguments: &str) -> Result<String> {
        let args: TopicBindingGetArgs = Self::parse_args(arguments, TOOL_TOPIC_BINDING_GET)?;
        let topic_id = Self::validate_non_empty(args.topic_id, "topic_id")?;

        let record = self
            .storage
            .get_topic_binding(self.user_id, topic_id)
            .await
            .map_err(|err| anyhow!("failed to get topic binding: {err}"))?;

        Self::to_json_string(json!({
            "ok": true,
            "found": record.is_some(),
            "binding": record
        }))
    }

    async fn execute_topic_binding_delete(&self, arguments: &str) -> Result<String> {
        let args: TopicBindingDeleteArgs = Self::parse_args(arguments, TOOL_TOPIC_BINDING_DELETE)?;
        let topic_id = Self::validate_non_empty(args.topic_id, "topic_id")?;
        let previous = self
            .storage
            .get_topic_binding(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current topic binding: {err}"))?;

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: Some(topic_id.clone()),
                    agent_id: None,
                    action: TOOL_TOPIC_BINDING_DELETE.to_string(),
                    payload: json!({
                        "topic_id": topic_id,
                        "previous": previous,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            let response = Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": "delete",
                        "topic_id": topic_id
                    },
                    "previous": previous
                }),
                audit_status,
            );

            return Self::to_json_string(response);
        }

        self.storage
            .delete_topic_binding(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to delete topic binding: {err}"))?;

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id.clone()),
                agent_id: None,
                action: TOOL_TOPIC_BINDING_DELETE.to_string(),
                payload: json!({
                    "topic_id": topic_id,
                    "previous": previous,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        let response = Self::attach_audit_status(json!({ "ok": true }), audit_status);
        Self::to_json_string(response)
    }

    async fn execute_agent_profile_upsert(&self, arguments: &str) -> Result<String> {
        let args: AgentProfileUpsertArgs = Self::parse_args(arguments, TOOL_AGENT_PROFILE_UPSERT)?;
        let agent_id = Self::validate_non_empty(args.agent_id, "agent_id")?;
        let profile = Self::validate_profile_object(args.profile)?;
        let previous = self
            .storage
            .get_agent_profile(self.user_id, agent_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current agent profile: {err}"))?;

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: None,
                    agent_id: Some(agent_id.clone()),
                    action: TOOL_AGENT_PROFILE_UPSERT.to_string(),
                    payload: json!({
                        "agent_id": agent_id,
                        "profile": profile,
                        "previous": previous,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            let response = Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": "upsert",
                        "agent_id": agent_id,
                        "profile": profile
                    },
                    "previous": previous
                }),
                audit_status,
            );

            return Self::to_json_string(response);
        }

        let record = self
            .storage
            .upsert_agent_profile(UpsertAgentProfileOptions {
                user_id: self.user_id,
                agent_id: agent_id.clone(),
                profile,
            })
            .await
            .map_err(|err| anyhow!("failed to upsert agent profile: {err}"))?;

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: None,
                agent_id: Some(agent_id),
                action: TOOL_AGENT_PROFILE_UPSERT.to_string(),
                payload: json!({
                    "agent_id": record.agent_id,
                    "version": record.version,
                    "previous": previous,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        let response =
            Self::attach_audit_status(json!({ "ok": true, "profile": record }), audit_status);
        Self::to_json_string(response)
    }

    async fn execute_agent_profile_get(&self, arguments: &str) -> Result<String> {
        let args: AgentProfileGetArgs = Self::parse_args(arguments, TOOL_AGENT_PROFILE_GET)?;
        let agent_id = Self::validate_non_empty(args.agent_id, "agent_id")?;

        let record = self
            .storage
            .get_agent_profile(self.user_id, agent_id)
            .await
            .map_err(|err| anyhow!("failed to get agent profile: {err}"))?;

        Self::to_json_string(json!({
            "ok": true,
            "found": record.is_some(),
            "profile": record
        }))
    }

    async fn execute_agent_profile_delete(&self, arguments: &str) -> Result<String> {
        let args: AgentProfileDeleteArgs = Self::parse_args(arguments, TOOL_AGENT_PROFILE_DELETE)?;
        let agent_id = Self::validate_non_empty(args.agent_id, "agent_id")?;
        let previous = self
            .storage
            .get_agent_profile(self.user_id, agent_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current agent profile: {err}"))?;

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: None,
                    agent_id: Some(agent_id.clone()),
                    action: TOOL_AGENT_PROFILE_DELETE.to_string(),
                    payload: json!({
                        "agent_id": agent_id,
                        "previous": previous,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            let response = Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": "delete",
                        "agent_id": agent_id
                    },
                    "previous": previous
                }),
                audit_status,
            );

            return Self::to_json_string(response);
        }

        self.storage
            .delete_agent_profile(self.user_id, agent_id.clone())
            .await
            .map_err(|err| anyhow!("failed to delete agent profile: {err}"))?;

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: None,
                agent_id: Some(agent_id.clone()),
                action: TOOL_AGENT_PROFILE_DELETE.to_string(),
                payload: json!({
                    "agent_id": agent_id,
                    "previous": previous,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        let response = Self::attach_audit_status(json!({ "ok": true }), audit_status);
        Self::to_json_string(response)
    }

    async fn execute_topic_binding_rollback(&self, arguments: &str) -> Result<String> {
        let args: TopicBindingRollbackArgs =
            Self::parse_args(arguments, TOOL_TOPIC_BINDING_ROLLBACK)?;
        let topic_id = Self::validate_non_empty(args.topic_id, "topic_id")?;
        let current = self
            .storage
            .get_topic_binding(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current topic binding: {err}"))?;
        let previous = match self.last_topic_binding_mutation(&topic_id).await? {
            Some(event) => Self::previous_from_payload::<TopicBindingRecord>(&event.payload)?,
            None => None,
        };

        let rollback_operation = if previous.is_some() {
            "restore"
        } else {
            "delete"
        };

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: Some(topic_id.clone()),
                    agent_id: previous.as_ref().map(|record| record.agent_id.clone()),
                    action: TOOL_TOPIC_BINDING_ROLLBACK.to_string(),
                    payload: json!({
                        "topic_id": topic_id,
                        "operation": rollback_operation,
                        "previous": current,
                        "restore_to": previous,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            let response = Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": rollback_operation,
                        "topic_id": topic_id
                    },
                    "current": current,
                    "restore_to": previous
                }),
                audit_status,
            );

            return Self::to_json_string(response);
        }

        let rolled_back_binding = if let Some(previous_binding) = previous.clone() {
            Some(
                self.storage
                    .upsert_topic_binding(UpsertTopicBindingOptions {
                        user_id: self.user_id,
                        topic_id: topic_id.clone(),
                        agent_id: previous_binding.agent_id,
                        binding_kind: Some(previous_binding.binding_kind),
                        chat_id: Self::restore_metadata_patch(previous_binding.chat_id),
                        thread_id: Self::restore_metadata_patch(previous_binding.thread_id),
                        expires_at: Self::restore_metadata_patch(previous_binding.expires_at),
                        last_activity_at: previous_binding.last_activity_at,
                    })
                    .await
                    .map_err(|err| anyhow!("failed to restore topic binding: {err}"))?,
            )
        } else {
            self.storage
                .delete_topic_binding(self.user_id, topic_id.clone())
                .await
                .map_err(|err| anyhow!("failed to delete topic binding during rollback: {err}"))?;
            None
        };

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id.clone()),
                agent_id: rolled_back_binding
                    .as_ref()
                    .map(|record| record.agent_id.clone()),
                action: TOOL_TOPIC_BINDING_ROLLBACK.to_string(),
                payload: json!({
                    "topic_id": topic_id,
                    "operation": rollback_operation,
                    "previous": current,
                    "restore_to": previous,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        let response = Self::attach_audit_status(
            json!({
                "ok": true,
                "rolled_back": true,
                "operation": rollback_operation,
                "binding": rolled_back_binding
            }),
            audit_status,
        );

        Self::to_json_string(response)
    }

    async fn execute_agent_profile_rollback(&self, arguments: &str) -> Result<String> {
        let args: AgentProfileRollbackArgs =
            Self::parse_args(arguments, TOOL_AGENT_PROFILE_ROLLBACK)?;
        let agent_id = Self::validate_non_empty(args.agent_id, "agent_id")?;
        let current = self
            .storage
            .get_agent_profile(self.user_id, agent_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current agent profile: {err}"))?;
        let previous = match self.last_agent_profile_mutation(&agent_id).await? {
            Some(event) => Self::previous_from_payload::<AgentProfileRecord>(&event.payload)?,
            None => None,
        };

        let rollback_operation = if previous.is_some() {
            "restore"
        } else {
            "delete"
        };

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: None,
                    agent_id: Some(agent_id.clone()),
                    action: TOOL_AGENT_PROFILE_ROLLBACK.to_string(),
                    payload: json!({
                        "agent_id": agent_id,
                        "operation": rollback_operation,
                        "previous": current,
                        "restore_to": previous,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            let response = Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": rollback_operation,
                        "agent_id": agent_id
                    },
                    "current": current,
                    "restore_to": previous
                }),
                audit_status,
            );

            return Self::to_json_string(response);
        }

        let rolled_back_profile = if let Some(previous_profile) = previous.clone() {
            Some(
                self.storage
                    .upsert_agent_profile(UpsertAgentProfileOptions {
                        user_id: self.user_id,
                        agent_id: agent_id.clone(),
                        profile: previous_profile.profile,
                    })
                    .await
                    .map_err(|err| anyhow!("failed to restore agent profile: {err}"))?,
            )
        } else {
            self.storage
                .delete_agent_profile(self.user_id, agent_id.clone())
                .await
                .map_err(|err| anyhow!("failed to delete agent profile during rollback: {err}"))?;
            None
        };

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: None,
                agent_id: Some(agent_id.clone()),
                action: TOOL_AGENT_PROFILE_ROLLBACK.to_string(),
                payload: json!({
                    "agent_id": agent_id,
                    "operation": rollback_operation,
                    "previous": current,
                    "restore_to": previous,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        let response = Self::attach_audit_status(
            json!({
                "ok": true,
                "rolled_back": true,
                "operation": rollback_operation,
                "profile": rolled_back_profile
            }),
            audit_status,
        );

        Self::to_json_string(response)
    }
}

#[async_trait]
impl ToolProvider for ManagerControlPlaneProvider {
    fn name(&self) -> &'static str {
        "manager_control_plane"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        Self::tools_definitions()
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        matches!(
            tool_name,
            TOOL_TOPIC_BINDING_SET
                | TOOL_TOPIC_BINDING_GET
                | TOOL_TOPIC_BINDING_DELETE
                | TOOL_TOPIC_BINDING_ROLLBACK
                | TOOL_AGENT_PROFILE_UPSERT
                | TOOL_AGENT_PROFILE_GET
                | TOOL_AGENT_PROFILE_DELETE
                | TOOL_AGENT_PROFILE_ROLLBACK
        )
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        _progress_tx: Option<&tokio::sync::mpsc::Sender<crate::agent::progress::AgentEvent>>,
        _cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        match tool_name {
            TOOL_TOPIC_BINDING_SET => self.execute_topic_binding_set(arguments).await,
            TOOL_TOPIC_BINDING_GET => self.execute_topic_binding_get(arguments).await,
            TOOL_TOPIC_BINDING_DELETE => self.execute_topic_binding_delete(arguments).await,
            TOOL_TOPIC_BINDING_ROLLBACK => self.execute_topic_binding_rollback(arguments).await,
            TOOL_AGENT_PROFILE_UPSERT => self.execute_agent_profile_upsert(arguments).await,
            TOOL_AGENT_PROFILE_GET => self.execute_agent_profile_get(arguments).await,
            TOOL_AGENT_PROFILE_DELETE => self.execute_agent_profile_delete(arguments).await,
            TOOL_AGENT_PROFILE_ROLLBACK => self.execute_agent_profile_rollback(arguments).await,
            _ => Err(anyhow!("Unknown manager control-plane tool: {tool_name}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::registry::ToolRegistry;
    use crate::storage::{AgentProfileRecord, AppendAuditEventOptions, TopicBindingRecord};
    use mockall::{predicate::eq, Sequence};

    fn binding(user_id: i64, topic_id: &str, agent_id: &str, version: u64) -> TopicBindingRecord {
        TopicBindingRecord {
            schema_version: 1,
            version,
            user_id,
            topic_id: topic_id.to_string(),
            agent_id: agent_id.to_string(),
            binding_kind: TopicBindingKind::Manual,
            chat_id: None,
            thread_id: None,
            expires_at: None,
            last_activity_at: Some(20),
            created_at: 10,
            updated_at: 20,
        }
    }

    fn audit_event(
        version: u64,
        topic_id: Option<&str>,
        agent_id: Option<&str>,
        action: &str,
        payload: serde_json::Value,
    ) -> crate::storage::AuditEventRecord {
        crate::storage::AuditEventRecord {
            schema_version: 1,
            version,
            event_id: format!("evt-{version}"),
            user_id: 77,
            topic_id: topic_id.map(str::to_string),
            agent_id: agent_id.map(str::to_string),
            action: action.to_string(),
            payload,
            created_at: 100,
        }
    }

    #[tokio::test]
    async fn topic_binding_set_rejects_empty_topic_id() {
        let storage = Arc::new(crate::storage::MockStorageProvider::new());
        let provider = ManagerControlPlaneProvider::new(storage, 77);
        let err = provider
            .execute(
                TOOL_TOPIC_BINDING_SET,
                r#"{"topic_id":"   ","agent_id":"agent-1"}"#,
                None,
                None,
            )
            .await
            .expect_err("expected validation error");

        assert!(err.to_string().contains("topic_id must not be empty"));
    }

    #[tokio::test]
    async fn topic_binding_get_rejects_unknown_fields() {
        let storage = Arc::new(crate::storage::MockStorageProvider::new());
        let provider = ManagerControlPlaneProvider::new(storage, 77);
        let err = provider
            .execute(
                TOOL_TOPIC_BINDING_GET,
                r#"{"topic_id":"topic-a","extra":true}"#,
                None,
                None,
            )
            .await
            .expect_err("expected strict serde validation error");

        assert!(err.to_string().contains("unknown field"));
    }

    #[tokio::test]
    async fn agent_profile_upsert_rejects_non_object_profile() {
        let storage = Arc::new(crate::storage::MockStorageProvider::new());
        let provider = ManagerControlPlaneProvider::new(storage, 77);
        let err = provider
            .execute(
                TOOL_AGENT_PROFILE_UPSERT,
                r#"{"agent_id":"agent-a","profile":[1,2,3]}"#,
                None,
                None,
            )
            .await
            .expect_err("expected profile validation error");

        assert!(err.to_string().contains("profile must be a JSON object"));
    }

    #[tokio::test]
    async fn topic_binding_set_persists_and_audits() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_get_topic_binding()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, _| Ok(None));

        mock.expect_upsert_topic_binding()
            .withf(|options| {
                options.user_id == 77
                    && options.topic_id == "topic-a"
                    && options.agent_id == "agent-a"
                    && options.binding_kind.is_none()
                    && options.chat_id == OptionalMetadataPatch::Keep
                    && options.thread_id == OptionalMetadataPatch::Keep
                    && options.expires_at == OptionalMetadataPatch::Keep
                    && options.last_activity_at.is_none()
            })
            .returning(|options| {
                Ok(TopicBindingRecord {
                    schema_version: 1,
                    version: 2,
                    user_id: options.user_id,
                    topic_id: options.topic_id,
                    agent_id: options.agent_id,
                    binding_kind: options.binding_kind.unwrap_or(TopicBindingKind::Manual),
                    chat_id: options.chat_id.for_new_record(),
                    thread_id: options.thread_id.for_new_record(),
                    expires_at: options.expires_at.for_new_record(),
                    last_activity_at: options.last_activity_at,
                    created_at: 100,
                    updated_at: 200,
                })
            });

        mock.expect_append_audit_event()
            .withf(|options: &AppendAuditEventOptions| {
                options.user_id == 77
                    && options.action == TOOL_TOPIC_BINDING_SET
                    && options.topic_id.as_deref() == Some("topic-a")
                    && options.agent_id.as_deref() == Some("agent-a")
            })
            .returning(|options| {
                Ok(crate::storage::AuditEventRecord {
                    schema_version: 1,
                    version: 1,
                    event_id: "evt-1".to_string(),
                    user_id: options.user_id,
                    topic_id: options.topic_id,
                    agent_id: options.agent_id,
                    action: options.action,
                    payload: options.payload,
                    created_at: 300,
                })
            });

        let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
        let response = provider
            .execute(
                TOOL_TOPIC_BINDING_SET,
                r#"{"topic_id":"topic-a","agent_id":"agent-a"}"#,
                None,
                None,
            )
            .await
            .expect("topic binding set should succeed");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be json");
        assert_eq!(parsed.get("ok"), Some(&serde_json::Value::Bool(true)));
        assert_eq!(parsed["binding"]["topic_id"], "topic-a");
        assert_eq!(parsed["audit_status"], "written");
    }

    #[tokio::test]
    async fn topic_binding_set_supports_explicit_null_to_clear_metadata() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_get_topic_binding()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, _| {
                Ok(Some(TopicBindingRecord {
                    schema_version: 1,
                    version: 1,
                    user_id: 77,
                    topic_id: "topic-a".to_string(),
                    agent_id: "agent-a".to_string(),
                    binding_kind: TopicBindingKind::Runtime,
                    chat_id: Some(100),
                    thread_id: Some(7),
                    expires_at: Some(10_000),
                    last_activity_at: Some(20),
                    created_at: 10,
                    updated_at: 20,
                }))
            });

        mock.expect_upsert_topic_binding()
            .withf(|options| {
                options.user_id == 77
                    && options.topic_id == "topic-a"
                    && options.agent_id == "agent-a"
                    && options.chat_id == OptionalMetadataPatch::Clear
                    && options.thread_id == OptionalMetadataPatch::Clear
                    && options.expires_at == OptionalMetadataPatch::Clear
            })
            .returning(|options| {
                Ok(TopicBindingRecord {
                    schema_version: 1,
                    version: 2,
                    user_id: options.user_id,
                    topic_id: options.topic_id,
                    agent_id: options.agent_id,
                    binding_kind: options.binding_kind.unwrap_or(TopicBindingKind::Manual),
                    chat_id: options.chat_id.for_new_record(),
                    thread_id: options.thread_id.for_new_record(),
                    expires_at: options.expires_at.for_new_record(),
                    last_activity_at: options.last_activity_at,
                    created_at: 10,
                    updated_at: 30,
                })
            });

        mock.expect_append_audit_event().returning(|options| {
            Ok(crate::storage::AuditEventRecord {
                schema_version: 1,
                version: 1,
                event_id: "evt-1".to_string(),
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                action: options.action,
                payload: options.payload,
                created_at: 300,
            })
        });

        let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
        let response = provider
            .execute(
                TOOL_TOPIC_BINDING_SET,
                r#"{"topic_id":"topic-a","agent_id":"agent-a","chat_id":null,"thread_id":null,"expires_at":null}"#,
                None,
                None,
            )
            .await
            .expect("topic binding set should support null clears");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be json");
        assert_eq!(parsed.get("ok"), Some(&serde_json::Value::Bool(true)));
        assert_eq!(parsed["binding"]["chat_id"], serde_json::Value::Null);
        assert_eq!(parsed["binding"]["thread_id"], serde_json::Value::Null);
        assert_eq!(parsed["binding"]["expires_at"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn topic_binding_set_succeeds_when_audit_write_fails() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_get_topic_binding()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, _| Ok(None));

        mock.expect_upsert_topic_binding()
            .withf(|options| {
                options.user_id == 77
                    && options.topic_id == "topic-a"
                    && options.agent_id == "agent-a"
            })
            .returning(|options| {
                Ok(TopicBindingRecord {
                    schema_version: 1,
                    version: 1,
                    user_id: options.user_id,
                    topic_id: options.topic_id,
                    agent_id: options.agent_id,
                    binding_kind: options.binding_kind.unwrap_or(TopicBindingKind::Manual),
                    chat_id: options.chat_id.for_new_record(),
                    thread_id: options.thread_id.for_new_record(),
                    expires_at: options.expires_at.for_new_record(),
                    last_activity_at: options.last_activity_at,
                    created_at: 100,
                    updated_at: 100,
                })
            });

        mock.expect_append_audit_event().returning(|_| {
            Err(crate::storage::StorageError::Config(
                "audit unavailable".to_string(),
            ))
        });

        let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
        let response = provider
            .execute(
                TOOL_TOPIC_BINDING_SET,
                r#"{"topic_id":"topic-a","agent_id":"agent-a"}"#,
                None,
                None,
            )
            .await
            .expect("mutation should succeed even when audit write fails");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be json");
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["binding"]["topic_id"], "topic-a");
        assert_eq!(parsed["audit_status"], "write_failed");
        assert!(parsed["audit_error"].as_str().is_some());
    }

    #[tokio::test]
    async fn topic_binding_set_dry_run_does_not_persist() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_get_topic_binding()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, _| Ok(None));
        mock.expect_append_audit_event()
            .withf(|options: &AppendAuditEventOptions| {
                options.user_id == 77
                    && options.action == TOOL_TOPIC_BINDING_SET
                    && options.payload.get("outcome") == Some(&json!("dry_run"))
            })
            .returning(|options| {
                Ok(crate::storage::AuditEventRecord {
                    schema_version: 1,
                    version: 1,
                    event_id: "evt-dry-run".to_string(),
                    user_id: options.user_id,
                    topic_id: options.topic_id,
                    agent_id: options.agent_id,
                    action: options.action,
                    payload: options.payload,
                    created_at: 300,
                })
            });

        let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
        let response = provider
            .execute(
                TOOL_TOPIC_BINDING_SET,
                r#"{"topic_id":"topic-a","agent_id":"agent-a","dry_run":true}"#,
                None,
                None,
            )
            .await
            .expect("dry-run set should succeed");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be json");
        assert_eq!(parsed["dry_run"], true);
        assert_eq!(parsed["preview"]["operation"], "upsert");
        assert_eq!(parsed["audit_status"], "written");
    }

    #[tokio::test]
    async fn topic_binding_set_dry_run_reports_audit_write_failure() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_get_topic_binding()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, _| Ok(None));
        mock.expect_upsert_topic_binding().times(0);
        mock.expect_append_audit_event().returning(|_| {
            Err(crate::storage::StorageError::Config(
                "audit unavailable".to_string(),
            ))
        });

        let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
        let response = provider
            .execute(
                TOOL_TOPIC_BINDING_SET,
                r#"{"topic_id":"topic-a","agent_id":"agent-a","dry_run":true}"#,
                None,
                None,
            )
            .await
            .expect("dry-run should succeed even when audit write fails");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be json");
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["dry_run"], true);
        assert_eq!(parsed["audit_status"], "write_failed");
    }

    #[tokio::test]
    async fn topic_binding_rollback_restores_previous_snapshot() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_get_topic_binding()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, topic_id| Ok(Some(binding(77, &topic_id, "agent-new", 4))));
        mock.expect_list_audit_events_page()
            .with(eq(77_i64), eq(None), eq(ROLLBACK_AUDIT_PAGE_SIZE))
            .returning(|_, _, _| {
                Ok(vec![crate::storage::AuditEventRecord {
                    schema_version: 1,
                    version: 9,
                    event_id: "evt-9".to_string(),
                    user_id: 77,
                    topic_id: Some("topic-a".to_string()),
                    agent_id: Some("agent-new".to_string()),
                    action: TOOL_TOPIC_BINDING_SET.to_string(),
                    payload: json!({
                        "topic_id": "topic-a",
                        "agent_id": "agent-new",
                        "previous": {
                            "schema_version": 1,
                            "version": 3,
                            "user_id": 77,
                            "topic_id": "topic-a",
                            "agent_id": "agent-old",
                            "created_at": 1,
                            "updated_at": 2
                        },
                        "outcome": "applied"
                    }),
                    created_at: 100,
                }])
            });
        mock.expect_upsert_topic_binding()
            .withf(|options| {
                options.user_id == 77
                    && options.topic_id == "topic-a"
                    && options.agent_id == "agent-old"
            })
            .returning(|options| {
                Ok(binding(
                    options.user_id,
                    &options.topic_id,
                    &options.agent_id,
                    5,
                ))
            });
        mock.expect_append_audit_event()
            .withf(|options: &AppendAuditEventOptions| {
                options.user_id == 77
                    && options.action == TOOL_TOPIC_BINDING_ROLLBACK
                    && options.payload.get("operation") == Some(&json!("restore"))
            })
            .returning(|options| {
                Ok(crate::storage::AuditEventRecord {
                    schema_version: 1,
                    version: 10,
                    event_id: "evt-10".to_string(),
                    user_id: options.user_id,
                    topic_id: options.topic_id,
                    agent_id: options.agent_id,
                    action: options.action,
                    payload: options.payload,
                    created_at: 110,
                })
            });

        let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
        let response = provider
            .execute(
                TOOL_TOPIC_BINDING_ROLLBACK,
                r#"{"topic_id":"topic-a"}"#,
                None,
                None,
            )
            .await
            .expect("topic rollback should succeed");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be json");
        assert_eq!(parsed["operation"], "restore");
        assert_eq!(parsed["binding"]["agent_id"], "agent-old");
        assert_eq!(parsed["audit_status"], "written");
    }

    #[tokio::test]
    async fn topic_binding_rollback_succeeds_when_audit_write_fails() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_get_topic_binding()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, topic_id| {
                Ok(Some(TopicBindingRecord {
                    schema_version: 1,
                    version: 4,
                    user_id: 77,
                    topic_id,
                    agent_id: "agent-new".to_string(),
                    binding_kind: TopicBindingKind::Manual,
                    chat_id: None,
                    thread_id: None,
                    expires_at: None,
                    last_activity_at: Some(20),
                    created_at: 10,
                    updated_at: 20,
                }))
            });
        mock.expect_list_audit_events_page()
            .with(eq(77_i64), eq(None), eq(ROLLBACK_AUDIT_PAGE_SIZE))
            .returning(|_, _, _| {
                Ok(vec![crate::storage::AuditEventRecord {
                    schema_version: 1,
                    version: 9,
                    event_id: "evt-9".to_string(),
                    user_id: 77,
                    topic_id: Some("topic-a".to_string()),
                    agent_id: Some("agent-new".to_string()),
                    action: TOOL_TOPIC_BINDING_SET.to_string(),
                    payload: json!({
                        "topic_id": "topic-a",
                        "previous": {
                            "schema_version": 1,
                            "version": 3,
                            "user_id": 77,
                            "topic_id": "topic-a",
                            "agent_id": "agent-old",
                            "created_at": 1,
                            "updated_at": 2
                        },
                        "outcome": "applied"
                    }),
                    created_at: 100,
                }])
            });
        mock.expect_upsert_topic_binding().returning(|options| {
            Ok(TopicBindingRecord {
                schema_version: 1,
                version: 5,
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                binding_kind: options.binding_kind.unwrap_or(TopicBindingKind::Manual),
                chat_id: options.chat_id.for_new_record(),
                thread_id: options.thread_id.for_new_record(),
                expires_at: options.expires_at.for_new_record(),
                last_activity_at: options.last_activity_at,
                created_at: 10,
                updated_at: 30,
            })
        });
        mock.expect_append_audit_event().returning(|_| {
            Err(crate::storage::StorageError::Config(
                "audit unavailable".to_string(),
            ))
        });

        let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
        let response = provider
            .execute(
                TOOL_TOPIC_BINDING_ROLLBACK,
                r#"{"topic_id":"topic-a"}"#,
                None,
                None,
            )
            .await
            .expect("rollback should succeed even when audit write fails");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be json");
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["operation"], "restore");
        assert_eq!(parsed["audit_status"], "write_failed");
    }

    #[tokio::test]
    async fn topic_binding_rollback_scans_multiple_audit_pages() {
        let mut mock = crate::storage::MockStorageProvider::new();
        let mut sequence = Sequence::new();
        mock.expect_get_topic_binding()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, topic_id| Ok(Some(binding(77, &topic_id, "agent-new", 8))));
        mock.expect_list_audit_events_page()
            .with(eq(77_i64), eq(None), eq(ROLLBACK_AUDIT_PAGE_SIZE))
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _, _| {
                Ok(vec![audit_event(
                    500,
                    Some("other-topic"),
                    Some("agent-z"),
                    TOOL_TOPIC_BINDING_SET,
                    json!({"outcome":"applied"}),
                )])
            });
        mock.expect_list_audit_events_page()
            .with(eq(77_i64), eq(Some(500_u64)), eq(ROLLBACK_AUDIT_PAGE_SIZE))
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _, _| {
                Ok(vec![audit_event(
                    499,
                    Some("topic-a"),
                    Some("agent-new"),
                    TOOL_TOPIC_BINDING_SET,
                    json!({
                        "topic_id": "topic-a",
                        "previous": {
                            "schema_version": 1,
                            "version": 7,
                            "user_id": 77,
                            "topic_id": "topic-a",
                            "agent_id": "agent-old",
                            "created_at": 1,
                            "updated_at": 2
                        },
                        "outcome": "applied"
                    }),
                )])
            });
        mock.expect_upsert_topic_binding()
            .withf(|options| {
                options.user_id == 77
                    && options.topic_id == "topic-a"
                    && options.agent_id == "agent-old"
            })
            .returning(|options| {
                Ok(binding(
                    options.user_id,
                    &options.topic_id,
                    &options.agent_id,
                    9,
                ))
            });
        mock.expect_append_audit_event().returning(|options| {
            Ok(crate::storage::AuditEventRecord {
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                action: options.action,
                payload: options.payload,
                ..audit_event(501, None, None, TOOL_TOPIC_BINDING_ROLLBACK, json!({}))
            })
        });

        let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
        let response = provider
            .execute(
                TOOL_TOPIC_BINDING_ROLLBACK,
                r#"{"topic_id":"topic-a"}"#,
                None,
                None,
            )
            .await
            .expect("rollback should search across audit pages");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be json");
        assert_eq!(parsed["operation"], "restore");
        assert_eq!(parsed["binding"]["agent_id"], "agent-old");
        assert_eq!(parsed["audit_status"], "written");
    }

    #[tokio::test]
    async fn agent_profile_rollback_deletes_when_previous_snapshot_absent() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_get_agent_profile()
            .with(eq(77_i64), eq("agent-a".to_string()))
            .returning(|_, agent_id| {
                Ok(Some(AgentProfileRecord {
                    schema_version: 1,
                    version: 2,
                    user_id: 77,
                    agent_id,
                    profile: json!({"mode":"current"}),
                    created_at: 10,
                    updated_at: 20,
                }))
            });
        mock.expect_list_audit_events_page()
            .with(eq(77_i64), eq(None), eq(ROLLBACK_AUDIT_PAGE_SIZE))
            .returning(|_, _, _| {
                Ok(vec![crate::storage::AuditEventRecord {
                    schema_version: 1,
                    version: 3,
                    event_id: "evt-3".to_string(),
                    user_id: 77,
                    topic_id: None,
                    agent_id: Some("agent-a".to_string()),
                    action: TOOL_AGENT_PROFILE_DELETE.to_string(),
                    payload: json!({"agent_id":"agent-a","previous":null,"outcome":"applied"}),
                    created_at: 30,
                }])
            });
        mock.expect_delete_agent_profile()
            .with(eq(77_i64), eq("agent-a".to_string()))
            .returning(|_, _| Ok(()));
        mock.expect_append_audit_event()
            .withf(|options: &AppendAuditEventOptions| {
                options.user_id == 77
                    && options.action == TOOL_AGENT_PROFILE_ROLLBACK
                    && options.payload.get("operation") == Some(&json!("delete"))
            })
            .returning(|options| {
                Ok(crate::storage::AuditEventRecord {
                    schema_version: 1,
                    version: 4,
                    event_id: "evt-4".to_string(),
                    user_id: options.user_id,
                    topic_id: options.topic_id,
                    agent_id: options.agent_id,
                    action: options.action,
                    payload: options.payload,
                    created_at: 40,
                })
            });

        let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
        let response = provider
            .execute(
                TOOL_AGENT_PROFILE_ROLLBACK,
                r#"{"agent_id":"agent-a"}"#,
                None,
                None,
            )
            .await
            .expect("agent rollback should succeed");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be json");
        assert_eq!(parsed["operation"], "delete");
        assert!(parsed["profile"].is_null());
        assert_eq!(parsed["audit_status"], "written");
    }

    #[tokio::test]
    async fn tool_registry_routes_to_manager_provider() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_get_agent_profile()
            .with(eq(77_i64), eq("agent-x".to_string()))
            .returning(|user_id, agent_id| {
                Ok(Some(AgentProfileRecord {
                    schema_version: 1,
                    version: 5,
                    user_id,
                    agent_id,
                    profile: json!({"role":"support"}),
                    created_at: 10,
                    updated_at: 20,
                }))
            });

        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ManagerControlPlaneProvider::new(
            Arc::new(mock),
            77,
        )));

        let response = registry
            .execute(
                TOOL_AGENT_PROFILE_GET,
                r#"{"agent_id":"agent-x"}"#,
                None,
                None,
            )
            .await
            .expect("registry execution should succeed");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be json");
        assert_eq!(parsed["found"], true);
        assert_eq!(parsed["profile"]["agent_id"], "agent-x");
    }

    #[tokio::test]
    async fn tool_registry_without_manager_provider_rejects_manager_tools() {
        let registry = ToolRegistry::new();
        let err = registry
            .execute(
                TOOL_TOPIC_BINDING_GET,
                r#"{"topic_id":"topic-a"}"#,
                None,
                None,
            )
            .await
            .expect_err("manager tools must be unavailable without provider");

        assert!(err.to_string().contains("Unknown tool"));
    }
}
