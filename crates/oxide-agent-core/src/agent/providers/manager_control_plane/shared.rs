use super::*;

impl ManagerControlPlaneProvider {
    pub(super) fn forum_topic_context_key(chat_id: i64, thread_id: i64) -> String {
        format!("{chat_id}:{thread_id}")
    }

    pub(super) fn forum_topic_binding_keys(chat_id: i64, thread_id: i64) -> Vec<String> {
        let context_key = Self::forum_topic_context_key(chat_id, thread_id);
        let raw_thread_key = thread_id.to_string();
        if raw_thread_key == context_key {
            vec![context_key]
        } else {
            vec![context_key, raw_thread_key]
        }
    }

    pub(super) fn resolve_default_forum_chat_id(&self) -> Option<i64> {
        self.topic_lifecycle
            .as_ref()
            .and_then(|lifecycle| lifecycle.default_forum_chat_id())
    }

    pub(super) fn forum_topic_catalog_entry_from_context(
        context_key: &str,
        context: &crate::storage::UserContextConfig,
    ) -> Option<ForumTopicCatalogEntry> {
        let chat_id = context.chat_id?;
        let thread_id = context.thread_id?;
        if chat_id >= 0 || thread_id <= 0 {
            return None;
        }

        let expected_key = Self::forum_topic_context_key(chat_id, thread_id);
        if context_key != expected_key {
            return None;
        }

        Some(ForumTopicCatalogEntry {
            topic_id: expected_key,
            chat_id,
            thread_id,
            name: context.forum_topic_name.clone(),
            icon_color: context.forum_topic_icon_color,
            icon_custom_emoji_id: context.forum_topic_icon_custom_emoji_id.clone(),
            closed: context.forum_topic_closed,
        })
    }

    pub(super) fn upsert_forum_topic_catalog_entry(
        config: &mut UserConfig,
        entry: &ForumTopicCatalogEntry,
    ) {
        let context = config.contexts.entry(entry.topic_id.clone()).or_default();
        context.chat_id = Some(entry.chat_id);
        context.thread_id = Some(entry.thread_id);
        context.forum_topic_name = entry.name.clone();
        context.forum_topic_icon_color = entry.icon_color;
        context.forum_topic_icon_custom_emoji_id = entry.icon_custom_emoji_id.clone();
        context.forum_topic_closed = entry.closed;
    }

    pub(super) fn existing_forum_topic_catalog_entry(
        config: &UserConfig,
        topic_id: &str,
    ) -> Option<ForumTopicCatalogEntry> {
        config
            .contexts
            .get(topic_id)
            .and_then(|context| Self::forum_topic_catalog_entry_from_context(topic_id, context))
    }

    pub(super) fn validate_non_empty(value: String, field_name: &str) -> Result<String> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            bail!("{field_name} must not be empty");
        }
        Ok(trimmed.to_string())
    }

    pub(super) fn validate_agents_md(value: String) -> Result<String> {
        validate_topic_agents_md_content(&value).map_err(Into::into)
    }

    pub(super) fn validate_topic_context(value: String) -> Result<String> {
        validate_topic_context_content(&value).map_err(Into::into)
    }

    pub(super) fn validate_thread_id(thread_id: i64) -> Result<i64> {
        if thread_id <= 0 {
            bail!("thread_id must be a positive integer");
        }
        Ok(thread_id)
    }

    pub(super) fn validate_optional_non_empty(
        value: Option<String>,
        field_name: &str,
    ) -> Result<Option<String>> {
        value
            .map(|inner| Self::validate_non_empty(inner, field_name))
            .transpose()
    }

    pub(super) fn normalize_tags(tags: Vec<String>) -> Vec<String> {
        let mut tags = tags
            .into_iter()
            .map(|tag| tag.trim().to_string())
            .filter(|tag| !tag.is_empty())
            .collect::<Vec<_>>();
        tags.sort();
        tags.dedup();
        tags
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

    pub(super) fn validate_topic_infra_args(
        args: TopicInfraUpsertArgs,
    ) -> Result<TopicInfraUpsertArgs> {
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

    pub(super) fn topic_infra_value_from_args(args: &TopicInfraUpsertArgs) -> serde_json::Value {
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

    pub(super) fn topic_infra_preview_record(
        &self,
        args: &TopicInfraUpsertArgs,
    ) -> TopicInfraConfigRecord {
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

    pub(super) async fn inspect_topic_infra_record(
        &self,
        record: &TopicInfraConfigRecord,
    ) -> crate::agent::providers::TopicInfraPreflightReport {
        inspect_topic_infra_config(&self.storage, self.user_id, &record.topic_id, record).await
    }

    pub(super) fn topic_lifecycle(&self) -> Result<&Arc<dyn ManagerTopicLifecycle>> {
        self.topic_lifecycle
            .as_ref()
            .ok_or_else(|| anyhow!("forum topic lifecycle service is unavailable"))
    }

    pub(super) fn validate_forum_icon_color(color: Option<u32>) -> Result<Option<u32>> {
        if let Some(value) = color {
            if !TELEGRAM_FORUM_ICON_COLORS.contains(&value) {
                bail!("icon_color is not one of Telegram allowed values");
            }
            return Ok(Some(value));
        }

        Ok(None)
    }

    pub(super) fn validate_profile_object(profile: serde_json::Value) -> Result<serde_json::Value> {
        if !profile.is_object() {
            bail!("profile must be a JSON object");
        }
        if profile.get("tools").is_some() {
            bail!(
                "profile.tools is not supported; use allowedTools/blockedTools or forum_topic_provision_ssh_agent"
            );
        }
        Ok(profile)
    }

    pub(super) fn is_canonical_forum_topic_id(value: &str) -> bool {
        let Some((chat_id, thread_id)) = value.split_once(':') else {
            return false;
        };
        chat_id.parse::<i64>().is_ok() && thread_id.parse::<i64>().ok().is_some_and(|id| id > 0)
    }

    pub(super) async fn resolve_mutation_topic_id(&self, topic_id: String) -> Result<String> {
        let topic_id = Self::validate_non_empty(topic_id, "topic_id")?;
        if Self::is_canonical_forum_topic_id(&topic_id) || self.topic_lifecycle.is_none() {
            return Ok(topic_id);
        }

        match self.resolve_forum_topic_id_alias(&topic_id).await? {
            Some(resolved) => Ok(resolved),
            None => bail!(
                "topic_id '{topic_id}' is not a canonical Telegram forum topic id. Use '<chat_id>:<thread_id>' from forum_topic_create / forum_topic_provision_ssh_agent results."
            ),
        }
    }

    pub(super) async fn resolve_lookup_topic_id(&self, topic_id: String) -> Result<String> {
        let topic_id = Self::validate_non_empty(topic_id, "topic_id")?;
        if Self::is_canonical_forum_topic_id(&topic_id) || self.topic_lifecycle.is_none() {
            return Ok(topic_id);
        }

        Ok(self
            .resolve_forum_topic_id_alias(&topic_id)
            .await?
            .unwrap_or(topic_id))
    }

    pub(super) async fn resolve_forum_topic_id_alias(&self, alias: &str) -> Result<Option<String>> {
        if self.topic_lifecycle.is_none() {
            return Ok(None);
        }

        let mut matches = self
            .list_forum_topic_catalog_entries(None, true)
            .await?
            .into_iter()
            .filter(|entry| entry.name.as_deref() == Some(alias))
            .collect::<Vec<_>>();

        matches.sort_by(|left, right| left.topic_id.cmp(&right.topic_id));
        matches.dedup_by(|left, right| left.topic_id == right.topic_id);

        match matches.len() {
            0 => Ok(None),
            1 => Ok(matches.pop().map(|entry| entry.topic_id)),
            _ => bail!(
                "topic alias '{alias}' is ambiguous across multiple forum topics; use canonical '<chat_id>:<thread_id>'"
            ),
        }
    }

    pub(super) fn parse_canonical_forum_topic_id(topic_id: &str) -> Option<(i64, i64)> {
        let (chat_id, thread_id) = topic_id.split_once(':')?;
        let chat_id = chat_id.parse::<i64>().ok()?;
        let thread_id = thread_id.parse::<i64>().ok()?;
        (thread_id > 0).then_some((chat_id, thread_id))
    }

    pub(super) fn to_json_string(value: serde_json::Value) -> Result<String> {
        serde_json::to_string(&value)
            .map_err(|err| anyhow!("failed to serialize tool response: {err}"))
    }

    pub(super) fn parse_args<T: for<'de> Deserialize<'de>>(
        arguments: &str,
        tool_name: &str,
    ) -> Result<T> {
        serde_json::from_str(arguments).map_err(|err| anyhow!("invalid {tool_name} args: {err}"))
    }

    pub(super) fn dry_run_outcome(dry_run: bool) -> &'static str {
        if dry_run {
            "dry_run"
        } else {
            "applied"
        }
    }

    pub(super) fn optional_metadata_payload_value(
        value: OptionalMetadataPatch<i64>,
    ) -> Option<i64> {
        match value {
            OptionalMetadataPatch::Set(inner) => Some(inner),
            OptionalMetadataPatch::Keep | OptionalMetadataPatch::Clear => None,
        }
    }

    pub(super) fn restore_metadata_patch(value: Option<i64>) -> OptionalMetadataPatch<i64> {
        value
            .map(OptionalMetadataPatch::Set)
            .unwrap_or(OptionalMetadataPatch::Clear)
    }
}
