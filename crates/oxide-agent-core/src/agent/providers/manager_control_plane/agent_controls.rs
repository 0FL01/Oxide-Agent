use super::*;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicAgentToolsGetArgs {
    topic_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicAgentToolsMutationArgs {
    topic_id: String,
    tools: Vec<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Clone, Debug)]
struct TopicAgentToolGroup {
    provider: &'static str,
    aliases: &'static [&'static str],
    tools: &'static [&'static str],
}

#[derive(Clone, Debug)]
pub(super) struct TopicAgentToolCatalog {
    groups: Vec<TopicAgentToolGroup>,
    tool_names: BTreeSet<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct TopicAgentToolGroupStatus {
    provider: String,
    available_tools: Vec<String>,
    active_tools: Vec<String>,
    blocked_tools: Vec<String>,
    enabled: bool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(super) struct TopicAgentToolSnapshot {
    policy_mode: String,
    available_tools: Vec<String>,
    active_tools: Vec<String>,
    blocked_tools: Vec<String>,
    allowed_tools_raw: Option<Vec<String>>,
    unknown_profile_tools: Vec<String>,
    provider_statuses: Vec<TopicAgentToolGroupStatus>,
}

#[derive(Debug)]
struct TopicAgentToolMutation {
    profile: serde_json::Value,
    changed: bool,
}

#[derive(Clone, Debug)]
struct TopicAgentToolMutationContext {
    topic_id: String,
    agent_id: String,
    requested_tools: Vec<String>,
    previous: Option<AgentProfileRecord>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicAgentHooksGetArgs {
    topic_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicAgentHooksMutationArgs {
    topic_id: String,
    hooks: Vec<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Clone, Debug)]
struct TopicAgentHookCatalog {
    manageable_hooks: BTreeSet<String>,
    protected_hooks: BTreeSet<String>,
    all_hooks: BTreeSet<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct TopicAgentHookStatus {
    hook: String,
    active: bool,
    manageable: bool,
    protected: bool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct TopicAgentHookSnapshot {
    policy_mode: String,
    available_hooks: Vec<String>,
    active_hooks: Vec<String>,
    disabled_hooks: Vec<String>,
    enabled_hooks_raw: Option<Vec<String>>,
    unknown_profile_hooks: Vec<String>,
    hook_statuses: Vec<TopicAgentHookStatus>,
}

#[derive(Debug)]
struct TopicAgentHookMutation {
    profile: serde_json::Value,
    changed: bool,
}

#[derive(Clone, Debug)]
struct TopicAgentHookMutationContext {
    topic_id: String,
    agent_id: String,
    requested_hooks: Vec<String>,
    previous: Option<AgentProfileRecord>,
}

impl ManagerControlPlaneProvider {
    pub(super) fn topic_agent_tools_management_definitions() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: TOOL_TOPIC_AGENT_TOOLS_GET.to_string(),
                description: "Inspect the effective tool set for the agent bound to a topic"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier or unique forum topic alias" }
                    },
                    "required": ["topic_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_AGENT_TOOLS_ENABLE.to_string(),
                description:
                    "Enable one or more tools or provider groups for the agent bound to a topic"
                        .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier or unique forum topic alias" },
                        "tools": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Tool names or provider aliases like ytdlp, ssh, sandbox, search, reminder"
                        },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without persisting" }
                    },
                    "required": ["topic_id", "tools"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_AGENT_TOOLS_DISABLE.to_string(),
                description:
                    "Disable one or more tools or provider groups for the agent bound to a topic"
                        .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier or unique forum topic alias" },
                        "tools": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Tool names or provider aliases like ytdlp, ssh, sandbox, search, reminder"
                        },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without persisting" }
                    },
                    "required": ["topic_id", "tools"]
                }),
            },
        ]
    }

    pub(super) fn topic_agent_hooks_management_definitions() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: TOOL_TOPIC_AGENT_HOOKS_GET.to_string(),
                description: "Inspect the effective hook set for the agent bound to a topic"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier or unique forum topic alias" }
                    },
                    "required": ["topic_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_AGENT_HOOKS_ENABLE.to_string(),
                description: "Enable one or more manageable hooks for the agent bound to a topic"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier or unique forum topic alias" },
                        "hooks": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Hook names such as workload_distributor, delegation_guard, search_budget, timeout_report"
                        },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without persisting" }
                    },
                    "required": ["topic_id", "hooks"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_AGENT_HOOKS_DISABLE.to_string(),
                description: "Disable one or more manageable hooks for the agent bound to a topic"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier or unique forum topic alias" },
                        "hooks": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Hook names such as workload_distributor, delegation_guard, search_budget, timeout_report"
                        },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without persisting" }
                    },
                    "required": ["topic_id", "hooks"]
                }),
            },
        ]
    }

    fn configured_search_tool_groups() -> Vec<TopicAgentToolGroup> {
        let mut groups = Vec::new();

        #[cfg(feature = "tavily")]
        if crate::config::is_tavily_enabled() {
            groups.push(TopicAgentToolGroup {
                provider: "tavily",
                aliases: &["search", "tavily"],
                tools: TOPIC_AGENT_TAVILY_TOOLS,
            });
        }

        #[cfg(feature = "searxng")]
        if crate::config::is_searxng_enabled() {
            groups.push(TopicAgentToolGroup {
                provider: "searxng",
                aliases: &["search", "searxng"],
                tools: TOPIC_AGENT_SEARXNG_TOOLS,
            });
        }

        #[cfg(feature = "crawl4ai")]
        if crate::config::is_crawl4ai_enabled() {
            groups.push(TopicAgentToolGroup {
                provider: "crawl4ai",
                aliases: &["search", "crawl4ai"],
                tools: TOPIC_AGENT_CRAWL4AI_TOOLS,
            });
        }

        groups
    }

    pub(super) async fn topic_agent_tool_catalog(
        &self,
        topic_id: &str,
    ) -> Result<TopicAgentToolCatalog> {
        let mut groups = vec![
            TopicAgentToolGroup {
                provider: "todos",
                aliases: &["todos"],
                tools: TOPIC_AGENT_TODOS_TOOLS,
            },
            TopicAgentToolGroup {
                provider: "agents_md",
                aliases: &["agents_md", "prompt"],
                tools: TOPIC_AGENT_AGENTS_MD_TOOLS,
            },
            TopicAgentToolGroup {
                provider: "sandbox",
                aliases: &["sandbox"],
                tools: TOPIC_AGENT_SANDBOX_TOOLS,
            },
            TopicAgentToolGroup {
                provider: "filehoster",
                aliases: &["filehoster", "files"],
                tools: TOPIC_AGENT_FILEHOSTER_TOOLS,
            },
            TopicAgentToolGroup {
                provider: "ytdlp",
                aliases: &["ytdlp", "youtube"],
                tools: TOPIC_AGENT_YTDLP_TOOLS,
            },
            TopicAgentToolGroup {
                provider: "delegation",
                aliases: &["delegation", "delegate"],
                tools: TOPIC_AGENT_DELEGATION_TOOLS,
            },
            TopicAgentToolGroup {
                provider: "reminder",
                aliases: &["reminder", "wakeups", "wakeup"],
                tools: TOPIC_AGENT_REMINDER_TOOLS,
            },
        ];

        groups.extend(Self::configured_search_tool_groups());

        let topic_infra = self
            .storage
            .get_topic_infra_config(self.user_id, topic_id.to_string())
            .await
            .map_err(|err| anyhow!("failed to get topic infra config: {err}"))?;
        if topic_infra.is_some() {
            groups.push(TopicAgentToolGroup {
                provider: "ssh",
                aliases: &["ssh"],
                tools: TOPIC_AGENT_SSH_TOOLS,
            });
        }

        #[cfg(feature = "jira")]
        {
            groups.push(TopicAgentToolGroup {
                provider: "jira",
                aliases: &["jira"],
                tools: TOPIC_AGENT_JIRA_TOOLS,
            });
        }

        #[cfg(feature = "mattermost")]
        {
            if crate::agent::providers::MattermostMcpConfig::from_env().is_some() {
                groups.push(TopicAgentToolGroup {
                    provider: "mattermost",
                    aliases: &["mattermost"],
                    tools: TOPIC_AGENT_MATTERMOST_TOOLS,
                });
            }
        }

        // TTS groups - always added as they're conditionally enabled via env vars at runtime
        groups.push(TopicAgentToolGroup {
            provider: "tts_en",
            aliases: &["tts", "tts_en", "kokoro"],
            tools: TOPIC_AGENT_TTS_EN_TOOLS,
        });
        groups.push(TopicAgentToolGroup {
            provider: "tts_ru",
            aliases: &["tts_ru", "silero"],
            tools: TOPIC_AGENT_TTS_RU_TOOLS,
        });

        let mut tool_names = BTreeSet::new();
        for group in &groups {
            for tool in group.tools {
                tool_names.insert((*tool).to_string());
            }
        }

        Ok(TopicAgentToolCatalog { groups, tool_names })
    }

    fn parse_profile_tool_set(
        profile: &serde_json::Value,
        camel_key: &str,
        snake_key: &str,
    ) -> Option<BTreeSet<String>> {
        let array = profile
            .get(camel_key)
            .and_then(serde_json::Value::as_array)
            .or_else(|| profile.get(snake_key).and_then(serde_json::Value::as_array))?;

        Some(
            array
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect(),
        )
    }

    fn write_profile_tool_set(
        profile: &mut serde_json::Value,
        camel_key: &str,
        snake_key: &str,
        values: Option<&BTreeSet<String>>,
        remove_when_empty: bool,
    ) -> Result<()> {
        let object = profile
            .as_object_mut()
            .ok_or_else(|| anyhow!("profile must be a JSON object"))?;
        object.remove(snake_key);

        match values {
            Some(values) if !(remove_when_empty && values.is_empty()) => {
                object.insert(
                    camel_key.to_string(),
                    serde_json::Value::Array(
                        values
                            .iter()
                            .cloned()
                            .map(serde_json::Value::String)
                            .collect(),
                    ),
                );
            }
            _ => {
                object.remove(camel_key);
            }
        }

        Ok(())
    }

    fn profile_tool_snapshot(
        profile: Option<&serde_json::Value>,
    ) -> (Option<Vec<String>>, Vec<String>, ToolAccessPolicy) {
        let Some(profile) = profile else {
            return (None, Vec::new(), ToolAccessPolicy::default());
        };

        let allowed = Self::parse_profile_tool_set(profile, "allowedTools", "allowed_tools")
            .map(|set| set.into_iter().collect::<Vec<_>>());
        let blocked = Self::parse_profile_tool_set(profile, "blockedTools", "blocked_tools")
            .unwrap_or_default()
            .into_iter()
            .collect::<Vec<_>>();
        let parsed = parse_agent_profile(profile);
        let policy = parsed
            .tool_policy
            .with_additional_allowed_tools(TOPIC_AGENT_REMINDER_TOOLS.iter().copied());

        (allowed, blocked, policy)
    }

    pub(super) fn topic_agent_tool_snapshot(
        catalog: &TopicAgentToolCatalog,
        profile: Option<&serde_json::Value>,
    ) -> TopicAgentToolSnapshot {
        let (allowed_tools_raw, blocked_tools, policy) = Self::profile_tool_snapshot(profile);
        let available_tools = catalog.tool_names.iter().cloned().collect::<Vec<_>>();
        let active_tools = available_tools
            .iter()
            .filter(|tool| policy.allows(tool))
            .cloned()
            .collect::<Vec<_>>();
        let known_tools = &catalog.tool_names;
        let unknown_profile_tools = allowed_tools_raw
            .iter()
            .flatten()
            .chain(blocked_tools.iter())
            .filter(|tool| !known_tools.contains(*tool))
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();

        let blocked_lookup = blocked_tools.iter().cloned().collect::<HashSet<_>>();
        let provider_statuses = catalog
            .groups
            .iter()
            .map(|group| {
                let available_tools = group
                    .tools
                    .iter()
                    .map(|tool| (*tool).to_string())
                    .collect::<Vec<_>>();
                let active_tools = available_tools
                    .iter()
                    .filter(|tool| policy.allows(tool))
                    .cloned()
                    .collect::<Vec<_>>();
                let blocked_tools = available_tools
                    .iter()
                    .filter(|tool| blocked_lookup.contains(*tool))
                    .cloned()
                    .collect::<Vec<_>>();

                TopicAgentToolGroupStatus {
                    provider: group.provider.to_string(),
                    enabled: !active_tools.is_empty(),
                    available_tools,
                    active_tools,
                    blocked_tools,
                }
            })
            .collect::<Vec<_>>();

        TopicAgentToolSnapshot {
            policy_mode: if allowed_tools_raw.is_some() {
                "allowlist".to_string()
            } else {
                "all_except_blocked".to_string()
            },
            available_tools,
            active_tools,
            blocked_tools,
            allowed_tools_raw,
            unknown_profile_tools,
            provider_statuses,
        }
    }

    fn expand_topic_agent_tools(
        catalog: &TopicAgentToolCatalog,
        requested_tools: Vec<String>,
    ) -> Result<Vec<String>> {
        let mut requested = BTreeSet::new();
        for raw in requested_tools {
            let token = raw.trim().to_ascii_lowercase();
            if token.is_empty() {
                continue;
            }

            if catalog.tool_names.contains(&token) {
                requested.insert(token);
                continue;
            }

            let matching_groups = catalog
                .groups
                .iter()
                .filter(|group| group.provider == token || group.aliases.contains(&token.as_str()))
                .collect::<Vec<_>>();

            if matching_groups.is_empty() {
                bail!("unknown tool or provider alias '{token}' for the topic agent");
            }

            for group in matching_groups {
                for tool in group.tools {
                    requested.insert((*tool).to_string());
                }
            }
        }

        if requested.is_empty() {
            bail!("tools must contain at least one non-empty tool name or provider alias");
        }

        Ok(requested.into_iter().collect())
    }

    fn enable_topic_agent_tools(
        profile: Option<&AgentProfileRecord>,
        tools: &[String],
    ) -> Result<TopicAgentToolMutation> {
        let mut next_profile = match profile {
            Some(profile) => Self::validate_profile_object(profile.profile.clone())?,
            None => json!({}),
        };
        let mut allowed =
            Self::parse_profile_tool_set(&next_profile, "allowedTools", "allowed_tools");
        let mut blocked =
            Self::parse_profile_tool_set(&next_profile, "blockedTools", "blocked_tools")
                .unwrap_or_default();

        for tool in tools {
            blocked.remove(tool);
            if let Some(allowed) = allowed.as_mut() {
                allowed.insert(tool.clone());
            }
        }

        Self::write_profile_tool_set(
            &mut next_profile,
            "allowedTools",
            "allowed_tools",
            allowed.as_ref(),
            false,
        )?;
        Self::write_profile_tool_set(
            &mut next_profile,
            "blockedTools",
            "blocked_tools",
            Some(&blocked),
            true,
        )?;

        let changed = match profile {
            Some(profile) => profile.profile != next_profile,
            None => next_profile != json!({}),
        };
        Ok(TopicAgentToolMutation {
            changed,
            profile: next_profile,
        })
    }

    fn disable_topic_agent_tools(
        profile: Option<&AgentProfileRecord>,
        tools: &[String],
    ) -> Result<TopicAgentToolMutation> {
        let mut next_profile = match profile {
            Some(profile) => Self::validate_profile_object(profile.profile.clone())?,
            None => json!({}),
        };
        let mut allowed =
            Self::parse_profile_tool_set(&next_profile, "allowedTools", "allowed_tools");
        let mut blocked =
            Self::parse_profile_tool_set(&next_profile, "blockedTools", "blocked_tools")
                .unwrap_or_default();

        for tool in tools {
            if let Some(allowed) = allowed.as_mut() {
                allowed.remove(tool);
            }
            blocked.insert(tool.clone());
        }

        Self::write_profile_tool_set(
            &mut next_profile,
            "allowedTools",
            "allowed_tools",
            allowed.as_ref(),
            false,
        )?;
        Self::write_profile_tool_set(
            &mut next_profile,
            "blockedTools",
            "blocked_tools",
            Some(&blocked),
            true,
        )?;

        let changed = match profile {
            Some(profile) => profile.profile != next_profile,
            None => next_profile != json!({}),
        };
        Ok(TopicAgentToolMutation {
            changed,
            profile: next_profile,
        })
    }

    fn topic_agent_tools_operation_name(action: &str) -> Result<&'static str> {
        match action {
            TOOL_TOPIC_AGENT_TOOLS_ENABLE => Ok("enable"),
            TOOL_TOPIC_AGENT_TOOLS_DISABLE => Ok("disable"),
            _ => bail!("unsupported topic agent tools action: {action}"),
        }
    }

    async fn prepare_topic_agent_tool_mutation(
        &self,
        raw_topic_id: String,
        requested_tools: Vec<String>,
    ) -> Result<(TopicAgentToolMutationContext, TopicAgentToolCatalog)> {
        let topic_id = self.resolve_mutation_topic_id(raw_topic_id).await?;
        let binding = self
            .storage
            .get_topic_binding(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get topic binding: {err}"))?
            .ok_or_else(|| anyhow!("topic_id '{topic_id}' is not bound to an agent"))?;
        let agent_id = binding.agent_id;
        let catalog = self.topic_agent_tool_catalog(&topic_id).await?;
        let requested_tools = Self::expand_topic_agent_tools(&catalog, requested_tools)?;
        let previous = self
            .storage
            .get_agent_profile(self.user_id, agent_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current agent profile: {err}"))?;

        Ok((
            TopicAgentToolMutationContext {
                topic_id,
                agent_id,
                requested_tools,
                previous,
            },
            catalog,
        ))
    }

    async fn append_topic_agent_tools_audit(
        &self,
        action: &str,
        context: &TopicAgentToolMutationContext,
        changed: bool,
        outcome: &str,
        version: Option<u64>,
        sandbox_cleanup: Option<serde_json::Value>,
    ) -> AuditStatus {
        self.append_audit_with_status(AppendAuditEventOptions {
            user_id: self.user_id,
            topic_id: Some(context.topic_id.clone()),
            agent_id: Some(context.agent_id.clone()),
            action: action.to_string(),
            payload: json!({
                "topic_id": context.topic_id.clone(),
                "agent_id": context.agent_id.clone(),
                "requested": context.requested_tools.clone(),
                "previous": context.previous.clone(),
                "changed": changed,
                "version": version,
                "sandbox_cleanup": sandbox_cleanup,
                "outcome": outcome
            }),
        })
        .await
    }

    fn topic_agent_tools_preview_response(
        operation: &str,
        context: TopicAgentToolMutationContext,
        changed: bool,
        profile: serde_json::Value,
        snapshot: TopicAgentToolSnapshot,
        audit_status: AuditStatus,
    ) -> Result<String> {
        Self::to_json_string(Self::attach_audit_status(
            json!({
                "ok": true,
                "dry_run": true,
                "preview": {
                    "operation": operation,
                    "topic_id": context.topic_id,
                    "agent_id": context.agent_id,
                    "requested_tools": context.requested_tools,
                    "changed": changed,
                    "profile": profile,
                    "tools": snapshot
                },
                "previous": context.previous
            }),
            audit_status,
        ))
    }

    fn topic_agent_tools_result_response(
        updated: bool,
        context: TopicAgentToolMutationContext,
        profile: Option<AgentProfileRecord>,
        snapshot: TopicAgentToolSnapshot,
        sandbox_cleanup: Option<serde_json::Value>,
        audit_status: AuditStatus,
    ) -> Result<String> {
        Self::to_json_string(Self::attach_audit_status(
            json!({
                "ok": true,
                "updated": updated,
                "topic_id": context.topic_id,
                "agent_id": context.agent_id,
                "requested_tools": context.requested_tools,
                "profile": profile,
                "tools": snapshot,
                "sandbox_cleanup": sandbox_cleanup
            }),
            audit_status,
        ))
    }

    fn topic_agent_hook_catalog() -> TopicAgentHookCatalog {
        let manageable_hooks = topic_agent_manageable_hooks()
            .into_iter()
            .collect::<BTreeSet<_>>();
        let protected_hooks = topic_agent_protected_hooks()
            .into_iter()
            .collect::<BTreeSet<_>>();
        let all_hooks = topic_agent_all_hooks().into_iter().collect::<BTreeSet<_>>();

        TopicAgentHookCatalog {
            manageable_hooks,
            protected_hooks,
            all_hooks,
        }
    }

    fn normalize_topic_agent_hook_name(token: &str) -> Option<&'static str> {
        match token {
            "workload" => Some("workload_distributor"),
            "delegation" => Some("delegation_guard"),
            "search" => Some("search_budget"),
            "timeout" => Some("timeout_report"),
            _ => None,
        }
    }

    fn profile_hook_snapshot(
        profile: Option<&serde_json::Value>,
    ) -> (Option<Vec<String>>, Vec<String>, HookAccessPolicy) {
        let Some(profile) = profile else {
            return (None, Vec::new(), HookAccessPolicy::default());
        };

        let enabled = Self::parse_profile_tool_set(profile, "enabledHooks", "enabled_hooks")
            .map(|set| set.into_iter().collect::<Vec<_>>());
        let disabled = Self::parse_profile_tool_set(profile, "disabledHooks", "disabled_hooks")
            .unwrap_or_default()
            .into_iter()
            .collect::<Vec<_>>();
        let parsed = parse_agent_profile(profile);

        (enabled, disabled, parsed.hook_policy)
    }

    fn topic_agent_hook_snapshot(
        catalog: &TopicAgentHookCatalog,
        profile: Option<&serde_json::Value>,
    ) -> TopicAgentHookSnapshot {
        let (enabled_hooks_raw, disabled_hooks_raw, policy) = Self::profile_hook_snapshot(profile);
        let available_hooks = catalog.all_hooks.iter().cloned().collect::<Vec<_>>();
        let active_hooks = available_hooks
            .iter()
            .filter(|hook| {
                catalog.protected_hooks.contains(*hook)
                    || (catalog.manageable_hooks.contains(*hook) && policy.allows(hook))
            })
            .cloned()
            .collect::<Vec<_>>();
        let disabled_hooks = catalog
            .manageable_hooks
            .iter()
            .filter(|hook| !policy.allows(hook))
            .cloned()
            .collect::<Vec<_>>();
        let unknown_profile_hooks = enabled_hooks_raw
            .iter()
            .flatten()
            .chain(disabled_hooks_raw.iter())
            .filter(|hook| !catalog.all_hooks.contains(*hook))
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let hook_statuses = available_hooks
            .iter()
            .map(|hook| {
                let protected = catalog.protected_hooks.contains(hook);
                let manageable = catalog.manageable_hooks.contains(hook);
                let active = protected || (manageable && policy.allows(hook));
                TopicAgentHookStatus {
                    hook: hook.clone(),
                    active,
                    manageable,
                    protected,
                }
            })
            .collect::<Vec<_>>();

        TopicAgentHookSnapshot {
            policy_mode: if enabled_hooks_raw.is_some() {
                "allowlist".to_string()
            } else {
                "all_except_disabled".to_string()
            },
            available_hooks,
            active_hooks,
            disabled_hooks,
            enabled_hooks_raw,
            unknown_profile_hooks,
            hook_statuses,
        }
    }

    fn expand_topic_agent_hooks(
        catalog: &TopicAgentHookCatalog,
        requested_hooks: Vec<String>,
    ) -> Result<Vec<String>> {
        let mut requested = BTreeSet::new();
        for raw in requested_hooks {
            let mut token = raw.trim().to_ascii_lowercase();
            if token.is_empty() {
                continue;
            }
            if let Some(alias) = Self::normalize_topic_agent_hook_name(&token) {
                token = alias.to_string();
            }

            if catalog.protected_hooks.contains(&token) {
                bail!("hook '{token}' is system-protected and cannot be toggled");
            }
            if !catalog.manageable_hooks.contains(&token) {
                bail!("unknown manageable hook '{token}' for the topic agent");
            }

            requested.insert(token);
        }

        if requested.is_empty() {
            bail!("hooks must contain at least one non-empty hook name");
        }

        Ok(requested.into_iter().collect())
    }

    fn enable_topic_agent_hooks(
        profile: Option<&AgentProfileRecord>,
        hooks: &[String],
    ) -> Result<TopicAgentHookMutation> {
        let mut next_profile = match profile {
            Some(profile) => Self::validate_profile_object(profile.profile.clone())?,
            None => json!({}),
        };
        let mut enabled =
            Self::parse_profile_tool_set(&next_profile, "enabledHooks", "enabled_hooks");
        let mut disabled =
            Self::parse_profile_tool_set(&next_profile, "disabledHooks", "disabled_hooks")
                .unwrap_or_default();

        for hook in hooks {
            disabled.remove(hook);
            if let Some(enabled) = enabled.as_mut() {
                enabled.insert(hook.clone());
            }
        }

        Self::write_profile_tool_set(
            &mut next_profile,
            "enabledHooks",
            "enabled_hooks",
            enabled.as_ref(),
            false,
        )?;
        Self::write_profile_tool_set(
            &mut next_profile,
            "disabledHooks",
            "disabled_hooks",
            Some(&disabled),
            true,
        )?;

        let changed = match profile {
            Some(profile) => profile.profile != next_profile,
            None => next_profile != json!({}),
        };
        Ok(TopicAgentHookMutation {
            profile: next_profile,
            changed,
        })
    }

    fn disable_topic_agent_hooks(
        profile: Option<&AgentProfileRecord>,
        hooks: &[String],
    ) -> Result<TopicAgentHookMutation> {
        let mut next_profile = match profile {
            Some(profile) => Self::validate_profile_object(profile.profile.clone())?,
            None => json!({}),
        };
        let mut enabled =
            Self::parse_profile_tool_set(&next_profile, "enabledHooks", "enabled_hooks");
        let mut disabled =
            Self::parse_profile_tool_set(&next_profile, "disabledHooks", "disabled_hooks")
                .unwrap_or_default();

        for hook in hooks {
            if let Some(enabled) = enabled.as_mut() {
                enabled.remove(hook);
            }
            disabled.insert(hook.clone());
        }

        Self::write_profile_tool_set(
            &mut next_profile,
            "enabledHooks",
            "enabled_hooks",
            enabled.as_ref(),
            false,
        )?;
        Self::write_profile_tool_set(
            &mut next_profile,
            "disabledHooks",
            "disabled_hooks",
            Some(&disabled),
            true,
        )?;

        let changed = match profile {
            Some(profile) => profile.profile != next_profile,
            None => next_profile != json!({}),
        };
        Ok(TopicAgentHookMutation {
            profile: next_profile,
            changed,
        })
    }

    fn topic_agent_hooks_operation_name(action: &str) -> Result<&'static str> {
        match action {
            TOOL_TOPIC_AGENT_HOOKS_ENABLE => Ok("enable"),
            TOOL_TOPIC_AGENT_HOOKS_DISABLE => Ok("disable"),
            _ => bail!("unsupported topic agent hooks action: {action}"),
        }
    }

    async fn prepare_topic_agent_hook_mutation(
        &self,
        raw_topic_id: String,
        requested_hooks: Vec<String>,
    ) -> Result<(TopicAgentHookMutationContext, TopicAgentHookCatalog)> {
        let topic_id = self.resolve_mutation_topic_id(raw_topic_id).await?;
        let binding = self
            .storage
            .get_topic_binding(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get topic binding: {err}"))?
            .ok_or_else(|| anyhow!("topic_id '{topic_id}' is not bound to an agent"))?;
        let agent_id = binding.agent_id;
        let catalog = Self::topic_agent_hook_catalog();
        let requested_hooks = Self::expand_topic_agent_hooks(&catalog, requested_hooks)?;
        let previous = self
            .storage
            .get_agent_profile(self.user_id, agent_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current agent profile: {err}"))?;

        Ok((
            TopicAgentHookMutationContext {
                topic_id,
                agent_id,
                requested_hooks,
                previous,
            },
            catalog,
        ))
    }

    async fn append_topic_agent_hooks_audit(
        &self,
        action: &str,
        context: &TopicAgentHookMutationContext,
        changed: bool,
        outcome: &str,
        version: Option<u64>,
    ) -> AuditStatus {
        self.append_audit_with_status(AppendAuditEventOptions {
            user_id: self.user_id,
            topic_id: Some(context.topic_id.clone()),
            agent_id: Some(context.agent_id.clone()),
            action: action.to_string(),
            payload: json!({
                "topic_id": context.topic_id.clone(),
                "agent_id": context.agent_id.clone(),
                "requested": context.requested_hooks.clone(),
                "previous": context.previous.clone(),
                "changed": changed,
                "version": version,
                "outcome": outcome
            }),
        })
        .await
    }

    fn topic_agent_hooks_preview_response(
        operation: &str,
        context: TopicAgentHookMutationContext,
        changed: bool,
        profile: serde_json::Value,
        snapshot: TopicAgentHookSnapshot,
        audit_status: AuditStatus,
    ) -> Result<String> {
        Self::to_json_string(Self::attach_audit_status(
            json!({
                "ok": true,
                "dry_run": true,
                "preview": {
                    "operation": operation,
                    "topic_id": context.topic_id,
                    "agent_id": context.agent_id,
                    "requested_hooks": context.requested_hooks,
                    "changed": changed,
                    "profile": profile,
                    "hooks": snapshot
                },
                "previous": context.previous
            }),
            audit_status,
        ))
    }

    fn topic_agent_hooks_result_response(
        updated: bool,
        context: TopicAgentHookMutationContext,
        profile: Option<AgentProfileRecord>,
        snapshot: TopicAgentHookSnapshot,
        audit_status: AuditStatus,
    ) -> Result<String> {
        Self::to_json_string(Self::attach_audit_status(
            json!({
                "ok": true,
                "updated": updated,
                "topic_id": context.topic_id,
                "agent_id": context.agent_id,
                "requested_hooks": context.requested_hooks,
                "profile": profile,
                "hooks": snapshot
            }),
            audit_status,
        ))
    }

    pub(super) fn sandbox_provider_enabled(snapshot: &TopicAgentToolSnapshot) -> bool {
        snapshot
            .provider_statuses
            .iter()
            .find(|status| status.provider == "sandbox")
            .is_some_and(|status| status.enabled)
    }

    pub(super) async fn execute_topic_agent_tools_get(&self, arguments: &str) -> Result<String> {
        let args: TopicAgentToolsGetArgs = Self::parse_args(arguments, TOOL_TOPIC_AGENT_TOOLS_GET)?;
        let topic_id = self.resolve_lookup_topic_id(args.topic_id).await?;
        let catalog = self.topic_agent_tool_catalog(&topic_id).await?;
        let binding = self
            .storage
            .get_topic_binding(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get topic binding: {err}"))?;

        let Some(binding) = binding else {
            return Self::to_json_string(json!({
                "ok": true,
                "found": false,
                "topic_id": topic_id
            }));
        };

        let profile = self
            .storage
            .get_agent_profile(self.user_id, binding.agent_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get agent profile: {err}"))?;
        let snapshot = Self::topic_agent_tool_snapshot(
            &catalog,
            profile.as_ref().map(|profile| &profile.profile),
        );

        Self::to_json_string(json!({
            "ok": true,
            "found": true,
            "topic_id": topic_id,
            "agent_id": binding.agent_id,
            "profile_found": profile.is_some(),
            "tools": snapshot
        }))
    }

    pub(super) async fn execute_topic_agent_tools_enable(&self, arguments: &str) -> Result<String> {
        let args: TopicAgentToolsMutationArgs =
            Self::parse_args(arguments, TOOL_TOPIC_AGENT_TOOLS_ENABLE)?;
        self.execute_topic_agent_tools_mutation(args, TOOL_TOPIC_AGENT_TOOLS_ENABLE)
            .await
    }

    pub(super) async fn execute_topic_agent_tools_disable(
        &self,
        arguments: &str,
    ) -> Result<String> {
        let args: TopicAgentToolsMutationArgs =
            Self::parse_args(arguments, TOOL_TOPIC_AGENT_TOOLS_DISABLE)?;
        self.execute_topic_agent_tools_mutation(args, TOOL_TOPIC_AGENT_TOOLS_DISABLE)
            .await
    }

    async fn execute_topic_agent_tools_mutation(
        &self,
        args: TopicAgentToolsMutationArgs,
        action: &str,
    ) -> Result<String> {
        let operation = Self::topic_agent_tools_operation_name(action)?;
        let (context, catalog) = self
            .prepare_topic_agent_tool_mutation(args.topic_id, args.tools)
            .await?;
        let previous_snapshot = Self::topic_agent_tool_snapshot(
            &catalog,
            context.previous.as_ref().map(|profile| &profile.profile),
        );
        let mutation = match action {
            TOOL_TOPIC_AGENT_TOOLS_ENABLE => {
                Self::enable_topic_agent_tools(context.previous.as_ref(), &context.requested_tools)?
            }
            TOOL_TOPIC_AGENT_TOOLS_DISABLE => Self::disable_topic_agent_tools(
                context.previous.as_ref(),
                &context.requested_tools,
            )?,
            _ => bail!("unsupported topic agent tools action: {action}"),
        };
        let snapshot = Self::topic_agent_tool_snapshot(&catalog, Some(&mutation.profile));

        if args.dry_run {
            let audit_status = self
                .append_topic_agent_tools_audit(
                    action,
                    &context,
                    mutation.changed,
                    Self::dry_run_outcome(true),
                    None,
                    None,
                )
                .await;

            return Self::topic_agent_tools_preview_response(
                operation,
                context,
                mutation.changed,
                mutation.profile,
                snapshot,
                audit_status,
            );
        }

        if !mutation.changed {
            let audit_status = self
                .append_topic_agent_tools_audit(action, &context, false, "noop", None, None)
                .await;

            return Self::topic_agent_tools_result_response(
                false,
                context,
                None,
                snapshot,
                None,
                audit_status,
            );
        }

        let record = self
            .storage
            .upsert_agent_profile(UpsertAgentProfileOptions {
                user_id: self.user_id,
                agent_id: context.agent_id.clone(),
                profile: mutation.profile,
            })
            .await
            .map_err(|err| anyhow!("failed to upsert agent profile: {err}"))?;
        let snapshot = Self::topic_agent_tool_snapshot(&catalog, Some(&record.profile));
        let sandbox_cleanup = if action == TOOL_TOPIC_AGENT_TOOLS_DISABLE
            && Self::sandbox_provider_enabled(&previous_snapshot)
            && !Self::sandbox_provider_enabled(&snapshot)
        {
            Some(
                self.cleanup_topic_sandbox_for_topic_id(&context.topic_id)
                    .await,
            )
        } else {
            None
        };

        let audit_status = self
            .append_topic_agent_tools_audit(
                action,
                &context,
                true,
                Self::dry_run_outcome(false),
                Some(record.version),
                sandbox_cleanup.clone(),
            )
            .await;

        Self::topic_agent_tools_result_response(
            true,
            TopicAgentToolMutationContext {
                agent_id: record.agent_id.clone(),
                ..context
            },
            Some(record),
            snapshot,
            sandbox_cleanup,
            audit_status,
        )
    }

    pub(super) async fn execute_topic_agent_hooks_get(&self, arguments: &str) -> Result<String> {
        let args: TopicAgentHooksGetArgs = Self::parse_args(arguments, TOOL_TOPIC_AGENT_HOOKS_GET)?;
        let topic_id = self.resolve_lookup_topic_id(args.topic_id).await?;
        let binding = self
            .storage
            .get_topic_binding(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get topic binding: {err}"))?;

        let Some(binding) = binding else {
            return Self::to_json_string(json!({
                "ok": true,
                "found": false,
                "topic_id": topic_id
            }));
        };

        let profile = self
            .storage
            .get_agent_profile(self.user_id, binding.agent_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get agent profile: {err}"))?;
        let snapshot = Self::topic_agent_hook_snapshot(
            &Self::topic_agent_hook_catalog(),
            profile.as_ref().map(|profile| &profile.profile),
        );

        Self::to_json_string(json!({
            "ok": true,
            "found": true,
            "topic_id": topic_id,
            "agent_id": binding.agent_id,
            "profile_found": profile.is_some(),
            "hooks": snapshot
        }))
    }

    pub(super) async fn execute_topic_agent_hooks_enable(&self, arguments: &str) -> Result<String> {
        let args: TopicAgentHooksMutationArgs =
            Self::parse_args(arguments, TOOL_TOPIC_AGENT_HOOKS_ENABLE)?;
        self.execute_topic_agent_hooks_mutation(args, TOOL_TOPIC_AGENT_HOOKS_ENABLE)
            .await
    }

    pub(super) async fn execute_topic_agent_hooks_disable(
        &self,
        arguments: &str,
    ) -> Result<String> {
        let args: TopicAgentHooksMutationArgs =
            Self::parse_args(arguments, TOOL_TOPIC_AGENT_HOOKS_DISABLE)?;
        self.execute_topic_agent_hooks_mutation(args, TOOL_TOPIC_AGENT_HOOKS_DISABLE)
            .await
    }

    async fn execute_topic_agent_hooks_mutation(
        &self,
        args: TopicAgentHooksMutationArgs,
        action: &str,
    ) -> Result<String> {
        let operation = Self::topic_agent_hooks_operation_name(action)?;
        let (context, catalog) = self
            .prepare_topic_agent_hook_mutation(args.topic_id, args.hooks)
            .await?;
        let mutation = match action {
            TOOL_TOPIC_AGENT_HOOKS_ENABLE => {
                Self::enable_topic_agent_hooks(context.previous.as_ref(), &context.requested_hooks)?
            }
            TOOL_TOPIC_AGENT_HOOKS_DISABLE => Self::disable_topic_agent_hooks(
                context.previous.as_ref(),
                &context.requested_hooks,
            )?,
            _ => bail!("unsupported topic agent hooks action: {action}"),
        };
        let snapshot = Self::topic_agent_hook_snapshot(&catalog, Some(&mutation.profile));

        if args.dry_run {
            let audit_status = self
                .append_topic_agent_hooks_audit(
                    action,
                    &context,
                    mutation.changed,
                    Self::dry_run_outcome(true),
                    None,
                )
                .await;

            return Self::topic_agent_hooks_preview_response(
                operation,
                context,
                mutation.changed,
                mutation.profile,
                snapshot,
                audit_status,
            );
        }

        if !mutation.changed {
            let audit_status = self
                .append_topic_agent_hooks_audit(action, &context, false, "noop", None)
                .await;

            return Self::topic_agent_hooks_result_response(
                false,
                context,
                None,
                snapshot,
                audit_status,
            );
        }

        let record = self
            .storage
            .upsert_agent_profile(UpsertAgentProfileOptions {
                user_id: self.user_id,
                agent_id: context.agent_id.clone(),
                profile: mutation.profile,
            })
            .await
            .map_err(|err| anyhow!("failed to upsert agent profile: {err}"))?;
        let snapshot = Self::topic_agent_hook_snapshot(&catalog, Some(&record.profile));

        let audit_status = self
            .append_topic_agent_hooks_audit(
                action,
                &context,
                true,
                Self::dry_run_outcome(false),
                Some(record.version),
            )
            .await;

        Self::topic_agent_hooks_result_response(
            true,
            TopicAgentHookMutationContext {
                agent_id: record.agent_id.clone(),
                ..context
            },
            Some(record),
            snapshot,
            audit_status,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_alias_expands_all_matching_search_groups() {
        let catalog = TopicAgentToolCatalog {
            groups: vec![
                TopicAgentToolGroup {
                    provider: "tavily",
                    aliases: &["search", "tavily"],
                    tools: &["web_search", "web_extract"],
                },
                TopicAgentToolGroup {
                    provider: "crawl4ai",
                    aliases: &["search", "crawl4ai"],
                    tools: &["deep_crawl", "web_markdown", "web_pdf"],
                },
            ],
            tool_names: [
                "web_search",
                "web_extract",
                "deep_crawl",
                "web_markdown",
                "web_pdf",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
        };

        let expanded = ManagerControlPlaneProvider::expand_topic_agent_tools(
            &catalog,
            vec!["search".to_string()],
        )
        .expect("search alias should expand across all enabled search providers");

        assert_eq!(
            expanded,
            vec![
                "deep_crawl".to_string(),
                "web_extract".to_string(),
                "web_markdown".to_string(),
                "web_pdf".to_string(),
                "web_search".to_string(),
            ]
        );
    }
}
