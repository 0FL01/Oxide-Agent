//! Provider-agnostic local LLM compact summary backend.

use super::prompt::{build_local_compaction_user_message, local_compaction_system_prompt};
use super::{CompactSummaryBackend, CompactSummaryError, CompactSummaryRequest};
use crate::config::ModelInfo;
use crate::llm::LlmClient;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;

/// Default compaction backend: ordinary text generation through a configured LLM route.
pub struct LocalLlmSummary {
    llm_client: Arc<LlmClient>,
    routes: Vec<ModelInfo>,
    timeout: Duration,
}

impl LocalLlmSummary {
    /// Create a local LLM summary backend.
    #[must_use]
    pub fn new(llm_client: Arc<LlmClient>, routes: Vec<ModelInfo>, timeout: Duration) -> Self {
        Self {
            llm_client,
            routes,
            timeout,
        }
    }

    fn first_usable_route(&self) -> Option<&ModelInfo> {
        Self::select_route(&self.routes)
    }

    fn select_route(routes: &[ModelInfo]) -> Option<&ModelInfo> {
        routes
            .iter()
            .find(|route| !route.id.trim().is_empty() && !route.provider.trim().is_empty())
    }

    fn timeout_secs(&self) -> u64 {
        self.timeout.as_secs().max(1)
    }
}

#[async_trait]
impl CompactSummaryBackend for LocalLlmSummary {
    async fn summarize(
        &self,
        request: CompactSummaryRequest<'_>,
    ) -> Result<super::CompactSummaryResult, CompactSummaryError> {
        let route = self
            .first_usable_route()
            .ok_or(CompactSummaryError::NoRoute)?;
        let user_message = build_local_compaction_user_message(
            request.task,
            request.previous_summary,
            request.messages,
        );
        let llm_call = self.llm_client.chat_completion_for_model_info(
            local_compaction_system_prompt(),
            &[],
            &user_message,
            route,
        );

        let output = tokio::time::timeout(self.timeout, llm_call)
            .await
            .map_err(|_| CompactSummaryError::Timeout {
                timeout_secs: self.timeout_secs(),
            })?
            .map_err(|error| CompactSummaryError::Provider(error.to_string()))?;
        let summary_text = output.trim().to_string();
        if summary_text.is_empty() {
            return Err(CompactSummaryError::EmptyOutput);
        }

        Ok(super::CompactSummaryResult {
            summary_text,
            provider: route.provider.clone(),
            route: route.id.clone(),
        })
    }

    fn selected_route(&self) -> Option<&ModelInfo> {
        self.first_usable_route()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::compaction::{
        CompactSummaryBackend, CompactSummaryError, CompactSummaryRequest,
    };
    use crate::agent::memory::AgentMessage;
    use crate::config::AgentSettings;
    use crate::llm::{LlmClient, MockLlmProvider};
    use mockall::predicate::always;

    fn route(id: &str, provider: &str) -> ModelInfo {
        ModelInfo {
            id: id.to_string(),
            provider: provider.to_string(),
            max_output_tokens: 128,
            context_window_tokens: 1024,
            weight: 1,
        }
    }

    fn backend_with_mock_response(response: &'static str) -> LocalLlmSummary {
        let settings = AgentSettings::default();
        let mut llm = LlmClient::new(&settings);
        let mut provider = MockLlmProvider::new();
        provider
            .expect_chat_completion()
            .with(always(), always(), always(), always(), always())
            .returning(move |_, _, _, _, _| Ok(response.to_string()));
        llm.register_provider("mock".to_string(), Arc::new(provider));
        LocalLlmSummary::new(
            Arc::new(llm),
            vec![route("compact-model", "mock")],
            Duration::from_secs(1),
        )
    }

    #[test]
    fn selected_route_skips_incomplete_routes() {
        let routes = vec![
            ModelInfo {
                id: String::new(),
                provider: "mock".to_string(),
                max_output_tokens: 128,
                context_window_tokens: 1024,
                weight: 1,
            },
            ModelInfo {
                id: "compact".to_string(),
                provider: "mock".to_string(),
                max_output_tokens: 128,
                context_window_tokens: 1024,
                weight: 1,
            },
        ];

        assert_eq!(
            LocalLlmSummary::select_route(&routes).map(|route| route.id.as_str()),
            Some("compact")
        );
    }

    #[tokio::test]
    async fn summarize_uses_plain_text_chat_completion_and_trims_output() {
        let backend = backend_with_mock_response("  Handoff summary.\n");
        let messages = vec![AgentMessage::user_task("Ship compaction")];

        let result = backend
            .summarize(CompactSummaryRequest {
                task: "Ship compaction",
                messages: &messages,
                previous_summary: None,
            })
            .await
            .expect("summary succeeds");

        assert_eq!(result.summary_text, "Handoff summary.");
        assert_eq!(result.provider, "mock");
        assert_eq!(result.route, "compact-model");
    }

    #[tokio::test]
    async fn summarize_rejects_empty_plain_text_output() {
        let backend = backend_with_mock_response("  \n  ");
        let err = backend
            .summarize(CompactSummaryRequest {
                task: "Ship compaction",
                messages: &[],
                previous_summary: None,
            })
            .await
            .expect_err("empty output is rejected");

        assert!(matches!(err, CompactSummaryError::EmptyOutput));
    }

    #[tokio::test]
    async fn summarize_fails_without_usable_route() {
        let settings = AgentSettings::default();
        let backend = LocalLlmSummary::new(
            Arc::new(LlmClient::new(&settings)),
            Vec::new(),
            Duration::from_secs(1),
        );

        let err = backend
            .summarize(CompactSummaryRequest {
                task: "Ship compaction",
                messages: &[],
                previous_summary: None,
            })
            .await
            .expect_err("missing route is rejected");

        assert!(matches!(err, CompactSummaryError::NoRoute));
    }
}
