#![allow(missing_docs)]

use super::parser::{BrowserDecisionParseError, BrowserDecisionValidation, parse_browser_decision};
use super::prompt::{
    BrowserDecisionPromptContext, build_dynamic_state_prompt, build_repair_prompt,
    stable_system_prompt,
};
use super::types::{BrowserDecision, Viewport};
use crate::llm::{LlmClient, LlmError};
use std::sync::Arc;
use thiserror::Error;

#[derive(Clone)]
pub struct BrowserMimoDecider {
    llm_client: Arc<LlmClient>,
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

impl BrowserMimoDecider {
    #[must_use]
    pub fn new(llm_client: Arc<LlmClient>) -> Self {
        Self { llm_client }
    }

    pub async fn decide(
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
        let raw = self
            .analyze(image_bytes.clone(), &dynamic_prompt, &model.id)
            .await?;
        match parse_browser_decision(&raw, validation) {
            Ok(decision) => Ok(decision),
            Err(error) => {
                let repair_prompt = build_repair_prompt(context, &raw, &error.to_string());
                let repaired = self.analyze(image_bytes, &repair_prompt, &model.id).await?;
                parse_browser_decision(&repaired, validation).map_err(BrowserMimoError::Parse)
            }
        }
    }

    async fn analyze(
        &self,
        image_bytes: Vec<u8>,
        text_prompt: &str,
        model_name: &str,
    ) -> Result<String, BrowserMimoError> {
        self.llm_client
            .analyze_image(image_bytes, text_prompt, stable_system_prompt(), model_name)
            .await
            .map_err(llm_error)
    }
}

fn llm_error(error: LlmError) -> BrowserMimoError {
    BrowserMimoError::Llm(error.to_string())
}

#[cfg(all(test, feature = "llm-opencode-go"))]
mod tests {
    use super::*;
    use crate::agent::providers::browser_live::types::{
        BrowserObservation, LoadingState, ScreenshotArtifact,
    };
    use crate::config::AgentSettings;
    use crate::llm::{LlmClient, MockLlmProvider};
    use mockall::predicate::always;

    #[tokio::test]
    async fn mimo_decider_uses_browser_vision_image_route() {
        let mut llm = test_llm();
        let mut provider = MockLlmProvider::new();
        provider
            .expect_analyze_image()
            .with(always(), always(), always(), always())
            .return_once(|image, text_prompt, system_prompt, model_id| {
                assert_eq!(image, b"png".to_vec());
                assert!(text_prompt.contains("Task: click login"));
                assert!(system_prompt.contains("Browser Live visual decision planner"));
                assert_eq!(model_id, "mimo-v2.5");
                Ok(valid_wait())
            });
        llm.register_provider("opencode-go".to_string(), Arc::new(provider));
        let decider = BrowserMimoDecider::new(Arc::new(llm));
        let observation = observation();
        let context = context(&observation);

        let decision = decider
            .decide(b"png".to_vec(), &context, Viewport::default())
            .await
            .expect("decision");

        assert_eq!(decision.schema_version, 1);
    }

    #[tokio::test]
    async fn mimo_decider_repairs_once_after_invalid_json() {
        let mut llm = test_llm();
        let mut provider = MockLlmProvider::new();
        provider
            .expect_analyze_image()
            .times(2)
            .returning(|_, text_prompt, _, _| {
                if text_prompt.contains("Repair once") {
                    Ok(valid_wait())
                } else {
                    Ok("not json".to_string())
                }
            });
        llm.register_provider("opencode-go".to_string(), Arc::new(provider));
        let decider = BrowserMimoDecider::new(Arc::new(llm));
        let observation = observation();
        let context = context(&observation);

        let decision = decider
            .decide(b"png".to_vec(), &context, Viewport::default())
            .await
            .expect("repaired decision");

        assert_eq!(decision.confidence, 0.8);
    }

    fn test_llm() -> LlmClient {
        let settings = AgentSettings {
            browser_agent_enabled: Some(true),
            browser_agent_mimo_model: Some("mimo-v2.5".to_string()),
            browser_agent_mimo_provider: Some("opencode-go".to_string()),
            ..AgentSettings::default()
        };
        LlmClient::new(&settings)
    }

    fn context<'a>(observation: &'a BrowserObservation) -> BrowserDecisionPromptContext<'a> {
        BrowserDecisionPromptContext {
            task: "click login",
            session_id: "br-1",
            observation,
            history_summary: Some("browser_session session_id=br-1 latest_screenshot_id=shot-1"),
        }
    }

    fn observation() -> BrowserObservation {
        BrowserObservation {
            observation_id: "obs-1".to_string(),
            action_seq: 1,
            captured_at: "2026-06-16T10:00:00Z".to_string(),
            url: "https://example.test".to_string(),
            title: "Example".to_string(),
            viewport: Viewport::default(),
            loading_state: LoadingState::Idle,
            screenshot: ScreenshotArtifact {
                screenshot_id: "shot-1".to_string(),
                artifact_uri: "artifact://browser/task/br-1/live.jpg".to_string(),
                mime_type: "image/jpeg".to_string(),
                width: 1365,
                height: 768,
                sha256: "abc".to_string(),
                captured_at: Some("2026-06-16T10:00:00Z".to_string()),
                redacted: true,
            },
            a11y_summary: Vec::new(),
            network_summary: None,
            console_summary: None,
        }
    }

    fn valid_wait() -> String {
        r#"{
          "schema_version": 1,
          "rationale": "Wait for the page to settle.",
          "action": {"kind": "wait", "timeout_ms": 500},
          "expected_result": "The page reaches a stable state",
          "confidence": 0.8,
          "risk": "low",
          "sensitive_action": {"required": false},
          "needs_debug": false
        }"#
        .to_string()
    }
}
