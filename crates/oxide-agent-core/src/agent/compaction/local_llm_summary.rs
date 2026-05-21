//! Provider-agnostic local LLM compact summary backend.

use super::prompt::{build_local_compaction_user_message, local_compaction_system_prompt};
use super::{CompactSummaryBackend, CompactSummaryError, CompactSummaryRequest};
use crate::llm::LlmClient;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;

/// Default compaction backend: ordinary text generation through a configured LLM route.
pub struct LocalLlmSummary {
    llm_client: Arc<LlmClient>,
    timeout: Duration,
}

impl LocalLlmSummary {
    /// Create a local LLM summary backend.
    #[must_use]
    pub fn new(llm_client: Arc<LlmClient>, timeout: Duration) -> Self {
        Self {
            llm_client,
            timeout,
        }
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
        let user_message = build_local_compaction_user_message(
            request.task,
            request.previous_summary,
            request.messages,
        );
        let llm_call = self.llm_client.chat_completion_for_model_info(
            local_compaction_system_prompt(),
            &[],
            &user_message,
            request.route,
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
            provider: request.route.provider.clone(),
            route: request.route.id.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::compaction::{
        CompactSummaryBackend, CompactSummaryError, CompactSummaryRequest,
    };
    use crate::agent::memory::AgentMessage;
    use crate::config::{AgentSettings, ModelInfo};
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
        LocalLlmSummary::new(Arc::new(llm), Duration::from_secs(1))
    }

    #[tokio::test]
    async fn summarize_uses_plain_text_chat_completion_and_trims_output() {
        let backend = backend_with_mock_response("  Handoff summary.\n");
        let messages = vec![AgentMessage::user_task("Ship compaction")];
        let route = route("agent-model", "mock");

        let result = backend
            .summarize(CompactSummaryRequest {
                task: "Ship compaction",
                route: &route,
                messages: &messages,
                previous_summary: None,
            })
            .await
            .expect("summary succeeds");

        assert_eq!(result.summary_text, "Handoff summary.");
        assert_eq!(result.provider, "mock");
        assert_eq!(result.route, "agent-model");
    }

    #[tokio::test]
    async fn summarize_rejects_empty_plain_text_output() {
        let backend = backend_with_mock_response("  \n  ");
        let route = route("agent-model", "mock");
        let err = backend
            .summarize(CompactSummaryRequest {
                task: "Ship compaction",
                route: &route,
                messages: &[],
                previous_summary: None,
            })
            .await
            .expect_err("empty output is rejected");

        assert!(matches!(err, CompactSummaryError::EmptyOutput));
    }
}
