use super::*;

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

impl ManagerControlPlaneProvider {
    pub(super) fn topic_binding_tools_definitions() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: TOOL_TOPIC_BINDING_SET.to_string(),
                description: "Low-level binding mutation. For newly created Telegram forum topics prefer forum_topic_provision_ssh_agent or pass the canonical topic_id '<chat_id>:<thread_id>'"
                    .to_string(),
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
        ]
    }

    pub(super) async fn execute_topic_binding_set(&self, arguments: &str) -> Result<String> {
        let args: TopicBindingSetArgs = Self::parse_args(arguments, TOOL_TOPIC_BINDING_SET)?;
        let topic_id = self.resolve_mutation_topic_id(args.topic_id).await?;
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

    pub(super) async fn execute_topic_binding_get(&self, arguments: &str) -> Result<String> {
        let args: TopicBindingGetArgs = Self::parse_args(arguments, TOOL_TOPIC_BINDING_GET)?;
        let topic_id = self.resolve_lookup_topic_id(args.topic_id).await?;

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

    pub(super) async fn execute_topic_binding_delete(&self, arguments: &str) -> Result<String> {
        let args: TopicBindingDeleteArgs = Self::parse_args(arguments, TOOL_TOPIC_BINDING_DELETE)?;
        let topic_id = self.resolve_lookup_topic_id(args.topic_id).await?;
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

    pub(super) async fn execute_topic_binding_rollback(&self, arguments: &str) -> Result<String> {
        let args: TopicBindingRollbackArgs =
            Self::parse_args(arguments, TOOL_TOPIC_BINDING_ROLLBACK)?;
        let topic_id = self.resolve_lookup_topic_id(args.topic_id).await?;
        let current = self
            .storage
            .get_topic_binding(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current topic binding: {err}"))?;
        let previous = match self.last_topic_binding_mutation(&topic_id).await? {
            Some(event) => Self::previous_from_payload::<TopicBindingRecord>(&event.payload)?,
            None => None,
        };

        let rollback_operation = previous.as_ref().map_or("delete", |_| "restore");

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
}
