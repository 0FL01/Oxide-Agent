//! Sidecar summarizer for compactable Agent Mode history.

use super::prompt::{build_compaction_user_message, compaction_system_prompt};
use super::types::{
    BudgetState, CompactionRequest, CompactionSnapshot, CompactionSummary, SummaryGenerationOutcome,
};
use crate::agent::memory::{AgentMessage, MessageRole};
use crate::config::ModelInfo;
use crate::llm::{LlmClient, LlmError};
use lazy_regex::lazy_regex;
use std::sync::Arc;
use std::time::Instant;
use tokio::time::{sleep, timeout, Duration};
use tracing::{debug, warn};

const DEFAULT_COMPACTION_SUMMARIZER_TIMEOUT_SECS: u64 = 5;
const DEFAULT_COMPACTION_SUMMARIZER_MAX_ATTEMPTS: usize = 5;
const DEFAULT_COMPACTION_SUMMARIZER_INITIAL_BACKOFF_MS: u64 = 1_000;
const DEFAULT_COMPACTION_SUMMARIZER_MAX_BACKOFF_MS: u64 = 12_000;

/// Configuration for the dedicated compaction summary model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionSummarizerConfig {
    /// Candidate compaction routes, with dedicated compaction model first when configured.
    pub model_routes: Vec<ModelInfo>,
    /// Request timeout for the sidecar summary call.
    pub timeout_secs: u64,
    /// Maximum number of attempts per model route before failing over.
    pub max_attempts: usize,
    /// Initial retry backoff in milliseconds.
    pub initial_backoff_ms: u64,
    /// Maximum retry backoff in milliseconds.
    pub max_backoff_ms: u64,
}

impl Default for CompactionSummarizerConfig {
    fn default() -> Self {
        Self {
            model_routes: Vec::new(),
            timeout_secs: DEFAULT_COMPACTION_SUMMARIZER_TIMEOUT_SECS,
            max_attempts: DEFAULT_COMPACTION_SUMMARIZER_MAX_ATTEMPTS,
            initial_backoff_ms: DEFAULT_COMPACTION_SUMMARIZER_INITIAL_BACKOFF_MS,
            max_backoff_ms: DEFAULT_COMPACTION_SUMMARIZER_MAX_BACKOFF_MS,
        }
    }
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
        let compactable_entries = snapshot.compactable_history.message_count;

        let mut attempted_model_name = None;

        for route in self
            .config
            .model_routes
            .iter()
            .filter(|route| !route.id.trim().is_empty() && !route.provider.trim().is_empty())
        {
            if !self.llm_client.is_provider_available(&route.provider) {
                warn!(
                    trigger = ?request.trigger,
                    model = %route.id,
                    provider = %route.provider,
                    compactable_entries,
                    budget_state = ?budget_state,
                    "Compaction summary route unavailable, trying next route"
                );
                continue;
            }

            attempted_model_name = Some(route.id.clone());
            debug!(
                model = %route.id,
                provider = %route.provider,
                compactable_entries,
                "Generating compaction summary"
            );

            let llm_started_at = Instant::now();
            match self.call_llm(&user_message, route).await {
                Ok(response) => match parse_summary_response(&response) {
                    Some(summary) => {
                        warn!(
                            trigger = ?request.trigger,
                            model = %route.id,
                            provider = %route.provider,
                            compactable_entries,
                            budget_state = ?budget_state,
                            elapsed_ms = llm_started_at.elapsed().as_millis(),
                            "Compaction generated structured summary"
                        );
                        return SummaryGenerationOutcome {
                            attempted: true,
                            used_fallback: false,
                            model_name: Some(route.id.clone()),
                            summary: Some(summary),
                        };
                    }
                    None => {
                        warn!(
                            trigger = ?request.trigger,
                            model = %route.id,
                            provider = %route.provider,
                            compactable_entries,
                            budget_state = ?budget_state,
                            elapsed_ms = llm_started_at.elapsed().as_millis(),
                            "Compaction summary response invalid, trying next route"
                        );
                    }
                },
                Err(error) => {
                    warn!(
                        trigger = ?request.trigger,
                        model = %route.id,
                        provider = %route.provider,
                        compactable_entries,
                        budget_state = ?budget_state,
                        elapsed_ms = llm_started_at.elapsed().as_millis(),
                        error = %error,
                        "Compaction LLM call failed, trying next route"
                    );
                }
            }
        }

        warn!(
            trigger = ?request.trigger,
            compactable_entries,
            budget_state = ?budget_state,
            attempted_model = attempted_model_name.as_deref().unwrap_or("none"),
            "Compaction routes exhausted, using deterministic fallback"
        );
        SummaryGenerationOutcome {
            attempted: true,
            used_fallback: true,
            model_name: attempted_model_name,
            summary: Some(fallback),
        }
    }

    async fn call_llm(&self, user_message: &str, route: &ModelInfo) -> Result<String, LlmError> {
        let system_prompt = compaction_system_prompt();
        let max_attempts = self.config.max_attempts.max(1);

        for attempt in 1..=max_attempts {
            let llm_call = self.llm_client.chat_completion_for_model_info(
                system_prompt,
                &[],
                user_message,
                route,
            );

            match timeout(Duration::from_secs(self.config.timeout_secs), llm_call).await {
                Ok(Ok(response)) => return Ok(response),
                Ok(Err(error)) => {
                    let Some(backoff) = self.retry_backoff_for_error(&error, attempt) else {
                        return Err(error);
                    };
                    warn!(
                        model = %route.id,
                        provider = %route.provider,
                        attempt,
                        max_attempts,
                        backoff_ms = backoff.as_millis(),
                        error = %error,
                        "Compaction summary attempt failed, retrying route"
                    );
                    sleep(backoff).await;
                }
                Err(_) => {
                    let Some(backoff) = self.retry_backoff_for_timeout(attempt) else {
                        return Err(LlmError::Unknown(format!(
                            "Compaction summary model timed out after {}s",
                            self.config.timeout_secs
                        )));
                    };
                    warn!(
                        model = %route.id,
                        provider = %route.provider,
                        attempt,
                        max_attempts,
                        timeout_secs = self.config.timeout_secs,
                        backoff_ms = backoff.as_millis(),
                        "Compaction summary attempt timed out, retrying route"
                    );
                    sleep(backoff).await;
                }
            }
        }

        Err(LlmError::ApiError(
            "Compaction summary retry attempts exhausted".to_string(),
        ))
    }

    fn retry_backoff_for_error(&self, error: &LlmError, attempt: usize) -> Option<Duration> {
        if attempt >= self.config.max_attempts.max(1) || !LlmClient::is_retryable_error(error) {
            return None;
        }

        match error {
            LlmError::RateLimit {
                wait_secs: Some(wait_secs),
                ..
            } => {
                let wait_with_buffer = wait_secs.saturating_add(1);
                let backoff = Duration::from_secs(wait_with_buffer);
                (backoff <= self.max_backoff()).then_some(backoff)
            }
            _ => Some(self.exponential_backoff(attempt)),
        }
    }

    fn retry_backoff_for_timeout(&self, attempt: usize) -> Option<Duration> {
        (attempt < self.config.max_attempts.max(1)).then(|| self.exponential_backoff(attempt))
    }

    fn exponential_backoff(&self, attempt: usize) -> Duration {
        let exponent = (attempt.saturating_sub(1)).min(31) as u32;
        let multiplier = 2u64.pow(exponent);
        let backoff_ms = self
            .config
            .initial_backoff_ms
            .saturating_mul(multiplier)
            .min(self.config.max_backoff_ms);
        Duration::from_millis(backoff_ms)
    }

    fn max_backoff(&self) -> Duration {
        Duration::from_millis(self.config.max_backoff_ms)
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

    matches!(
        request.trigger,
        super::CompactionTrigger::Manual | super::CompactionTrigger::PostRun
    ) || matches!(
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
    use crate::config::ModelInfo;
    use crate::llm::{LlmClient, MockLlmProvider};
    use crate::testing::mock_llm_simple;
    use std::sync::Arc;

    fn route(model_name: &str, provider_name: &str, max_output_tokens: u32) -> ModelInfo {
        ModelInfo {
            id: model_name.to_string(),
            provider: provider_name.to_string(),
            max_output_tokens,
            context_window_tokens: 0,
            weight: 1,
        }
    }

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
            compaction_model_max_output_tokens: Some(256),
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
                model_routes: vec![route("compact-model", "mock", 256)],
                timeout_secs: 5,
                ..CompactionSummarizerConfig::default()
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
                model_routes: vec![route("compact-model", "missing", 256)],
                timeout_secs: 1,
                ..CompactionSummarizerConfig::default()
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
            compaction_model_max_output_tokens: Some(256),
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
                model_routes: vec![route("compact-model", "mock", 256)],
                timeout_secs: 5,
                ..CompactionSummarizerConfig::default()
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

    #[tokio::test]
    async fn summarize_if_needed_sends_payload_as_user_message() {
        let settings = Arc::new(crate::config::AgentSettings {
            compaction_model_id: Some("compact-model".to_string()),
            compaction_model_provider: Some("mock".to_string()),
            compaction_model_max_output_tokens: Some(256),
            compaction_model_timeout_secs: Some(5),
            ..crate::config::AgentSettings::default()
        });
        let mut provider = MockLlmProvider::new();
        provider
            .expect_chat_completion()
            .withf(|system_prompt, history, user_message, model_name, max_tokens| {
                system_prompt.contains("Return ONLY valid JSON")
                    && history.is_empty()
                    && user_message.contains("## Entry")
                    && !user_message.trim().is_empty()
                    && user_message.contains("Older request")
                    && model_name == "compact-model"
                    && *max_tokens == 256
            })
            .return_once(|_, _, _, _, _| {
                Ok(r#"{"goal":"Ship stage 7","constraints":[],"decisions":[],"discoveries":[],"relevant_files_entities":[],"remaining_work":[],"risks":[]}"#.to_string())
            });
        provider
            .expect_transcribe_audio()
            .returning(|_, _, _| Err(crate::llm::LlmError::Unknown("Not implemented".to_string())));
        provider.expect_analyze_image().returning(|_, _, _, _| {
            Err(crate::llm::LlmError::Unknown("Not implemented".to_string()))
        });
        provider
            .expect_chat_with_tools()
            .returning(|_| Err(crate::llm::LlmError::Unknown("Not implemented".to_string())));

        let mut llm_client = LlmClient::new(settings.as_ref());
        llm_client.register_provider("mock".to_string(), Arc::new(provider));
        let summarizer = CompactionSummarizer::new(
            Arc::new(llm_client),
            CompactionSummarizerConfig {
                model_routes: vec![route("compact-model", "mock", 256)],
                timeout_secs: 5,
                ..CompactionSummarizerConfig::default()
            },
        );
        let messages = vec![
            AgentMessage::user("Older request"),
            AgentMessage::assistant("Older response"),
            AgentMessage::user("Recent request 1"),
            AgentMessage::assistant("Recent response 1"),
            AgentMessage::user("Recent request 2"),
            AgentMessage::assistant("Recent response 2"),
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
        assert_eq!(outcome.summary.expect("summary").goal, "Ship stage 7");
    }
}
