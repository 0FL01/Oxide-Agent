use super::*;

#[derive(Debug, Clone)]
pub(super) struct MutationAuditTarget {
    pub topic_id: Option<String>,
    pub agent_id: Option<String>,
}

#[derive(Debug)]
pub(super) struct MutationExecutionResult {
    pub response_body: serde_json::Value,
    pub audit_payload: serde_json::Value,
}

#[derive(Debug)]
pub(super) struct PreviewableMutationPlan {
    pub dry_run: bool,
    pub preview: serde_json::Value,
    pub previous: serde_json::Value,
    pub dry_run_payload: serde_json::Value,
}

impl ManagerControlPlaneProvider {
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

    pub(super) fn to_json_string(value: serde_json::Value) -> Result<String> {
        serde_json::to_string(&value)
            .map_err(|err| anyhow!("failed to serialize tool response: {err}"))
    }

    pub(super) fn to_json_value(value: impl Serialize) -> Result<serde_json::Value> {
        serde_json::to_value(value)
            .map_err(|err| anyhow!("failed to serialize tool payload: {err}"))
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

    pub(super) async fn execute_previewable_mutation<F, Fut>(
        &self,
        action: &str,
        audit_target: MutationAuditTarget,
        plan: PreviewableMutationPlan,
        apply: F,
    ) -> Result<String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<MutationExecutionResult>>,
    {
        if plan.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: audit_target.topic_id,
                    agent_id: audit_target.agent_id,
                    action: action.to_string(),
                    payload: plan.dry_run_payload,
                })
                .await;

            return Self::to_json_string(Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": plan.preview,
                    "previous": plan.previous,
                }),
                audit_status,
            ));
        }

        let result = apply().await?;
        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: audit_target.topic_id,
                agent_id: audit_target.agent_id,
                action: action.to_string(),
                payload: result.audit_payload,
            })
            .await;

        Self::to_json_string(Self::attach_audit_status(
            result.response_body,
            audit_status,
        ))
    }
}
