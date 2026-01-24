//! Loop detection service coordinating multiple detectors.

use super::config::LoopDetectionConfig;
use super::content_detector::ContentLoopDetector;
use super::llm_detector::{LlmLoopDetector, LoopScoutClient};
use super::tool_detector::ToolCallDetector;
use super::types::{LoopDetectedEvent, LoopDetectionError, LoopType};
use crate::agent::memory::AgentMemory;
use chrono::Utc;
use std::sync::Arc;
use tracing::{debug, warn};

/// Central coordinator for loop detection.
pub struct LoopDetectionService {
    config: Arc<LoopDetectionConfig>,
    session_id: String,
    loop_detected: bool,
    disabled_for_session: bool,
    tool_detector: ToolCallDetector,
    content_detector: ContentLoopDetector,
    llm_detector: LlmLoopDetector,
}

impl LoopDetectionService {
    /// Create a new loop detection service.
    #[must_use]
    pub fn new(client: Arc<dyn LoopScoutClient>, config: Arc<LoopDetectionConfig>) -> Self {
        Self {
            tool_detector: ToolCallDetector::new(config.tool_call_threshold),
            content_detector: ContentLoopDetector::new(
                config.content_chunk_size,
                config.content_loop_threshold,
                config.max_history_length,
            ),
            llm_detector: LlmLoopDetector::new(client, &config),
            config,
            session_id: String::new(),
            loop_detected: false,
            disabled_for_session: false,
        }
    }

    /// Reset state for a new session.
    pub fn reset(&mut self, session_id: String) {
        self.session_id = session_id;
        self.tool_detector.reset();
        self.content_detector.reset();
        self.llm_detector.reset(&self.config);
        self.loop_detected = false;
        self.disabled_for_session = false;
    }

    /// Disable loop detection for the current session.
    pub fn disable_for_session(&mut self) {
        self.disabled_for_session = true;
    }

    /// Reset content tracking state without affecting tool/LLM detectors.
    pub fn reset_content_tracking(&mut self) {
        self.content_detector.reset_tracking();
    }

    /// Check a tool call for repetition.
    pub fn check_tool_call(
        &mut self,
        tool_name: &str,
        args: &str,
    ) -> Result<bool, LoopDetectionError> {
        if !self.is_enabled() {
            debug!(session_id = %self.session_id, "loop_service: detection disabled");
            return Ok(false);
        }
        if self.loop_detected {
            debug!(session_id = %self.session_id, "loop_service: already detected loop");
            return Ok(true);
        }

        self.content_detector.reset_tracking();
        let detected = self.tool_detector.check(tool_name, args);

        if detected {
            warn!(
                session_id = %self.session_id,
                tool_name,
                loop_type = "ToolCallLoop",
                "loop_service: LOOP DETECTED via tool_detector"
            );
        }

        self.loop_detected = detected;
        Ok(detected)
    }

    /// Check content for repetition loops.
    pub fn check_content(&mut self, content: &str) -> Result<bool, LoopDetectionError> {
        if !self.is_enabled() {
            return Ok(false);
        }
        if self.loop_detected {
            return Ok(true);
        }

        let content_preview: String = content.chars().take(80).collect();
        debug!(
            session_id = %self.session_id,
            content_preview,
            content_len = content.len(),
            "loop_service: checking content"
        );

        let detected = self.content_detector.check(content);

        if detected {
            warn!(
                session_id = %self.session_id,
                loop_type = "ContentLoop",
                "loop_service: LOOP DETECTED via content_detector"
            );
        }

        self.loop_detected = detected;
        Ok(detected)
    }

    /// Run the LLM loop detector if needed.
    pub async fn check_llm_periodic(
        &mut self,
        memory: &AgentMemory,
        iteration: usize,
    ) -> Result<bool, LoopDetectionError> {
        if !self.is_enabled() {
            return Ok(false);
        }
        if self.loop_detected {
            return Ok(true);
        }

        if !self.llm_detector.should_check(iteration) {
            debug!(
                session_id = %self.session_id,
                iteration,
                "loop_service: skipping LLM check (not due yet)"
            );
            return Ok(false);
        }

        debug!(
            session_id = %self.session_id,
            iteration,
            "loop_service: running LLM periodic check"
        );

        let detected = self.llm_detector.check(memory, iteration).await?;

        if detected {
            warn!(
                session_id = %self.session_id,
                iteration,
                loop_type = "LlmLoop",
                "loop_service: LOOP DETECTED via llm_detector"
            );
        }

        self.loop_detected = detected;
        Ok(detected)
    }

    /// Create a loop detection event for logging and UI.
    #[must_use]
    pub fn create_event(&self, loop_type: LoopType, iteration: usize) -> LoopDetectedEvent {
        LoopDetectedEvent {
            loop_type,
            session_id: self.session_id.clone(),
            iteration,
            timestamp: Utc::now(),
        }
    }

    /// Whether detection is enabled.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.config.enabled && !self.disabled_for_session
    }
}

#[cfg(test)]
mod tests {
    use super::LoopDetectionService;
    use crate::agent::loop_detection::LoopDetectionConfig;
    use crate::llm::{LlmError, Message};
    use async_trait::async_trait;
    use std::sync::Arc;

    struct MockScout;

    #[async_trait]
    impl super::LoopScoutClient for MockScout {
        async fn chat_completion(
            &self,
            _system_prompt: &str,
            _history: &[Message],
            _user_message: &str,
            _model_name: &str,
        ) -> Result<String, LlmError> {
            Ok(r#"{"is_stuck":false,"confidence":0.1,"reasoning":"ok"}"#.to_string())
        }
    }

    #[test]
    fn disables_for_session() {
        let config = LoopDetectionConfig::default();
        let mut service = LoopDetectionService::new(Arc::new(MockScout), Arc::new(config));
        service.disable_for_session();
        assert!(!service.check_tool_call("tool", "{}").unwrap_or(false));
    }

    #[test]
    fn tool_call_detection_triggers() {
        let config = LoopDetectionConfig::default();
        let mut service = LoopDetectionService::new(Arc::new(MockScout), Arc::new(config));
        service.reset("session".to_string());
        for _ in 0..4 {
            assert!(!service.check_tool_call("tool", "{}").unwrap_or(false));
        }
        assert!(service.check_tool_call("tool", "{}").unwrap_or(false));
    }
}
