//! Narrator module for generating human-readable status updates
//!
//! Uses a lightweight sidecar LLM to interpret the primary agent's reasoning
//! and tool calls into concise narrative updates.

use crate::llm::{LlmClient, LlmError, Message, ToolCall};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, warn};

/// Narrative update for user display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Narrative {
    /// Short action-oriented title (3-5 words)
    pub headline: String,
    /// Detailed context explanation (2-3 sentences)
    pub content: String,
}

/// Narrator for generating human-readable status updates
pub struct Narrator {
    llm_client: Arc<LlmClient>,
}

impl Narrator {
    /// Create a new narrator with the given LLM client
    #[must_use]
    pub fn new(llm_client: Arc<LlmClient>) -> Self {
        Self { llm_client }
    }

    /// Generate a narrative from agent reasoning and tool calls
    ///
    /// Returns `None` on failure for graceful fallback to static templates.
    pub async fn generate(
        &self,
        reasoning_content: Option<&str>,
        tool_calls: &[ToolCall],
    ) -> Option<Narrative> {
        // Skip if no meaningful input
        if reasoning_content.is_none() && tool_calls.is_empty() {
            return None;
        }

        let model = &self.llm_client.narrator_model;
        let provider = &self.llm_client.narrator_provider;

        if !self.llm_client.is_provider_available(provider) {
            warn!("Narrator disabled: {provider} provider not configured");
            return None;
        }

        let user_message = self.build_user_message(reasoning_content, tool_calls);

        debug!(
            model = %model,
            provider = %provider,
            tool_count = tool_calls.len(),
            has_reasoning = reasoning_content.is_some(),
            "Generating narrative"
        );

        match self.call_llm(&user_message, model).await {
            Ok(response) => self.parse_narrative(&response),
            Err(e) => {
                warn!(error = %e, "Narrator LLM call failed, using fallback");
                None
            }
        }
    }

    /// Build user message for narrator LLM
    fn build_user_message(&self, reasoning: Option<&str>, tool_calls: &[ToolCall]) -> String {
        let mut parts = Vec::new();

        if let Some(r) = reasoning {
            let truncated = crate::utils::truncate_str(r, 500);
            parts.push(format!("## Agent Reasoning\n{truncated}"));
        }

        if !tool_calls.is_empty() {
            let tools: Vec<String> = tool_calls
                .iter()
                .map(|tc| {
                    let args_preview = crate::utils::truncate_str(&tc.function.arguments, 100);
                    format!("- `{}`: {}", tc.function.name, args_preview)
                })
                .collect();
            parts.push(format!("## Tool Calls\n{}", tools.join("\n")));
        }

        parts.join("\n\n")
    }

    /// Call the narrator LLM with retry logic
    async fn call_llm(&self, user_message: &str, model: &str) -> Result<String, LlmError> {
        let system_prompt = Self::system_prompt();
        let messages = [Message::user(user_message)];

        // Use chat_completion which has retry logic built-in
        self.llm_client
            .chat_completion(&system_prompt, &messages, "", model)
            .await
    }

    /// Parse JSON response into Narrative struct
    fn parse_narrative(&self, response: &str) -> Option<Narrative> {
        // Try to extract JSON from response (may be wrapped in markdown code block)
        let json_str = Self::extract_json(response);

        match serde_json::from_str::<Narrative>(json_str) {
            Ok(narrative) => {
                // Validate non-empty fields
                if narrative.headline.trim().is_empty() || narrative.content.trim().is_empty() {
                    warn!("Narrator returned empty fields");
                    return None;
                }
                Some(narrative)
            }
            Err(e) => {
                warn!(error = %e, response = %response, "Failed to parse narrator response");
                None
            }
        }
    }

    /// Extract JSON from potentially markdown-wrapped response
    fn extract_json(response: &str) -> &str {
        let trimmed = response.trim();

        // Handle ```json ... ``` wrapper
        if let Some(start) = trimmed.find('{') {
            if let Some(end) = trimmed.rfind('}') {
                return &trimmed[start..=end];
            }
        }

        trimmed
    }

    /// System prompt for narrator LLM
    fn system_prompt() -> String {
        r#"You are a technical narrator for an AI agent execution log.
Your task: Convert raw agent reasoning and tool calls into a concise, user-friendly status update.

Output ONLY valid JSON in this exact format:
{"headline": "Short Action Title", "content": "1-2 sentences describing WHAT the agent is doing right now."}

Rules:
- headline: 3-5 words, action-oriented (e.g., "Searching for Dependencies", "Writing Python Script")
- content: Describe ONLY the action. Do not explain why it is useful. Never use phrases like "This will help", "To achieve this", "In order to".
- Use User's language for output
- Be concise and professional
- Do NOT include raw technical details or code

Example:
{"headline": "Analyzing Project Structure", "content": "Agent examines file structure and configuration files."}"#
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_plain() {
        let input = r#"{"headline": "Test", "content": "Content"}"#;
        let result = Narrator::extract_json(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_extract_json_markdown_wrapped() {
        let input = r#"```json
{"headline": "Test", "content": "Content"}
```"#;
        let result = Narrator::extract_json(input);
        assert!(result.starts_with('{'));
        assert!(result.ends_with('}'));
    }

    #[test]
    fn test_parse_narrative_valid_json() {
        let input = r#"{"headline": "Test", "content": "Content"}"#;
        let parsed: Result<Narrative, _> = serde_json::from_str(input);
        assert!(parsed.is_ok());
        let narrative = parsed.expect("Failed to parse valid JSON in test");
        assert_eq!(narrative.headline, "Test");
        assert_eq!(narrative.content, "Content");
    }
}
