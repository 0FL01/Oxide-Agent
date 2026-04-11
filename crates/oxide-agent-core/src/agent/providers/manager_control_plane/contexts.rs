use super::shared::{MutationAuditTarget, MutationExecutionResult, PreviewableMutationPlan};
use super::*;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicContextUpsertArgs {
    topic_id: String,
    context: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicContextGetArgs {
    topic_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicContextDeleteArgs {
    topic_id: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicContextRollbackArgs {
    topic_id: String,
    #[serde(default)]
    dry_run: bool,
}

impl ManagerControlPlaneProvider {
    pub(super) fn topic_context_tools_definitions() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: TOOL_TOPIC_CONTEXT_UPSERT.to_string(),
                description: format!(
                    "Create or update short topic operational context for current user (max {TOPIC_CONTEXT_MAX_LINES} lines / {TOPIC_CONTEXT_MAX_CHARS} chars; not for AGENTS.md)"
                ),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier" },
                        "context": { "type": "string", "description": "Short operational context injected into the agent prompt; use topic_agents_md_upsert for AGENTS.md documents" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without persisting" }
                    },
                    "required": ["topic_id", "context"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_CONTEXT_GET.to_string(),
                description: "Get topic-specific execution context for current user".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier" }
                    },
                    "required": ["topic_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_CONTEXT_DELETE.to_string(),
                description: "Delete topic-specific execution context for current user".to_string(),
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
                name: TOOL_TOPIC_CONTEXT_ROLLBACK.to_string(),
                description: "Rollback last topic context mutation for current user".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier" },
                        "dry_run": { "type": "boolean", "description": "Preview rollback without persisting" }
                    },
                    "required": ["topic_id"]
                }),
            },
        ]
    }

    pub(super) async fn execute_topic_context_upsert(&self, arguments: &str) -> Result<String> {
        let args: TopicContextUpsertArgs = Self::parse_args(arguments, TOOL_TOPIC_CONTEXT_UPSERT)?;
        let topic_id = self.resolve_mutation_topic_id(args.topic_id).await?;
        let context = Self::validate_topic_context(args.context)?;
        let previous = self
            .storage
            .get_topic_context(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current topic context: {err}"))?;
        let previous_value = Self::to_json_value(&previous)?;
        let applied_previous = previous_value.clone();
        let preview = json!({
            "operation": "upsert",
            "topic_id": topic_id.clone(),
            "context": context.clone(),
        });
        let dry_run_payload = json!({
            "topic_id": topic_id.clone(),
            "context": context.clone(),
            "previous": previous_value.clone(),
            "outcome": Self::dry_run_outcome(true)
        });
        let storage = Arc::clone(&self.storage);
        let user_id = self.user_id;

        self.execute_previewable_mutation(
            TOOL_TOPIC_CONTEXT_UPSERT,
            MutationAuditTarget {
                topic_id: Some(topic_id.clone()),
                agent_id: None,
            },
            PreviewableMutationPlan {
                dry_run: args.dry_run,
                preview,
                previous: previous_value,
                dry_run_payload,
            },
            || async move {
                let record = storage
                    .upsert_topic_context(UpsertTopicContextOptions {
                        user_id,
                        topic_id: topic_id.clone(),
                        context,
                    })
                    .await
                    .map_err(|err| anyhow!("failed to upsert topic context: {err}"))?;
                let version = record.version;

                Ok(MutationExecutionResult {
                    response_body: json!({ "ok": true, "topic_context": record }),
                    audit_payload: json!({
                        "topic_id": topic_id,
                        "version": version,
                        "previous": applied_previous,
                        "outcome": Self::dry_run_outcome(false)
                    }),
                })
            },
        )
        .await
    }

    pub(super) async fn execute_topic_context_get(&self, arguments: &str) -> Result<String> {
        let args: TopicContextGetArgs = Self::parse_args(arguments, TOOL_TOPIC_CONTEXT_GET)?;
        let topic_id = self.resolve_lookup_topic_id(args.topic_id).await?;

        let record = self
            .storage
            .get_topic_context(self.user_id, topic_id)
            .await
            .map_err(|err| anyhow!("failed to get topic context: {err}"))?;

        Self::to_json_string(json!({
            "ok": true,
            "found": record.is_some(),
            "topic_context": record
        }))
    }

    pub(super) async fn execute_topic_context_delete(&self, arguments: &str) -> Result<String> {
        let args: TopicContextDeleteArgs = Self::parse_args(arguments, TOOL_TOPIC_CONTEXT_DELETE)?;
        let topic_id = self.resolve_lookup_topic_id(args.topic_id).await?;
        let previous = self
            .storage
            .get_topic_context(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current topic context: {err}"))?;

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: Some(topic_id.clone()),
                    agent_id: None,
                    action: TOOL_TOPIC_CONTEXT_DELETE.to_string(),
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
            .delete_topic_context(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to delete topic context: {err}"))?;

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id.clone()),
                agent_id: None,
                action: TOOL_TOPIC_CONTEXT_DELETE.to_string(),
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

    pub(super) async fn execute_topic_context_rollback(&self, arguments: &str) -> Result<String> {
        let args: TopicContextRollbackArgs =
            Self::parse_args(arguments, TOOL_TOPIC_CONTEXT_ROLLBACK)?;
        let topic_id = self.resolve_lookup_topic_id(args.topic_id).await?;
        let current = self
            .storage
            .get_topic_context(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current topic context: {err}"))?;
        let previous = match self.last_topic_context_mutation(&topic_id).await? {
            Some(event) => Self::previous_from_payload::<TopicContextRecord>(&event.payload)?,
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
                    agent_id: None,
                    action: TOOL_TOPIC_CONTEXT_ROLLBACK.to_string(),
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

        let rolled_back_context = if let Some(previous_context) = previous.clone() {
            Some(
                self.storage
                    .upsert_topic_context(UpsertTopicContextOptions {
                        user_id: self.user_id,
                        topic_id: topic_id.clone(),
                        context: previous_context.context,
                    })
                    .await
                    .map_err(|err| anyhow!("failed to restore topic context: {err}"))?,
            )
        } else {
            self.storage
                .delete_topic_context(self.user_id, topic_id.clone())
                .await
                .map_err(|err| anyhow!("failed to delete topic context during rollback: {err}"))?;
            None
        };

        let response_topic_id = topic_id.clone();
        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id),
                agent_id: None,
                action: TOOL_TOPIC_CONTEXT_ROLLBACK.to_string(),
                payload: json!({
                    "topic_id": response_topic_id,
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
                "operation": rollback_operation,
                "topic_context": rolled_back_context
            }),
            audit_status,
        );

        Self::to_json_string(response)
    }
}
