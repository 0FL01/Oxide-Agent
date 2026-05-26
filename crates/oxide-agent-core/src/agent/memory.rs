//! Agent memory management for Agent Mode sessions.
//!
//! Provides conversation memory for the agent and lightweight token
//! accounting utilities. Compaction orchestration lives outside this module.

use crate::agent::compaction::{
    count_tokens_cached, AgentMessageKind, ArchiveRef, CompactedSummaryMetadata,
    CompactionRetention, OXIDE_COMPACTED_SUMMARY_PREFIX,
};
use crate::agent::providers::TodoList;
use crate::agent::recovery::{repair_agent_message_history_runtime, HistoryRepairOutcome};
use crate::llm::{TokenUsage, ToolCall, ToolCallCorrelation};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tracing::warn;

pub(crate) const TOPIC_AGENTS_MD_SYSTEM_PREFIX: &str = "[TOPIC_AGENTS_MD]\n";
/// Default pause length that makes a new Agent Mode user task a potential new topic.
pub const TEMPORAL_BOUNDARY_THRESHOLD_SECONDS: i64 = 2 * 60 * 60;

#[must_use]
fn current_timestamp_unix_secs() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs() as i64,
        Err(_) => 0,
    }
}

/// A message in the agent's conversation memory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    /// Semantic kind used by compaction policies.
    pub kind: AgentMessageKind,
    /// Role of the message sender
    pub role: MessageRole,
    /// Text content of the message
    pub content: String,
    /// Unix timestamp for when this Agent Mode memory entry was created.
    #[serde(default)]
    pub created_at_unix: Option<i64>,
    /// Optional reasoning/thinking content (for models that support it, e.g., GLM-4.7)
    /// This is counted towards token limits but not shown to user
    pub reasoning: Option<String>,
    /// Provider-facing tool call id echoed by chat-like providers.
    pub tool_call_id: Option<String>,
    /// Canonical correlation metadata for a tool result message.
    #[serde(default)]
    pub tool_call_correlation: Option<ToolCallCorrelation>,
    /// Tool name (for tool responses)
    pub tool_name: Option<String>,
    /// Tool calls made by assistant
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Canonical correlation metadata for assistant tool call batches.
    #[serde(default)]
    pub tool_call_correlations: Option<Vec<ToolCallCorrelation>>,
    /// Metadata for payloads that were externalized outside hot memory.
    #[serde(default)]
    pub externalized_payload: Option<ExternalizedPayload>,
    /// Metadata for tool payloads already pruned down to a placeholder.
    #[serde(default)]
    pub pruned_artifact: Option<PrunedArtifact>,
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

/// Outcome of an atomic compacted-history replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactedHistoryReplacementOutcome {
    /// Approximate token count before replacement.
    pub token_before: usize,
    /// Approximate token count after replacement.
    pub token_after: usize,
    /// Message count before replacement.
    pub history_items_before: usize,
    /// Message count after replacement.
    pub history_items_after: usize,
    /// Validation repair result. Normal replacement requires `applied == false`.
    pub repair_outcome: HistoryRepairOutcome,
}

/// Error that prevents compacted-history replacement before mutation.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CompactedHistoryReplacementError {
    /// Replacement would require runtime history repair, so the builder is unsafe.
    #[error("compacted history replacement failed tool-history validation")]
    InvalidToolHistory(HistoryRepairOutcome),
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
            created_at_unix: Some(current_timestamp_unix_secs()),
            reasoning: None,
            tool_call_id: None,
            tool_call_correlation: None,
            tool_name: None,
            tool_calls: None,
            tool_call_correlations: None,
            externalized_payload: None,
            pruned_artifact: None,
        }
    }

    /// Create a system-facing note for assistant text that was generated but not delivered.
    pub fn undelivered_assistant_draft(content: impl Into<String>) -> Self {
        Self {
            kind: AgentMessageKind::UndeliveredAssistantDraft,
            ..Self::system_context(content)
        }
    }

    /// Create a pinned system message carrying topic-scoped `AGENTS.md` content.
    pub fn topic_agents_md(content: impl AsRef<str>) -> Self {
        Self {
            kind: AgentMessageKind::TopicAgentsMd,
            role: MessageRole::System,
            content: format!("{TOPIC_AGENTS_MD_SYSTEM_PREFIX}{}", content.as_ref().trim()),
            created_at_unix: Some(current_timestamp_unix_secs()),
            reasoning: None,
            tool_call_id: None,
            tool_call_correlation: None,
            tool_name: None,
            tool_calls: None,
            tool_call_correlations: None,
            externalized_payload: None,
            pruned_artifact: None,
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
            created_at_unix: Some(current_timestamp_unix_secs()),
            reasoning: None,
            tool_call_id: None,
            tool_call_correlation: None,
            tool_name: None,
            tool_calls: None,
            tool_call_correlations: None,
            externalized_payload: None,
            pruned_artifact: None,
        }
    }

    /// Create a user runtime-context injection message.
    pub fn runtime_context(content: impl Into<String>) -> Self {
        Self {
            kind: AgentMessageKind::RuntimeContext,
            role: MessageRole::User,
            content: content.into(),
            created_at_unix: Some(current_timestamp_unix_secs()),
            reasoning: None,
            tool_call_id: None,
            tool_call_correlation: None,
            tool_name: None,
            tool_calls: None,
            tool_call_correlations: None,
            externalized_payload: None,
            pruned_artifact: None,
        }
    }

    /// Create a generic user turn.
    pub fn user_turn(content: impl Into<String>) -> Self {
        Self {
            kind: AgentMessageKind::UserTurn,
            role: MessageRole::User,
            content: content.into(),
            created_at_unix: Some(current_timestamp_unix_secs()),
            reasoning: None,
            tool_call_id: None,
            tool_call_correlation: None,
            tool_name: None,
            tool_calls: None,
            tool_call_correlations: None,
            externalized_payload: None,
            pruned_artifact: None,
        }
    }

    /// Create a new assistant message
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            kind: AgentMessageKind::AssistantResponse,
            role: MessageRole::Assistant,
            content: content.into(),
            created_at_unix: Some(current_timestamp_unix_secs()),
            reasoning: None,
            tool_call_id: None,
            tool_call_correlation: None,
            tool_name: None,
            tool_calls: None,
            tool_call_correlations: None,
            externalized_payload: None,
            pruned_artifact: None,
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
            created_at_unix: Some(current_timestamp_unix_secs()),
            reasoning: Some(reasoning.into()),
            tool_call_id: None,
            tool_call_correlation: None,
            tool_name: None,
            tool_calls: None,
            tool_call_correlations: None,
            externalized_payload: None,
            pruned_artifact: None,
        }
    }

    /// Create a new tool response message
    pub fn tool(tool_call_id: &str, name: &str, content: &str) -> Self {
        Self::tool_with_correlation(
            tool_call_id,
            ToolCallCorrelation::new(tool_call_id),
            name,
            content,
        )
    }

    /// Create a new tool response message with explicit canonical correlation metadata.
    pub fn tool_with_correlation(
        tool_call_id: &str,
        tool_call_correlation: ToolCallCorrelation,
        name: &str,
        content: &str,
    ) -> Self {
        Self {
            kind: AgentMessageKind::ToolResult,
            role: MessageRole::Tool,
            content: content.into(),
            created_at_unix: Some(current_timestamp_unix_secs()),
            reasoning: None,
            tool_call_id: Some(tool_call_id.to_string()),
            tool_call_correlation: Some(tool_call_correlation),
            tool_name: Some(name.to_string()),
            tool_calls: None,
            tool_call_correlations: None,
            externalized_payload: None,
            pruned_artifact: None,
        }
    }

    /// Create a tool result placeholder that points at an externalized artifact.
    pub fn externalized_tool(
        tool_call_id: &str,
        name: &str,
        content: impl Into<String>,
        externalized_payload: ExternalizedPayload,
    ) -> Self {
        Self::externalized_tool_with_correlation(
            tool_call_id,
            ToolCallCorrelation::new(tool_call_id),
            name,
            content,
            externalized_payload,
        )
    }

    /// Create a tool result placeholder with explicit canonical correlation metadata.
    pub fn externalized_tool_with_correlation(
        tool_call_id: &str,
        tool_call_correlation: ToolCallCorrelation,
        name: &str,
        content: impl Into<String>,
        externalized_payload: ExternalizedPayload,
    ) -> Self {
        Self {
            kind: AgentMessageKind::ToolResult,
            role: MessageRole::Tool,
            content: content.into(),
            created_at_unix: Some(current_timestamp_unix_secs()),
            reasoning: None,
            tool_call_id: Some(tool_call_id.to_string()),
            tool_call_correlation: Some(tool_call_correlation),
            tool_name: Some(name.to_string()),
            tool_calls: None,
            tool_call_correlations: None,
            externalized_payload: Some(externalized_payload),
            pruned_artifact: None,
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
        Self::pruned_tool_with_correlation(
            tool_call_id,
            ToolCallCorrelation::new(tool_call_id),
            name,
            content,
            pruned_artifact,
            externalized_payload,
        )
    }

    /// Create a pruned tool result placeholder with explicit canonical correlation metadata.
    pub fn pruned_tool_with_correlation(
        tool_call_id: &str,
        tool_call_correlation: ToolCallCorrelation,
        name: &str,
        content: impl Into<String>,
        pruned_artifact: PrunedArtifact,
        externalized_payload: Option<ExternalizedPayload>,
    ) -> Self {
        Self {
            kind: AgentMessageKind::ToolResult,
            role: MessageRole::Tool,
            content: content.into(),
            created_at_unix: Some(current_timestamp_unix_secs()),
            reasoning: None,
            tool_call_id: Some(tool_call_id.to_string()),
            tool_call_correlation: Some(tool_call_correlation),
            tool_name: Some(name.to_string()),
            tool_calls: None,
            tool_call_correlations: None,
            externalized_payload,
            pruned_artifact: Some(pruned_artifact),
        }
    }

    /// Create a new assistant message with tool calls
    pub fn assistant_with_tools(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self::assistant_with_tools_and_reasoning(content, None, tool_calls)
    }

    /// Create a new assistant message with optional reasoning and tool calls.
    pub fn assistant_with_tools_and_reasoning(
        content: impl Into<String>,
        reasoning: Option<String>,
        tool_calls: Vec<ToolCall>,
    ) -> Self {
        let tool_call_correlations = (!tool_calls.is_empty())
            .then(|| tool_calls.iter().map(ToolCall::correlation).collect());
        Self {
            kind: AgentMessageKind::AssistantToolCall,
            role: MessageRole::Assistant,
            content: content.into(),
            created_at_unix: Some(current_timestamp_unix_secs()),
            reasoning,
            tool_call_id: None,
            tool_call_correlation: None,
            tool_name: None,
            tool_calls: Some(tool_calls),
            tool_call_correlations,
            externalized_payload: None,
            pruned_artifact: None,
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

    /// Create a Codex-style runtime compacted summary message.
    pub fn compacted_summary(
        summary_text: impl AsRef<str>,
        metadata: &CompactedSummaryMetadata,
    ) -> Self {
        Self {
            kind: AgentMessageKind::Summary,
            role: MessageRole::System,
            content: format_compacted_summary(summary_text.as_ref(), metadata),
            created_at_unix: Some(current_timestamp_unix_secs()),
            reasoning: None,
            tool_call_id: None,
            tool_call_correlation: None,
            tool_name: None,
            tool_calls: None,
            tool_call_correlations: None,
            externalized_payload: None,
            pruned_artifact: None,
        }
    }

    /// Resolve the semantic kind for this message.
    #[must_use]
    pub fn resolved_kind(&self) -> AgentMessageKind {
        self.kind
    }

    /// Return a copy with an explicit creation timestamp.
    #[must_use]
    pub fn with_created_at_unix(mut self, created_at_unix: Option<i64>) -> Self {
        self.created_at_unix = created_at_unix;
        self
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

    /// Resolve the canonical correlation for a tool result message.
    #[must_use]
    pub fn resolved_tool_call_correlation(&self) -> Option<ToolCallCorrelation> {
        self.tool_call_correlation
            .clone()
            .or_else(|| self.tool_call_id.as_deref().map(ToolCallCorrelation::new))
    }

    /// Resolve canonical correlations for an assistant tool call batch.
    #[must_use]
    pub fn resolved_tool_call_correlations(&self) -> Option<Vec<ToolCallCorrelation>> {
        let tool_calls = self.tool_calls.as_ref()?;
        let derived: Vec<ToolCallCorrelation> =
            tool_calls.iter().map(ToolCall::correlation).collect();

        match &self.tool_call_correlations {
            Some(correlations) if correlations.len() == derived.len() => Some(correlations.clone()),
            _ => Some(derived),
        }
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
        self.messages.push(msg);
        self.recalculate_token_count();
        self.repair_history_after_mutation("add_message");
    }

    /// Build a short temporal boundary context before appending a new top-level user task.
    #[must_use]
    pub fn soft_temporal_boundary_before_user_task(
        &self,
        current_user_message: &AgentMessage,
    ) -> Option<String> {
        let gap_seconds = self.temporal_gap_before_user_task(current_user_message)?;
        (gap_seconds > TEMPORAL_BOUNDARY_THRESHOLD_SECONDS)
            .then(|| format_soft_temporal_boundary(gap_seconds))
    }

    /// Compute the gap between a new top-level user task and the previous top-level user task.
    #[must_use]
    pub fn temporal_gap_before_user_task(
        &self,
        current_user_message: &AgentMessage,
    ) -> Option<i64> {
        if current_user_message.resolved_kind() != AgentMessageKind::UserTask {
            return None;
        }
        let current_created_at = current_user_message.created_at_unix?;
        let previous_created_at = self
            .messages
            .iter()
            .rev()
            .find(|message| message.resolved_kind() == AgentMessageKind::UserTask)?
            .created_at_unix?;

        Some(current_created_at.saturating_sub(previous_created_at))
    }

    /// Returns true when memory already contains a pinned topic `AGENTS.md` message.
    #[must_use]
    pub fn has_topic_agents_md(&self) -> bool {
        self.messages.iter().any(AgentMessage::is_topic_agents_md)
    }

    /// Insert or replace the pinned topic `AGENTS.md` message while preserving order.
    pub fn upsert_topic_agents_md(&mut self, content: impl AsRef<str>) {
        let replacement = AgentMessage::topic_agents_md(content);
        let mut messages = self.messages.clone();

        if let Some(first_idx) = messages.iter().position(AgentMessage::is_topic_agents_md) {
            messages[first_idx] = replacement;
            let mut seen_first = false;
            messages.retain(|message| {
                if !message.is_topic_agents_md() {
                    return true;
                }

                if !seen_first {
                    seen_first = true;
                    return true;
                }

                false
            });
        } else {
            messages.insert(0, replacement);
        }

        self.replace_messages(messages);
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

    /// Update the configured maximum token count.
    pub fn set_max_tokens(&mut self, max_tokens: usize) {
        self.max_tokens = max_tokens;
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
            tracing::debug!(
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
        self.last_api_usage = None;
        self.recalculate_token_count();
        self.repair_history_after_mutation("replace_messages");
    }

    /// Atomically replace hot memory with prevalidated compacted history.
    ///
    /// Unlike `replace_messages`, this method validates the replacement before
    /// mutation and rejects histories that would need tool-call repair.
    pub fn replace_compacted_history(
        &mut self,
        messages: Vec<AgentMessage>,
    ) -> Result<CompactedHistoryReplacementOutcome, CompactedHistoryReplacementError> {
        let token_before = self.token_count;
        let history_items_before = self.messages.len();
        let (validated, repair_outcome) = repair_agent_message_history_runtime(&messages);
        if repair_outcome.applied || validated.len() != messages.len() {
            return Err(CompactedHistoryReplacementError::InvalidToolHistory(
                repair_outcome,
            ));
        }

        self.messages = messages;
        self.last_api_usage = None;
        self.recalculate_token_count();

        Ok(CompactedHistoryReplacementOutcome {
            token_before,
            token_after: self.token_count,
            history_items_before,
            history_items_after: self.messages.len(),
            repair_outcome,
        })
    }

    fn repair_history_after_mutation(&mut self, boundary: &'static str) {
        let (repaired_messages, outcome) = repair_agent_message_history_runtime(&self.messages);
        if !outcome.applied {
            return;
        }

        self.messages = repaired_messages;
        self.recalculate_token_count();
        self.last_api_usage = None;
        warn!(
            boundary,
            dropped_tool_results = outcome.dropped_tool_results,
            trimmed_tool_calls = outcome.trimmed_tool_calls,
            converted_tool_call_messages = outcome.converted_tool_call_messages,
            dropped_tool_call_messages = outcome.dropped_tool_call_messages,
            "Agent memory repaired invalid tool history after mutation"
        );
    }

    fn recalculate_token_count(&mut self) {
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
    }

    /// Count tokens in a string using cached cl100k tokenizer (GPT-4/Claude compatible)
    fn count_tokens(text: &str) -> usize {
        count_tokens_cached(text)
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

fn format_soft_temporal_boundary(gap_seconds: i64) -> String {
    format!(
        "[TEMPORAL_CONTEXT]\nThere was a long pause of about {} since the previous top-level user request in this Agent Mode flow. Treat the latest request as potentially a new topic unless the user clearly connects it to earlier context.",
        format_duration_for_prompt(gap_seconds)
    )
}

fn format_duration_for_prompt(gap_seconds: i64) -> String {
    let total_minutes = (gap_seconds.max(0) / 60).max(1);
    let days = total_minutes / (24 * 60);
    let hours = (total_minutes % (24 * 60)) / 60;
    let minutes = total_minutes % 60;

    if days > 0 {
        let mut parts = vec![format_duration_part(days, "day")];
        if hours > 0 {
            parts.push(format_duration_part(hours, "hour"));
        }
        return parts.join(" ");
    }

    if hours > 0 {
        let mut parts = vec![format_duration_part(hours, "hour")];
        if minutes > 0 {
            parts.push(format_duration_part(minutes, "minute"));
        }
        return parts.join(" ");
    }

    format_duration_part(minutes, "minute")
}

fn format_duration_part(value: i64, unit: &str) -> String {
    let suffix = if value == 1 { "" } else { "s" };
    format!("{value} {unit}{suffix}")
}

fn format_compacted_summary(summary_text: &str, metadata: &CompactedSummaryMetadata) -> String {
    let guidance = compacted_summary_guidance(metadata.wiki_memory_lookup_available);
    let wiki_metadata_line = if metadata.wiki_memory_lookup_available {
        "wiki_memory_lookup_available: true\n"
    } else {
        ""
    };
    format!(
        "{prefix}\n\
generation: {generation}\n\
reason: {reason:?}\n\
phase: {phase:?}\n\
provider: {provider}\n\
route: {route}\n\
backend: {backend}\n\
token_before: {token_before}\n\
token_after: {token_after}\n\
history_items_before: {history_items_before}\n\
history_items_after: {history_items_after}\n\
created_at: {created_at}\n\
previous_summary_detected: {previous_summary_detected}\n\
repair_applied: {repair_applied}\n\
{wiki_metadata_line}\n\
{guidance}\n\n\
{summary}",
        prefix = OXIDE_COMPACTED_SUMMARY_PREFIX,
        generation = metadata.generation,
        reason = metadata.reason,
        phase = metadata.phase,
        provider = metadata.provider,
        route = metadata.route,
        backend = metadata.backend.as_str(),
        token_before = metadata.token_before,
        token_after = metadata.token_after,
        history_items_before = metadata.history_items_before,
        history_items_after = metadata.history_items_after,
        created_at = metadata.created_at,
        previous_summary_detected = metadata.previous_summary_detected,
        repair_applied = metadata.repair_applied,
        wiki_metadata_line = wiki_metadata_line,
        guidance = guidance,
        summary = summary_text.trim(),
    )
}

fn compacted_summary_guidance(wiki_memory_lookup_available: bool) -> &'static str {
    if wiki_memory_lookup_available {
        "Agent Mode hot context compacted; continue from this handoff summary and remaining raw messages. If a missing durable fact, preference, decision, procedure, or project detail matters, use wiki_memory_search, wiki_memory_read, or wiki_memory_list before guessing. Wiki Memory is durable background, not a full transcript."
    } else {
        "Agent Mode hot context compacted; continue from this handoff summary and remaining raw messages."
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
    use serde_json::json;

    fn tool_call(id: &str, name: &str) -> ToolCall {
        ToolCall::new(
            id.to_string(),
            crate::llm::ToolCallFunction {
                name: name.to_string(),
                arguments: "{}".to_string(),
            },
            false,
        )
    }

    #[test]
    fn test_memory_add_message() {
        let mut memory = AgentMemory::new(100_000);
        memory.add_message(AgentMessage::user("Hello, agent!"));
        assert_eq!(memory.get_messages().len(), 1);
        assert!(memory.token_count() > 0);
    }

    #[test]
    fn new_agent_messages_include_creation_timestamp() {
        let message = AgentMessage::user_task("Ship temporal context");

        assert!(message.created_at_unix.is_some());
    }

    #[test]
    fn agent_message_deserializes_without_creation_timestamp() {
        let value = json!({
            "kind": "UserTask",
            "role": "User",
            "content": "old task",
            "reasoning": null,
            "tool_call_id": null,
            "tool_call_correlation": null,
            "tool_name": null,
            "tool_calls": null,
            "tool_call_correlations": null,
            "externalized_payload": null,
            "pruned_artifact": null
        });

        let message: AgentMessage = serde_json::from_value(value).expect("message deserializes");

        assert_eq!(message.created_at_unix, None);
    }

    #[test]
    fn temporal_gap_detects_long_pause_between_top_level_user_tasks() {
        let mut memory = AgentMemory::new(100_000);
        memory.add_message(AgentMessage::user_task("first").with_created_at_unix(Some(100)));
        let current = AgentMessage::user_task("second")
            .with_created_at_unix(Some(100 + TEMPORAL_BOUNDARY_THRESHOLD_SECONDS + 60));

        let boundary = memory
            .soft_temporal_boundary_before_user_task(&current)
            .expect("long pause should produce context");

        assert!(boundary.contains("[TEMPORAL_CONTEXT]"));
        assert!(boundary.contains("about 2 hours 1 minute"));
        assert!(!boundary.contains("7260"));
    }

    #[test]
    fn temporal_gap_ignores_short_pause() {
        let mut memory = AgentMemory::new(100_000);
        memory.add_message(AgentMessage::user_task("first").with_created_at_unix(Some(100)));
        let current = AgentMessage::user_task("second")
            .with_created_at_unix(Some(100 + TEMPORAL_BOUNDARY_THRESHOLD_SECONDS));

        assert!(memory
            .soft_temporal_boundary_before_user_task(&current)
            .is_none());
    }

    #[test]
    fn temporal_gap_skips_when_previous_user_task_has_no_timestamp() {
        let mut memory = AgentMemory::new(100_000);
        memory.add_message(AgentMessage::user_task("first").with_created_at_unix(None));
        let current = AgentMessage::user_task("second")
            .with_created_at_unix(Some(100 + TEMPORAL_BOUNDARY_THRESHOLD_SECONDS + 1));

        assert_eq!(memory.temporal_gap_before_user_task(&current), None);
    }

    #[test]
    fn temporal_gap_uses_previous_user_task_not_runtime_context() {
        let mut memory = AgentMemory::new(100_000);
        memory.add_message(AgentMessage::user_task("first").with_created_at_unix(Some(100)));
        memory.add_message(
            AgentMessage::runtime_context("mid-run note").with_created_at_unix(Some(200)),
        );
        let current = AgentMessage::user_task("second").with_created_at_unix(Some(7_401));

        assert_eq!(memory.temporal_gap_before_user_task(&current), Some(7_301));
    }

    #[test]
    fn test_memory_runtime_repair_drops_orphaned_tool_results() {
        let mut memory = AgentMemory::new(100_000);

        memory.add_message(AgentMessage::user("Hello"));
        memory.add_message(AgentMessage::tool("call-orphan", "search", "result"));

        assert_eq!(memory.get_messages().len(), 1);
        assert_eq!(memory.get_messages()[0].content, "Hello");
    }

    #[test]
    fn test_memory_runtime_repair_preserves_open_terminal_tool_batch() {
        let mut memory = AgentMemory::new(100_000);

        memory.add_message(AgentMessage::assistant_with_tools(
            "Calling tools",
            vec![
                tool_call("call-1", "search"),
                tool_call("call-2", "read_file"),
            ],
        ));
        memory.add_message(AgentMessage::tool("call-1", "search", "result-1"));

        assert_eq!(memory.get_messages().len(), 2);
        let tool_calls = memory.get_messages()[0]
            .tool_calls
            .as_ref()
            .expect("tool batch should be preserved");
        assert_eq!(tool_calls.len(), 2);
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
    fn replace_compacted_history_replaces_valid_history_atomically() {
        let mut memory = AgentMemory::new(100_000);
        memory.add_message(AgentMessage::user("Before"));
        let token_before = memory.token_count();

        let outcome = memory
            .replace_compacted_history(vec![
                AgentMessage::summary("[OXIDE_COMPACTED_SUMMARY_V1]\nsummary"),
                AgentMessage::user("After"),
            ])
            .expect("valid compacted history replaces memory");

        assert_eq!(outcome.token_before, token_before);
        assert_eq!(outcome.history_items_before, 1);
        assert_eq!(outcome.history_items_after, 2);
        assert!(!outcome.repair_outcome.applied);
        assert_eq!(memory.get_messages()[1].content, "After");
        assert!(memory.token_count() > 0);
    }

    #[test]
    fn replace_compacted_history_rejects_invalid_history_without_mutation() {
        let mut memory = AgentMemory::new(100_000);
        memory.add_message(AgentMessage::user("Before"));
        let token_before = memory.token_count();

        let err = memory
            .replace_compacted_history(vec![AgentMessage::tool(
                "call-orphan",
                "search",
                "orphan result",
            )])
            .expect_err("orphan tool result is rejected");

        assert!(matches!(
            err,
            CompactedHistoryReplacementError::InvalidToolHistory(_)
        ));
        assert_eq!(memory.get_messages().len(), 1);
        assert_eq!(memory.get_messages()[0].content, "Before");
        assert_eq!(memory.token_count(), token_before);
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
    fn upsert_topic_agents_md_replaces_existing_pinned_message() {
        let mut memory = AgentMemory::new(100_000);
        memory.add_message(AgentMessage::topic_agents_md(
            "# Topic AGENTS\nOld guidance.",
        ));
        memory.add_message(AgentMessage::user("Keep going"));

        memory.upsert_topic_agents_md("# Topic AGENTS\nNew guidance.");

        let pinned: Vec<_> = memory
            .get_messages()
            .iter()
            .filter(|message| message.is_topic_agents_md())
            .collect();
        assert_eq!(pinned.len(), 1);
        assert!(pinned[0].content.contains("New guidance."));
        assert_eq!(memory.get_messages()[1].content, "Keep going");
    }

    #[test]
    fn test_message_kinds_capture_compaction_intent() {
        let topic_agents_md = AgentMessage::topic_agents_md("# Topic AGENTS");
        let task = AgentMessage::user_task("Investigate failure");
        let runtime = AgentMessage::runtime_context("User added a new constraint");
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

        assert_eq!(tool.resolved_kind(), AgentMessageKind::ToolResult);
        assert_eq!(tool.retention(), CompactionRetention::PrunableArtifact);

        assert_eq!(summary.resolved_kind(), AgentMessageKind::Summary);
        assert_eq!(summary.retention(), CompactionRetention::Pinned);
    }

    fn compacted_summary_metadata(wiki_memory_lookup_available: bool) -> CompactedSummaryMetadata {
        CompactedSummaryMetadata {
            generation: 1,
            reason: crate::agent::compaction::CompactionReason::Manual,
            phase: crate::agent::compaction::CompactionPhase::Manual,
            token_before: 100,
            token_after: 10,
            history_items_before: 3,
            history_items_after: 1,
            provider: "mock".to_string(),
            route: "mock-model".to_string(),
            backend: crate::agent::compaction::CompactionBackend::LocalLlmSummary,
            created_at: "2026-05-21T20:10:00+03:00".to_string(),
            previous_summary_detected: false,
            repair_applied: false,
            wiki_memory_lookup_available,
        }
    }

    #[test]
    fn compacted_summary_guidance_mentions_wiki_when_lookup_tools_available() {
        let summary = AgentMessage::compacted_summary(
            "Current state and next steps.",
            &compacted_summary_metadata(true),
        );

        assert!(summary.content.contains("Agent Mode hot context compacted"));
        assert!(summary
            .content
            .contains("wiki_memory_lookup_available: true"));
        assert!(summary.content.contains("wiki_memory_search"));
        assert!(summary
            .content
            .contains("Wiki Memory is durable background, not a full transcript."));
    }

    #[test]
    fn compacted_summary_fallback_guidance_does_not_mention_wiki() {
        let summary = AgentMessage::compacted_summary(
            "Current state and next steps.",
            &compacted_summary_metadata(false),
        );

        assert!(summary.content.contains(
            "Agent Mode hot context compacted; continue from this handoff summary and remaining raw messages."
        ));
        assert!(!summary.content.to_ascii_lowercase().contains("wiki"));
    }

    #[test]
    fn test_tool_message_serialization_includes_canonical_correlation_fields() {
        let message = AgentMessage::tool("call-1", "execute_command", "stdout");
        let value = serde_json::to_value(&message).expect("message serializes");

        assert_eq!(value["tool_call_id"], json!("call-1"));
        assert_eq!(
            value["tool_call_correlation"]["invocation_id"],
            json!("call-1")
        );
    }

    #[test]
    fn test_tool_message_resolves_correlation_from_tool_call_id() {
        let value = json!({
            "kind": "ToolResult",
            "role": "Tool",
            "content": "stdout",
            "reasoning": null,
            "tool_call_id": "call-wire",
            "tool_name": "execute_command",
            "tool_calls": null,
            "externalized_payload": null,
            "pruned_artifact": null
        });
        let message: AgentMessage = serde_json::from_value(value).expect("message deserializes");

        assert_eq!(message.tool_call_correlation, None);
        assert_eq!(
            message.resolved_tool_call_correlation(),
            Some(ToolCallCorrelation::new("call-wire"))
        );
    }

    #[test]
    fn test_assistant_tool_batch_serialization_includes_correlation_vector() {
        let message = AgentMessage::assistant_with_tools(
            "Calling tools",
            vec![tool_call("call-1", "search")],
        );
        let value = serde_json::to_value(&message).expect("message serializes");

        assert_eq!(value["tool_calls"][0]["id"], json!("call-1"));
        assert_eq!(
            value["tool_call_correlations"][0]["invocation_id"],
            json!("call-1")
        );
    }

    #[test]
    fn test_assistant_tool_batch_preserves_reasoning() {
        let message = AgentMessage::assistant_with_tools_and_reasoning(
            "Calling tools",
            Some("thinking trace".to_string()),
            vec![tool_call("call-1", "search")],
        );
        let value = serde_json::to_value(&message).expect("message serializes");

        assert_eq!(value["reasoning"], json!("thinking trace"));
        assert_eq!(value["tool_calls"][0]["id"], json!("call-1"));
    }

    #[test]
    fn test_assistant_tool_batch_resolves_correlations_from_tool_call_ids() {
        let value = json!({
            "kind": "AssistantToolCall",
            "role": "Assistant",
            "content": "Calling tools",
            "reasoning": null,
            "tool_call_id": null,
            "tool_name": null,
            "tool_calls": [{
                "id": "call-wire",
                "function": {
                    "name": "search",
                    "arguments": "{}"
                },
                "is_recovered": false
            }],
            "externalized_payload": null,
            "pruned_artifact": null
        });
        let message: AgentMessage = serde_json::from_value(value).expect("message deserializes");

        assert_eq!(message.tool_call_correlations, None);
        assert_eq!(
            message.resolved_tool_call_correlations(),
            Some(vec![ToolCallCorrelation::new("call-wire")])
        );
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
