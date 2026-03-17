use super::super::ssh_mcp::inspect_topic_infra_config;
use super::*;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicInfraUpsertArgs {
    topic_id: String,
    target_name: String,
    host: String,
    #[serde(default = "super::default_ssh_port")]
    port: u16,
    remote_user: String,
    #[serde(default)]
    auth_mode: TopicInfraAuthMode,
    #[serde(default)]
    secret_ref: Option<String>,
    #[serde(default)]
    sudo_secret_ref: Option<String>,
    #[serde(default)]
    environment: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default = "super::default_infra_allowed_tool_modes")]
    allowed_tool_modes: Vec<TopicInfraToolMode>,
    #[serde(default)]
    approval_required_modes: Vec<TopicInfraToolMode>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicInfraGetArgs {
    topic_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicInfraDeleteArgs {
    topic_id: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicInfraRollbackArgs {
    topic_id: String,
    #[serde(default)]
    dry_run: bool,
}

impl ManagerControlPlaneProvider {
    pub(super) fn topic_infra_tools_definitions() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: TOOL_TOPIC_INFRA_UPSERT.to_string(),
                description: "Low-level infra mutation. For newly created Telegram forum topics prefer forum_topic_provision_ssh_agent or pass the canonical topic_id '<chat_id>:<thread_id>'"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier" },
                        "target_name": { "type": "string", "description": "Human-readable target name" },
                        "host": { "type": "string", "description": "SSH host or DNS name" },
                        "port": { "type": "integer", "description": "SSH port, defaults to 22" },
                        "remote_user": { "type": "string", "description": "Remote SSH username" },
                        "auth_mode": { "type": "string", "enum": ["none", "password", "private_key"], "description": "SSH authentication mode" },
                        "secret_ref": { "type": "string", "description": "Opaque secret reference for SSH auth material" },
                        "sudo_secret_ref": { "type": "string", "description": "Opaque secret reference for sudo password material" },
                        "environment": { "type": "string", "description": "Optional environment label such as prod or stage" },
                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Optional free-form target tags" },
                        "allowed_tool_modes": { "type": "array", "items": { "type": "string", "enum": ["exec", "sudo_exec", "read_file", "apply_file_edit", "check_process"] }, "description": "Allowlisted SSH tool modes" },
                        "approval_required_modes": { "type": "array", "items": { "type": "string", "enum": ["exec", "sudo_exec", "read_file", "apply_file_edit", "check_process"] }, "description": "Modes that always require operator approval" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without persisting" }
                    },
                    "required": ["topic_id", "target_name", "host", "remote_user"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_INFRA_GET.to_string(),
                description: "Get topic-scoped infra target config for current user".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier" }
                    },
                    "required": ["topic_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_INFRA_DELETE.to_string(),
                description: "Delete topic-scoped infra target config for current user".to_string(),
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
                name: TOOL_TOPIC_INFRA_ROLLBACK.to_string(),
                description: "Rollback last topic infra config mutation for current user"
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

    pub(super) fn normalize_tool_modes(modes: Vec<TopicInfraToolMode>) -> Vec<TopicInfraToolMode> {
        let mut modes = modes;
        modes.sort_by_key(|mode| match mode {
            TopicInfraToolMode::Exec => 0,
            TopicInfraToolMode::SudoExec => 1,
            TopicInfraToolMode::ReadFile => 2,
            TopicInfraToolMode::ApplyFileEdit => 3,
            TopicInfraToolMode::CheckProcess => 4,
        });
        modes.dedup();
        modes
    }

    fn validate_topic_infra_args(args: TopicInfraUpsertArgs) -> Result<TopicInfraUpsertArgs> {
        let topic_id = Self::validate_non_empty(args.topic_id, "topic_id")?;
        let target_name = Self::validate_non_empty(args.target_name, "target_name")?;
        let host = Self::validate_non_empty(args.host, "host")?;
        let remote_user = Self::validate_non_empty(args.remote_user, "remote_user")?;
        if args.port == 0 {
            bail!("port must be a positive integer");
        }

        let secret_ref = Self::validate_optional_non_empty(args.secret_ref, "secret_ref")?;
        let sudo_secret_ref =
            Self::validate_optional_non_empty(args.sudo_secret_ref, "sudo_secret_ref")?;
        let environment = Self::validate_optional_non_empty(args.environment, "environment")?;
        let allowed_tool_modes = Self::normalize_tool_modes(args.allowed_tool_modes);
        if allowed_tool_modes.is_empty() {
            bail!("allowed_tool_modes must not be empty");
        }
        let approval_required_modes = Self::normalize_tool_modes(args.approval_required_modes);

        Ok(TopicInfraUpsertArgs {
            topic_id,
            target_name,
            host,
            port: args.port,
            remote_user,
            auth_mode: args.auth_mode,
            secret_ref,
            sudo_secret_ref,
            environment,
            tags: Self::normalize_tags(args.tags),
            allowed_tool_modes,
            approval_required_modes,
            dry_run: args.dry_run,
        })
    }

    fn topic_infra_value_from_args(args: &TopicInfraUpsertArgs) -> serde_json::Value {
        json!({
            "topic_id": args.topic_id,
            "target_name": args.target_name,
            "host": args.host,
            "port": args.port,
            "remote_user": args.remote_user,
            "auth_mode": args.auth_mode,
            "secret_ref": args.secret_ref,
            "sudo_secret_ref": args.sudo_secret_ref,
            "environment": args.environment,
            "tags": args.tags,
            "allowed_tool_modes": args.allowed_tool_modes,
            "approval_required_modes": args.approval_required_modes,
        })
    }

    pub(super) fn topic_infra_value_from_record(
        record: &TopicInfraConfigRecord,
    ) -> serde_json::Value {
        json!({
            "topic_id": record.topic_id,
            "version": record.version,
            "target_name": record.target_name,
            "host": record.host,
            "port": record.port,
            "remote_user": record.remote_user,
            "auth_mode": record.auth_mode,
            "secret_ref": record.secret_ref,
            "sudo_secret_ref": record.sudo_secret_ref,
            "environment": record.environment,
            "tags": record.tags,
            "allowed_tool_modes": record.allowed_tool_modes,
            "approval_required_modes": record.approval_required_modes,
        })
    }

    fn topic_infra_preview_record(&self, args: &TopicInfraUpsertArgs) -> TopicInfraConfigRecord {
        TopicInfraConfigRecord {
            schema_version: 1,
            version: 0,
            user_id: self.user_id,
            topic_id: args.topic_id.clone(),
            target_name: args.target_name.clone(),
            host: args.host.clone(),
            port: args.port,
            remote_user: args.remote_user.clone(),
            auth_mode: args.auth_mode,
            secret_ref: args.secret_ref.clone(),
            sudo_secret_ref: args.sudo_secret_ref.clone(),
            environment: args.environment.clone(),
            tags: args.tags.clone(),
            allowed_tool_modes: args.allowed_tool_modes.clone(),
            approval_required_modes: args.approval_required_modes.clone(),
            created_at: 0,
            updated_at: 0,
        }
    }

    pub(super) fn topic_infra_preview_record_from_plan(
        &self,
        topic_id: String,
        plan: &ForumTopicProvisionSshAgentPlan,
    ) -> TopicInfraConfigRecord {
        TopicInfraConfigRecord {
            schema_version: 1,
            version: 0,
            user_id: self.user_id,
            topic_id,
            target_name: plan.target_name.clone(),
            host: plan.host.clone(),
            port: plan.port,
            remote_user: plan.remote_user.clone(),
            auth_mode: plan.auth_mode,
            secret_ref: plan.secret_ref.clone(),
            sudo_secret_ref: plan.sudo_secret_ref.clone(),
            environment: plan.environment.clone(),
            tags: plan.tags.clone(),
            allowed_tool_modes: plan.allowed_tool_modes.clone(),
            approval_required_modes: plan.approval_required_modes.clone(),
            created_at: 0,
            updated_at: 0,
        }
    }

    pub(super) async fn inspect_topic_infra_record(
        &self,
        record: &TopicInfraConfigRecord,
    ) -> crate::agent::providers::TopicInfraPreflightReport {
        inspect_topic_infra_config(&self.storage, self.user_id, &record.topic_id, record).await
    }

    async fn restore_or_delete_topic_infra(
        &self,
        topic_id: &str,
        previous: Option<TopicInfraConfigRecord>,
    ) -> Result<Option<TopicInfraConfigRecord>> {
        if let Some(previous_infra) = previous {
            return self
                .storage
                .upsert_topic_infra_config(UpsertTopicInfraConfigOptions {
                    user_id: self.user_id,
                    topic_id: topic_id.to_string(),
                    target_name: previous_infra.target_name,
                    host: previous_infra.host,
                    port: previous_infra.port,
                    remote_user: previous_infra.remote_user,
                    auth_mode: previous_infra.auth_mode,
                    secret_ref: previous_infra.secret_ref,
                    sudo_secret_ref: previous_infra.sudo_secret_ref,
                    environment: previous_infra.environment,
                    tags: previous_infra.tags,
                    allowed_tool_modes: previous_infra.allowed_tool_modes,
                    approval_required_modes: previous_infra.approval_required_modes,
                })
                .await
                .map(Some)
                .map_err(|err| anyhow!("failed to restore topic infra config: {err}"));
        }

        self.storage
            .delete_topic_infra_config(self.user_id, topic_id.to_string())
            .await
            .map_err(|err| anyhow!("failed to delete topic infra config during rollback: {err}"))?;
        Ok(None)
    }

    pub(super) async fn execute_topic_infra_upsert(&self, arguments: &str) -> Result<String> {
        let mut args =
            Self::validate_topic_infra_args(Self::parse_args(arguments, TOOL_TOPIC_INFRA_UPSERT)?)?;
        args.topic_id = self.resolve_mutation_topic_id(args.topic_id).await?;
        let desired = Self::topic_infra_value_from_args(&args);
        let previous = self
            .storage
            .get_topic_infra_config(self.user_id, args.topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current topic infra config: {err}"))?;

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: Some(args.topic_id.clone()),
                    agent_id: None,
                    action: TOOL_TOPIC_INFRA_UPSERT.to_string(),
                    payload: json!({
                        "desired": desired,
                        "previous": previous,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            return Self::to_json_string(Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": "upsert",
                        "desired": desired,
                        "preflight": self
                            .inspect_topic_infra_record(&self.topic_infra_preview_record(&args))
                            .await
                    },
                    "previous": previous
                }),
                audit_status,
            ));
        }

        let record = self
            .storage
            .upsert_topic_infra_config(UpsertTopicInfraConfigOptions {
                user_id: self.user_id,
                topic_id: args.topic_id.clone(),
                target_name: args.target_name,
                host: args.host,
                port: args.port,
                remote_user: args.remote_user,
                auth_mode: args.auth_mode,
                secret_ref: args.secret_ref,
                sudo_secret_ref: args.sudo_secret_ref,
                environment: args.environment,
                tags: args.tags,
                allowed_tool_modes: args.allowed_tool_modes,
                approval_required_modes: args.approval_required_modes,
            })
            .await
            .map_err(|err| anyhow!("failed to upsert topic infra config: {err}"))?;
        let preflight = self.inspect_topic_infra_record(&record).await;

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(args.topic_id),
                agent_id: None,
                action: TOOL_TOPIC_INFRA_UPSERT.to_string(),
                payload: json!({
                    "record": Self::topic_infra_value_from_record(&record),
                    "previous": previous,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        Self::to_json_string(Self::attach_audit_status(
            json!({ "ok": true, "topic_infra": record, "preflight": preflight }),
            audit_status,
        ))
    }

    pub(super) async fn execute_topic_infra_get(&self, arguments: &str) -> Result<String> {
        let args: TopicInfraGetArgs = Self::parse_args(arguments, TOOL_TOPIC_INFRA_GET)?;
        let topic_id = self.resolve_lookup_topic_id(args.topic_id).await?;

        let record = self
            .storage
            .get_topic_infra_config(self.user_id, topic_id)
            .await
            .map_err(|err| anyhow!("failed to get topic infra config: {err}"))?;
        let preflight = match record.as_ref() {
            Some(record) => Some(self.inspect_topic_infra_record(record).await),
            None => None,
        };

        Self::to_json_string(json!({
            "ok": true,
            "found": record.is_some(),
            "topic_infra": record,
            "preflight": preflight
        }))
    }

    pub(super) async fn execute_topic_infra_delete(&self, arguments: &str) -> Result<String> {
        let args: TopicInfraDeleteArgs = Self::parse_args(arguments, TOOL_TOPIC_INFRA_DELETE)?;
        let topic_id = self.resolve_lookup_topic_id(args.topic_id).await?;
        let previous = self
            .storage
            .get_topic_infra_config(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current topic infra config: {err}"))?;

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: Some(topic_id.clone()),
                    agent_id: None,
                    action: TOOL_TOPIC_INFRA_DELETE.to_string(),
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
            .delete_topic_infra_config(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to delete topic infra config: {err}"))?;

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id.clone()),
                agent_id: None,
                action: TOOL_TOPIC_INFRA_DELETE.to_string(),
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

    pub(super) async fn execute_topic_infra_rollback(&self, arguments: &str) -> Result<String> {
        let args: TopicInfraRollbackArgs = Self::parse_args(arguments, TOOL_TOPIC_INFRA_ROLLBACK)?;
        let topic_id = self.resolve_lookup_topic_id(args.topic_id).await?;
        let current = self
            .storage
            .get_topic_infra_config(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current topic infra config: {err}"))?;
        let previous = match self.last_topic_infra_mutation(&topic_id).await? {
            Some(event) => Self::previous_from_payload::<TopicInfraConfigRecord>(&event.payload)?,
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
                    action: TOOL_TOPIC_INFRA_ROLLBACK.to_string(),
                    payload: json!({
                        "topic_id": topic_id,
                        "operation": rollback_operation,
                        "previous": current,
                        "restore_to": previous,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            return Self::to_json_string(Self::attach_audit_status(
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
            ));
        }

        let rolled_back_infra = self
            .restore_or_delete_topic_infra(&topic_id, previous.clone())
            .await?;
        let preflight = match rolled_back_infra.as_ref() {
            Some(record) => Some(self.inspect_topic_infra_record(record).await),
            None => None,
        };

        let response_topic_id = topic_id.clone();
        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id),
                agent_id: None,
                action: TOOL_TOPIC_INFRA_ROLLBACK.to_string(),
                payload: json!({
                    "topic_id": response_topic_id,
                    "operation": rollback_operation,
                    "previous": current,
                    "restore_to": previous,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        Self::to_json_string(Self::attach_audit_status(
            json!({
                "ok": true,
                "operation": rollback_operation,
                "topic_infra": rolled_back_infra,
                "preflight": preflight
            }),
            audit_status,
        ))
    }
}
