use super::*;

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum TopicSandboxPruneReason {
    TopicMissing,
    BindingMissing,
    SandboxDisabled,
    #[default]
    All,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicSandboxListArgs {
    #[serde(default)]
    orphaned_only: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicSandboxGetArgs {
    #[serde(default)]
    topic_id: Option<String>,
    #[serde(default)]
    container_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicSandboxCreateArgs {
    topic_id: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicSandboxRecreateArgs {
    topic_id: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicSandboxDeleteArgs {
    #[serde(default)]
    topic_id: Option<String>,
    #[serde(default)]
    container_name: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicSandboxPruneArgs {
    #[serde(default)]
    reason: TopicSandboxPruneReason,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
struct TopicSandboxInventoryRecord {
    container_id: String,
    container_name: String,
    image: Option<String>,
    created_at: Option<i64>,
    state: Option<String>,
    status: Option<String>,
    running: bool,
    topic_id: Option<String>,
    chat_id: Option<i64>,
    thread_id: Option<i64>,
    labels: std::collections::HashMap<String, String>,
    bound_topic_exists: bool,
    binding_found: bool,
    sandbox_tools_enabled: Option<bool>,
    orphan_reason: Option<String>,
}

#[derive(Debug)]
enum TopicSandboxTarget {
    TopicId(String),
    ContainerName(String),
}

impl ManagerControlPlaneProvider {
    pub(super) fn topic_sandbox_tools_definitions() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: TOOL_TOPIC_SANDBOX_LIST.to_string(),
                description: "List user-owned topic sandbox containers and orphan status"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "orphaned_only": { "type": "boolean", "description": "Return only containers that look orphaned or disabled" }
                    }
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_SANDBOX_GET.to_string(),
                description: "Inspect a topic sandbox by topic_id or Docker container name"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Canonical topic identifier or unique forum topic alias" },
                        "container_name": { "type": "string", "description": "Exact Docker container name" }
                    }
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_SANDBOX_CREATE.to_string(),
                description: "Ensure a sandbox container exists for a tracked forum topic"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Canonical topic identifier or unique forum topic alias" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without mutating Docker" }
                    },
                    "required": ["topic_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_SANDBOX_RECREATE.to_string(),
                description: "Recreate a topic sandbox container, wiping previous workspace state"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Canonical topic identifier or unique forum topic alias" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without mutating Docker" }
                    },
                    "required": ["topic_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_SANDBOX_DELETE.to_string(),
                description:
                    "Delete a topic sandbox container by topic_id or Docker container name"
                        .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Canonical topic identifier or unique forum topic alias" },
                        "container_name": { "type": "string", "description": "Exact Docker container name" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without mutating Docker" }
                    }
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_SANDBOX_PRUNE.to_string(),
                description:
                    "Delete orphaned or disabled topic sandbox containers for the current user"
                        .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "reason": { "type": "string", "enum": ["topic_missing", "binding_missing", "sandbox_disabled", "all"], "description": "Which orphan class to delete; defaults to all" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without mutating Docker" }
                    }
                }),
            },
        ]
    }

    fn topic_sandbox_scope(&self, topic_id: &str) -> Result<SandboxScope> {
        let (chat_id, thread_id) = Self::parse_canonical_forum_topic_id(topic_id).ok_or_else(|| {
            anyhow!(
                "topic_id '{topic_id}' is not a canonical Telegram forum topic id. Use '<chat_id>:<thread_id>'"
            )
        })?;

        Ok(SandboxScope::new(self.user_id, topic_id.to_string())
            .with_transport_metadata(Some(chat_id), Some(thread_id)))
    }

    fn prune_reason_matches(
        reason: TopicSandboxPruneReason,
        record: &TopicSandboxInventoryRecord,
    ) -> bool {
        match reason {
            TopicSandboxPruneReason::TopicMissing => {
                record.orphan_reason.as_deref() == Some("topic_missing")
            }
            TopicSandboxPruneReason::BindingMissing => {
                record.orphan_reason.as_deref() == Some("binding_missing")
            }
            TopicSandboxPruneReason::SandboxDisabled => {
                record.orphan_reason.as_deref() == Some("sandbox_disabled")
            }
            TopicSandboxPruneReason::All => matches!(
                record.orphan_reason.as_deref(),
                Some("topic_missing" | "binding_missing" | "sandbox_disabled")
            ),
        }
    }

    async fn ensure_tracked_forum_topic(&self, topic_id: &str) -> Result<()> {
        let config = self
            .storage
            .get_user_config(self.user_id)
            .await
            .map_err(|err| anyhow!("failed to load user config for topic sandbox: {err}"))?;
        if config.contexts.contains_key(topic_id) {
            return Ok(());
        }

        bail!("topic_id '{topic_id}' is not tracked in the user topic catalog")
    }

    async fn build_topic_sandbox_inventory(
        &self,
        containers: Vec<SandboxContainerRecord>,
    ) -> Result<Vec<TopicSandboxInventoryRecord>> {
        let config = self
            .storage
            .get_user_config(self.user_id)
            .await
            .map_err(|err| {
                anyhow!("failed to load user config for topic sandbox inventory: {err}")
            })?;

        let mut records = Vec::with_capacity(containers.len());
        for container in containers {
            let topic_id = container.scope.clone();
            let canonical_topic_id = topic_id
                .as_deref()
                .filter(|topic_id| Self::is_canonical_forum_topic_id(topic_id))
                .map(str::to_string);
            let bound_topic_exists = canonical_topic_id
                .as_ref()
                .is_some_and(|topic_id| config.contexts.contains_key(topic_id));

            let (binding_found, sandbox_tools_enabled) =
                if let Some(topic_id) = canonical_topic_id.as_ref() {
                    let binding = self
                        .storage
                        .get_topic_binding(self.user_id, topic_id.clone())
                        .await
                        .map_err(|err| {
                            anyhow!("failed to get topic binding for topic sandbox: {err}")
                        })?;

                    if let Some(binding) = binding {
                        let catalog = self.topic_agent_tool_catalog(topic_id).await?;
                        let profile = self
                            .storage
                            .get_agent_profile(self.user_id, binding.agent_id)
                            .await
                            .map_err(|err| {
                                anyhow!("failed to get agent profile for topic sandbox: {err}")
                            })?;
                        let snapshot = Self::topic_agent_tool_snapshot(
                            &catalog,
                            profile.as_ref().map(|profile| &profile.profile),
                        );
                        (true, Some(Self::sandbox_provider_enabled(&snapshot)))
                    } else {
                        (false, None)
                    }
                } else {
                    (false, None)
                };

            let orphan_reason = if canonical_topic_id.is_none() {
                Some("non_topic_scope".to_string())
            } else if !bound_topic_exists {
                Some("topic_missing".to_string())
            } else if !binding_found {
                Some("binding_missing".to_string())
            } else if sandbox_tools_enabled == Some(false) {
                Some("sandbox_disabled".to_string())
            } else {
                None
            };

            records.push(TopicSandboxInventoryRecord {
                container_id: container.container_id,
                container_name: container.container_name,
                image: container.image,
                created_at: container.created_at,
                state: container.state,
                status: container.status,
                running: container.running,
                topic_id,
                chat_id: container.chat_id,
                thread_id: container.thread_id,
                labels: container.labels,
                bound_topic_exists,
                binding_found,
                sandbox_tools_enabled,
                orphan_reason,
            });
        }

        records.sort_by(|left, right| left.container_name.cmp(&right.container_name));
        Ok(records)
    }

    async fn get_topic_sandbox_inventory_by_name(
        &self,
        container_name: &str,
    ) -> Result<Option<TopicSandboxInventoryRecord>> {
        let Some(container) = self
            .sandbox_control
            .get_topic_sandbox(self.user_id, container_name)
            .await?
        else {
            return Ok(None);
        };

        Ok(self
            .build_topic_sandbox_inventory(vec![container])
            .await?
            .into_iter()
            .next())
    }

    async fn get_topic_sandbox_inventory_by_topic(
        &self,
        topic_id: &str,
    ) -> Result<Option<TopicSandboxInventoryRecord>> {
        let scope = self.topic_sandbox_scope(topic_id)?;
        self.get_topic_sandbox_inventory_by_name(&scope.container_name())
            .await
    }

    async fn resolve_topic_sandbox_target(
        &self,
        topic_id: Option<String>,
        container_name: Option<String>,
        mutation: bool,
    ) -> Result<TopicSandboxTarget> {
        match (topic_id, container_name) {
            (Some(topic_id), None) => {
                let topic_id = if mutation {
                    self.resolve_mutation_topic_id(topic_id).await?
                } else {
                    self.resolve_lookup_topic_id(topic_id).await?
                };
                Ok(TopicSandboxTarget::TopicId(topic_id))
            }
            (None, Some(container_name)) => Ok(TopicSandboxTarget::ContainerName(
                Self::validate_non_empty(container_name, "container_name")?,
            )),
            (Some(_), Some(_)) => {
                bail!("provide either topic_id or container_name, not both")
            }
            (None, None) => bail!("either topic_id or container_name is required"),
        }
    }

    pub(super) async fn cleanup_topic_sandbox_for_topic_id(
        &self,
        topic_id: &str,
    ) -> serde_json::Value {
        let Some(topic) = Self::forum_topic_action_from_topic_id(topic_id) else {
            return json!({
                "skipped": true,
                "reason": "topic_id is not a canonical Telegram forum topic id"
            });
        };

        match self
            .sandbox_cleanup
            .cleanup_topic_sandbox(self.user_id, &topic)
            .await
        {
            Ok(()) => json!({
                "skipped": false,
                "deleted_container": true,
                "topic_id": topic_id,
            }),
            Err(err) => json!({
                "skipped": false,
                "deleted_container": false,
                "topic_id": topic_id,
                "error": err.to_string(),
            }),
        }
    }

    pub(super) async fn execute_topic_sandbox_list(&self, arguments: &str) -> Result<String> {
        let args: TopicSandboxListArgs = Self::parse_args(arguments, TOOL_TOPIC_SANDBOX_LIST)?;
        let sandboxes = self
            .build_topic_sandbox_inventory(
                self.sandbox_control
                    .list_topic_sandboxes(self.user_id)
                    .await?,
            )
            .await?;
        let sandboxes = if args.orphaned_only {
            sandboxes
                .into_iter()
                .filter(|record| Self::prune_reason_matches(TopicSandboxPruneReason::All, record))
                .collect::<Vec<_>>()
        } else {
            sandboxes
        };

        Self::to_json_string(json!({
            "ok": true,
            "orphaned_only": args.orphaned_only,
            "count": sandboxes.len(),
            "sandboxes": sandboxes,
        }))
    }

    pub(super) async fn execute_topic_sandbox_get(&self, arguments: &str) -> Result<String> {
        let args: TopicSandboxGetArgs = Self::parse_args(arguments, TOOL_TOPIC_SANDBOX_GET)?;
        let target = self
            .resolve_topic_sandbox_target(args.topic_id, args.container_name, false)
            .await?;
        let sandbox = match &target {
            TopicSandboxTarget::TopicId(topic_id) => {
                self.get_topic_sandbox_inventory_by_topic(topic_id).await?
            }
            TopicSandboxTarget::ContainerName(container_name) => {
                self.get_topic_sandbox_inventory_by_name(container_name)
                    .await?
            }
        };

        let response = match target {
            TopicSandboxTarget::TopicId(topic_id) => json!({
                "ok": true,
                "found": sandbox.is_some(),
                "topic_id": topic_id,
                "sandbox": sandbox,
            }),
            TopicSandboxTarget::ContainerName(container_name) => json!({
                "ok": true,
                "found": sandbox.is_some(),
                "container_name": container_name,
                "sandbox": sandbox,
            }),
        };

        Self::to_json_string(response)
    }

    pub(super) async fn execute_topic_sandbox_create(&self, arguments: &str) -> Result<String> {
        let args: TopicSandboxCreateArgs = Self::parse_args(arguments, TOOL_TOPIC_SANDBOX_CREATE)?;
        let topic_id = self.resolve_mutation_topic_id(args.topic_id).await?;
        self.ensure_tracked_forum_topic(&topic_id).await?;
        let scope = self.topic_sandbox_scope(&topic_id)?;
        let previous = self.get_topic_sandbox_inventory_by_topic(&topic_id).await?;
        let container_name = scope.container_name();

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: Some(topic_id.clone()),
                    agent_id: None,
                    action: TOOL_TOPIC_SANDBOX_CREATE.to_string(),
                    payload: json!({
                        "topic_id": topic_id,
                        "container_name": container_name,
                        "previous": previous.clone(),
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            return Self::to_json_string(Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": "create",
                        "topic_id": topic_id,
                        "container_name": container_name,
                    },
                    "previous": previous,
                }),
                audit_status,
            ));
        }

        let sandbox = self
            .build_topic_sandbox_inventory(vec![
                self.sandbox_control.ensure_topic_sandbox(scope).await?,
            ])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("topic sandbox inventory is empty after create"))?;
        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id.clone()),
                agent_id: None,
                action: TOOL_TOPIC_SANDBOX_CREATE.to_string(),
                payload: json!({
                    "topic_id": topic_id,
                    "container_name": sandbox.container_name.clone(),
                    "previous": previous,
                    "sandbox": sandbox.clone(),
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        Self::to_json_string(Self::attach_audit_status(
            json!({
                "ok": true,
                "sandbox": sandbox,
            }),
            audit_status,
        ))
    }

    pub(super) async fn execute_topic_sandbox_recreate(&self, arguments: &str) -> Result<String> {
        let args: TopicSandboxRecreateArgs =
            Self::parse_args(arguments, TOOL_TOPIC_SANDBOX_RECREATE)?;
        let topic_id = self.resolve_mutation_topic_id(args.topic_id).await?;
        let scope = self.topic_sandbox_scope(&topic_id)?;
        let previous = self.get_topic_sandbox_inventory_by_topic(&topic_id).await?;
        let container_name = scope.container_name();

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: Some(topic_id.clone()),
                    agent_id: None,
                    action: TOOL_TOPIC_SANDBOX_RECREATE.to_string(),
                    payload: json!({
                        "topic_id": topic_id,
                        "container_name": container_name,
                        "previous": previous.clone(),
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            return Self::to_json_string(Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": "recreate",
                        "topic_id": topic_id,
                        "container_name": container_name,
                    },
                    "previous": previous,
                }),
                audit_status,
            ));
        }

        let sandbox = self
            .build_topic_sandbox_inventory(vec![
                self.sandbox_control.recreate_topic_sandbox(scope).await?,
            ])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("topic sandbox inventory is empty after recreate"))?;
        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id.clone()),
                agent_id: None,
                action: TOOL_TOPIC_SANDBOX_RECREATE.to_string(),
                payload: json!({
                    "topic_id": topic_id,
                    "container_name": sandbox.container_name.clone(),
                    "previous": previous,
                    "sandbox": sandbox.clone(),
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        Self::to_json_string(Self::attach_audit_status(
            json!({
                "ok": true,
                "sandbox": sandbox,
            }),
            audit_status,
        ))
    }

    async fn topic_sandbox_delete_preview(
        &self,
        target: &TopicSandboxTarget,
        previous: Option<TopicSandboxInventoryRecord>,
    ) -> Result<String> {
        let (topic_id, container_name, preview_target) = match target {
            TopicSandboxTarget::TopicId(topic_id) => {
                let scope = self.topic_sandbox_scope(topic_id)?;
                (
                    Some(topic_id.clone()),
                    scope.container_name(),
                    json!({ "topic_id": topic_id }),
                )
            }
            TopicSandboxTarget::ContainerName(container_name) => (
                previous
                    .as_ref()
                    .and_then(|sandbox| sandbox.topic_id.clone()),
                container_name.clone(),
                json!({ "container_name": container_name }),
            ),
        };
        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: topic_id.clone(),
                agent_id: None,
                action: TOOL_TOPIC_SANDBOX_DELETE.to_string(),
                payload: json!({
                    "topic_id": topic_id,
                    "container_name": container_name,
                    "previous": previous.clone(),
                    "outcome": Self::dry_run_outcome(true)
                }),
            })
            .await;

        Self::to_json_string(Self::attach_audit_status(
            json!({
                "ok": true,
                "dry_run": true,
                "preview": {
                    "operation": "delete",
                    "target": preview_target,
                },
                "previous": previous,
            }),
            audit_status,
        ))
    }

    async fn apply_topic_sandbox_delete(
        &self,
        target: TopicSandboxTarget,
        previous: Option<TopicSandboxInventoryRecord>,
    ) -> Result<String> {
        let (deleted, topic_id, container_name) = match target {
            TopicSandboxTarget::TopicId(topic_id) => {
                let scope = self.topic_sandbox_scope(&topic_id)?;
                let deleted = self
                    .sandbox_control
                    .delete_topic_sandbox_by_scope(scope.clone())
                    .await?;
                (deleted, Some(topic_id), scope.container_name())
            }
            TopicSandboxTarget::ContainerName(container_name) => {
                let deleted = self
                    .sandbox_control
                    .delete_topic_sandbox_by_name(self.user_id, &container_name)
                    .await?;
                (
                    deleted,
                    previous
                        .as_ref()
                        .and_then(|sandbox| sandbox.topic_id.clone()),
                    container_name,
                )
            }
        };

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: topic_id.clone(),
                agent_id: None,
                action: TOOL_TOPIC_SANDBOX_DELETE.to_string(),
                payload: json!({
                    "topic_id": topic_id,
                    "container_name": container_name,
                    "previous": previous.clone(),
                    "deleted": deleted,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        Self::to_json_string(Self::attach_audit_status(
            json!({
                "ok": true,
                "deleted": deleted,
                "container_name": container_name,
                "sandbox": previous,
            }),
            audit_status,
        ))
    }

    pub(super) async fn execute_topic_sandbox_delete(&self, arguments: &str) -> Result<String> {
        let args: TopicSandboxDeleteArgs = Self::parse_args(arguments, TOOL_TOPIC_SANDBOX_DELETE)?;
        let target = self
            .resolve_topic_sandbox_target(args.topic_id, args.container_name, true)
            .await?;
        let previous = match &target {
            TopicSandboxTarget::TopicId(topic_id) => {
                self.get_topic_sandbox_inventory_by_topic(topic_id).await?
            }
            TopicSandboxTarget::ContainerName(container_name) => {
                self.get_topic_sandbox_inventory_by_name(container_name)
                    .await?
            }
        };

        if args.dry_run {
            return self.topic_sandbox_delete_preview(&target, previous).await;
        }

        self.apply_topic_sandbox_delete(target, previous).await
    }

    pub(super) async fn execute_topic_sandbox_prune(&self, arguments: &str) -> Result<String> {
        let args: TopicSandboxPruneArgs = Self::parse_args(arguments, TOOL_TOPIC_SANDBOX_PRUNE)?;
        let candidates = self
            .build_topic_sandbox_inventory(
                self.sandbox_control
                    .list_topic_sandboxes(self.user_id)
                    .await?,
            )
            .await?
            .into_iter()
            .filter(|record| Self::prune_reason_matches(args.reason, record))
            .collect::<Vec<_>>();

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: None,
                    agent_id: None,
                    action: TOOL_TOPIC_SANDBOX_PRUNE.to_string(),
                    payload: json!({
                        "reason": args.reason,
                        "count": candidates.len(),
                        "candidates": candidates.clone(),
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            return Self::to_json_string(Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "reason": args.reason,
                    "count": candidates.len(),
                    "candidates": candidates,
                }),
                audit_status,
            ));
        }

        let mut deleted = Vec::new();
        let mut errors = Vec::new();
        for candidate in &candidates {
            match self
                .sandbox_control
                .delete_topic_sandbox_by_name(self.user_id, &candidate.container_name)
                .await
            {
                Ok(true) => deleted.push(candidate.container_name.clone()),
                Ok(false) => {}
                Err(err) => errors.push(format!(
                    "failed to delete {}: {err}",
                    candidate.container_name
                )),
            }
        }

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: None,
                agent_id: None,
                action: TOOL_TOPIC_SANDBOX_PRUNE.to_string(),
                payload: json!({
                    "reason": args.reason,
                    "count": candidates.len(),
                    "candidates": candidates.clone(),
                    "deleted": deleted.clone(),
                    "errors": errors.clone(),
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        Self::to_json_string(Self::attach_audit_status(
            json!({
                "ok": true,
                "reason": args.reason,
                "count": candidates.len(),
                "candidates": candidates,
                "deleted": deleted,
                "errors": errors,
            }),
            audit_status,
        ))
    }
}
