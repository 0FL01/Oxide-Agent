use super::shared::{MutationAuditTarget, MutationExecutionResult, PreviewableMutationPlan};
use super::*;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicAgentsMdUpsertArgs {
    topic_id: String,
    agents_md: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicAgentsMdGetArgs {
    topic_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicAgentsMdDeleteArgs {
    topic_id: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicAgentsMdRollbackArgs {
    topic_id: String,
    #[serde(default)]
    dry_run: bool,
}

impl ManagerControlPlaneProvider {
    pub(super) fn topic_agents_md_tools_definitions() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: TOOL_TOPIC_AGENTS_MD_UPSERT.to_string(),
                description:
                    "Create or update topic-scoped AGENTS.md for new flows (max 300 lines)"
                        .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier" },
                        "agents_md": { "type": "string", "description": "Full AGENTS.md content injected once when a new flow starts" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without persisting" }
                    },
                    "required": ["topic_id", "agents_md"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_AGENTS_MD_GET.to_string(),
                description: "Get topic-scoped AGENTS.md for current user".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier" }
                    },
                    "required": ["topic_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_AGENTS_MD_DELETE.to_string(),
                description: "Delete topic-scoped AGENTS.md for current user".to_string(),
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
                name: TOOL_TOPIC_AGENTS_MD_ROLLBACK.to_string(),
                description: "Rollback last topic-scoped AGENTS.md mutation for current user"
                    .to_string(),
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

    pub(super) async fn execute_topic_agents_md_upsert(&self, arguments: &str) -> Result<String> {
        let args: TopicAgentsMdUpsertArgs =
            Self::parse_args(arguments, TOOL_TOPIC_AGENTS_MD_UPSERT)?;
        let topic_id = self.resolve_mutation_topic_id(args.topic_id).await?;
        let agents_md = Self::validate_agents_md(args.agents_md)?;
        let previous = self
            .storage
            .get_topic_agents_md(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current topic AGENTS.md: {err}"))?;
        let previous_value = Self::to_json_value(&previous)?;
        let applied_previous = previous_value.clone();
        let preview = json!({
            "operation": "upsert",
            "topic_id": topic_id.clone(),
            "agents_md": agents_md.clone(),
        });
        let dry_run_payload = json!({
            "topic_id": topic_id.clone(),
            "agents_md": agents_md.clone(),
            "previous": previous_value.clone(),
            "outcome": Self::dry_run_outcome(true)
        });
        let storage = Arc::clone(&self.storage);
        let user_id = self.user_id;

        self.execute_previewable_mutation(
            TOOL_TOPIC_AGENTS_MD_UPSERT,
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
                    .upsert_topic_agents_md(UpsertTopicAgentsMdOptions {
                        user_id,
                        topic_id: topic_id.clone(),
                        agents_md,
                    })
                    .await
                    .map_err(|err| anyhow!("failed to upsert topic AGENTS.md: {err}"))?;
                let version = record.version;

                Ok(MutationExecutionResult {
                    response_body: json!({ "ok": true, "topic_agents_md": record }),
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

    pub(super) async fn execute_topic_agents_md_get(&self, arguments: &str) -> Result<String> {
        let args: TopicAgentsMdGetArgs = Self::parse_args(arguments, TOOL_TOPIC_AGENTS_MD_GET)?;
        let topic_id = self.resolve_lookup_topic_id(args.topic_id).await?;

        let record = self
            .storage
            .get_topic_agents_md(self.user_id, topic_id)
            .await
            .map_err(|err| anyhow!("failed to get topic AGENTS.md: {err}"))?;

        Self::to_json_string(json!({
            "ok": true,
            "found": record.is_some(),
            "topic_agents_md": record
        }))
    }

    pub(super) async fn execute_topic_agents_md_delete(&self, arguments: &str) -> Result<String> {
        let args: TopicAgentsMdDeleteArgs =
            Self::parse_args(arguments, TOOL_TOPIC_AGENTS_MD_DELETE)?;
        let topic_id = self.resolve_lookup_topic_id(args.topic_id).await?;
        let previous = self
            .storage
            .get_topic_agents_md(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current topic AGENTS.md: {err}"))?;

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: Some(topic_id.clone()),
                    agent_id: None,
                    action: TOOL_TOPIC_AGENTS_MD_DELETE.to_string(),
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
            .delete_topic_agents_md(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to delete topic AGENTS.md: {err}"))?;

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id),
                agent_id: None,
                action: TOOL_TOPIC_AGENTS_MD_DELETE.to_string(),
                payload: json!({
                    "previous": previous,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        let response = Self::attach_audit_status(json!({ "ok": true }), audit_status);
        Self::to_json_string(response)
    }

    pub(super) async fn execute_topic_agents_md_rollback(&self, arguments: &str) -> Result<String> {
        let args: TopicAgentsMdRollbackArgs =
            Self::parse_args(arguments, TOOL_TOPIC_AGENTS_MD_ROLLBACK)?;
        let topic_id = self.resolve_lookup_topic_id(args.topic_id).await?;
        let current = self
            .storage
            .get_topic_agents_md(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current topic AGENTS.md: {err}"))?;
        let previous = match self.last_topic_agents_md_mutation(&topic_id).await? {
            Some(event) => Self::previous_from_payload::<TopicAgentsMdRecord>(&event.payload)?,
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
                    action: TOOL_TOPIC_AGENTS_MD_ROLLBACK.to_string(),
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

        let rolled_back_agents_md = if let Some(previous_agents_md) = previous.clone() {
            Some(
                self.storage
                    .upsert_topic_agents_md(UpsertTopicAgentsMdOptions {
                        user_id: self.user_id,
                        topic_id: topic_id.clone(),
                        agents_md: previous_agents_md.agents_md,
                    })
                    .await
                    .map_err(|err| anyhow!("failed to restore topic AGENTS.md: {err}"))?,
            )
        } else {
            self.storage
                .delete_topic_agents_md(self.user_id, topic_id.clone())
                .await
                .map_err(|err| {
                    anyhow!("failed to delete topic AGENTS.md during rollback: {err}")
                })?;
            None
        };

        let response_topic_id = topic_id.clone();
        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id),
                agent_id: None,
                action: TOOL_TOPIC_AGENTS_MD_ROLLBACK.to_string(),
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
                "topic_agents_md": rolled_back_agents_md
            }),
            audit_status,
        );

        Self::to_json_string(response)
    }
}
