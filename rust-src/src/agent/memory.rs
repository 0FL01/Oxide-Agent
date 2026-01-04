//! Agent memory management with auto-compaction
//!
//! Provides conversation memory for the agent with automatic compaction
//! when token count approaches the limit. Uses tiktoken for token counting.

use crate::agent::providers::TodoList;
use crate::config::AGENT_COMPACT_THRESHOLD;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tiktoken_rs::cl100k_base;
use tracing::{info, warn};

/// A message in the agent's conversation memory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    pub role: MessageRole,
    pub content: String,
}

/// Role of a message sender
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MessageRole {
    System,
    User,
    Assistant,
}

impl AgentMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
        }
    }
}

/// Agent memory with auto-compaction support
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMemory {
    messages: Vec<AgentMessage>,
    pub todos: TodoList,
    token_count: usize,
    max_tokens: usize,
    compact_threshold: usize,
}

impl AgentMemory {
    /// Create a new agent memory with the specified maximum token limit
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
        let msg_tokens = self.count_tokens(&msg.content);
        self.token_count += msg_tokens;
        self.messages.push(msg);

        // Check if we need to compact
        if self.token_count > self.compact_threshold {
            if let Err(e) = self.compact() {
                warn!("Failed to compact agent memory: {}", e);
            }
        }
    }

    /// Get all messages in memory
    pub fn get_messages(&self) -> &[AgentMessage] {
        &self.messages
    }

    /// Get current token count
    pub fn token_count(&self) -> usize {
        self.token_count
    }

    /// Clear all messages from memory
    pub fn clear(&mut self) {
        self.messages.clear();
        self.todos.clear();
        self.token_count = 0;
    }

    /// Count tokens in a string using cl100k tokenizer (GPT-4/Claude compatible)
    fn count_tokens(&self, text: &str) -> usize {
        match cl100k_base() {
            Ok(bpe) => bpe.encode_with_special_tokens(text).len(),
            Err(_) => {
                // Fallback: rough estimate of 4 chars per token
                text.len() / 4
            }
        }
    }

    /// Compact memory by summarizing older messages
    ///
    /// Strategy:
    /// 1. Keep the last 20% of messages intact (recent context)
    /// 2. Summarize the first 80% into a single system message
    ///
    /// Note: In this iteration, we use a simple truncation strategy.
    /// Full LLM-based summarization will be added when MCP tools are integrated.
    fn compact(&mut self) -> Result<()> {
        if self.messages.len() < 5 {
            return Ok(()); // Not enough messages to compact
        }

        info!(
            "Compacting agent memory: {} tokens, {} messages",
            self.token_count,
            self.messages.len()
        );

        // Calculate split point (keep last 20%)
        let keep_count = (self.messages.len() as f64 * 0.2).ceil() as usize;
        let split_at = self.messages.len().saturating_sub(keep_count);

        if split_at == 0 {
            return Ok(());
        }

        // Extract messages to summarize
        let to_summarize: Vec<_> = self.messages.drain(..split_at).collect();

        // Create a summary of the old messages (simple version)
        let summary = self.create_simple_summary(&to_summarize);
        let summary_msg = AgentMessage::system(format!("[Предыдущий контекст сжат]\n{}", summary));

        // Insert summary at the beginning
        self.messages.insert(0, summary_msg);

        // Recalculate token count
        self.token_count = self
            .messages
            .iter()
            .map(|m| self.count_tokens(&m.content))
            .sum();

        info!(
            "Memory compacted: {} tokens, {} messages remaining",
            self.token_count,
            self.messages.len()
        );

        Ok(())
    }

    /// Create a simple summary of messages (no LLM, just extraction of key points)
    fn create_simple_summary(&self, messages: &[AgentMessage]) -> String {
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
                    summary_parts.push(format!("• Запрос: {}", truncated));
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
                        summary_parts.push(format!("• Ответ: {}...", first_part));
                    }
                }
                MessageRole::System => {
                    // Skip system messages in summary
                }
            }
        }

        // Limit summary to last 10 items
        let items: Vec<_> = summary_parts.into_iter().rev().take(10).collect();
        items.into_iter().rev().collect::<Vec<_>>().join("\n")
    }

    /// Check if memory needs compaction soon
    pub fn needs_compaction(&self) -> bool {
        self.token_count > self.compact_threshold
    }

    /// Get percentage of memory used
    pub fn usage_percent(&self) -> u8 {
        ((self.token_count as f64 / self.max_tokens as f64) * 100.0).min(100.0) as u8
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
