//! Agent memory management for Agent Mode sessions.
//!
//! Provides conversation memory for the agent and lightweight token
//! accounting utilities. Compaction orchestration lives outside this module.

use crate::agent::compaction::{AgentMessageKind, CompactionRetention};
use crate::agent::providers::TodoList;
use crate::config::AGENT_COMPACT_THRESHOLD;
use crate::llm::ToolCall;
use serde::{Deserialize, Serialize};
use tiktoken_rs::cl100k_base;
use tracing::info;

const TOPIC_AGENTS_MD_SYSTEM_PREFIX: &str = "[TOPIC_AGENTS_MD]\n";

/// A message in the agent's conversation memory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    /// Semantic kind used by compaction policies.
    #[serde(default)]
    pub kind: AgentMessageKind,
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
        Self::system_context(content)
    }

    /// Create a generic system-context message.
    pub fn system_context(content: impl Into<String>) -> Self {
        Self {
            kind: AgentMessageKind::SystemContext,
            role: MessageRole::System,
            content: content.into(),
            reasoning: None,
            tool_call_id: None,
            tool_name: None,
            tool_calls: None,
        }
    }

    /// Create a pinned system message carrying topic-scoped `AGENTS.md` content.
    pub fn topic_agents_md(content: impl AsRef<str>) -> Self {
        Self {
            kind: AgentMessageKind::TopicAgentsMd,
            role: MessageRole::System,
            content: format!("{TOPIC_AGENTS_MD_SYSTEM_PREFIX}{}", content.as_ref().trim()),
            reasoning: None,
            tool_call_id: None,
            tool_name: None,
            tool_calls: None,
        }
    }

    /// Create a new user message
    pub fn user(content: impl Into<String>) -> Self {
        Self::user_turn(content)
    }

    /// Create the primary task message for an agent run.
    pub fn user_task(content: impl Into<String>) -> Self {
        Self {
            kind: AgentMessageKind::UserTask,
            role: MessageRole::User,
            content: content.into(),
            reasoning: None,
            tool_call_id: None,
            tool_name: None,
            tool_calls: None,
        }
    }

    /// Create a user runtime-context injection message.
    pub fn runtime_context(content: impl Into<String>) -> Self {
        Self {
            kind: AgentMessageKind::RuntimeContext,
            role: MessageRole::User,
            content: content.into(),
            reasoning: None,
            tool_call_id: None,
            tool_name: None,
            tool_calls: None,
        }
    }

    /// Create a generic user turn.
    pub fn user_turn(content: impl Into<String>) -> Self {
        Self {
            kind: AgentMessageKind::UserTurn,
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
            kind: AgentMessageKind::AssistantResponse,
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
            kind: AgentMessageKind::AssistantReasoning,
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
            kind: AgentMessageKind::ToolResult,
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
            kind: AgentMessageKind::AssistantToolCall,
            role: MessageRole::Assistant,
            content: content.into(),
            reasoning: None,
            tool_call_id: None,
            tool_name: None,
            tool_calls: Some(tool_calls),
        }
    }

    /// Create a message carrying dynamically loaded skill instructions.
    pub fn skill_context(content: impl Into<String>) -> Self {
        Self {
            kind: AgentMessageKind::SkillContext,
            ..Self::system_context(content)
        }
    }

    /// Create a system message instructing the agent to replay an approved action.
    pub fn approval_replay(content: impl Into<String>) -> Self {
        Self {
            kind: AgentMessageKind::ApprovalReplay,
            ..Self::system_context(content)
        }
    }

    /// Create a protected infra status message.
    pub fn infra_status(content: impl Into<String>) -> Self {
        Self {
            kind: AgentMessageKind::InfraStatus,
            ..Self::system_context(content)
        }
    }

    /// Create a summary message generated by the compaction pipeline.
    pub fn summary(content: impl Into<String>) -> Self {
        Self {
            kind: AgentMessageKind::Summary,
            ..Self::system_context(content)
        }
    }

    /// Create a lightweight reference to archived context.
    pub fn archive_reference(content: impl Into<String>) -> Self {
        Self {
            kind: AgentMessageKind::ArchiveReference,
            ..Self::system_context(content)
        }
    }

    /// Resolve the semantic kind for this message, including legacy fallbacks.
    #[must_use]
    pub fn resolved_kind(&self) -> AgentMessageKind {
        if self.kind != AgentMessageKind::Legacy {
            return self.kind;
        }

        if self.is_topic_agents_md() {
            return AgentMessageKind::TopicAgentsMd;
        }

        match self.role {
            MessageRole::System => AgentMessageKind::SystemContext,
            MessageRole::User => AgentMessageKind::UserTurn,
            MessageRole::Assistant if self.tool_calls.is_some() => {
                AgentMessageKind::AssistantToolCall
            }
            MessageRole::Assistant if self.reasoning.is_some() => {
                AgentMessageKind::AssistantReasoning
            }
            MessageRole::Assistant => AgentMessageKind::AssistantResponse,
            MessageRole::Tool => AgentMessageKind::ToolResult,
        }
    }

    /// Retention class used by compaction policies.
    #[must_use]
    pub fn retention(&self) -> CompactionRetention {
        self.resolved_kind().retention()
    }
}

/// Agent memory for the active hot context window
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMemory {
    messages: Vec<AgentMessage>,
    /// Task list for the agent
    pub todos: TodoList,
    token_count: usize,
    max_tokens: usize,
    compact_threshold: usize,
    /// Last synchronized token count from API
    #[serde(default)]
    last_api_token_count: Option<usize>,
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
            last_api_token_count: None,
        }
    }

    /// Add a message to memory and update token accounting.
    pub fn add_message(&mut self, msg: AgentMessage) {
        let mut msg_tokens = Self::count_tokens(&msg.content);

        // Also count reasoning tokens (GLM-4.7 thinking process)
        if let Some(ref reasoning) = msg.reasoning {
            msg_tokens += Self::count_tokens(reasoning);
        }

        self.token_count += msg_tokens;
        self.messages.push(msg);
    }

    /// Returns true when memory already contains a pinned topic `AGENTS.md` message.
    #[must_use]
    pub fn has_topic_agents_md(&self) -> bool {
        self.messages.iter().any(AgentMessage::is_topic_agents_md)
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

    /// Get the configured maximum token count.
    #[must_use]
    pub const fn max_tokens(&self) -> usize {
        self.max_tokens
    }

    /// Get the legacy compaction threshold retained during the migration.
    #[must_use]
    pub const fn compact_threshold(&self) -> usize {
        self.compact_threshold
    }

    /// Get the last synchronized API token count
    #[must_use]
    pub const fn api_token_count(&self) -> Option<usize> {
        self.last_api_token_count
    }

    /// Synchronize token count with actual API usage data
    ///
    /// This replaces the local heuristic estimate with the authoritative
    /// count from the API response.
    pub fn sync_token_count(&mut self, real_total_tokens: usize) {
        let diff = real_total_tokens as i64 - self.token_count as i64;

        tracing::info!(
            total = real_total_tokens,
            diff = diff,
            "METRIC: Token usage synchronized from API"
        );

        if diff.abs() > 100 {
            tracing::warn!(
                local = self.token_count,
                real = real_total_tokens,
                diff = diff,
                "Token sync: significant drift detected"
            );
        }
        self.token_count = real_total_tokens;
        self.last_api_token_count = Some(real_total_tokens);
    }

    /// Clear all messages from memory
    pub fn clear(&mut self) {
        self.messages.clear();
        self.todos.clear();
        self.token_count = 0;
        self.last_api_token_count = None;
    }

    /// Count tokens in a string using cl100k tokenizer (GPT-4/Claude compatible)
    fn count_tokens(text: &str) -> usize {
        cl100k_base().map_or(text.len() / 4, |bpe| {
            bpe.encode_with_special_tokens(text).len()
        })
    }

    /// Apply the legacy local compaction strategy explicitly.
    ///
    /// This method remains available as a temporary migration fallback while the
    /// new orchestration-based compaction pipeline is introduced.
    #[allow(dead_code)]
    pub(crate) fn apply_legacy_local_compaction(&mut self) {
        if self.messages.len() < 5 {
            return; // Not enough messages to compact
        }

        let mut pinned_messages = Vec::new();
        let mut regular_messages = Vec::new();
        for message in self.messages.drain(..) {
            if matches!(
                message.retention(),
                CompactionRetention::Pinned | CompactionRetention::ProtectedLive
            ) {
                pinned_messages.push(message);
            } else {
                regular_messages.push(message);
            }
        }

        if regular_messages.len() < 5 {
            self.messages = pinned_messages;
            self.messages.extend(regular_messages);
            return;
        }

        info!(
            "Compacting agent memory: {} tokens, {} messages",
            self.token_count,
            pinned_messages.len() + regular_messages.len()
        );

        // Calculate split point (keep last 20%)
        let keep_count = (regular_messages.len() * 2).div_ceil(10);
        let split_at = regular_messages.len().saturating_sub(keep_count);

        if split_at == 0 {
            self.messages = pinned_messages;
            self.messages.extend(regular_messages);
            return;
        }

        // Extract messages to summarize
        let to_summarize: Vec<_> = regular_messages.drain(..split_at).collect();

        // Create a summary of the old messages (simple version)
        let summary = Self::create_simple_summary(&to_summarize);
        let summary_msg =
            AgentMessage::summary(format!("[Previous context compressed]\n{summary}"));

        self.messages = pinned_messages;
        self.messages.push(summary_msg);
        self.messages.extend(regular_messages);

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

        // Reset API token count since we compacted and lost the 1:1 mapping
        self.last_api_token_count = None;
    }

    /// Create a simple summary of messages (no LLM, just extraction of key points)
    #[allow(dead_code)]
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
                    summary_parts.push(format!("• Request: {truncated}"));
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
                        summary_parts.push(format!("• Answer: {first_part}..."));
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
                        summary_parts.push(format!("• Tool {name}: {truncated}"));
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

impl AgentMessage {
    #[must_use]
    fn is_topic_agents_md(&self) -> bool {
        self.role == MessageRole::System && self.content.starts_with(TOPIC_AGENTS_MD_SYSTEM_PREFIX)
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

    #[test]
    fn test_sync_token_count() {
        let mut memory = AgentMemory::new(100_000);
        memory.add_message(AgentMessage::user("Hello"));

        // Initial state
        assert_eq!(memory.api_token_count(), None);

        // Sync
        memory.sync_token_count(1234);
        assert_eq!(memory.api_token_count(), Some(1234));
        assert_eq!(memory.token_count(), 1234);

        // Add more messages (local count increases, api count stays same)
        memory.add_message(AgentMessage::user("More text"));
        assert!(memory.token_count() > 1234);
        assert_eq!(memory.api_token_count(), Some(1234));

        // Clear
        memory.clear();
        assert_eq!(memory.api_token_count(), None);
    }

    #[test]
    fn test_topic_agents_md_detection() {
        let mut memory = AgentMemory::new(100_000);
        assert!(!memory.has_topic_agents_md());

        memory.add_message(AgentMessage::topic_agents_md("# Topic AGENTS"));

        assert!(memory.has_topic_agents_md());
        assert!(memory.get_messages()[0]
            .content
            .starts_with(TOPIC_AGENTS_MD_SYSTEM_PREFIX));
    }

    #[test]
    fn test_message_kinds_capture_compaction_intent() {
        let topic_agents_md = AgentMessage::topic_agents_md("# Topic AGENTS");
        let task = AgentMessage::user_task("Investigate failure");
        let runtime = AgentMessage::runtime_context("User added a new constraint");
        let skill = AgentMessage::skill_context("[Loaded skill: deploy]\nUse checklist");
        let tool = AgentMessage::tool("call-1", "execute_command", "cargo check");
        let summary = AgentMessage::summary("[Previous context compressed]\n...");

        assert_eq!(
            topic_agents_md.resolved_kind(),
            AgentMessageKind::TopicAgentsMd
        );
        assert_eq!(topic_agents_md.retention(), CompactionRetention::Pinned);

        assert_eq!(task.resolved_kind(), AgentMessageKind::UserTask);
        assert_eq!(task.retention(), CompactionRetention::ProtectedLive);

        assert_eq!(runtime.resolved_kind(), AgentMessageKind::RuntimeContext);
        assert_eq!(runtime.retention(), CompactionRetention::ProtectedLive);

        assert_eq!(skill.resolved_kind(), AgentMessageKind::SkillContext);
        assert_eq!(skill.retention(), CompactionRetention::ProtectedLive);

        assert_eq!(tool.resolved_kind(), AgentMessageKind::ToolResult);
        assert_eq!(tool.retention(), CompactionRetention::PrunableArtifact);

        assert_eq!(summary.resolved_kind(), AgentMessageKind::Summary);
        assert_eq!(summary.retention(), CompactionRetention::Pinned);
    }

    #[test]
    fn test_legacy_messages_resolve_to_role_based_kinds() {
        let legacy_assistant = AgentMessage {
            kind: AgentMessageKind::Legacy,
            role: MessageRole::Assistant,
            content: "Done".to_string(),
            reasoning: None,
            tool_call_id: None,
            tool_name: None,
            tool_calls: None,
        };
        let legacy_tool = AgentMessage {
            kind: AgentMessageKind::Legacy,
            role: MessageRole::Tool,
            content: "stdout".to_string(),
            reasoning: None,
            tool_call_id: Some("call-1".to_string()),
            tool_name: Some("execute_command".to_string()),
            tool_calls: None,
        };

        assert_eq!(
            legacy_assistant.resolved_kind(),
            AgentMessageKind::AssistantResponse
        );
        assert_eq!(legacy_tool.resolved_kind(), AgentMessageKind::ToolResult);
    }

    #[test]
    fn test_memory_does_not_auto_compact() {
        let mut memory = AgentMemory::new(300);
        memory.add_message(AgentMessage::topic_agents_md(
            "# Topic AGENTS\nAlways respect deployment windows.",
        ));

        for idx in 0..12 {
            memory.add_message(AgentMessage::user(format!(
                "Message {idx}: {}",
                "x".repeat(80)
            )));
        }

        assert!(memory.has_topic_agents_md());
        assert_eq!(memory.get_messages().len(), 13);
        assert!(!memory
            .get_messages()
            .iter()
            .any(|message| message.content.starts_with("[Previous context compressed]")));
    }

    #[test]
    fn test_explicit_legacy_compaction_preserves_topic_agents_md_message() {
        let mut memory = AgentMemory::new(300);
        memory.add_message(AgentMessage::topic_agents_md(
            "# Topic AGENTS\nAlways respect deployment windows.",
        ));

        for idx in 0..12 {
            memory.add_message(AgentMessage::user(format!(
                "Message {idx}: {}",
                "x".repeat(80)
            )));
        }

        memory.apply_legacy_local_compaction();

        assert!(memory.has_topic_agents_md());
        assert!(memory
            .get_messages()
            .iter()
            .any(|message| message.content.starts_with(TOPIC_AGENTS_MD_SYSTEM_PREFIX)));
        assert!(memory
            .get_messages()
            .iter()
            .any(|message| message.content.starts_with("[Previous context compressed]")));
    }

    #[test]
    fn test_explicit_legacy_compaction_preserves_protected_live_entries() {
        let mut memory = AgentMemory::new(320);
        memory.add_message(AgentMessage::user_task("Ship Stage 2 compaction typing"));
        memory.add_message(AgentMessage::runtime_context(
            "The user clarified that tool schemas must stay untouched.",
        ));
        memory.add_message(AgentMessage::skill_context(
            "[Loaded skill: release]\nPrefer safe rollouts.",
        ));

        for idx in 0..12 {
            memory.add_message(AgentMessage::assistant(format!(
                "Response {idx}: {}",
                "y".repeat(70)
            )));
        }

        memory.apply_legacy_local_compaction();

        assert!(memory
            .get_messages()
            .iter()
            .any(|message| message.resolved_kind() == AgentMessageKind::UserTask));
        assert!(memory
            .get_messages()
            .iter()
            .any(|message| message.resolved_kind() == AgentMessageKind::RuntimeContext));
        assert!(memory
            .get_messages()
            .iter()
            .any(|message| message.resolved_kind() == AgentMessageKind::SkillContext));
    }
}
