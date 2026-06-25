//! Loop detection service coordinating multiple detectors.

use super::config::LoopDetectionConfig;
use super::content_detector::ContentLoopDetector;
use super::llm_detector::{LlmLoopDetector, LoopScoutClient};
use super::tool_detector::ToolCallDetector;
use super::types::{LoopDetectedEvent, LoopDetectionError, LoopDetectionOutcome, LoopType};
use crate::agent::memory::AgentMemory;
use chrono::Utc;
use std::sync::Arc;
use tracing::{debug, warn};

/// Default maximum re-prompt attempts before halting.
const DEFAULT_MAX_RE_PROMPTS: usize = 2;

/// Central coordinator for loop detection.
pub struct LoopDetectionService {
    config: Arc<LoopDetectionConfig>,
    session_id: String,
    re_prompt_count: usize,
    max_re_prompts: usize,
    halted: bool,
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
            max_re_prompts: DEFAULT_MAX_RE_PROMPTS,
            config,
            session_id: String::new(),
            re_prompt_count: 0,
            halted: false,
            disabled_for_session: false,
        }
    }

    /// Reset state for a new session.
    pub fn reset(&mut self, session_id: String) {
        self.session_id = session_id;
        self.tool_detector.reset();
        self.content_detector.reset();
        self.llm_detector.reset(&self.config);
        self.re_prompt_count = 0;
        self.halted = false;
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

    /// Current re-prompt count (for testing/logging).
    #[cfg(test)]
    pub(crate) fn re_prompt_count(&self) -> usize {
        self.re_prompt_count
    }

    /// Check a tool call for repetition.
    pub fn check_tool_call(
        &mut self,
        tool_name: &str,
        args: &str,
    ) -> Result<LoopDetectionOutcome, LoopDetectionError> {
        if !self.is_enabled() || self.halted {
            debug!(session_id = %self.session_id, "loop_service: detection disabled or halted");
            return Ok(LoopDetectionOutcome::NoLoop);
        }

        self.content_detector.reset_tracking();
        let detected = self.tool_detector.check(tool_name, args);

        if detected {
            warn!(
                session_id = %self.session_id,
                tool_name,
                re_prompt_count = self.re_prompt_count,
                max_re_prompts = self.max_re_prompts,
                "loop_service: tool call cycle detected"
            );
            return Ok(self.handle_detection(LoopType::ToolCallLoop));
        }

        Ok(LoopDetectionOutcome::NoLoop)
    }

    /// Check content for repetition loops.
    pub fn check_content(
        &mut self,
        content: &str,
    ) -> Result<LoopDetectionOutcome, LoopDetectionError> {
        if !self.is_enabled() || self.halted {
            return Ok(LoopDetectionOutcome::NoLoop);
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
                re_prompt_count = self.re_prompt_count,
                max_re_prompts = self.max_re_prompts,
                "loop_service: content loop detected"
            );
            return Ok(self.handle_detection(LoopType::ContentLoop));
        }

        Ok(LoopDetectionOutcome::NoLoop)
    }

    /// Run the LLM loop detector if needed.
    pub async fn check_llm_periodic(
        &mut self,
        memory: &AgentMemory,
        iteration: usize,
    ) -> Result<LoopDetectionOutcome, LoopDetectionError> {
        if !self.is_enabled() || self.halted {
            return Ok(LoopDetectionOutcome::NoLoop);
        }

        if !self.llm_detector.should_check(iteration) {
            debug!(
                session_id = %self.session_id,
                iteration,
                "loop_service: skipping LLM check (not due yet)"
            );
            return Ok(LoopDetectionOutcome::NoLoop);
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
                re_prompt_count = self.re_prompt_count,
                max_re_prompts = self.max_re_prompts,
                "loop_service: LLM cognitive loop detected"
            );
            return Ok(self.handle_detection(LoopType::CognitiveLoop));
        }

        Ok(LoopDetectionOutcome::NoLoop)
    }

    /// Handle a loop detection: either re-prompt (inject context, reset detectors)
    /// or halt (max re-prompts exhausted).
    fn handle_detection(&mut self, loop_type: LoopType) -> LoopDetectionOutcome {
        if self.re_prompt_count >= self.max_re_prompts {
            self.halted = true;
            return LoopDetectionOutcome::Halt { loop_type };
        }

        self.re_prompt_count += 1;

        // Reset detectors so we can detect new loops after the re-prompt.
        self.tool_detector.reset();
        self.content_detector.reset();

        LoopDetectionOutcome::RePrompt {
            context: Self::re_prompt_message(loop_type),
            loop_type,
        }
    }

    fn re_prompt_message(loop_type: LoopType) -> String {
        match loop_type {
            LoopType::ToolCallLoop => "You are repeating the same tool calls in a cycle. Try a different approach, use different arguments, or stop if the task is complete.".to_string(),
            LoopType::ContentLoop => "You are repeating the same content. Try a different approach or provide a concise final answer.".to_string(),
            LoopType::CognitiveLoop => "You appear to be stuck in a loop. Try a different approach, use different tools, or ask the user for help.".to_string(),
        }
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
    use super::super::types::LoopDetectionOutcome;
    use super::LoopDetectionService;
    use crate::agent::loop_detection::LoopDetectionConfig;
    use crate::agent::loop_detection::LoopType;
    use crate::llm::LlmError;
    use async_trait::async_trait;
    use std::sync::Arc;

    struct MockScout;

    #[async_trait]
    impl super::LoopScoutClient for MockScout {
        async fn complete_internal_text(
            &self,
            _system_prompt: &str,
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
        let outcome = service.check_tool_call("tool", "{}").expect("loop check");
        assert!(matches!(outcome, LoopDetectionOutcome::NoLoop));
    }

    #[test]
    fn tool_call_detection_re_prompts_then_halts() {
        let config = LoopDetectionConfig::default();
        let mut service = LoopDetectionService::new(Arc::new(MockScout), Arc::new(config));
        service.reset("session".to_string());

        // First 4 calls: no detection (threshold=5)
        for _ in 0..4 {
            let outcome = service.check_tool_call("tool", "{}").expect("loop check");
            assert!(matches!(outcome, LoopDetectionOutcome::NoLoop));
        }

        // 5th call: first detection → RePrompt
        let outcome = service.check_tool_call("tool", "{}").expect("loop check");
        assert!(matches!(outcome, LoopDetectionOutcome::RePrompt { .. }));
        assert_eq!(service.re_prompt_count(), 1);

        // Detector was reset, so next 4 calls: no detection
        for _ in 0..4 {
            let outcome = service.check_tool_call("tool", "{}").expect("loop check");
            assert!(matches!(outcome, LoopDetectionOutcome::NoLoop));
        }

        // 5th call again: second detection → RePrompt
        let outcome = service.check_tool_call("tool", "{}").expect("loop check");
        assert!(matches!(outcome, LoopDetectionOutcome::RePrompt { .. }));
        assert_eq!(service.re_prompt_count(), 2);

        // Detector was reset again, next 4 calls: no detection
        for _ in 0..4 {
            let outcome = service.check_tool_call("tool", "{}").expect("loop check");
            assert!(matches!(outcome, LoopDetectionOutcome::NoLoop));
        }

        // 5th call: third detection → Halt (max_re_prompts=2 exhausted)
        let outcome = service.check_tool_call("tool", "{}").expect("loop check");
        assert!(matches!(outcome, LoopDetectionOutcome::Halt { .. }));
    }

    #[test]
    fn tool_call_abab_cycle_detected() {
        let config = LoopDetectionConfig::default();
        let mut service = LoopDetectionService::new(Arc::new(MockScout), Arc::new(config));
        service.reset("session".to_string());

        // A-B-A-B-A: should be detected as a cycle
        let calls = [
            ("tool_a", r#"{"x":1}"#),
            ("tool_b", r#"{"y":2}"#),
            ("tool_a", r#"{"x":1}"#),
            ("tool_b", r#"{"y":2}"#),
        ];
        for (name, args) in &calls {
            let outcome = service.check_tool_call(name, args).expect("loop check");
            assert!(matches!(outcome, LoopDetectionOutcome::NoLoop));
        }
        let outcome = service
            .check_tool_call("tool_a", r#"{"x":1}"#)
            .expect("loop check");
        assert!(matches!(outcome, LoopDetectionOutcome::RePrompt { .. }));
    }

    #[test]
    fn re_prompt_includes_loop_type() {
        let config = LoopDetectionConfig::default();
        let mut service = LoopDetectionService::new(Arc::new(MockScout), Arc::new(config));
        service.reset("session".to_string());

        for _ in 0..4 {
            service.check_tool_call("tool", "{}").expect("loop check");
        }
        let outcome = service.check_tool_call("tool", "{}").expect("loop check");
        if let LoopDetectionOutcome::RePrompt { loop_type, .. } = outcome {
            assert_eq!(loop_type, LoopType::ToolCallLoop);
        } else {
            panic!("expected RePrompt");
        }
    }

    #[test]
    fn recovered_calls_detected() {
        // Simulates the scenario where is_recovered=true tool calls are fed
        // into the detector. Before Phase 4, these were bypassed in the runner.
        // Now all tool calls go through the detector regardless of is_recovered.
        let config = LoopDetectionConfig::default();
        let mut service = LoopDetectionService::new(Arc::new(MockScout), Arc::new(config));
        service.reset("session".to_string());

        // The detector only sees tool_name + args, not is_recovered.
        // If the same call repeats threshold times, it's detected —
        // whether or not the call was recovered.
        for _ in 0..4 {
            let outcome = service
                .check_tool_call("recovered_tool", r#"{"arg":1}"#)
                .expect("loop check");
            assert!(matches!(outcome, LoopDetectionOutcome::NoLoop));
        }
        let outcome = service
            .check_tool_call("recovered_tool", r#"{"arg":1}"#)
            .expect("loop check");
        assert!(matches!(outcome, LoopDetectionOutcome::RePrompt { .. }));
    }
}
