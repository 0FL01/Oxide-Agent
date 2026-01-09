//! Agent memory management with auto-compaction
//!
//! Provides conversation memory for the agent with automatic compaction
//! when token count approaches the limit. Uses tiktoken for token counting.

use crate::agent::providers::TodoList;
use crate::config::AGENT_COMPACT_THRESHOLD;
use crate::llm::ToolCall;
use serde::{Deserialize, Serialize};
use tiktoken_rs::cl100k_base;
use tracing::info;

/// A message in the agent's conversation memory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    /// Role of the message sender
    pub role: MessageRole,
    /// Text content of the message
    pub content: String,
    /// Optional reasoning/thinking content (for models that support it, e.g., GLM-4.7)
    /// This is counted towards token limits but not shown to user
    pub reasoning: Option<String>,
    /// Tool call ID (for tool responses)
    pub tool_call_id: Option<String>,
    /// Tool name (for tool responses)
    pub tool_name: Option<String>,
    /// Tool calls made by assistant
    pub tool_calls: Option<Vec<ToolCall>>,
}

/// Role of a message sender in agent memory
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MessageRole {
    /// System message for core instructions
    System,
    /// Message from the user
    User,
    /// Response from the assistant/agent
    Assistant,
    /// Tool response message
    Tool,
}

impl AgentMessage {
    /// Create a new system message
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: content.into(),
            reasoning: None,
            tool_call_id: None,
            tool_name: None,
            tool_calls: None,
        }
    }

    /// Create a new user message
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
            reasoning: None,
            tool_call_id: None,
            tool_name: None,
            tool_calls: None,
        }
    }

    /// Create a new assistant message
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
            reasoning: None,
            tool_call_id: None,
            tool_name: None,
            tool_calls: None,
        }
    }

    /// Create a new assistant message with reasoning/thinking
    pub fn assistant_with_reasoning(
        content: impl Into<String>,
        reasoning: impl Into<String>,
    ) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
            reasoning: Some(reasoning.into()),
            tool_call_id: None,
            tool_name: None,
            tool_calls: None,
        }
    }

    /// Create a new tool response message
    pub fn tool(tool_call_id: &str, name: &str, content: &str) -> Self {
        Self {
            role: MessageRole::Tool,
            content: content.into(),
            reasoning: None,
            tool_call_id: Some(tool_call_id.to_string()),
            tool_name: Some(name.to_string()),
            tool_calls: None,
        }
    }

    /// Create a new assistant message with tool calls
    pub fn assistant_with_tools(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
            reasoning: None,
            tool_call_id: None,
            tool_name: None,
            tool_calls: Some(tool_calls),
        }
    }
}

/// Agent memory with auto-compaction support
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMemory {
    messages: Vec<AgentMessage>,
    /// Task list for the agent
    pub todos: TodoList,
    token_count: usize,
    max_tokens: usize,
    compact_threshold: usize,
}

impl AgentMemory {
    /// Create a new agent memory with the specified maximum token limit
    #[must_use]
    pub fn new(max_tokens: usize) -> Self {
        Self {
            messages: Vec::new(),
            todos: TodoList::new(),
            token_count: 0,
            max_tokens,
            compact_threshold: AGENT_COMPACT_THRESHOLD,
        }
    }

    /// Add a message to memory, triggering compaction if needed
    pub fn add_message(&mut self, msg: AgentMessage) {
        let mut msg_tokens = Self::count_tokens(&msg.content);

        // Also count reasoning tokens (GLM-4.7 thinking process)
        if let Some(ref reasoning) = msg.reasoning {
            msg_tokens += Self::count_tokens(reasoning);
        }

        self.token_count += msg_tokens;
        self.messages.push(msg);

        // Check if we need to compact
        if self.token_count > self.compact_threshold {
            self.compact();
        }
    }

    /// Get all messages in memory
    #[must_use]
    pub fn get_messages(&self) -> &[AgentMessage] {
        &self.messages
    }

    /// Get current token count
    #[must_use]
    pub const fn token_count(&self) -> usize {
        self.token_count
    }

    /// Synchronize token count with actual API usage data
    ///
    /// This replaces the local heuristic estimate with the authoritative
    /// count from the API response.
    pub fn sync_token_count(&mut self, real_total_tokens: usize) {
        let diff = real_total_tokens as i64 - self.token_count as i64;
        if diff.abs() > 100 {
            tracing::debug!(
                local = self.token_count,
                real = real_total_tokens,
                diff = diff,
                "Token sync: significant drift detected"
            );
        }
        self.token_count = real_total_tokens;
    }

    /// Clear all messages from memory
    pub fn clear(&mut self) {
        self.messages.clear();
        self.todos.clear();
        self.token_count = 0;
    }

    /// Count tokens in a string using cl100k tokenizer (GPT-4/Claude compatible)
    fn count_tokens(text: &str) -> usize {
        cl100k_base().map_or(text.len() / 4, |bpe| {
            bpe.encode_with_special_tokens(text).len()
        })
    }

    /// Compact memory by summarizing older messages
    ///
    /// Strategy:
    /// 1. Keep the last 20% of messages intact (recent context)
    /// 2. Summarize the first 80% into a single system message
    ///
    /// Note: In this iteration, we use a simple truncation strategy.
    /// Full LLM-based summarization will be added when MCP tools are integrated.
    fn compact(&mut self) {
        if self.messages.len() < 5 {
            return; // Not enough messages to compact
        }

        info!(
            "Compacting agent memory: {} tokens, {} messages",
            self.token_count,
            self.messages.len()
        );

        // Calculate split point (keep last 20%)
        let keep_count = (self.messages.len() * 2).div_ceil(10);
        let split_at = self.messages.len().saturating_sub(keep_count);

        if split_at == 0 {
            return;
        }

        // Extract messages to summarize
        let to_summarize: Vec<_> = self.messages.drain(..split_at).collect();

        // Create a summary of the old messages (simple version)
        let summary = Self::create_simple_summary(&to_summarize);
        let summary_msg = AgentMessage::system(format!("[Предыдущий контекст сжат]\n{summary}"));

        // Insert summary at the beginning
        self.messages.insert(0, summary_msg);

        // Recalculate token count
        self.token_count = self
            .messages
            .iter()
            .map(|m| {
                let mut tokens = Self::count_tokens(&m.content);
                // Also count reasoning tokens (GLM-4.7 thinking process)
                if let Some(ref reasoning) = m.reasoning {
                    tokens += Self::count_tokens(reasoning);
                }
                tokens
            })
            .sum();

        info!(
            "Memory compacted: {} tokens, {} messages remaining",
            self.token_count,
            self.messages.len()
        );
    }

    /// Create a simple summary of messages (no LLM, just extraction of key points)
    fn create_simple_summary(messages: &[AgentMessage]) -> String {
        let mut summary_parts = Vec::new();

        // Extract user requests and assistant conclusions
        for msg in messages {
            match msg.role {
                MessageRole::User => {
                    let truncated = if msg.content.len() > 200 {
                        format!("{}...", &msg.content.chars().take(200).collect::<String>())
                    } else {
                        msg.content.clone()
                    };
                    summary_parts.push(format!("• Запрос: {truncated}"));
                }
                MessageRole::Assistant => {
                    // Extract first sentence or first 150 chars
                    let first_part = msg
                        .content
                        .split('.')
                        .next()
                        .unwrap_or(&msg.content)
                        .chars()
                        .take(150)
                        .collect::<String>();
                    if !first_part.is_empty() {
                        summary_parts.push(format!("• Ответ: {first_part}..."));
                    }
                }
                MessageRole::System => {
                    // Skip system messages in summary
                }
                MessageRole::Tool => {
                    // Include tool results in summary
                    if let Some(ref name) = msg.tool_name {
                        let truncated = if msg.content.len() > 150 {
                            format!("{}...", &msg.content.chars().take(150).collect::<String>())
                        } else {
                            msg.content.clone()
                        };
                        summary_parts.push(format!("• Инструмент {name}: {truncated}"));
                    }
                }
            }
        }

        // Limit summary to last 10 items
        summary_parts
            .into_iter()
            .rev()
            .take(10)
            .rev()
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Check if memory needs compaction soon
    #[must_use]
    pub const fn needs_compaction(&self) -> bool {
        self.token_count > self.compact_threshold
    }

    /// Get percentage of memory used
    #[must_use]
    pub fn usage_percent(&self) -> u8 {
        if self.max_tokens == 0 {
            return 100;
        }
        let percent = (self.token_count * 100) / self.max_tokens;
        u8::try_from(percent.min(100)).unwrap_or(100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_add_message() {
        let mut memory = AgentMemory::new(100_000);
        memory.add_message(AgentMessage::user("Hello, agent!"));
        assert_eq!(memory.get_messages().len(), 1);
        assert!(memory.token_count() > 0);
    }

    #[test]
    fn test_memory_clear() {
        let mut memory = AgentMemory::new(100_000);
        memory.add_message(AgentMessage::user("Test"));
        memory.clear();
        assert_eq!(memory.get_messages().len(), 0);
        assert_eq!(memory.token_count(), 0);
    }
}
