//! Provider-agnostic local LLM compact summary backend.

use super::prompt::{build_local_compaction_user_message, local_compaction_system_prompt};
use super::{CompactSummaryBackend, CompactSummaryError, CompactSummaryRequest};
use crate::config::ModelInfo;
use crate::llm::{InternalTextPurpose, LlmClient};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

const SUMMARY_MAX_ATTEMPTS: usize = 3;
const SUMMARY_MAX_OUTPUT_TOKENS: u32 = 48_000;

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

fn compact_summary_route(route: &ModelInfo) -> ModelInfo {
    let mut summary_route = route.clone();
    if summary_route.max_output_tokens > SUMMARY_MAX_OUTPUT_TOKENS {
        summary_route.max_output_tokens = SUMMARY_MAX_OUTPUT_TOKENS;
    }
    summary_route
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
        let summary_route = compact_summary_route(request.route);

        let mut attempt = 1;
        let output = loop {
            let llm_call = self.llm_client.complete_internal_text(
                InternalTextPurpose::CompactionSummary,
                local_compaction_system_prompt(),
                &user_message,
                &summary_route,
            );

            match tokio::time::timeout(self.timeout, llm_call).await {
                Ok(Ok(output)) => break output,
                Ok(Err(error)) => {
                    let Some(backoff) = (attempt < SUMMARY_MAX_ATTEMPTS)
                        .then(|| LlmClient::get_retry_delay(&error, attempt))
                        .flatten()
                    else {
                        return Err(CompactSummaryError::Provider(error.to_string()));
                    };

                    warn!(
                        attempt,
                        max_attempts = SUMMARY_MAX_ATTEMPTS,
                        backoff_ms = backoff.as_millis(),
                        provider = %summary_route.provider,
                        route = %summary_route.id,
                        error = %error,
                        "Retrying compaction summary provider request"
                    );
                    tokio::time::sleep(backoff).await;
                    attempt += 1;
                }
                Err(_) => {
                    return Err(CompactSummaryError::Timeout {
                        timeout_secs: self.timeout_secs(),
                    });
                }
            }
        };

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
    use crate::llm::{LlmClient, LlmError, MockLlmProvider};
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

    fn backend_with_provider(provider: MockLlmProvider) -> LocalLlmSummary {
        let settings = AgentSettings::default();
        let mut llm = LlmClient::new(&settings);
        llm.register_provider("mock".to_string(), Arc::new(provider));
        LocalLlmSummary::new(Arc::new(llm), Duration::from_secs(1))
    }

    fn backend_with_mock_response(response: &'static str) -> LocalLlmSummary {
        let mut provider = MockLlmProvider::new();
        provider
            .expect_chat_completion()
            .with(always(), always(), always(), always(), always())
            .returning(move |_, _, _, _, _| Ok(response.to_string()));
        backend_with_provider(provider)
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
    async fn summarize_retries_retryable_provider_error_and_caps_output_tokens() {
        let mut provider = MockLlmProvider::new();
        let mut sequence = mockall::Sequence::new();
        provider
            .expect_chat_completion()
            .times(1)
            .in_sequence(&mut sequence)
            .return_once(|_, _, _, model_id, max_tokens| {
                assert_eq!(model_id, "agent-model");
                assert_eq!(max_tokens, SUMMARY_MAX_OUTPUT_TOKENS);
                Err(LlmError::NetworkError(
                    "temporary network failure".to_string(),
                ))
            });
        provider
            .expect_chat_completion()
            .times(1)
            .in_sequence(&mut sequence)
            .return_once(|_, _, _, model_id, max_tokens| {
                assert_eq!(model_id, "agent-model");
                assert_eq!(max_tokens, SUMMARY_MAX_OUTPUT_TOKENS);
                Ok("Recovered handoff summary.".to_string())
            });
        let backend = backend_with_provider(provider);
        let messages = vec![AgentMessage::user_task("Ship compaction")];
        let mut route = route("agent-model", "mock");
        route.max_output_tokens = 64_000;

        let result = backend
            .summarize(CompactSummaryRequest {
                task: "Ship compaction",
                route: &route,
                messages: &messages,
                previous_summary: None,
            })
            .await
            .expect("summary retries and succeeds");

        assert_eq!(result.summary_text, "Recovered handoff summary.");
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
