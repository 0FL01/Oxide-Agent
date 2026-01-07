//! LLM-based loop detector.

use super::config::LoopDetectionConfig;
use super::types::LoopDetectionError;
use crate::agent::memory::{AgentMemory, AgentMessage, MessageRole};
use crate::llm::{LlmClient, LlmError, Message};
use async_trait::async_trait;
use serde::Deserialize;
use std::sync::Arc;
use tokio::time::{timeout, Duration};
use tracing::{debug, warn};

const MIN_INTERVAL: usize = 3;
const MAX_INTERVAL: usize = 15;
const LLM_TIMEOUT_SECS: u64 = 30;

const SYSTEM_PROMPT: &str = "You are an AI diagnostic agent. Analyze the conversation for \
unproductive loops (repetitive actions, cognitive loops, or alternating patterns). \
Differentiate legitimate incremental progress from looping. Respond ONLY with JSON.";

const USER_PROMPT: &str = r#"Return JSON:
{
  "is_stuck": bool,
  "confidence": 0.0-1.0,
  "reasoning": "short explanation"
}"#;

#[derive(Debug, Deserialize)]
struct LlmLoopResponse {
    #[serde(default)]
    is_stuck: bool,
    #[serde(default)]
    confidence: f64,
    #[serde(default)]
    reasoning: String,
}

#[async_trait]
pub trait LoopScoutClient: Send + Sync {
    async fn chat_completion(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_name: &str,
    ) -> Result<String, LlmError>;
}

#[async_trait]
impl LoopScoutClient for LlmClient {
    async fn chat_completion(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_name: &str,
    ) -> Result<String, LlmError> {
        self.chat_completion(system_prompt, history, user_message, model_name)
            .await
    }
}

/// LLM-based loop detector with adaptive interval.
pub struct LlmLoopDetector {
    client: Arc<dyn LoopScoutClient>,
    check_after_turns: usize,
    check_interval: usize,
    last_check_turn: usize,
    confidence_threshold: f64,
    history_count: usize,
    scout_model: String,
}

impl LlmLoopDetector {
    /// Create a new LLM loop detector.
    #[must_use]
    pub fn new(client: Arc<dyn LoopScoutClient>, config: &LoopDetectionConfig) -> Self {
        Self {
            client,
            check_after_turns: config.llm_check_after_turns,
            check_interval: config.llm_check_interval.max(MIN_INTERVAL),
            last_check_turn: 0,
            confidence_threshold: config.llm_confidence_threshold,
            history_count: config.llm_history_count,
            scout_model: config.scout_model.clone(),
        }
    }

    /// Reset the internal counters and intervals.
    pub fn reset(&mut self, config: &LoopDetectionConfig) {
        self.check_after_turns = config.llm_check_after_turns;
        self.check_interval = config.llm_check_interval.max(MIN_INTERVAL);
        self.last_check_turn = 0;
        self.confidence_threshold = config.llm_confidence_threshold;
        self.history_count = config.llm_history_count;
        self.scout_model = config.scout_model.clone();
    }

    /// Whether a check is due for this iteration.
    #[must_use]
    pub fn should_check(&self, iteration: usize) -> bool {
        let turn = iteration.saturating_add(1);
        if turn < self.check_after_turns {
            return false;
        }
        if self.last_check_turn == 0 {
            return true;
        }
        turn.saturating_sub(self.last_check_turn) >= self.check_interval
    }

    /// Run the LLM check.
    pub async fn check(
        &mut self,
        memory: &AgentMemory,
        iteration: usize,
    ) -> Result<bool, LoopDetectionError> {
        if !self.should_check(iteration) {
            return Ok(false);
        }

        let turn = iteration.saturating_add(1);
        self.last_check_turn = turn;

        let history = self.prepare_history(memory);
        if history.is_empty() {
            return Ok(false);
        }

        debug!(
            iteration = iteration,
            interval = self.check_interval,
            history_size = history.len(),
            "LLM loop check triggered"
        );

        let llm_response = timeout(
            Duration::from_secs(LLM_TIMEOUT_SECS),
            self.client
                .chat_completion(SYSTEM_PROMPT, &history, USER_PROMPT, &self.scout_model),
        )
        .await
        .map_err(|e| LoopDetectionError::LlmFailure(format!("LLM timeout: {e}")))?;

        let llm_response =
            llm_response.map_err(|e| LoopDetectionError::LlmFailure(e.to_string()))?;

        let parsed = Self::parse_response(&llm_response)?;
        debug!(
            confidence = parsed.confidence,
            is_stuck = parsed.is_stuck,
            reasoning = %parsed.reasoning,
            "LLM loop check response"
        );
        self.update_interval(parsed.confidence);

        Ok(parsed.is_stuck && parsed.confidence >= self.confidence_threshold)
    }

    fn update_interval(&mut self, confidence: f64) {
        let bounded = confidence.clamp(0.0, 1.0);
        let interval = MIN_INTERVAL as f64 + (MAX_INTERVAL - MIN_INTERVAL) as f64 * (1.0 - bounded);
        let interval = interval.round() as usize;
        self.check_interval = interval.clamp(MIN_INTERVAL, MAX_INTERVAL);
    }

    fn prepare_history(&self, memory: &AgentMemory) -> Vec<Message> {
        let mut messages: Vec<AgentMessage> = memory.get_messages().to_vec();
        if messages.len() > self.history_count {
            messages = messages[messages.len() - self.history_count..].to_vec();
        }

        while let Some(last) = messages.last() {
            if Self::is_tool_call_message(last) {
                messages.pop();
            } else {
                break;
            }
        }

        while let Some(first) = messages.first() {
            if Self::is_tool_response_message(first) {
                messages.remove(0);
            } else {
                break;
            }
        }

        messages.iter().map(Self::convert_message).collect()
    }

    fn is_tool_call_message(message: &AgentMessage) -> bool {
        message.role == MessageRole::Assistant && message.tool_calls.is_some()
    }

    fn is_tool_response_message(message: &AgentMessage) -> bool {
        message.role == MessageRole::Tool
    }

    fn convert_message(message: &AgentMessage) -> Message {
        let role = match message.role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        };
        Message {
            role: role.to_string(),
            content: message.content.clone(),
            tool_call_id: message.tool_call_id.clone(),
            name: message.tool_name.clone(),
            tool_calls: message.tool_calls.clone(),
        }
    }

    fn parse_response(raw: &str) -> Result<LlmLoopResponse, LoopDetectionError> {
        if let Ok(parsed) = serde_json::from_str::<LlmLoopResponse>(raw) {
            return Ok(parsed);
        }

        if let Some(json_str) = Self::extract_first_json_object(raw) {
            let parsed = serde_json::from_str::<LlmLoopResponse>(&json_str).map_err(|e| {
                LoopDetectionError::LlmFailure(format!("Invalid JSON response: {e}"))
            })?;
            return Ok(parsed);
        }

        warn!(response = %raw, "LLM loop check returned non-JSON response");
        Err(LoopDetectionError::LlmFailure(
            "LLM response missing JSON object".to_string(),
        ))
    }

    fn extract_first_json_object(input: &str) -> Option<String> {
        let mut depth: usize = 0;
        let mut start_idx = None;
        let mut in_string = false;
        let mut escaped = false;

        for (idx, ch) in input.char_indices() {
            match ch {
                '"' if !escaped => in_string = !in_string,
                '\\' if in_string => escaped = !escaped,
                '{' if !in_string => {
                    if start_idx.is_none() {
                        start_idx = Some(idx);
                    }
                    depth = depth.saturating_add(1);
                }
                '}' if !in_string => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        if let Some(start) = start_idx {
                            return Some(input[start..=idx].to_string());
                        }
                    }
                }
                _ => {
                    escaped = false;
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{LlmLoopDetector, LoopScoutClient};
    use crate::agent::loop_detection::LoopDetectionConfig;
    use crate::agent::memory::{AgentMemory, AgentMessage};
    use crate::llm::Message;
    use async_trait::async_trait;
    use std::sync::Arc;

    struct MockLoopScout {
        responses: Vec<String>,
        index: std::sync::Mutex<usize>,
    }

    #[async_trait]
    impl LoopScoutClient for MockLoopScout {
        async fn chat_completion(
            &self,
            _system_prompt: &str,
            _history: &[Message],
            _user_message: &str,
            _model_name: &str,
        ) -> Result<String, crate::llm::LlmError> {
            let mut index = self
                .index
                .lock()
                .map_err(|_| crate::llm::LlmError::Unknown("Mutex poisoned in mock".to_string()))?;
            let response = self.responses.get(*index).cloned().unwrap_or_else(|| {
                r#"{"is_stuck":false,"confidence":0.0,"reasoning":""}"#.to_string()
            });
            *index = index.saturating_add(1);
            Ok(response)
        }
    }

    fn create_memory() -> AgentMemory {
        let mut memory = AgentMemory::new(1000);
        memory.add_message(AgentMessage::user("Task"));
        memory.add_message(AgentMessage::assistant("Working"));
        memory
    }

    #[tokio::test]
    async fn detects_loop_when_confident() {
        let config = LoopDetectionConfig::default();
        let client = Arc::new(MockLoopScout {
            responses: vec![r#"{"is_stuck":true,"confidence":0.95,"reasoning":"loop"}"#.to_string()],
            index: std::sync::Mutex::new(0),
        });
        let mut detector = LlmLoopDetector::new(client, &config);
        let memory = create_memory();
        let detected = detector.check(&memory, 40).await.unwrap_or(false);
        assert!(detected);
    }

    #[tokio::test]
    async fn skips_before_threshold() {
        let config = LoopDetectionConfig::default();
        let client = Arc::new(MockLoopScout {
            responses: vec![r#"{"is_stuck":true,"confidence":0.95,"reasoning":"loop"}"#.to_string()],
            index: std::sync::Mutex::new(0),
        });
        let mut detector = LlmLoopDetector::new(client, &config);
        let memory = create_memory();
        let detected = detector.check(&memory, 1).await.unwrap_or(false);
        assert!(!detected);
    }
}
