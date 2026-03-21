//! Lightweight topic-scoped `AGENTS.md` tools for the active agent context.

use crate::agent::provider::ToolProvider;
use crate::llm::ToolDefinition;
use crate::storage::{
    validate_topic_agents_md_content, StorageProvider, UpsertTopicAgentsMdOptions,
};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

const TOOL_AGENTS_MD_GET: &str = "agents_md_get";
const TOOL_AGENTS_MD_UPDATE: &str = "agents_md_update";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentsMdUpdateArgs {
    agents_md: String,
    #[serde(default)]
    dry_run: bool,
}

/// Tool names that should stay available to top-level agents by default.
pub fn agents_md_tool_names() -> Vec<String> {
    vec![
        TOOL_AGENTS_MD_GET.to_string(),
        TOOL_AGENTS_MD_UPDATE.to_string(),
    ]
}

/// Tool provider that lets the current agent read and update its own topic `AGENTS.md`.
pub struct AgentsMdProvider {
    storage: Arc<dyn StorageProvider>,
    user_id: i64,
    topic_id: String,
}

impl AgentsMdProvider {
    /// Create a provider bound to the current user and topic scope.
    #[must_use]
    pub fn new(storage: Arc<dyn StorageProvider>, user_id: i64, topic_id: String) -> Self {
        Self {
            storage,
            user_id,
            topic_id,
        }
    }

    fn tools_definitions() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: TOOL_AGENTS_MD_GET.to_string(),
                description: "Get the current topic AGENTS.md".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false,
                }),
            },
            ToolDefinition {
                name: TOOL_AGENTS_MD_UPDATE.to_string(),
                description: "Update the current topic AGENTS.md for future and current flow steps (max 300 lines)".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "agents_md": {
                            "type": "string",
                            "description": "Full AGENTS.md content for the current topic"
                        },
                        "dry_run": {
                            "type": "boolean",
                            "description": "Validate and preview without persisting"
                        }
                    },
                    "required": ["agents_md"],
                    "additionalProperties": false,
                }),
            },
        ]
    }

    fn parse_args<T: for<'de> Deserialize<'de>>(arguments: &str, tool_name: &str) -> Result<T> {
        serde_json::from_str(arguments)
            .map_err(|error| anyhow!("invalid arguments for {tool_name}: {error}"))
    }

    async fn execute_get(&self, arguments: &str) -> Result<String> {
        let _: serde_json::Map<String, serde_json::Value> =
            Self::parse_args(arguments, TOOL_AGENTS_MD_GET)?;

        let record = self
            .storage
            .get_topic_agents_md(self.user_id, self.topic_id.clone())
            .await
            .map_err(|error| anyhow!("failed to get topic AGENTS.md: {error}"))?;

        Ok(json!({
            "ok": true,
            "found": record.is_some(),
            "topic_id": self.topic_id.clone(),
            "topic_agents_md": record,
        })
        .to_string())
    }

    async fn execute_update(&self, arguments: &str) -> Result<String> {
        let args: AgentsMdUpdateArgs = Self::parse_args(arguments, TOOL_AGENTS_MD_UPDATE)?;
        let agents_md = validate_topic_agents_md_content(&args.agents_md)?;
        let previous = self
            .storage
            .get_topic_agents_md(self.user_id, self.topic_id.clone())
            .await
            .map_err(|error| anyhow!("failed to get current topic AGENTS.md: {error}"))?;

        if args.dry_run {
            return Ok(json!({
                "ok": true,
                "dry_run": true,
                "topic_id": self.topic_id.clone(),
                "preview": {
                    "operation": "update",
                    "agents_md": agents_md,
                },
                "previous": previous,
            })
            .to_string());
        }

        let record = self
            .storage
            .upsert_topic_agents_md(UpsertTopicAgentsMdOptions {
                user_id: self.user_id,
                topic_id: self.topic_id.clone(),
                agents_md,
            })
            .await
            .map_err(|error| anyhow!("failed to update topic AGENTS.md: {error}"))?;

        Ok(json!({
            "ok": true,
            "topic_id": self.topic_id.clone(),
            "topic_agents_md": record,
            "previous": previous,
        })
        .to_string())
    }
}

#[async_trait]
impl ToolProvider for AgentsMdProvider {
    fn name(&self) -> &'static str {
        "agents_md"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        Self::tools_definitions()
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        matches!(tool_name, TOOL_AGENTS_MD_GET | TOOL_AGENTS_MD_UPDATE)
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        _progress_tx: Option<&tokio::sync::mpsc::Sender<crate::agent::progress::AgentEvent>>,
        _cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        match tool_name {
            TOOL_AGENTS_MD_GET => self.execute_get(arguments).await,
            TOOL_AGENTS_MD_UPDATE => self.execute_update(arguments).await,
            _ => Err(anyhow!("Unknown AGENTS.md tool: {tool_name}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{MockStorageProvider, TopicAgentsMdRecord};
    use mockall::predicate::eq;

    #[tokio::test]
    async fn get_returns_current_topic_record() {
        let mut storage = MockStorageProvider::new();
        storage
            .expect_get_topic_agents_md()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|user_id, topic_id| {
                Ok(Some(TopicAgentsMdRecord {
                    schema_version: 1,
                    version: 2,
                    user_id,
                    topic_id,
                    agents_md: "# Topic AGENTS\nStay focused.".to_string(),
                    created_at: 10,
                    updated_at: 20,
                }))
            });

        let provider = AgentsMdProvider::new(Arc::new(storage), 77, "topic-a".to_string());
        let result = provider
            .execute(TOOL_AGENTS_MD_GET, "{}", None, None)
            .await
            .expect("get must succeed");

        let parsed: serde_json::Value = serde_json::from_str(&result).expect("valid json");
        assert_eq!(parsed["found"], true);
        assert_eq!(
            parsed["topic_agents_md"]["agents_md"],
            "# Topic AGENTS\nStay focused."
        );
    }

    #[tokio::test]
    async fn update_dry_run_validates_without_persisting() {
        let mut storage = MockStorageProvider::new();
        storage
            .expect_get_topic_agents_md()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, _| Ok(None));
        storage.expect_upsert_topic_agents_md().times(0);

        let provider = AgentsMdProvider::new(Arc::new(storage), 77, "topic-a".to_string());
        let arguments = json!({
            "agents_md": "# Topic AGENTS\nUse checklists",
            "dry_run": true,
        })
        .to_string();
        let result = provider
            .execute(TOOL_AGENTS_MD_UPDATE, &arguments, None, None)
            .await
            .expect("dry run must succeed");

        let parsed: serde_json::Value = serde_json::from_str(&result).expect("valid json");
        assert_eq!(parsed["dry_run"], true);
        assert_eq!(
            parsed["preview"]["agents_md"],
            "# Topic AGENTS\nUse checklists"
        );
    }

    #[tokio::test]
    async fn update_persists_current_topic_record() {
        let mut storage = MockStorageProvider::new();
        storage
            .expect_get_topic_agents_md()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, _| Ok(None));
        storage
            .expect_upsert_topic_agents_md()
            .withf(|options| {
                options.user_id == 77
                    && options.topic_id == "topic-a"
                    && options.agents_md == "# Topic AGENTS\nUse checklists"
            })
            .returning(|options| {
                Ok(TopicAgentsMdRecord {
                    schema_version: 1,
                    version: 3,
                    user_id: options.user_id,
                    topic_id: options.topic_id,
                    agents_md: options.agents_md,
                    created_at: 10,
                    updated_at: 30,
                })
            });

        let provider = AgentsMdProvider::new(Arc::new(storage), 77, "topic-a".to_string());
        let arguments = json!({
            "agents_md": "# Topic AGENTS\nUse checklists",
        })
        .to_string();
        let result = provider
            .execute(TOOL_AGENTS_MD_UPDATE, &arguments, None, None)
            .await
            .expect("update must succeed");

        let parsed: serde_json::Value = serde_json::from_str(&result).expect("valid json");
        assert_eq!(parsed["topic_agents_md"]["topic_id"], "topic-a");
        assert_eq!(parsed["topic_agents_md"]["version"], 3);
    }

    #[tokio::test]
    async fn update_rejects_more_than_300_lines() {
        let storage = MockStorageProvider::new();
        let provider = AgentsMdProvider::new(Arc::new(storage), 77, "topic-a".to_string());
        let oversized = vec!["line"; 301].join("\n");
        let arguments = json!({ "agents_md": oversized }).to_string();

        let error = provider
            .execute(TOOL_AGENTS_MD_UPDATE, &arguments, None, None)
            .await
            .expect_err("oversized AGENTS.md must be rejected");

        assert!(error
            .to_string()
            .contains("agents_md must not exceed 300 lines"));
    }
}
