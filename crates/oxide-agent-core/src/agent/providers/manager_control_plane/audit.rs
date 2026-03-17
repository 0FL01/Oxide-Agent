use super::*;

pub(super) enum AuditStatus {
    Written,
    WriteFailed(String),
}

impl ManagerControlPlaneProvider {
    pub(super) fn previous_from_payload<T: DeserializeOwned>(
        payload: &serde_json::Value,
    ) -> Result<Option<T>> {
        let Some(previous) = payload.get("previous") else {
            return Ok(None);
        };

        if previous.is_null() {
            return Ok(None);
        }

        serde_json::from_value(previous.clone())
            .map(Some)
            .map_err(|err| anyhow!("invalid previous snapshot in audit payload: {err}"))
    }

    pub(super) fn is_applied_mutation_event(event: &crate::storage::AuditEventRecord) -> bool {
        !matches!(
            event
                .payload
                .get("outcome")
                .and_then(serde_json::Value::as_str),
            Some("dry_run" | "noop")
        )
    }

    pub(super) fn action_matches(action: &str, candidates: &[&str]) -> bool {
        candidates.contains(&action)
    }

    pub(super) async fn append_audit_with_status(
        &self,
        options: AppendAuditEventOptions,
    ) -> AuditStatus {
        match self.storage.append_audit_event(options).await {
            Ok(_) => AuditStatus::Written,
            Err(err) => AuditStatus::WriteFailed(err.to_string()),
        }
    }

    pub(super) fn attach_audit_status(
        mut response: serde_json::Value,
        status: AuditStatus,
    ) -> serde_json::Value {
        if let Some(response_object) = response.as_object_mut() {
            match status {
                AuditStatus::Written => {
                    response_object.insert("audit_status".to_string(), json!("written"));
                }
                AuditStatus::WriteFailed(error) => {
                    response_object.insert("audit_status".to_string(), json!("write_failed"));
                    response_object.insert("audit_error".to_string(), json!(error));
                }
            }
        }

        response
    }

    pub(super) async fn find_latest_applied_mutation<F>(
        &self,
        mut predicate: F,
    ) -> Result<Option<crate::storage::AuditEventRecord>>
    where
        F: FnMut(&crate::storage::AuditEventRecord) -> bool,
    {
        let mut cursor = None;

        loop {
            let events = self
                .storage
                .list_audit_events_page(self.user_id, cursor, ROLLBACK_AUDIT_PAGE_SIZE)
                .await
                .map_err(|err| anyhow!("failed to list audit events: {err}"))?;

            if events.is_empty() {
                return Ok(None);
            }

            if let Some(event) = events
                .iter()
                .find(|event| Self::is_applied_mutation_event(event) && predicate(event))
            {
                return Ok(Some(event.clone()));
            }

            cursor = events.last().map(|event| event.version);
            if cursor.is_none() {
                return Ok(None);
            }
        }
    }

    pub(super) async fn last_topic_binding_mutation(
        &self,
        topic_id: &str,
    ) -> Result<Option<crate::storage::AuditEventRecord>> {
        self.find_latest_applied_mutation(|event| {
            event.topic_id.as_deref() == Some(topic_id)
                && Self::action_matches(
                    event.action.as_str(),
                    &[
                        TOOL_TOPIC_BINDING_SET,
                        TOOL_TOPIC_BINDING_DELETE,
                        TOOL_TOPIC_BINDING_ROLLBACK,
                    ],
                )
        })
        .await
    }

    pub(super) async fn last_agent_profile_mutation(
        &self,
        agent_id: &str,
    ) -> Result<Option<crate::storage::AuditEventRecord>> {
        self.find_latest_applied_mutation(|event| {
            event.agent_id.as_deref() == Some(agent_id)
                && Self::action_matches(
                    event.action.as_str(),
                    &[
                        TOOL_AGENT_PROFILE_UPSERT,
                        TOOL_AGENT_PROFILE_DELETE,
                        TOOL_TOPIC_AGENT_TOOLS_ENABLE,
                        TOOL_TOPIC_AGENT_TOOLS_DISABLE,
                        TOOL_TOPIC_AGENT_HOOKS_ENABLE,
                        TOOL_TOPIC_AGENT_HOOKS_DISABLE,
                        TOOL_AGENT_PROFILE_ROLLBACK,
                    ],
                )
        })
        .await
    }

    pub(super) async fn last_topic_context_mutation(
        &self,
        topic_id: &str,
    ) -> Result<Option<crate::storage::AuditEventRecord>> {
        self.find_latest_applied_mutation(|event| {
            event.topic_id.as_deref() == Some(topic_id)
                && Self::action_matches(
                    event.action.as_str(),
                    &[
                        TOOL_TOPIC_CONTEXT_UPSERT,
                        TOOL_TOPIC_CONTEXT_DELETE,
                        TOOL_TOPIC_CONTEXT_ROLLBACK,
                    ],
                )
        })
        .await
    }

    pub(super) async fn last_topic_agents_md_mutation(
        &self,
        topic_id: &str,
    ) -> Result<Option<crate::storage::AuditEventRecord>> {
        self.find_latest_applied_mutation(|event| {
            event.topic_id.as_deref() == Some(topic_id)
                && Self::action_matches(
                    event.action.as_str(),
                    &[
                        TOOL_TOPIC_AGENTS_MD_UPSERT,
                        TOOL_TOPIC_AGENTS_MD_DELETE,
                        TOOL_TOPIC_AGENTS_MD_ROLLBACK,
                    ],
                )
        })
        .await
    }

    pub(super) async fn last_topic_infra_mutation(
        &self,
        topic_id: &str,
    ) -> Result<Option<crate::storage::AuditEventRecord>> {
        self.find_latest_applied_mutation(|event| {
            event.topic_id.as_deref() == Some(topic_id)
                && Self::action_matches(
                    event.action.as_str(),
                    &[
                        TOOL_TOPIC_INFRA_UPSERT,
                        TOOL_TOPIC_INFRA_DELETE,
                        TOOL_TOPIC_INFRA_ROLLBACK,
                    ],
                )
        })
        .await
    }
}
