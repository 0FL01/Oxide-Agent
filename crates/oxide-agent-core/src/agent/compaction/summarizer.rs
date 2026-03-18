//! Sidecar summarizer for compactable Agent Mode history.

use super::prompt::{build_compaction_user_message, compaction_system_prompt};
use super::types::{
    BudgetState, CompactionRequest, CompactionSnapshot, CompactionSummary, SummaryGenerationOutcome,
};
use crate::agent::memory::{AgentMessage, MessageRole};
use crate::llm::{LlmClient, LlmError, Message};
use lazy_regex::lazy_regex;
use std::sync::Arc;
use tokio::time::{timeout, Duration};
use tracing::{debug, warn};

/// Configuration for the dedicated compaction summary model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionSummarizerConfig {
    /// Model name registered in `LlmClient`.
    pub model_name: String,
    /// Provider name for availability checks.
    pub provider_name: String,
    /// Request timeout for the sidecar summary call.
    pub timeout_secs: u64,
}

/// Generates structured summaries for compactable history slices.
#[derive(Clone)]
pub struct CompactionSummarizer {
    llm_client: Arc<LlmClient>,
    config: CompactionSummarizerConfig,
}

impl CompactionSummarizer {
    /// Create a new summarizer with the dedicated compaction model config.
    #[must_use]
    pub fn new(llm_client: Arc<LlmClient>, config: CompactionSummarizerConfig) -> Self {
        Self { llm_client, config }
    }

    /// Generate a structured summary when the current checkpoint needs compaction.
    pub async fn summarize_if_needed(
        &self,
        request: &CompactionRequest<'_>,
        budget_state: BudgetState,
        snapshot: &CompactionSnapshot,
        messages: &[AgentMessage],
    ) -> SummaryGenerationOutcome {
        if !should_attempt_summary(request, budget_state, snapshot) {
            return SummaryGenerationOutcome::default();
        }

        let user_message = build_compaction_user_message(snapshot, messages);
        let fallback = deterministic_fallback_summary(request, snapshot, messages);

        if self.config.model_name.is_empty()
            || self.config.provider_name.is_empty()
            || !self
                .llm_client
                .is_provider_available(&self.config.provider_name)
        {
            return SummaryGenerationOutcome {
                attempted: true,
                used_fallback: true,
                model_name: None,
                summary: Some(fallback),
            };
        }

        debug!(
            model = %self.config.model_name,
            provider = %self.config.provider_name,
            compactable_entries = snapshot.compactable_history.message_count,
            "Generating compaction summary"
        );

        match self.call_llm(&user_message).await {
            Ok(response) => match parse_summary_response(&response) {
                Some(summary) => SummaryGenerationOutcome {
                    attempted: true,
                    used_fallback: false,
                    model_name: Some(self.config.model_name.clone()),
                    summary: Some(summary),
                },
                None => SummaryGenerationOutcome {
                    attempted: true,
                    used_fallback: true,
                    model_name: Some(self.config.model_name.clone()),
                    summary: Some(fallback),
                },
            },
            Err(error) => {
                warn!(error = %error, "Compaction LLM call failed, using fallback summary");
                SummaryGenerationOutcome {
                    attempted: true,
                    used_fallback: true,
                    model_name: Some(self.config.model_name.clone()),
                    summary: Some(fallback),
                }
            }
        }
    }

    async fn call_llm(&self, user_message: &str) -> Result<String, LlmError> {
        let system_prompt = compaction_system_prompt();
        let messages = [Message::user(user_message)];
        let llm_call =
            self.llm_client
                .chat_completion(system_prompt, &messages, "", &self.config.model_name);

        timeout(Duration::from_secs(self.config.timeout_secs), llm_call)
            .await
            .map_err(|_| {
                LlmError::Unknown(format!(
                    "Compaction summary model timed out after {}s",
                    self.config.timeout_secs
                ))
            })?
    }
}

fn should_attempt_summary(
    request: &CompactionRequest<'_>,
    budget_state: BudgetState,
    snapshot: &CompactionSnapshot,
) -> bool {
    if !snapshot.entries.iter().any(|entry| {
        entry.retention == super::CompactionRetention::CompactableHistory
            && !entry.preserve_in_raw_window
    }) {
        return false;
    }

    matches!(request.trigger, super::CompactionTrigger::Manual)
        || matches!(
            budget_state,
            BudgetState::ShouldCompact | BudgetState::OverLimit
        )
}

fn parse_summary_response(response: &str) -> Option<CompactionSummary> {
    let json = extract_json(response);
    match serde_json::from_str::<CompactionSummary>(json) {
        Ok(summary) => normalize_summary(summary),
        Err(error) => {
            warn!(error = %error, response = %response, "Failed to parse compaction summary JSON");
            None
        }
    }
}

fn normalize_summary(summary: CompactionSummary) -> Option<CompactionSummary> {
    let normalized = CompactionSummary {
        goal: normalize_scalar(summary.goal, 240),
        constraints: normalize_items(summary.constraints, 8, 240),
        decisions: normalize_items(summary.decisions, 8, 240),
        discoveries: normalize_items(summary.discoveries, 8, 240),
        relevant_files_entities: normalize_items(summary.relevant_files_entities, 10, 240),
        remaining_work: normalize_items(summary.remaining_work, 8, 240),
        risks: normalize_items(summary.risks, 8, 240),
    };

    summary_has_signal(&normalized).then_some(normalized)
}

fn summary_has_signal(summary: &CompactionSummary) -> bool {
    !summary.goal.trim().is_empty()
        || !summary.constraints.is_empty()
        || !summary.decisions.is_empty()
        || !summary.discoveries.is_empty()
        || !summary.relevant_files_entities.is_empty()
        || !summary.remaining_work.is_empty()
        || !summary.risks.is_empty()
}

fn extract_json(response: &str) -> &str {
    let trimmed = response.trim();
    if let Some(start) = trimmed.find('{') {
        if let Some(end) = trimmed.rfind('}') {
            return &trimmed[start..=end];
        }
    }
    trimmed
}

fn deterministic_fallback_summary(
    request: &CompactionRequest<'_>,
    snapshot: &CompactionSnapshot,
    messages: &[AgentMessage],
) -> CompactionSummary {
    let compactable_messages: Vec<&AgentMessage> = snapshot
        .entries
        .iter()
        .filter(|entry| entry.retention == super::CompactionRetention::CompactableHistory)
        .filter(|entry| !entry.preserve_in_raw_window)
        .filter_map(|entry| messages.get(entry.index))
        .collect();

    let goal = compactable_messages
        .iter()
        .rev()
        .find(|message| message.role == MessageRole::User)
        .map_or_else(
            || request.task.trim().to_string(),
            |message| first_sentence(&message.content),
        );

    CompactionSummary {
        goal,
        constraints: dedupe_limit(
            extract_lines(
                &compactable_messages,
                &["must", "never", "do not", "should", "need to"],
                false,
            ),
            6,
        ),
        decisions: dedupe_limit(
            extract_lines(
                &compactable_messages,
                &[
                    "decided",
                    "implemented",
                    "using",
                    "will use",
                    "chose",
                    "updated",
                    "added",
                ],
                true,
            ),
            6,
        ),
        discoveries: dedupe_limit(
            extract_lines(
                &compactable_messages,
                &[
                    "found",
                    "discovered",
                    "observed",
                    "error",
                    "warning",
                    "failed",
                    "returned",
                ],
                true,
            ),
            6,
        ),
        relevant_files_entities: dedupe_limit(extract_entities(&compactable_messages), 8),
        remaining_work: vec![
            "Continue from the preserved live context and recent raw window.".to_string(),
        ],
        risks: dedupe_limit(
            extract_lines(
                &compactable_messages,
                &["risk", "warning", "error", "timeout", "approval", "secret"],
                true,
            ),
            5,
        ),
    }
}

fn extract_lines(
    messages: &[&AgentMessage],
    keywords: &[&str],
    allow_assistant: bool,
) -> Vec<String> {
    messages
        .iter()
        .filter(|message| allow_assistant || message.role == MessageRole::User)
        .flat_map(|message| message.content.lines())
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            keywords.iter().any(|keyword| lower.contains(keyword))
        })
        .map(first_sentence)
        .collect()
}

fn extract_entities(messages: &[&AgentMessage]) -> Vec<String> {
    static FILE_RE: lazy_regex::Lazy<regex::Regex> =
        lazy_regex!(r"[A-Za-z0-9_./-]+\.[A-Za-z0-9_./-]+");
    static TICK_RE: lazy_regex::Lazy<regex::Regex> = lazy_regex!(r"`([^`]+)`");

    let mut entities = Vec::new();
    for message in messages {
        for capture in FILE_RE.find_iter(&message.content) {
            entities.push(capture.as_str().to_string());
        }
        for capture in TICK_RE.captures_iter(&message.content) {
            if let Some(entity) = capture.get(1) {
                entities.push(entity.as_str().to_string());
            }
        }
    }
    entities
}

fn dedupe_limit(items: Vec<String>, limit: usize) -> Vec<String> {
    let mut deduped = Vec::new();
    for item in items {
        let trimmed = item.trim();
        if trimmed.is_empty() || deduped.iter().any(|existing: &String| existing == trimmed) {
            continue;
        }
        deduped.push(trimmed.to_string());
        if deduped.len() == limit {
            break;
        }
    }
    deduped
}

fn normalize_items(items: Vec<String>, limit: usize, max_chars: usize) -> Vec<String> {
    dedupe_limit(
        items
            .into_iter()
            .map(|item| normalize_scalar(item, max_chars))
            .collect(),
        limit,
    )
}

fn normalize_scalar(value: String, max_chars: usize) -> String {
    value.trim().chars().take(max_chars).collect()
}

fn first_sentence(text: &str) -> String {
    let trimmed = text.trim();
    let mut boundary = trimmed.len();
    let mut chars = trimmed.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if ch == '\n' {
            boundary = index;
            break;
        }
        if matches!(ch, '.' | '!' | '?')
            && chars.peek().is_none_or(|(_, next)| next.is_whitespace())
        {
            boundary = index;
            break;
        }
    }

    let sentence = trimmed[..boundary].trim();
    sentence.chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::{parse_summary_response, CompactionSummarizer, CompactionSummarizerConfig};
    use crate::agent::compaction::{
        classify_hot_memory, BudgetState, CompactionRequest, CompactionTrigger,
    };
    use crate::agent::memory::AgentMessage;
    use crate::llm::LlmClient;
    use crate::testing::mock_llm_simple;
    use std::sync::Arc;

    #[test]
    fn parse_summary_response_accepts_json() {
        let parsed = parse_summary_response(
            r#"{"goal":"Ship stage 7","constraints":["Keep schemas raw"],"decisions":["Use sidecar model"],"discoveries":[],"relevant_files_entities":["src/agent/compaction"],"remaining_work":["Rebuild hot context"],"risks":["Fallback may lose nuance"]}"#,
        );

        assert!(parsed.is_some());
        assert_eq!(parsed.expect("summary").goal, "Ship stage 7");
    }

    #[test]
    fn parse_summary_response_normalizes_duplicate_and_whitespace_entries() {
        let parsed = parse_summary_response(
            r#"{
                "goal":"  Ship stage 12 safely  ",
                "constraints":[" Keep AGENTS.md pinned ","Keep AGENTS.md pinned",""],
                "decisions":["Use fallback on invalid JSON","Use fallback on invalid JSON"],
                "discoveries":[],
                "relevant_files_entities":["  crates/oxide-agent-core/src/agent/compaction/service.rs  ","crates/oxide-agent-core/src/agent/compaction/service.rs"],
                "remaining_work":[" Add hardening tests "],
                "risks":[" Timeout risk ","Timeout risk"]
            }"#,
        )
        .expect("normalized summary");

        assert_eq!(parsed.goal, "Ship stage 12 safely");
        assert_eq!(parsed.constraints, vec!["Keep AGENTS.md pinned"]);
        assert_eq!(parsed.decisions, vec!["Use fallback on invalid JSON"]);
        assert_eq!(
            parsed.relevant_files_entities,
            vec!["crates/oxide-agent-core/src/agent/compaction/service.rs"]
        );
        assert_eq!(parsed.remaining_work, vec!["Add hardening tests"]);
        assert_eq!(parsed.risks, vec!["Timeout risk"]);
    }

    #[test]
    fn first_sentence_preserves_file_like_tokens() {
        assert_eq!(
            super::first_sentence("Preserve AGENTS.md during compaction. Keep going."),
            "Preserve AGENTS.md during compaction"
        );
    }

    #[tokio::test]
    async fn summarize_if_needed_uses_llm_when_model_is_available() {
        let settings = Arc::new(crate::config::AgentSettings {
            compaction_model_id: Some("compact-model".to_string()),
            compaction_model_provider: Some("mock".to_string()),
            compaction_model_max_tokens: Some(256),
            compaction_model_timeout_secs: Some(5),
            ..crate::config::AgentSettings::default()
        });
        let mut llm_client = LlmClient::new(settings.as_ref());
        llm_client.register_provider(
            "mock".to_string(),
            Arc::new(mock_llm_simple(
                r#"{"goal":"Ship stage 7","constraints":[],"decisions":["Use a sidecar model"],"discoveries":[],"relevant_files_entities":[],"remaining_work":["Implement rebuild"],"risks":[]}"#,
            )),
        );
        let summarizer = CompactionSummarizer::new(
            Arc::new(llm_client),
            CompactionSummarizerConfig {
                model_name: "compact-model".to_string(),
                provider_name: "mock".to_string(),
                timeout_secs: 5,
            },
        );
        let messages = vec![
            AgentMessage::user("We need to ship stage 7 soon."),
            AgentMessage::assistant("I will use a sidecar model for summaries."),
            AgentMessage::user("Keep AGENTS.md pinned while compaction runs."),
            AgentMessage::assistant("Recent response 1."),
            AgentMessage::user("Recent response 2 input."),
            AgentMessage::assistant("Recent response 2 output."),
        ];
        let snapshot = classify_hot_memory(&messages);
        let request = CompactionRequest::new(
            CompactionTrigger::Manual,
            "Ship stage 7",
            "system prompt",
            &[],
            "agent-model",
            512,
            false,
        );

        let outcome = summarizer
            .summarize_if_needed(&request, BudgetState::ShouldCompact, &snapshot, &messages)
            .await;

        assert!(outcome.attempted);
        assert!(!outcome.used_fallback);
        assert_eq!(outcome.model_name.as_deref(), Some("compact-model"));
        assert_eq!(
            outcome.summary.expect("summary").remaining_work,
            vec!["Implement rebuild"]
        );
    }

    #[tokio::test]
    async fn summarize_if_needed_falls_back_without_provider() {
        let llm_client = Arc::new(LlmClient::new(&crate::config::AgentSettings::default()));
        let summarizer = CompactionSummarizer::new(
            llm_client,
            CompactionSummarizerConfig {
                model_name: "compact-model".to_string(),
                provider_name: "missing".to_string(),
                timeout_secs: 1,
            },
        );
        let messages = vec![
            AgentMessage::user("We must preserve AGENTS.md and tool schemas."),
            AgentMessage::assistant(
                "I found `crates/oxide-agent-core/src/agent/compaction/service.rs`.",
            ),
            AgentMessage::user("Do not lose the active task during rebuild."),
            AgentMessage::assistant("Recent response 1."),
            AgentMessage::user("Recent response 2 input."),
            AgentMessage::assistant("Recent response 2 output."),
        ];
        let snapshot = classify_hot_memory(&messages);
        let request = CompactionRequest::new(
            CompactionTrigger::Manual,
            "Ship stage 7",
            "system prompt",
            &[],
            "agent-model",
            512,
            false,
        );

        let outcome = summarizer
            .summarize_if_needed(&request, BudgetState::OverLimit, &snapshot, &messages)
            .await;

        assert!(outcome.attempted);
        assert!(outcome.used_fallback);
        let summary = outcome.summary.expect("fallback summary");
        assert!(summary
            .constraints
            .iter()
            .any(|item| item.contains("preserve AGENTS")));
        assert!(summary
            .relevant_files_entities
            .iter()
            .any(|item| item.contains("crates/oxide-agent-core/src/agent/compaction/service.rs")));
    }

    #[tokio::test]
    async fn summarize_if_needed_falls_back_on_invalid_json() {
        let settings = Arc::new(crate::config::AgentSettings {
            compaction_model_id: Some("compact-model".to_string()),
            compaction_model_provider: Some("mock".to_string()),
            compaction_model_max_tokens: Some(256),
            compaction_model_timeout_secs: Some(5),
            ..crate::config::AgentSettings::default()
        });
        let mut llm_client = LlmClient::new(settings.as_ref());
        llm_client.register_provider(
            "mock".to_string(),
            Arc::new(mock_llm_simple("```json\n{\"goal\":123}\n```")),
        );
        let summarizer = CompactionSummarizer::new(
            Arc::new(llm_client),
            CompactionSummarizerConfig {
                model_name: "compact-model".to_string(),
                provider_name: "mock".to_string(),
                timeout_secs: 5,
            },
        );
        let messages = vec![
            AgentMessage::user("We must preserve AGENTS.md and tool schemas."),
            AgentMessage::assistant("Older response with findings."),
            AgentMessage::user("Keep the current task and todos intact."),
            AgentMessage::assistant("Recent response 1."),
            AgentMessage::user("Recent response 2 input."),
            AgentMessage::assistant("Recent response 2 output."),
        ];
        let snapshot = classify_hot_memory(&messages);
        let request = CompactionRequest::new(
            CompactionTrigger::Manual,
            "Ship stage 12",
            "system prompt",
            &[],
            "agent-model",
            512,
            false,
        );

        let outcome = summarizer
            .summarize_if_needed(&request, BudgetState::ShouldCompact, &snapshot, &messages)
            .await;

        assert!(outcome.attempted);
        assert!(outcome.used_fallback);
        assert_eq!(outcome.model_name.as_deref(), Some("compact-model"));
        assert!(outcome
            .summary
            .expect("fallback summary")
            .constraints
            .iter()
            .any(|item| item.contains("preserve AGENTS.md")));
    }
}
