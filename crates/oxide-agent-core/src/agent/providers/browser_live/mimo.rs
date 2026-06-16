#![allow(missing_docs)]

use super::metrics::BrowserMetricsCollector;
use super::parser::{BrowserDecisionParseError, BrowserDecisionValidation, parse_browser_decision};
use super::prompt::{
    BrowserDecisionPromptContext, build_dynamic_state_prompt, build_repair_prompt,
    stable_system_prompt,
};
use super::types::{BrowserDecision, Viewport};
use crate::llm::{LlmClient, LlmError, TokenUsage};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Instant;
use thiserror::Error;
use tracing::instrument;

#[derive(Clone)]
pub struct BrowserMimoDecider {
    llm_client: Arc<LlmClient>,
    metrics: Option<Arc<BrowserMetricsCollector>>,
}

#[derive(Debug, Error)]
pub enum BrowserMimoError {
    #[error("browser MiMo route error: {0}")]
    Route(String),
    #[error("browser MiMo image call failed: {0}")]
    Llm(String),
    #[error("browser MiMo decision parse failed: {0}")]
    Parse(#[from] BrowserDecisionParseError),
}

#[async_trait]
pub trait BrowserDecisionEngine: Send + Sync {
    async fn decide(
        &self,
        image_bytes: Vec<u8>,
        context: &BrowserDecisionPromptContext<'_>,
        viewport: Viewport,
    ) -> Result<BrowserDecision, BrowserMimoError>;
}

impl BrowserMimoDecider {
    #[must_use]
    pub fn new(llm_client: Arc<LlmClient>) -> Self {
        Self {
            llm_client,
            metrics: None,
        }
    }

    /// Attach a metrics collector that will record MiMo request counts, latency,
    /// and token usage.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<BrowserMetricsCollector>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    #[instrument(
        name = "browser_mimo_decide",
        skip(self, image_bytes, context),
        fields(session_id = %context.session_id),
    )]
    async fn decide_inner(
        &self,
        image_bytes: Vec<u8>,
        context: &BrowserDecisionPromptContext<'_>,
        viewport: Viewport,
    ) -> Result<BrowserDecision, BrowserMimoError> {
        let model = self
            .llm_client
            .resolve_browser_vision_model_for_image()
            .map_err(|error| BrowserMimoError::Route(error.to_string()))?;
        let validation = BrowserDecisionValidation::for_viewport(viewport);
        let dynamic_prompt = build_dynamic_state_prompt(context);
        let (raw, usage) = self
            .analyze(image_bytes.clone(), &dynamic_prompt, &model.id)
            .await?;
        match parse_browser_decision(&raw, validation) {
            Ok(decision) => Ok(decision),
            Err(error) => {
                if let Some(metrics) = &self.metrics {
                    metrics.record_mimo_repair_attempt();
                    metrics.record_mimo_invalid_json();
                }
                let repair_prompt = build_repair_prompt(context, &raw, &error.to_string());
                let (repaired, repair_usage) =
                    self.analyze(image_bytes, &repair_prompt, &model.id).await?;
                let combined_usage = combine_usage(usage, repair_usage);
                if let Some(metrics) = &self.metrics {
                    metrics.record_mimo_usage(combined_usage.as_ref());
                }
                parse_browser_decision(&repaired, validation).map_err(BrowserMimoError::Parse)
            }
        }
    }

    async fn analyze(
        &self,
        image_bytes: Vec<u8>,
        text_prompt: &str,
        model_name: &str,
    ) -> Result<(String, Option<TokenUsage>), BrowserMimoError> {
        let start = Instant::now();
        let result: Result<(String, Option<TokenUsage>), LlmError> = self
            .llm_client
            .analyze_image_with_usage(image_bytes, text_prompt, stable_system_prompt(), model_name)
            .await;
        let latency = start.elapsed();
        match result {
            Ok((text, usage)) => {
                if let Some(metrics) = &self.metrics {
                    metrics.record_mimo_request(latency, usage.as_ref());
                }
                Ok((text, usage))
            }
            Err(error) => {
                if let Some(metrics) = &self.metrics {
                    metrics.record_mimo_error();
                }
                Err(llm_error(error))
            }
        }
    }
}

#[async_trait]
impl BrowserDecisionEngine for BrowserMimoDecider {
    async fn decide(
        &self,
        image_bytes: Vec<u8>,
        context: &BrowserDecisionPromptContext<'_>,
        viewport: Viewport,
    ) -> Result<BrowserDecision, BrowserMimoError> {
        self.decide_inner(image_bytes, context, viewport).await
    }
}

fn llm_error(error: LlmError) -> BrowserMimoError {
    BrowserMimoError::Llm(error.to_string())
}

fn combine_usage(a: Option<TokenUsage>, b: Option<TokenUsage>) -> Option<TokenUsage> {
    match (a, b) {
        (Some(a), Some(b)) => Some(TokenUsage {
            prompt_tokens: a.prompt_tokens.saturating_add(b.prompt_tokens),
            completion_tokens: a.completion_tokens.saturating_add(b.completion_tokens),
            total_tokens: a.total_tokens.saturating_add(b.total_tokens),
            cached_tokens: add_optional(a.cached_tokens, b.cached_tokens),
            cache_creation_tokens: add_optional(a.cache_creation_tokens, b.cache_creation_tokens),
        }),
        (Some(a), None) | (None, Some(a)) => Some(a),
        (None, None) => None,
    }
}

fn add_optional(a: Option<u32>, b: Option<u32>) -> Option<u32> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.saturating_add(b)),
        (Some(a), None) | (None, Some(a)) => Some(a),
        (None, None) => None,
    }
}

#[cfg(all(test, feature = "llm-opencode-go"))]
mod tests {
    use super::*;
    use crate::agent::providers::browser_live::types::{
        BrowserObservation, LoadingState, ScreenshotArtifact,
    };
    use crate::config::AgentSettings;
    use crate::llm::{LlmClient, LlmError, MockLlmProvider, TokenUsage};
    use mockall::predicate::always;

    #[tokio::test]
    async fn mimo_decider_uses_browser_vision_image_route() {
        let mut provider = MockLlmProvider::new();
        provider
            .expect_analyze_image_with_usage()
            .with(always(), always(), always(), always())
            .return_once(|image, text_prompt, system_prompt, model_id| {
                assert_eq!(image, b"png".to_vec());
                assert!(text_prompt.contains("Task: click login"));
                assert!(system_prompt.contains("Browser Live visual decision planner"));
                assert_eq!(model_id, "mimo-v2.5");
                Ok((r#"{"schema_version":1,"rationale":"ok","action":{"kind":"click_xy","x":1,"y":2,"target_description":"button"},"expected_result":"click","confidence":0.9,"risk":"low","sensitive_action":{"required":false},"needs_debug":false}"#.to_string(), None))
            });
        let llm = test_llm_with_analyze(provider);
        let decider = BrowserMimoDecider::new(Arc::clone(&llm));
        let decision = decider
            .decide(
                b"png".to_vec(),
                &test_context("click login"),
                Viewport::default(),
            )
            .await
            .expect("decide");
        assert!(matches!(
            decision.action,
            super::super::types::BrowserDecisionAction::ClickXy { .. }
        ));
    }

    #[tokio::test]
    async fn mimo_decider_records_metrics_and_token_usage() {
        let mut provider = MockLlmProvider::new();
        provider
            .expect_analyze_image_with_usage()
            .return_once(|_, _, _, _| {
                Ok((
                    r#"{"schema_version":1,"rationale":"ok","action":{"kind":"click_xy","x":1,"y":2,"target_description":"button"},"expected_result":"click","confidence":0.9,"risk":"low","sensitive_action":{"required":false},"needs_debug":false}"#.to_string(),
                    Some(TokenUsage {
                        prompt_tokens: 1000,
                        completion_tokens: 200,
                        total_tokens: 1200,
                        cached_tokens: Some(500),
                        cache_creation_tokens: Some(50),
                    }),
                ))
            });
        let llm = test_llm_with_analyze(provider);
        let metrics = Arc::new(BrowserMetricsCollector::new());
        let decider = BrowserMimoDecider::new(Arc::clone(&llm)).with_metrics(Arc::clone(&metrics));
        decider
            .decide(
                b"png".to_vec(),
                &test_context("click login"),
                Viewport::default(),
            )
            .await
            .expect("decide");
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.mimo_requests, 1);
        assert_eq!(snapshot.mimo_prompt_tokens, 1000);
        assert_eq!(snapshot.mimo_completion_tokens, 200);
        assert_eq!(snapshot.mimo_cached_tokens, 500);
        assert_eq!(snapshot.mimo_cache_creation_tokens, 50);
    }

    #[tokio::test]
    async fn mimo_decider_records_repair_and_invalid_json_metrics() {
        let mut provider = MockLlmProvider::new();
        provider
            .expect_analyze_image_with_usage()
            .times(2)
            .returning(|_, _, _, _| {
                Ok((
                    "not json".to_string(),
                    Some(TokenUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                        total_tokens: 15,
                        cached_tokens: None,
                        cache_creation_tokens: None,
                    }),
                ))
            });
        let llm = test_llm_with_analyze(provider);
        let metrics = Arc::new(BrowserMetricsCollector::new());
        let decider = BrowserMimoDecider::new(Arc::clone(&llm)).with_metrics(Arc::clone(&metrics));
        let _ = decider
            .decide(
                b"png".to_vec(),
                &test_context("click login"),
                Viewport::default(),
            )
            .await;
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.mimo_requests, 2);
        assert_eq!(snapshot.mimo_repair_attempts, 1);
        assert_eq!(snapshot.mimo_invalid_json, 1);
    }

    #[tokio::test]
    async fn mimo_decider_records_error_metric() {
        let mut provider = MockLlmProvider::new();
        provider
            .expect_analyze_image_with_usage()
            .return_once(|_, _, _, _| Err(LlmError::ApiError("provider 429".to_string())));
        let llm = test_llm_with_analyze(provider);
        let metrics = Arc::new(BrowserMetricsCollector::new());
        let decider = BrowserMimoDecider::new(Arc::clone(&llm)).with_metrics(Arc::clone(&metrics));
        let _ = decider
            .decide(
                b"png".to_vec(),
                &test_context("click login"),
                Viewport::default(),
            )
            .await;
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.mimo_requests, 0);
        assert_eq!(snapshot.mimo_errors, 1);
    }

    fn test_llm_with_analyze(provider: MockLlmProvider) -> Arc<LlmClient> {
        let settings = AgentSettings {
            browser_agent_enabled: Some(true),
            browser_agent_mimo_model: Some("mimo-v2.5".to_string()),
            browser_agent_mimo_provider: Some("opencode-go".to_string()),
            ..AgentSettings::default()
        };
        let mut llm = LlmClient::new(&settings);
        llm.register_provider("opencode-go".to_string(), Arc::new(provider));
        Arc::new(llm)
    }

    fn test_context(task: &'static str) -> BrowserDecisionPromptContext<'static> {
        let observation = Box::leak(Box::new(BrowserObservation {
            observation_id: "obs-1".to_string(),
            action_seq: 1,
            captured_at: "2026-06-16T00:00:00Z".to_string(),
            viewport: Viewport::default(),
            url: "https://example.test".to_string(),
            title: "Example".to_string(),
            loading_state: LoadingState::Idle,
            screenshot: ScreenshotArtifact {
                screenshot_id: "sc-1".to_string(),
                artifact_uri: "artifact://browser/test/1.png".to_string(),
                mime_type: "image/png".to_string(),
                width: 100,
                height: 100,
                sha256: "abcd".to_string(),
                captured_at: Some("2026-06-16T00:00:00Z".to_string()),
                redacted: true,
                byte_size: 0,
            },
            a11y_summary: Vec::new(),
            network_summary: None,
            console_summary: None,
        }));
        BrowserDecisionPromptContext {
            task,
            session_id: "session-1",
            observation,
            history_summary: None,
        }
    }
}
