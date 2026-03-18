//! Agent memory management for Agent Mode sessions.
//!
//! Provides conversation memory for the agent and lightweight token
//! accounting utilities. Compaction orchestration lives outside this module.

use crate::agent::compaction::{
    AgentMessageKind, ArchiveRef, CompactionRetention, CompactionSummary,
};
use crate::agent::providers::TodoList;
use crate::llm::{TokenUsage, ToolCall};
use serde::{Deserialize, Serialize};
use tiktoken_rs::cl100k_base;

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
    /// Metadata for payloads that were externalized outside hot memory.
    #[serde(default)]
    pub externalized_payload: Option<ExternalizedPayload>,
    /// Metadata for tool payloads already pruned down to a placeholder.
    #[serde(default)]
    pub pruned_artifact: Option<PrunedArtifact>,
    /// Structured summary metadata for compaction-generated summary entries.
    #[serde(default)]
    pub structured_summary: Option<CompactionSummary>,
    /// Lightweight archive ref for displaced context chunks.
    #[serde(default)]
    pub archive_ref: Option<ArchiveRef>,
}

/// Metadata describing a tool payload externalized out of hot memory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalizedPayload {
    /// Reference to the persisted artifact.
    pub archive_ref: ArchiveRef,
    /// Approximate original token count before replacement.
    pub estimated_tokens: usize,
    /// Original visible character count before replacement.
    pub original_chars: usize,
    /// Inline preview retained in hot memory.
    pub preview: String,
    /// Hidden fallback payload retained when no external sink is configured.
    #[serde(default)]
    pub inline_fallback: Option<String>,
}

/// Metadata describing a tool payload that was pruned down to a placeholder.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrunedArtifact {
    /// Approximate original token count before replacement.
    pub estimated_tokens: usize,
    /// Original visible character count before replacement.
    pub original_chars: usize,
    /// Inline preview retained in the pruned placeholder.
    pub preview: String,
    /// Optional archive reference when the payload was also externalized.
    #[serde(default)]
    pub archive_ref: Option<ArchiveRef>,
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
            externalized_payload: None,
            pruned_artifact: None,
            structured_summary: None,
            archive_ref: None,
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
            externalized_payload: None,
            pruned_artifact: None,
            structured_summary: None,
            archive_ref: None,
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
            externalized_payload: None,
            pruned_artifact: None,
            structured_summary: None,
            archive_ref: None,
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
            externalized_payload: None,
            pruned_artifact: None,
            structured_summary: None,
            archive_ref: None,
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
            externalized_payload: None,
            pruned_artifact: None,
            structured_summary: None,
            archive_ref: None,
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
            externalized_payload: None,
            pruned_artifact: None,
            structured_summary: None,
            archive_ref: None,
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
            externalized_payload: None,
            pruned_artifact: None,
            structured_summary: None,
            archive_ref: None,
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
            externalized_payload: None,
            pruned_artifact: None,
            structured_summary: None,
            archive_ref: None,
        }
    }

    /// Create a tool result placeholder that points at an externalized artifact.
    pub fn externalized_tool(
        tool_call_id: &str,
        name: &str,
        content: impl Into<String>,
        externalized_payload: ExternalizedPayload,
    ) -> Self {
        Self {
            kind: AgentMessageKind::ToolResult,
            role: MessageRole::Tool,
            content: content.into(),
            reasoning: None,
            tool_call_id: Some(tool_call_id.to_string()),
            tool_name: Some(name.to_string()),
            tool_calls: None,
            externalized_payload: Some(externalized_payload),
            pruned_artifact: None,
            structured_summary: None,
            archive_ref: None,
        }
    }

    /// Create a pruned tool result placeholder.
    pub fn pruned_tool(
        tool_call_id: &str,
        name: &str,
        content: impl Into<String>,
        pruned_artifact: PrunedArtifact,
        externalized_payload: Option<ExternalizedPayload>,
    ) -> Self {
        Self {
            kind: AgentMessageKind::ToolResult,
            role: MessageRole::Tool,
            content: content.into(),
            reasoning: None,
            tool_call_id: Some(tool_call_id.to_string()),
            tool_name: Some(name.to_string()),
            tool_calls: None,
            externalized_payload,
            pruned_artifact: Some(pruned_artifact),
            structured_summary: None,
            archive_ref: None,
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
            externalized_payload: None,
            pruned_artifact: None,
            structured_summary: None,
            archive_ref: None,
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

    /// Create a summary entry backed by structured compaction data.
    pub fn from_compaction_summary(summary: CompactionSummary) -> Self {
        Self {
            kind: AgentMessageKind::Summary,
            role: MessageRole::System,
            content: format_compaction_summary(&summary),
            reasoning: None,
            tool_call_id: None,
            tool_name: None,
            tool_calls: None,
            externalized_payload: None,
            pruned_artifact: None,
            structured_summary: Some(summary),
            archive_ref: None,
        }
    }

    /// Create a lightweight reference to archived context.
    pub fn archive_reference(content: impl Into<String>) -> Self {
        Self {
            kind: AgentMessageKind::ArchiveReference,
            ..Self::archive_reference_with_ref(content, None)
        }
    }

    /// Create a lightweight archive reference entry backed by structured metadata.
    pub fn archive_reference_with_ref(
        content: impl Into<String>,
        archive_ref: Option<ArchiveRef>,
    ) -> Self {
        Self {
            kind: AgentMessageKind::ArchiveReference,
            role: MessageRole::System,
            content: content.into(),
            reasoning: None,
            tool_call_id: None,
            tool_name: None,
            tool_calls: None,
            externalized_payload: None,
            pruned_artifact: None,
            structured_summary: None,
            archive_ref,
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

    /// Returns true when the large payload was externalized out of hot memory.
    #[must_use]
    pub fn is_externalized(&self) -> bool {
        self.externalized_payload.is_some()
    }

    /// Returns true when the tool payload has already been pruned.
    #[must_use]
    pub fn is_pruned(&self) -> bool {
        self.pruned_artifact.is_some()
    }

    /// Returns structured compaction summary metadata when available.
    #[must_use]
    pub fn summary_payload(&self) -> Option<&CompactionSummary> {
        self.structured_summary.as_ref()
    }

    /// Returns structured archive ref metadata when available.
    #[must_use]
    pub fn archive_ref_payload(&self) -> Option<&ArchiveRef> {
        self.archive_ref.as_ref()
    }
}

/// Agent memory for the active hot context window
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMemory {
    messages: Vec<AgentMessage>,
    /// Task list for the agent
    pub todos: TodoList,
    /// Estimated tokens currently represented by hot memory messages.
    token_count: usize,
    max_tokens: usize,
    /// Last request-scoped token usage reported by the LLM API.
    #[serde(default)]
    last_api_usage: Option<TokenUsage>,
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
            last_api_usage: None,
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

    /// Get the estimated hot-memory token count.
    #[must_use]
    pub const fn token_count(&self) -> usize {
        self.token_count
    }

    /// Get the configured maximum token count.
    #[must_use]
    pub const fn max_tokens(&self) -> usize {
        self.max_tokens
    }

    /// Get the last request-scoped token count reported by the API.
    #[must_use]
    pub fn api_token_count(&self) -> Option<usize> {
        self.last_api_usage
            .as_ref()
            .map(|usage| usage.total_tokens as usize)
    }

    /// Get the last request-scoped token usage reported by the API.
    #[must_use]
    pub const fn api_usage(&self) -> Option<&TokenUsage> {
        self.last_api_usage.as_ref()
    }

    /// Record request-scoped token usage from the API for diagnostics.
    ///
    /// This does NOT overwrite the hot-memory estimate because provider usage
    /// counts represent a single rendered request, not the current memory size.
    pub fn sync_api_usage(&mut self, usage: TokenUsage) {
        let real_total_tokens = usage.total_tokens as usize;
        let diff = real_total_tokens as i64 - self.token_count as i64;

        tracing::info!(
            prompt_tokens = usage.prompt_tokens,
            completion_tokens = usage.completion_tokens,
            total_tokens = usage.total_tokens,
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
        self.last_api_usage = Some(usage);
    }

    /// Clear all messages from memory
    pub fn clear(&mut self) {
        self.messages.clear();
        self.todos.clear();
        self.token_count = 0;
        self.last_api_usage = None;
    }

    /// Replace hot memory messages and recalculate token accounting.
    pub fn replace_messages(&mut self, messages: Vec<AgentMessage>) {
        self.messages = messages;
        self.token_count = self
            .messages
            .iter()
            .map(|m| {
                let mut tokens = Self::count_tokens(&m.content);
                if let Some(ref reasoning) = m.reasoning {
                    tokens += Self::count_tokens(reasoning);
                }
                tokens
            })
            .sum();
        self.last_api_usage = None;
    }

    /// Count tokens in a string using cl100k tokenizer (GPT-4/Claude compatible)
    fn count_tokens(text: &str) -> usize {
        cl100k_base().map_or(text.len() / 4, |bpe| {
            bpe.encode_with_special_tokens(text).len()
        })
    }

    /// Get percentage of memory used based on the hot-memory estimate.
    #[must_use]
    pub fn usage_percent(&self) -> u8 {
        if self.max_tokens == 0 {
            return 100;
        }
        let percent = (self.token_count * 100) / self.max_tokens;
        u8::try_from(percent.min(100)).unwrap_or(100)
    }
}

fn format_compaction_summary(summary: &CompactionSummary) -> String {
    let mut sections = vec!["[COMPACTION_SUMMARY]".to_string()];

    if !summary.goal.trim().is_empty() {
        sections.push(format!("Goal:\n{}", summary.goal.trim()));
    }
    push_summary_list(&mut sections, "Constraints", &summary.constraints);
    push_summary_list(&mut sections, "Decisions", &summary.decisions);
    push_summary_list(&mut sections, "Discoveries", &summary.discoveries);
    push_summary_list(
        &mut sections,
        "Relevant Files And Entities",
        &summary.relevant_files_entities,
    );
    push_summary_list(&mut sections, "Remaining Work", &summary.remaining_work);
    push_summary_list(&mut sections, "Risks", &summary.risks);

    sections.join("\n\n")
}

fn push_summary_list(sections: &mut Vec<String>, title: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }

    sections.push(format!("{title}:\n- {}", items.join("\n- ")));
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
        let estimated_before_sync = memory.token_count();

        // Initial state
        assert_eq!(memory.api_token_count(), None);

        // Sync request-scoped API usage without overwriting the hot-memory estimate.
        memory.sync_api_usage(TokenUsage {
            prompt_tokens: 1000,
            completion_tokens: 234,
            total_tokens: 1234,
        });
        assert_eq!(memory.api_token_count(), Some(1234));
        assert_eq!(memory.token_count(), estimated_before_sync);

        // Add more messages (memory estimate increases, last API request stays same)
        memory.add_message(AgentMessage::user("More text"));
        assert!(memory.token_count() > estimated_before_sync);
        assert_eq!(memory.api_token_count(), Some(1234));

        // Clear
        memory.clear();
        assert_eq!(memory.api_token_count(), None);
    }

    #[test]
    fn test_replace_messages_recalculates_memory_tokens_and_clears_last_api_usage() {
        let mut memory = AgentMemory::new(100_000);
        memory.add_message(AgentMessage::user("Hello"));
        memory.sync_api_usage(TokenUsage {
            prompt_tokens: 1024,
            completion_tokens: 1024,
            total_tokens: 2048,
        });

        memory.replace_messages(vec![
            AgentMessage::user_task("Ship stage 3"),
            AgentMessage::assistant("Recent response"),
        ]);

        assert!(memory.token_count() > 0);
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
    fn test_structured_summary_message_keeps_payload() {
        let summary = CompactionSummary {
            goal: "Ship stage 8".to_string(),
            decisions: vec!["Use a first-class summary entry.".to_string()],
            ..CompactionSummary::default()
        };

        let message = AgentMessage::from_compaction_summary(summary.clone());

        assert_eq!(message.resolved_kind(), AgentMessageKind::Summary);
        assert_eq!(message.summary_payload(), Some(&summary));
        assert!(message.content.contains("[COMPACTION_SUMMARY]"));
        assert!(message.content.contains("Goal:"));
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
            externalized_payload: None,
            pruned_artifact: None,
            structured_summary: None,
            archive_ref: None,
        };
        let legacy_tool = AgentMessage {
            kind: AgentMessageKind::Legacy,
            role: MessageRole::Tool,
            content: "stdout".to_string(),
            reasoning: None,
            tool_call_id: Some("call-1".to_string()),
            tool_name: Some("execute_command".to_string()),
            tool_calls: None,
            externalized_payload: None,
            pruned_artifact: None,
            structured_summary: None,
            archive_ref: None,
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
}
