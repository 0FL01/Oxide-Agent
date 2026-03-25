use super::fixtures::{manual_request, pre_iteration_request};
use crate::agent::compaction::{
    classify_hot_memory, CompactionPolicy, CompactionService, CompactionSummarizer,
    CompactionSummarizerConfig,
};
use crate::agent::memory::AgentMessage;
use crate::agent::{AgentContext, EphemeralSession};
use crate::config::{AgentSettings, ModelInfo};
use crate::llm::{ChatWithToolsRequest, LlmClient, LlmError, LlmProvider};
use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

fn compactable_history_messages() -> Vec<AgentMessage> {
    vec![
        AgentMessage::user(
            "Older request: inspect `crates/oxide-agent-core/src/agent/compaction/service.rs` and preserve AGENTS.md.",
        ),
        AgentMessage::assistant(
            "Older response: discovered warning paths and decided to keep summaries structured.",
        ),
        AgentMessage::user("Recent request 1."),
        AgentMessage::assistant("Recent response 1."),
        AgentMessage::user("Recent request 2."),
        AgentMessage::assistant("Recent response 2."),
    ]
}

fn recent_only_messages() -> Vec<AgentMessage> {
    vec![
        AgentMessage::user("Recent request 1."),
        AgentMessage::assistant("Recent response 1."),
        AgentMessage::user("Recent request 2."),
        AgentMessage::assistant("Recent response 2."),
    ]
}

fn route(model_id: &str, provider: &str, max_output_tokens: u32) -> ModelInfo {
    ModelInfo {
        id: model_id.to_string(),
        provider: provider.to_string(),
        max_output_tokens,
        context_window_tokens: 0,
        weight: 1,
    }
}

#[derive(Clone)]
enum ProbeBehavior {
    Return(&'static str),
    Sleep(Duration),
    UnknownError(&'static str),
    NetworkError(&'static str),
    RateLimit(Option<u64>),
}

struct ProbeProvider {
    calls: Arc<AtomicUsize>,
    behaviors: Mutex<VecDeque<ProbeBehavior>>,
    default_behavior: ProbeBehavior,
}

#[async_trait]
impl LlmProvider for ProbeProvider {
    async fn chat_completion(
        &self,
        _system_prompt: &str,
        _history: &[crate::llm::Message],
        _user_message: &str,
        _model_id: &str,
        _max_tokens: u32,
    ) -> Result<String, LlmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let behavior = self
            .behaviors
            .lock()
            .expect("probe behaviors lock")
            .pop_front()
            .unwrap_or_else(|| self.default_behavior.clone());
        match behavior {
            ProbeBehavior::Return(value) => Ok(value.to_string()),
            ProbeBehavior::Sleep(duration) => {
                tokio::time::sleep(duration).await;
                Ok(r#"{"goal":"late","constraints":[],"decisions":[],"discoveries":[],"relevant_files_entities":[],"remaining_work":[],"risks":[]}"#.to_string())
            }
            ProbeBehavior::UnknownError(message) => Err(LlmError::Unknown(message.to_string())),
            ProbeBehavior::NetworkError(message) => {
                Err(LlmError::NetworkError(message.to_string()))
            }
            ProbeBehavior::RateLimit(wait_secs) => Err(LlmError::RateLimit {
                wait_secs,
                message: "rate limited".to_string(),
            }),
        }
    }

    async fn transcribe_audio(
        &self,
        _audio_bytes: Vec<u8>,
        _mime_type: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown("Not implemented".to_string()))
    }

    async fn analyze_image(
        &self,
        _image_bytes: Vec<u8>,
        _text_prompt: &str,
        _system_prompt: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown("Not implemented".to_string()))
    }

    async fn chat_with_tools<'a>(
        &self,
        _request: ChatWithToolsRequest<'a>,
    ) -> Result<crate::llm::ChatResponse, LlmError> {
        Err(LlmError::Unknown("Not implemented".to_string()))
    }
}

fn probe_summarizer(
    behaviors: Vec<ProbeBehavior>,
    timeout_secs: u64,
) -> (CompactionSummarizer, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut llm_client = LlmClient::new(&AgentSettings {
        compaction_model_id: Some("compact-model".to_string()),
        compaction_model_provider: Some("probe".to_string()),
        compaction_model_max_output_tokens: Some(256),
        compaction_model_timeout_secs: Some(timeout_secs),
        ..AgentSettings::default()
    });
    llm_client.register_provider(
        "probe".to_string(),
        Arc::new(ProbeProvider {
            calls: Arc::clone(&calls),
            default_behavior: behaviors
                .last()
                .cloned()
                .unwrap_or(ProbeBehavior::UnknownError("probe behavior missing")),
            behaviors: Mutex::new(VecDeque::from(behaviors)),
        }),
    );

    (
        CompactionSummarizer::new(
            Arc::new(llm_client),
            CompactionSummarizerConfig {
                model_routes: vec![route("compact-model", "probe", 256)],
                timeout_secs,
                initial_backoff_ms: 0,
                max_backoff_ms: 12_000,
                ..CompactionSummarizerConfig::default()
            },
        ),
        calls,
    )
}

#[tokio::test]
async fn summary_is_not_attempted_for_warning_budget_state() {
    let (summarizer, calls) = probe_summarizer(
        vec![ProbeBehavior::Return(
            r#"{"goal":"should not run","constraints":[],"decisions":[],"discoveries":[],"relevant_files_entities":[],"remaining_work":[],"risks":[]}"#,
        )],
        5,
    );
    let messages = compactable_history_messages();
    let snapshot = classify_hot_memory(&messages);

    let outcome = summarizer
        .summarize_if_needed(
            &pre_iteration_request("Inspect summary trigger"),
            crate::agent::compaction::BudgetState::Warning,
            &snapshot,
            &messages,
        )
        .await;

    assert!(!outcome.attempted);
    assert!(!outcome.used_fallback);
    assert!(outcome.summary.is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn summary_is_not_attempted_when_only_recent_raw_window_exists() {
    let (summarizer, calls) = probe_summarizer(
        vec![ProbeBehavior::Return(
            r#"{"goal":"should not run","constraints":[],"decisions":[],"discoveries":[],"relevant_files_entities":[],"remaining_work":[],"risks":[]}"#,
        )],
        5,
    );
    let messages = recent_only_messages();
    let snapshot = classify_hot_memory(&messages);

    let outcome = summarizer
        .summarize_if_needed(
            &manual_request("Inspect recent-only history"),
            crate::agent::compaction::BudgetState::OverLimit,
            &snapshot,
            &messages,
        )
        .await;

    assert!(!outcome.attempted);
    assert!(outcome.summary.is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn service_does_not_attempt_summary_for_should_prune_budget() {
    let (summarizer, calls) = probe_summarizer(
        vec![ProbeBehavior::Return(
            r#"{"goal":"should not run","constraints":[],"decisions":[],"discoveries":[],"relevant_files_entities":[],"remaining_work":[],"risks":[]}"#,
        )],
        5,
    );
    let service = CompactionService::new(CompactionPolicy {
        warning_threshold_percent: 5,
        prune_threshold_percent: 20,
        compact_threshold_percent: 60,
        over_limit_threshold_percent: 90,
        hard_reserve_tokens: 0,
        externalize_threshold_tokens: usize::MAX,
        externalize_threshold_chars: usize::MAX,
        prune_min_tokens: usize::MAX,
        prune_min_chars: usize::MAX,
        ..CompactionPolicy::default()
    })
    .with_summarizer(summarizer);
    let mut session = EphemeralSession::new(2_000);
    for message in [
        AgentMessage::user(format!("Older request: {}", "A".repeat(3_000))),
        AgentMessage::assistant("Older response: preserve AGENTS.md and continue safely."),
        AgentMessage::user("Recent request 1."),
        AgentMessage::assistant("Recent response 1."),
        AgentMessage::user("Recent request 2."),
        AgentMessage::assistant("Recent response 2."),
    ] {
        session.memory_mut().add_message(message);
    }

    let outcome = service
        .prepare_for_run(
            &pre_iteration_request("Inspect summary trigger"),
            &mut session,
        )
        .await
        .expect("pre-iteration checkpoint succeeds");

    assert_eq!(
        outcome.budget.state,
        crate::agent::compaction::BudgetState::ShouldPrune
    );
    assert!(!outcome.summary_generation.attempted);
    assert!(!outcome.rebuild.applied);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn summary_timeout_falls_back_to_deterministic_summary() {
    let (summarizer, calls) =
        probe_summarizer(vec![ProbeBehavior::Sleep(Duration::from_millis(20))], 0);
    let messages = compactable_history_messages();
    let snapshot = classify_hot_memory(&messages);

    let outcome = summarizer
        .summarize_if_needed(
            &manual_request("Inspect timeout fallback"),
            crate::agent::compaction::BudgetState::ShouldCompact,
            &snapshot,
            &messages,
        )
        .await;

    assert!(outcome.attempted);
    assert!(outcome.used_fallback);
    assert_eq!(outcome.model_name.as_deref(), Some("compact-model"));
    assert!(outcome.summary.is_some());
    assert_eq!(calls.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn summary_retries_same_route_before_succeeding() {
    let (summarizer, calls) = probe_summarizer(
        vec![
            ProbeBehavior::NetworkError("transient network failure"),
            ProbeBehavior::Return(
                r#"{"goal":"recovered","constraints":[],"decisions":[],"discoveries":[],"relevant_files_entities":[],"remaining_work":["continue"],"risks":[]}"#,
            ),
        ],
        5,
    );
    let messages = compactable_history_messages();
    let snapshot = classify_hot_memory(&messages);

    let outcome = summarizer
        .summarize_if_needed(
            &manual_request("Inspect retry success"),
            crate::agent::compaction::BudgetState::ShouldCompact,
            &snapshot,
            &messages,
        )
        .await;

    assert!(outcome.attempted);
    assert!(!outcome.used_fallback);
    assert_eq!(outcome.model_name.as_deref(), Some("compact-model"));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        outcome.summary.expect("summary after retry").remaining_work,
        vec!["continue"]
    );
}

#[tokio::test]
async fn summary_uses_next_route_when_primary_route_fails() {
    let primary_calls = Arc::new(AtomicUsize::new(0));
    let fallback_calls = Arc::new(AtomicUsize::new(0));
    let mut llm_client = LlmClient::new(&AgentSettings {
        compaction_model_max_output_tokens: Some(256),
        compaction_model_timeout_secs: Some(5),
        ..AgentSettings::default()
    });
    llm_client.register_provider(
        "primary".to_string(),
        Arc::new(ProbeProvider {
            calls: Arc::clone(&primary_calls),
            default_behavior: ProbeBehavior::UnknownError("primary failed"),
            behaviors: Mutex::new(VecDeque::from(vec![ProbeBehavior::UnknownError(
                "primary failed",
            )])),
        }),
    );
    llm_client.register_provider(
        "fallback".to_string(),
        Arc::new(ProbeProvider {
            calls: Arc::clone(&fallback_calls),
            default_behavior: ProbeBehavior::Return(
                r#"{"goal":"fallback","constraints":[],"decisions":[],"discoveries":[],"relevant_files_entities":[],"remaining_work":["continue"],"risks":[]}"#,
            ),
            behaviors: Mutex::new(VecDeque::from(vec![ProbeBehavior::Return(
                r#"{"goal":"fallback","constraints":[],"decisions":[],"discoveries":[],"relevant_files_entities":[],"remaining_work":["continue"],"risks":[]}"#,
            )])),
        }),
    );
    let summarizer = CompactionSummarizer::new(
        Arc::new(llm_client),
        CompactionSummarizerConfig {
            model_routes: vec![
                route("compact-primary", "primary", 256),
                route("compact-fallback", "fallback", 256),
            ],
            timeout_secs: 5,
            initial_backoff_ms: 0,
            ..CompactionSummarizerConfig::default()
        },
    );
    let messages = compactable_history_messages();
    let snapshot = classify_hot_memory(&messages);

    let outcome = summarizer
        .summarize_if_needed(
            &manual_request("Inspect route fallback"),
            crate::agent::compaction::BudgetState::ShouldCompact,
            &snapshot,
            &messages,
        )
        .await;

    assert!(outcome.attempted);
    assert!(!outcome.used_fallback);
    assert_eq!(outcome.model_name.as_deref(), Some("compact-fallback"));
    assert_eq!(primary_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fallback_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        outcome.summary.expect("llm summary").remaining_work,
        vec!["continue"]
    );
}

#[tokio::test]
async fn summary_skips_retry_when_rate_limit_wait_exceeds_max_backoff() {
    let primary_calls = Arc::new(AtomicUsize::new(0));
    let fallback_calls = Arc::new(AtomicUsize::new(0));
    let mut llm_client = LlmClient::new(&AgentSettings {
        compaction_model_max_output_tokens: Some(256),
        compaction_model_timeout_secs: Some(5),
        ..AgentSettings::default()
    });
    llm_client.register_provider(
        "primary".to_string(),
        Arc::new(ProbeProvider {
            calls: Arc::clone(&primary_calls),
            default_behavior: ProbeBehavior::RateLimit(Some(30)),
            behaviors: Mutex::new(VecDeque::from(vec![ProbeBehavior::RateLimit(Some(30))])),
        }),
    );
    llm_client.register_provider(
        "fallback".to_string(),
        Arc::new(ProbeProvider {
            calls: Arc::clone(&fallback_calls),
            default_behavior: ProbeBehavior::Return(
                r#"{"goal":"fallback","constraints":[],"decisions":[],"discoveries":[],"relevant_files_entities":[],"remaining_work":["continue"],"risks":[]}"#,
            ),
            behaviors: Mutex::new(VecDeque::from(vec![ProbeBehavior::Return(
                r#"{"goal":"fallback","constraints":[],"decisions":[],"discoveries":[],"relevant_files_entities":[],"remaining_work":["continue"],"risks":[]}"#,
            )])),
        }),
    );
    let summarizer = CompactionSummarizer::new(
        Arc::new(llm_client),
        CompactionSummarizerConfig {
            model_routes: vec![
                route("compact-primary", "primary", 256),
                route("compact-fallback", "fallback", 256),
            ],
            timeout_secs: 5,
            initial_backoff_ms: 0,
            max_backoff_ms: 12_000,
            ..CompactionSummarizerConfig::default()
        },
    );
    let messages = compactable_history_messages();
    let snapshot = classify_hot_memory(&messages);

    let outcome = summarizer
        .summarize_if_needed(
            &manual_request("Inspect rate limit failover"),
            crate::agent::compaction::BudgetState::ShouldCompact,
            &snapshot,
            &messages,
        )
        .await;

    assert!(outcome.attempted);
    assert!(!outcome.used_fallback);
    assert_eq!(outcome.model_name.as_deref(), Some("compact-fallback"));
    assert_eq!(primary_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fallback_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn service_invalid_summary_response_uses_fallback_and_rebuilds() {
    let (summarizer, calls) = probe_summarizer(
        vec![ProbeBehavior::Return("```json\n{\"goal\":123}\n```")],
        5,
    );
    let service = CompactionService::default().with_summarizer(summarizer);
    let mut session = EphemeralSession::new(20_000);
    for message in compactable_history_messages() {
        session.memory_mut().add_message(message);
    }

    let outcome = service
        .prepare_for_run(
            &manual_request("Inspect invalid summary fallback"),
            &mut session,
        )
        .await
        .expect("manual compaction succeeds");

    assert!(outcome.summary_generation.attempted);
    assert!(outcome.summary_generation.used_fallback);
    assert!(outcome.rebuild.applied);
    assert!(session
        .memory()
        .get_messages()
        .iter()
        .any(|message| message.summary_payload().is_some()));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
