use super::shared::{MutationAuditTarget, MutationExecutionResult, PreviewableMutationPlan};
use super::*;

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

impl ManagerControlPlaneProvider {
    pub(super) fn agent_profile_tools_definitions() -> Vec<ToolDefinition> {
        let mut tools = vec![
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
        ];
        tools.extend(Self::topic_agent_tools_management_definitions());
        tools.extend(Self::topic_agent_hooks_management_definitions());
        tools
    }

    pub(super) async fn execute_agent_profile_upsert(&self, arguments: &str) -> Result<String> {
        let args: AgentProfileUpsertArgs = Self::parse_args(arguments, TOOL_AGENT_PROFILE_UPSERT)?;
        let agent_id = Self::validate_non_empty(args.agent_id, "agent_id")?;
        let profile = Self::validate_profile_object(args.profile)?;
        let previous = self
            .storage
            .get_agent_profile(self.user_id, agent_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current agent profile: {err}"))?;
        let previous_value = Self::to_json_value(&previous)?;
        let applied_previous = previous_value.clone();
        let preview = json!({
            "operation": "upsert",
            "agent_id": agent_id.clone(),
            "profile": profile.clone(),
        });
        let dry_run_payload = json!({
            "agent_id": agent_id.clone(),
            "profile": profile.clone(),
            "previous": previous_value.clone(),
            "outcome": Self::dry_run_outcome(true)
        });
        let storage = Arc::clone(&self.storage);
        let user_id = self.user_id;

        self.execute_previewable_mutation(
            TOOL_AGENT_PROFILE_UPSERT,
            MutationAuditTarget {
                topic_id: None,
                agent_id: Some(agent_id.clone()),
            },
            PreviewableMutationPlan {
                dry_run: args.dry_run,
                preview,
                previous: previous_value,
                dry_run_payload,
            },
            || async move {
                let record = storage
                    .upsert_agent_profile(UpsertAgentProfileOptions {
                        user_id,
                        agent_id: agent_id.clone(),
                        profile,
                    })
                    .await
                    .map_err(|err| anyhow!("failed to upsert agent profile: {err}"))?;
                let version = record.version;

                Ok(MutationExecutionResult {
                    response_body: json!({ "ok": true, "profile": record }),
                    audit_payload: json!({
                        "agent_id": agent_id,
                        "version": version,
                        "previous": applied_previous,
                        "outcome": Self::dry_run_outcome(false)
                    }),
                })
            },
        )
        .await
    }

    pub(super) async fn execute_agent_profile_get(&self, arguments: &str) -> Result<String> {
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

    pub(super) async fn execute_agent_profile_delete(&self, arguments: &str) -> Result<String> {
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

    pub(super) async fn execute_agent_profile_rollback(&self, arguments: &str) -> Result<String> {
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
