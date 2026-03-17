use super::*;

impl ManagerControlPlaneProvider {
    async fn persist_forum_topic_catalog_entry(
        &self,
        entry: &ForumTopicCatalogEntry,
    ) -> Result<()> {
        let mut config = self
            .storage
            .get_user_config(self.user_id)
            .await
            .map_err(|err| anyhow!("failed to load user config for {}: {err}", entry.topic_id))?;
        Self::upsert_forum_topic_catalog_entry(&mut config, entry);
        self.storage
            .update_user_config(self.user_id, config)
            .await
            .map_err(|err| anyhow!("failed to update user config for {}: {err}", entry.topic_id))
    }

    pub(super) async fn list_forum_topic_catalog_entries(
        &self,
        requested_chat_id: Option<i64>,
        include_closed: bool,
    ) -> Result<Vec<ForumTopicCatalogEntry>> {
        let config = self
            .storage
            .get_user_config(self.user_id)
            .await
            .map_err(|err| anyhow!("failed to load user config for forum topic listing: {err}"))?;
        let effective_chat_id = requested_chat_id.or_else(|| self.resolve_default_forum_chat_id());
        let mut topics = config
            .contexts
            .iter()
            .filter_map(|(context_key, context)| {
                Self::forum_topic_catalog_entry_from_context(context_key, context)
            })
            .filter(|entry| effective_chat_id.is_none_or(|chat_id| entry.chat_id == chat_id))
            .filter(|entry| include_closed || !entry.closed)
            .collect::<Vec<_>>();
        topics.sort_by_key(|entry| (entry.chat_id, entry.thread_id));
        Ok(topics)
    }

    async fn cleanup_forum_topic_artifacts(
        &self,
        topic: &ForumTopicActionResult,
    ) -> (serde_json::Value, Option<String>) {
        let context_key = Self::forum_topic_context_key(topic.chat_id, topic.thread_id);
        let binding_keys = Self::forum_topic_binding_keys(topic.chat_id, topic.thread_id);
        let mut errors = Vec::new();
        let deleted_agent_memory = self
            .clear_forum_topic_agent_memory(&context_key, &mut errors)
            .await;
        let deleted_chat_history_for_context = self
            .clear_forum_topic_chat_history(&context_key, &mut errors)
            .await;
        let deleted_chat_history = deleted_chat_history_for_context;
        let deleted_topic_context = self
            .delete_forum_topic_context_record(&context_key, &mut errors)
            .await;
        let deleted_topic_agents_md = self
            .delete_forum_topic_agents_md_record(&context_key, &mut errors)
            .await;
        let deleted_topic_infra = self
            .delete_forum_topic_infra_record(&context_key, &mut errors)
            .await;
        self.delete_forum_topic_bindings(&binding_keys, &mut errors)
            .await;
        let removed_context_config = self
            .remove_forum_topic_context_config(&context_key, &mut errors)
            .await;
        let deleted_container = self
            .cleanup_forum_topic_sandbox(topic, &context_key, &mut errors)
            .await;

        let cleanup = json!({
            "context_key": context_key,
            "deleted_chat_history": deleted_chat_history,
            "deleted_chat_history_for_context": deleted_chat_history_for_context,
            "deleted_agent_memory": deleted_agent_memory,
            "deleted_topic_context": deleted_topic_context,
            "deleted_topic_agents_md": deleted_topic_agents_md,
            "deleted_topic_infra": deleted_topic_infra,
            "deleted_topic_binding_keys": binding_keys,
            "removed_context_config": removed_context_config,
            "deleted_container": deleted_container,
            "errors": errors,
        });

        let error = cleanup
            .get("errors")
            .and_then(|value| value.as_array())
            .filter(|errors| !errors.is_empty())
            .map(|errors| {
                errors
                    .iter()
                    .filter_map(|value| value.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            });

        (cleanup, error)
    }

    async fn clear_forum_topic_agent_memory(
        &self,
        context_key: &str,
        errors: &mut Vec<String>,
    ) -> bool {
        match self
            .storage
            .clear_agent_memory_for_context(self.user_id, context_key.to_string())
            .await
        {
            Ok(()) => true,
            Err(err) => {
                errors.push(format!(
                    "failed to clear agent memory for {context_key}: {err}"
                ));
                false
            }
        }
    }

    async fn clear_forum_topic_chat_history(
        &self,
        context_key: &str,
        errors: &mut Vec<String>,
    ) -> bool {
        match self
            .storage
            .clear_chat_history_for_context(self.user_id, context_key.to_string())
            .await
        {
            Ok(()) => true,
            Err(err) => {
                errors.push(format!(
                    "failed to clear chat history for {context_key}: {err}"
                ));
                false
            }
        }
    }

    async fn delete_forum_topic_context_record(
        &self,
        context_key: &str,
        errors: &mut Vec<String>,
    ) -> bool {
        match self
            .storage
            .delete_topic_context(self.user_id, context_key.to_string())
            .await
        {
            Ok(()) => true,
            Err(err) => {
                errors.push(format!(
                    "failed to delete topic context for {context_key}: {err}"
                ));
                false
            }
        }
    }

    async fn delete_forum_topic_agents_md_record(
        &self,
        context_key: &str,
        errors: &mut Vec<String>,
    ) -> bool {
        match self
            .storage
            .delete_topic_agents_md(self.user_id, context_key.to_string())
            .await
        {
            Ok(()) => true,
            Err(err) => {
                errors.push(format!(
                    "failed to delete topic AGENTS.md for {context_key}: {err}"
                ));
                false
            }
        }
    }

    async fn delete_forum_topic_infra_record(
        &self,
        context_key: &str,
        errors: &mut Vec<String>,
    ) -> bool {
        match self
            .storage
            .delete_topic_infra_config(self.user_id, context_key.to_string())
            .await
        {
            Ok(()) => true,
            Err(err) => {
                errors.push(format!(
                    "failed to delete topic infra config for {context_key}: {err}"
                ));
                false
            }
        }
    }

    async fn delete_forum_topic_bindings(&self, binding_keys: &[String], errors: &mut Vec<String>) {
        for topic_binding_key in binding_keys {
            if let Err(err) = self
                .storage
                .delete_topic_binding(self.user_id, topic_binding_key.clone())
                .await
            {
                errors.push(format!(
                    "failed to delete topic binding {topic_binding_key}: {err}"
                ));
            }
        }
    }

    async fn remove_forum_topic_context_config(
        &self,
        context_key: &str,
        errors: &mut Vec<String>,
    ) -> bool {
        match self.storage.get_user_config(self.user_id).await {
            Ok(mut config) => {
                let removed_context_config = config.contexts.remove(context_key).is_some();
                if let Err(err) = self.storage.update_user_config(self.user_id, config).await {
                    errors.push(format!(
                        "failed to update user config for {context_key}: {err}"
                    ));
                }
                removed_context_config
            }
            Err(err) => {
                errors.push(format!(
                    "failed to load user config for {context_key}: {err}"
                ));
                false
            }
        }
    }

    async fn cleanup_forum_topic_sandbox(
        &self,
        topic: &ForumTopicActionResult,
        context_key: &str,
        errors: &mut Vec<String>,
    ) -> bool {
        match self
            .sandbox_cleanup
            .cleanup_topic_sandbox(self.user_id, topic)
            .await
        {
            Ok(()) => true,
            Err(err) => {
                errors.push(format!(
                    "failed to destroy sandbox for {context_key}: {err}"
                ));
                false
            }
        }
    }

    fn forum_topic_icon_color_schema() -> serde_json::Value {
        json!({
            "type": "integer",
            "enum": TELEGRAM_FORUM_ICON_COLORS,
            "description": "Optional Telegram forum icon color"
        })
    }

    fn forum_topic_provision_ssh_agent_definition() -> ToolDefinition {
        ToolDefinition {
            name: TOOL_FORUM_TOPIC_PROVISION_SSH_AGENT.to_string(),
            description: "Atomically create a Telegram forum topic, derive the canonical topic_id, create an SSH-ready agent profile, bind the topic, and attach topic-scoped SSH infra"
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "chat_id": { "type": "integer", "description": "Optional target forum chat id; omit to use the current manager forum chat" },
                    "name": { "type": "string", "description": "Forum topic name; also used as default agent_id and target_name when omitted" },
                    "icon_color": Self::forum_topic_icon_color_schema(),
                    "icon_custom_emoji_id": { "type": "string", "description": "Optional custom emoji icon id" },
                    "agent_id": { "type": "string", "description": "Optional explicit agent id; defaults to the topic name" },
                    "system_prompt": { "type": "string", "description": "Optional agent system prompt instructions" },
                    "description": { "type": "string", "description": "Optional human-readable profile description" },
                    "topic_context": { "type": "string", "description": "Optional persistent topic context" },
                    "target_name": { "type": "string", "description": "Optional infra target name; defaults to the topic name" },
                    "host": { "type": "string", "description": "SSH host or DNS name" },
                    "port": { "type": "integer", "description": "SSH port, defaults to 22" },
                    "remote_user": { "type": "string", "description": "Remote SSH username" },
                    "auth_mode": { "type": "string", "enum": ["none", "password", "private_key"], "description": "SSH authentication mode" },
                    "secret_ref": { "type": "string", "description": "Opaque secret reference for SSH auth material" },
                    "sudo_secret_ref": { "type": "string", "description": "Opaque secret reference for sudo password material" },
                    "environment": { "type": "string", "description": "Optional environment label such as prod or stage" },
                    "tags": { "type": "array", "items": { "type": "string" }, "description": "Optional free-form target tags" },
                    "allowed_tool_modes": { "type": "array", "items": { "type": "string", "enum": ["exec", "sudo_exec", "read_file", "apply_file_edit", "check_process"] }, "description": "Allowlisted SSH tool modes; defaults to all SSH modes" },
                    "approval_required_modes": { "type": "array", "items": { "type": "string", "enum": ["exec", "sudo_exec", "read_file", "apply_file_edit", "check_process"] }, "description": "Modes that always require approval; defaults to sudo_exec and apply_file_edit" },
                    "dry_run": { "type": "boolean", "description": "Validate and preview without mutating Telegram or storage" }
                },
                "required": ["name", "host", "remote_user", "auth_mode"]
            }),
        }
    }

    pub(super) fn lifecycle_tools_definitions() -> Vec<ToolDefinition> {
        vec![
            Self::forum_topic_provision_ssh_agent_definition(),
            ToolDefinition {
                name: TOOL_FORUM_TOPIC_CREATE.to_string(),
                description: "Create Telegram forum topic; omit chat_id to use current forum chat"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "chat_id": { "type": "integer", "description": "Optional target chat identifier; omit to use the current forum chat when available" },
                        "name": { "type": "string", "description": "Forum topic name" },
                        "icon_color": Self::forum_topic_icon_color_schema(),
                        "icon_custom_emoji_id": { "type": "string", "description": "Optional custom emoji icon id" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without mutation" }
                    },
                    "required": ["name"]
                }),
            },
            ToolDefinition {
                name: TOOL_FORUM_TOPIC_EDIT.to_string(),
                description: "Edit Telegram forum topic; omit chat_id to use current forum chat"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "chat_id": { "type": "integer", "description": "Optional target chat identifier; omit to use the current forum chat when available" },
                        "thread_id": { "type": "integer", "description": "Forum topic thread identifier" },
                        "name": { "type": "string", "description": "Optional new topic name" },
                        "icon_custom_emoji_id": { "type": "string", "description": "Optional icon emoji id; empty clears icon" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without mutation" }
                    },
                    "required": ["thread_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_FORUM_TOPIC_CLOSE.to_string(),
                description: "Close Telegram forum topic; omit chat_id to use current forum chat"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "chat_id": { "type": "integer", "description": "Optional target chat identifier; omit to use the current forum chat when available" },
                        "thread_id": { "type": "integer", "description": "Forum topic thread identifier" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without mutation" }
                    },
                    "required": ["thread_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_FORUM_TOPIC_REOPEN.to_string(),
                description: "Reopen Telegram forum topic; omit chat_id to use current forum chat"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "chat_id": { "type": "integer", "description": "Optional target chat identifier; omit to use the current forum chat when available" },
                        "thread_id": { "type": "integer", "description": "Forum topic thread identifier" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without mutation" }
                    },
                    "required": ["thread_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_FORUM_TOPIC_DELETE.to_string(),
                description: "Delete Telegram forum topic; omit chat_id to use current forum chat"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "chat_id": { "type": "integer", "description": "Optional target chat identifier; omit to use the current forum chat when available" },
                        "thread_id": { "type": "integer", "description": "Forum topic thread identifier" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without mutation" }
                    },
                    "required": ["thread_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_FORUM_TOPIC_LIST.to_string(),
                description:
                    "List active Telegram forum topics tracked in persisted S3 topic records"
                        .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "chat_id": { "type": "integer", "description": "Optional target chat identifier; omit to use the current forum chat when available" },
                        "include_closed": { "type": "boolean", "description": "Include closed topics in the result" }
                    }
                }),
            },
        ]
    }

    pub(super) fn forum_topic_action_from_topic_id(
        topic_id: &str,
    ) -> Option<ForumTopicActionResult> {
        let (chat_id, thread_id) = Self::parse_canonical_forum_topic_id(topic_id)?;
        Some(ForumTopicActionResult { chat_id, thread_id })
    }

    fn forum_topic_payload(result: &ForumTopicCreateResult) -> serde_json::Value {
        json!({
            "chat_id": result.chat_id,
            "thread_id": result.thread_id,
            "topic_id": Self::forum_topic_context_key(result.chat_id, result.thread_id),
            "name": result.name,
            "icon_color": result.icon_color,
            "icon_custom_emoji_id": result.icon_custom_emoji_id,
        })
    }

    fn build_default_ssh_agent_profile(
        agent_id: &str,
        topic_name: &str,
        system_prompt: Option<String>,
        description: Option<String>,
        host: &str,
    ) -> serde_json::Value {
        let default_description = format!("SSH agent for managing server at {host}");
        json!({
            "name": topic_name,
            "agentId": agent_id,
            "description": description.unwrap_or(default_description),
            "systemPrompt": system_prompt,
            "allowedTools": default_ssh_agent_allowed_tools(),
            "blockedTools": topic_agent_default_blocked_tools(),
        })
    }

    fn build_forum_topic_provision_plan(
        &self,
        args: ForumTopicProvisionSshAgentArgs,
    ) -> Result<ForumTopicProvisionSshAgentPlan> {
        let name = Self::validate_non_empty(args.name, "name")?;
        let icon_custom_emoji_id =
            Self::validate_optional_non_empty(args.icon_custom_emoji_id, "icon_custom_emoji_id")?;
        let icon_color = Self::validate_forum_icon_color(args.icon_color)?;
        let agent_id = Self::validate_optional_non_empty(args.agent_id, "agent_id")?
            .unwrap_or_else(|| name.clone());
        let system_prompt = Self::validate_optional_non_empty(args.system_prompt, "system_prompt")?;
        let description = Self::validate_optional_non_empty(args.description, "description")?;
        let topic_context = Self::validate_optional_non_empty(args.topic_context, "topic_context")?;
        let target_name = Self::validate_optional_non_empty(args.target_name, "target_name")?
            .unwrap_or_else(|| name.clone());
        let host = Self::validate_non_empty(args.host, "host")?;
        let remote_user = Self::validate_non_empty(args.remote_user, "remote_user")?;
        if args.port == 0 {
            bail!("port must be a positive integer");
        }

        let secret_ref = Self::validate_optional_non_empty(args.secret_ref, "secret_ref")?;
        let sudo_secret_ref =
            Self::validate_optional_non_empty(args.sudo_secret_ref, "sudo_secret_ref")?;
        let environment = Self::validate_optional_non_empty(args.environment, "environment")?;
        let tags = Self::normalize_tags(args.tags);
        let allowed_tool_modes = Self::normalize_tool_modes(args.allowed_tool_modes);
        if allowed_tool_modes.is_empty() {
            bail!("allowed_tool_modes must not be empty");
        }
        let approval_required_modes = Self::normalize_tool_modes(args.approval_required_modes);
        let profile = Self::validate_profile_object(Self::build_default_ssh_agent_profile(
            &agent_id,
            &name,
            system_prompt,
            description,
            &host,
        ))?;

        Ok(ForumTopicProvisionSshAgentPlan {
            request: ForumTopicCreateRequest {
                chat_id: args.chat_id,
                name,
                icon_color,
                icon_custom_emoji_id,
            },
            agent_id,
            profile,
            topic_context,
            target_name,
            host,
            port: args.port,
            remote_user,
            auth_mode: args.auth_mode,
            secret_ref,
            sudo_secret_ref,
            environment,
            tags,
            allowed_tool_modes,
            approval_required_modes,
            dry_run: args.dry_run,
        })
    }

    async fn dry_run_forum_topic_provision_ssh_agent(
        &self,
        plan: &ForumTopicProvisionSshAgentPlan,
    ) -> Result<String> {
        let preview_infra =
            self.topic_infra_preview_record_from_plan("<created_topic_id>".to_string(), plan);
        let preview_preflight = self.inspect_topic_infra_record(&preview_infra).await;
        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: None,
                agent_id: Some(plan.agent_id.clone()),
                action: TOOL_FORUM_TOPIC_PROVISION_SSH_AGENT.to_string(),
                payload: json!({
                    "name": plan.request.name,
                    "agent_id": plan.agent_id,
                    "host": plan.host,
                    "port": plan.port,
                    "remote_user": plan.remote_user,
                    "auth_mode": plan.auth_mode,
                    "secret_ref": plan.secret_ref,
                    "sudo_secret_ref": plan.sudo_secret_ref,
                    "topic_context": plan.topic_context,
                    "outcome": Self::dry_run_outcome(true)
                }),
            })
            .await;

        Self::to_json_string(Self::attach_audit_status(
            json!({
                "ok": true,
                "dry_run": true,
                "preview": {
                    "forum_topic_request": plan.request,
                    "agent_id": plan.agent_id,
                    "profile": plan.profile,
                    "topic_context": plan.topic_context,
                    "topic_infra": Self::topic_infra_value_from_record(&preview_infra),
                    "preflight": preview_preflight,
                    "canonical_topic_id_note": "topic_id will be derived automatically as '<chat_id>:<thread_id>' after Telegram creates the topic"
                }
            }),
            audit_status,
        ))
    }

    async fn execute_forum_topic_provision_substeps(
        &self,
        topic_id: &str,
        created_topic: &ForumTopicCreateResult,
        plan: &ForumTopicProvisionSshAgentPlan,
    ) -> Result<(String, Option<String>, String, String)> {
        let profile_response = self
            .execute_agent_profile_upsert(&Self::to_json_string(json!({
                "agent_id": plan.agent_id,
                "profile": plan.profile,
            }))?)
            .await?;
        let topic_context_response = match plan.topic_context.as_ref() {
            Some(context) => Some(
                self.execute_topic_context_upsert(&Self::to_json_string(json!({
                    "topic_id": topic_id,
                    "context": context,
                }))?)
                .await?,
            ),
            None => None,
        };
        let binding_response = self
            .execute_topic_binding_set(&Self::to_json_string(json!({
                "topic_id": topic_id,
                "agent_id": plan.agent_id,
                "binding_kind": "manual",
                "chat_id": created_topic.chat_id,
                "thread_id": created_topic.thread_id,
            }))?)
            .await?;
        let infra_response = self
            .execute_topic_infra_upsert(&Self::to_json_string(json!({
                "topic_id": topic_id,
                "target_name": plan.target_name,
                "host": plan.host,
                "port": plan.port,
                "remote_user": plan.remote_user,
                "auth_mode": plan.auth_mode,
                "secret_ref": plan.secret_ref,
                "sudo_secret_ref": plan.sudo_secret_ref,
                "environment": plan.environment,
                "tags": plan.tags,
                "allowed_tool_modes": plan.allowed_tool_modes,
                "approval_required_modes": plan.approval_required_modes,
            }))?)
            .await?;

        Ok((
            profile_response,
            topic_context_response,
            binding_response,
            infra_response,
        ))
    }

    async fn cleanup_failed_forum_topic_provision(&self, created_topic: &ForumTopicCreateResult) {
        if let Some(lifecycle) = &self.topic_lifecycle {
            let _ = lifecycle
                .forum_topic_delete(ForumTopicThreadRequest {
                    chat_id: Some(created_topic.chat_id),
                    thread_id: created_topic.thread_id,
                })
                .await;
        }
    }

    pub(super) async fn execute_forum_topic_provision_ssh_agent(
        &self,
        arguments: &str,
    ) -> Result<String> {
        let args: ForumTopicProvisionSshAgentArgs =
            Self::parse_args(arguments, TOOL_FORUM_TOPIC_PROVISION_SSH_AGENT)?;
        let plan = self.build_forum_topic_provision_plan(args)?;
        if plan.dry_run {
            return self.dry_run_forum_topic_provision_ssh_agent(&plan).await;
        }

        let created_topic = self
            .topic_lifecycle()?
            .forum_topic_create(plan.request.clone())
            .await?;
        let topic_id =
            Self::forum_topic_context_key(created_topic.chat_id, created_topic.thread_id);
        self.persist_forum_topic_catalog_entry(&ForumTopicCatalogEntry {
            topic_id: topic_id.clone(),
            chat_id: created_topic.chat_id,
            thread_id: created_topic.thread_id,
            name: Some(created_topic.name.clone()),
            icon_color: Some(created_topic.icon_color),
            icon_custom_emoji_id: created_topic.icon_custom_emoji_id.clone(),
            closed: false,
        })
        .await?;

        let (profile_response, topic_context_response, binding_response, infra_response) =
            match self
                .execute_forum_topic_provision_substeps(&topic_id, &created_topic, &plan)
                .await
            {
                Ok(result) => result,
                Err(error) => {
                    self.cleanup_failed_forum_topic_provision(&created_topic)
                        .await;
                    return Err(error);
                }
            };

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id.clone()),
                agent_id: Some(plan.agent_id.clone()),
                action: TOOL_FORUM_TOPIC_PROVISION_SSH_AGENT.to_string(),
                payload: json!({
                    "topic_id": topic_id,
                    "agent_id": plan.agent_id,
                    "host": plan.host,
                    "port": plan.port,
                    "remote_user": plan.remote_user,
                    "auth_mode": plan.auth_mode,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        let parsed_profile: serde_json::Value = serde_json::from_str(&profile_response)
            .map_err(|err| anyhow!("failed to parse profile response: {err}"))?;
        let parsed_binding: serde_json::Value = serde_json::from_str(&binding_response)
            .map_err(|err| anyhow!("failed to parse binding response: {err}"))?;
        let parsed_infra: serde_json::Value = serde_json::from_str(&infra_response)
            .map_err(|err| anyhow!("failed to parse infra response: {err}"))?;
        let parsed_context = match topic_context_response {
            Some(response) => Some(
                serde_json::from_str::<serde_json::Value>(&response)
                    .map_err(|err| anyhow!("failed to parse topic context response: {err}"))?,
            ),
            None => None,
        };

        Self::to_json_string(Self::attach_audit_status(
            json!({
                "ok": true,
                "provisioned": true,
                "topic": Self::forum_topic_payload(&created_topic),
                "binding": parsed_binding.get("binding").cloned().unwrap_or(serde_json::Value::Null),
                "profile": parsed_profile.get("profile").cloned().unwrap_or(serde_json::Value::Null),
                "topic_context": parsed_context.as_ref().and_then(|value| value.get("topic_context")).cloned(),
                "topic_infra": parsed_infra.get("topic_infra").cloned().unwrap_or(serde_json::Value::Null),
                "preflight": parsed_infra.get("preflight").cloned().unwrap_or(serde_json::Value::Null),
            }),
            audit_status,
        ))
    }

    pub(super) async fn execute_forum_topic_create(&self, arguments: &str) -> Result<String> {
        let args: ForumTopicCreateArgs = Self::parse_args(arguments, TOOL_FORUM_TOPIC_CREATE)?;
        let name = Self::validate_non_empty(args.name, "name")?;
        let icon_custom_emoji_id =
            Self::validate_optional_non_empty(args.icon_custom_emoji_id, "icon_custom_emoji_id")?;
        let icon_color = Self::validate_forum_icon_color(args.icon_color)?;
        let request = ForumTopicCreateRequest {
            chat_id: args.chat_id,
            name,
            icon_color,
            icon_custom_emoji_id,
        };

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: None,
                    agent_id: None,
                    action: TOOL_FORUM_TOPIC_CREATE.to_string(),
                    payload: json!({
                        "request": request,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            let response = Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": TOOL_FORUM_TOPIC_CREATE,
                        "request": request
                    }
                }),
                audit_status,
            );

            return Self::to_json_string(response);
        }

        let result = self
            .topic_lifecycle()?
            .forum_topic_create(request.clone())
            .await?;
        self.persist_forum_topic_catalog_entry(&ForumTopicCatalogEntry {
            topic_id: Self::forum_topic_context_key(result.chat_id, result.thread_id),
            chat_id: result.chat_id,
            thread_id: result.thread_id,
            name: Some(result.name.clone()),
            icon_color: Some(result.icon_color),
            icon_custom_emoji_id: result.icon_custom_emoji_id.clone(),
            closed: false,
        })
        .await?;
        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(Self::forum_topic_context_key(
                    result.chat_id,
                    result.thread_id,
                )),
                agent_id: None,
                action: TOOL_FORUM_TOPIC_CREATE.to_string(),
                payload: json!({
                    "request": request,
                    "result": result,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        let response = Self::attach_audit_status(
            json!({ "ok": true, "topic": Self::forum_topic_payload(&result) }),
            audit_status,
        );
        Self::to_json_string(response)
    }

    pub(super) async fn execute_forum_topic_edit(&self, arguments: &str) -> Result<String> {
        let args: ForumTopicEditArgs = Self::parse_args(arguments, TOOL_FORUM_TOPIC_EDIT)?;
        let thread_id = Self::validate_thread_id(args.thread_id)?;
        let name = Self::validate_optional_non_empty(args.name, "name")?;
        if name.is_none() && args.icon_custom_emoji_id.is_none() {
            bail!("forum_topic_edit requires at least one mutable field");
        }
        let request = ForumTopicEditRequest {
            chat_id: args.chat_id,
            thread_id,
            name,
            icon_custom_emoji_id: args.icon_custom_emoji_id,
        };

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: None,
                    agent_id: None,
                    action: TOOL_FORUM_TOPIC_EDIT.to_string(),
                    payload: json!({
                        "request": request,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            let response = Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": TOOL_FORUM_TOPIC_EDIT,
                        "request": request
                    }
                }),
                audit_status,
            );

            return Self::to_json_string(response);
        }

        let result = self
            .topic_lifecycle()?
            .forum_topic_edit(request.clone())
            .await?;
        let topic_id = Self::forum_topic_context_key(result.chat_id, result.thread_id);
        let mut config = self
            .storage
            .get_user_config(self.user_id)
            .await
            .map_err(|err| anyhow!("failed to load user config for {topic_id}: {err}"))?;
        let mut entry = Self::existing_forum_topic_catalog_entry(&config, &topic_id).unwrap_or(
            ForumTopicCatalogEntry {
                topic_id: topic_id.clone(),
                chat_id: result.chat_id,
                thread_id: result.thread_id,
                name: None,
                icon_color: None,
                icon_custom_emoji_id: None,
                closed: false,
            },
        );
        if let Some(name) = result.name.clone() {
            entry.name = Some(name);
        }
        if result.icon_custom_emoji_id.is_some() {
            entry.icon_custom_emoji_id = result.icon_custom_emoji_id.clone();
        }
        Self::upsert_forum_topic_catalog_entry(&mut config, &entry);
        self.storage
            .update_user_config(self.user_id, config)
            .await
            .map_err(|err| anyhow!("failed to update user config for {topic_id}: {err}"))?;
        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id),
                agent_id: None,
                action: TOOL_FORUM_TOPIC_EDIT.to_string(),
                payload: json!({
                    "request": request,
                    "result": result,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        let response =
            Self::attach_audit_status(json!({ "ok": true, "topic": result }), audit_status);
        Self::to_json_string(response)
    }

    pub(super) async fn execute_forum_topic_thread_action(
        &self,
        arguments: &str,
        tool_name: &str,
    ) -> Result<String> {
        let args: ForumTopicThreadArgs = Self::parse_args(arguments, tool_name)?;
        let request = ForumTopicThreadRequest {
            chat_id: args.chat_id,
            thread_id: Self::validate_thread_id(args.thread_id)?,
        };

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: None,
                    agent_id: None,
                    action: tool_name.to_string(),
                    payload: json!({
                        "request": request,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            let response = Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": tool_name,
                        "request": request
                    }
                }),
                audit_status,
            );

            return Self::to_json_string(response);
        }

        let lifecycle = self.topic_lifecycle()?;
        let result = match tool_name {
            TOOL_FORUM_TOPIC_CLOSE => lifecycle.forum_topic_close(request.clone()).await?,
            TOOL_FORUM_TOPIC_REOPEN => lifecycle.forum_topic_reopen(request.clone()).await?,
            TOOL_FORUM_TOPIC_DELETE => lifecycle.forum_topic_delete(request.clone()).await?,
            _ => bail!("unsupported forum topic thread action: {tool_name}"),
        };
        let derived_topic_id = Self::forum_topic_context_key(result.chat_id, result.thread_id);
        if tool_name != TOOL_FORUM_TOPIC_DELETE {
            let mut config = self
                .storage
                .get_user_config(self.user_id)
                .await
                .map_err(|err| {
                    anyhow!("failed to load user config for {derived_topic_id}: {err}")
                })?;
            let mut entry = Self::existing_forum_topic_catalog_entry(&config, &derived_topic_id)
                .unwrap_or(ForumTopicCatalogEntry {
                    topic_id: derived_topic_id.clone(),
                    chat_id: result.chat_id,
                    thread_id: result.thread_id,
                    name: None,
                    icon_color: None,
                    icon_custom_emoji_id: None,
                    closed: tool_name == TOOL_FORUM_TOPIC_CLOSE,
                });
            entry.closed = tool_name == TOOL_FORUM_TOPIC_CLOSE;
            Self::upsert_forum_topic_catalog_entry(&mut config, &entry);
            self.storage
                .update_user_config(self.user_id, config)
                .await
                .map_err(|err| {
                    anyhow!("failed to update user config for {derived_topic_id}: {err}")
                })?;
        }
        let (cleanup, cleanup_error) = if tool_name == TOOL_FORUM_TOPIC_DELETE {
            self.cleanup_forum_topic_artifacts(&result).await
        } else {
            (json!({ "skipped": true }), None)
        };

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(derived_topic_id),
                agent_id: None,
                action: tool_name.to_string(),
                payload: json!({
                    "request": request,
                    "result": result,
                    "cleanup": cleanup,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        if let Some(cleanup_error) = cleanup_error {
            bail!("forum topic deleted but cleanup failed: {cleanup_error}");
        }

        let response = Self::attach_audit_status(
            json!({ "ok": true, "topic": result, "cleanup": cleanup }),
            audit_status,
        );
        Self::to_json_string(response)
    }

    pub(super) async fn execute_forum_topic_list(&self, arguments: &str) -> Result<String> {
        let args: ForumTopicListArgs = Self::parse_args(arguments, TOOL_FORUM_TOPIC_LIST)?;
        let effective_chat_id = args
            .chat_id
            .or_else(|| self.resolve_default_forum_chat_id());
        let topics = self
            .list_forum_topic_catalog_entries(args.chat_id, args.include_closed)
            .await?;
        Self::to_json_string(json!({
            "ok": true,
            "chat_id": effective_chat_id,
            "include_closed": args.include_closed,
            "count": topics.len(),
            "topics": topics,
        }))
    }
}
