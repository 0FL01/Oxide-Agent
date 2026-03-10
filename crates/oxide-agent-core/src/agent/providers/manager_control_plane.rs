//! Manager control-plane provider.
//!
//! Exposes user-scoped CRUD tools for topic bindings and agent profiles.

use crate::agent::provider::ToolProvider;
use crate::llm::ToolDefinition;
use crate::storage::{
    AppendAuditEventOptions, StorageProvider, UpsertAgentProfileOptions, UpsertTopicBindingOptions,
};
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

const TOOL_TOPIC_BINDING_SET: &str = "topic_binding_set";
const TOOL_TOPIC_BINDING_GET: &str = "topic_binding_get";
const TOOL_TOPIC_BINDING_DELETE: &str = "topic_binding_delete";
const TOOL_AGENT_PROFILE_UPSERT: &str = "agent_profile_upsert";
const TOOL_AGENT_PROFILE_GET: &str = "agent_profile_get";
const TOOL_AGENT_PROFILE_DELETE: &str = "agent_profile_delete";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicBindingSetArgs {
    topic_id: String,
    agent_id: String,
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
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentProfileUpsertArgs {
    agent_id: String,
    profile: serde_json::Value,
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

    fn tools_definitions() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: TOOL_TOPIC_BINDING_SET.to_string(),
                description: "Set or update topic-to-agent binding for current user".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier" },
                        "agent_id": { "type": "string", "description": "Target agent identifier" }
                    },
                    "required": ["topic_id", "agent_id"]
                }),
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
                name: TOOL_TOPIC_BINDING_DELETE.to_string(),
                description: "Delete topic-to-agent binding for current user".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier" }
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
                        "profile": { "type": "object", "description": "Arbitrary JSON profile payload" }
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
                        "agent_id": { "type": "string", "description": "Stable agent identifier" }
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

    async fn execute_topic_binding_set(&self, arguments: &str) -> Result<String> {
        let args: TopicBindingSetArgs = Self::parse_args(arguments, TOOL_TOPIC_BINDING_SET)?;
        let topic_id = Self::validate_non_empty(args.topic_id, "topic_id")?;
        let agent_id = Self::validate_non_empty(args.agent_id, "agent_id")?;

        let record = self
            .storage
            .upsert_topic_binding(UpsertTopicBindingOptions {
                user_id: self.user_id,
                topic_id: topic_id.clone(),
                agent_id: agent_id.clone(),
            })
            .await
            .map_err(|err| anyhow!("failed to upsert topic binding: {err}"))?;

        let _audit = self
            .storage
            .append_audit_event(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id),
                agent_id: Some(agent_id),
                action: TOOL_TOPIC_BINDING_SET.to_string(),
                payload: json!({
                    "topic_id": record.topic_id,
                    "agent_id": record.agent_id,
                    "version": record.version
                }),
            })
            .await
            .map_err(|err| anyhow!("failed to append audit event: {err}"))?;

        Self::to_json_string(json!({ "ok": true, "binding": record }))
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

        self.storage
            .delete_topic_binding(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to delete topic binding: {err}"))?;

        let _audit = self
            .storage
            .append_audit_event(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id.clone()),
                agent_id: None,
                action: TOOL_TOPIC_BINDING_DELETE.to_string(),
                payload: json!({ "topic_id": topic_id }),
            })
            .await
            .map_err(|err| anyhow!("failed to append audit event: {err}"))?;

        Self::to_json_string(json!({ "ok": true }))
    }

    async fn execute_agent_profile_upsert(&self, arguments: &str) -> Result<String> {
        let args: AgentProfileUpsertArgs = Self::parse_args(arguments, TOOL_AGENT_PROFILE_UPSERT)?;
        let agent_id = Self::validate_non_empty(args.agent_id, "agent_id")?;
        let profile = Self::validate_profile_object(args.profile)?;

        let record = self
            .storage
            .upsert_agent_profile(UpsertAgentProfileOptions {
                user_id: self.user_id,
                agent_id: agent_id.clone(),
                profile,
            })
            .await
            .map_err(|err| anyhow!("failed to upsert agent profile: {err}"))?;

        let _audit = self
            .storage
            .append_audit_event(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: None,
                agent_id: Some(agent_id),
                action: TOOL_AGENT_PROFILE_UPSERT.to_string(),
                payload: json!({
                    "agent_id": record.agent_id,
                    "version": record.version
                }),
            })
            .await
            .map_err(|err| anyhow!("failed to append audit event: {err}"))?;

        Self::to_json_string(json!({ "ok": true, "profile": record }))
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

        self.storage
            .delete_agent_profile(self.user_id, agent_id.clone())
            .await
            .map_err(|err| anyhow!("failed to delete agent profile: {err}"))?;

        let _audit = self
            .storage
            .append_audit_event(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: None,
                agent_id: Some(agent_id.clone()),
                action: TOOL_AGENT_PROFILE_DELETE.to_string(),
                payload: json!({ "agent_id": agent_id }),
            })
            .await
            .map_err(|err| anyhow!("failed to append audit event: {err}"))?;

        Self::to_json_string(json!({ "ok": true }))
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
                | TOOL_AGENT_PROFILE_UPSERT
                | TOOL_AGENT_PROFILE_GET
                | TOOL_AGENT_PROFILE_DELETE
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
            TOOL_AGENT_PROFILE_UPSERT => self.execute_agent_profile_upsert(arguments).await,
            TOOL_AGENT_PROFILE_GET => self.execute_agent_profile_get(arguments).await,
            TOOL_AGENT_PROFILE_DELETE => self.execute_agent_profile_delete(arguments).await,
            _ => Err(anyhow!("Unknown manager control-plane tool: {tool_name}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::registry::ToolRegistry;
    use crate::storage::{AgentProfileRecord, AppendAuditEventOptions, TopicBindingRecord};
    use mockall::predicate::eq;

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
        mock.expect_upsert_topic_binding()
            .withf(|options| {
                options.user_id == 77
                    && options.topic_id == "topic-a"
                    && options.agent_id == "agent-a"
            })
            .returning(|options| {
                Ok(TopicBindingRecord {
                    schema_version: 1,
                    version: 2,
                    user_id: options.user_id,
                    topic_id: options.topic_id,
                    agent_id: options.agent_id,
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
}
