use super::*;
use crate::llm::Message;

const DEFAULT_MEMORY_CLASSIFIER_TIMEOUT_SECS: u64 = 6;
const DEFAULT_MEMORY_CLASSIFIER_MAX_ATTEMPTS: usize = 3;
const DEFAULT_MEMORY_CLASSIFIER_INITIAL_BACKOFF_MS: u64 = 500;
const DEFAULT_MEMORY_CLASSIFIER_MAX_BACKOFF_MS: u64 = 4_000;
const MEMORY_CLASSIFIER_MAX_TASK_CHARS: usize = 2_000;
const MEMORY_CLASSIFIER_MAX_TOP_K: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MemoryClassificationClass {
    Smalltalk,
    EpisodeHistory,
    ExternalFreshFact,
    #[serde(rename = "procedure_howto")]
    ProcedureHowTo,
    ConstraintPolicy,
    PreferenceRecall,
    DecisionRecall,
    DurableProjectFact,
    General,
}

impl MemoryClassificationClass {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Smalltalk => "smalltalk",
            Self::EpisodeHistory => "episode_history",
            Self::ExternalFreshFact => "external_fresh_fact",
            Self::ProcedureHowTo => "procedure_howto",
            Self::ConstraintPolicy => "constraint_policy",
            Self::PreferenceRecall => "preference_recall",
            Self::DecisionRecall => "decision_recall",
            Self::DurableProjectFact => "durable_project_fact",
            Self::General => "general",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct MemoryReadPolicy {
    #[serde(default)]
    pub inject_prompt_memory: bool,
    #[serde(default)]
    pub search_episodes: bool,
    #[serde(default)]
    pub search_memories: bool,
    #[serde(default)]
    pub memory_type: Option<MemoryType>,
    #[serde(default)]
    pub allow_vector_only_memory: bool,
    #[serde(default = "default_classifier_min_importance")]
    pub min_importance: f32,
    #[serde(default = "default_classifier_top_k")]
    pub top_k: usize,
    #[serde(default)]
    pub allow_full_thread_read: bool,
}

impl Default for MemoryReadPolicy {
    fn default() -> Self {
        Self {
            inject_prompt_memory: false,
            search_episodes: false,
            search_memories: false,
            memory_type: None,
            allow_vector_only_memory: false,
            min_importance: default_classifier_min_importance(),
            top_k: default_classifier_top_k(),
            allow_full_thread_read: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct MemoryWritePolicy {
    #[serde(default)]
    pub allow_llm_durable_writes: bool,
    #[serde(default = "default_allow_tool_draft_writes")]
    pub allow_tool_draft_writes: bool,
    #[serde(default = "default_episode_only")]
    pub episode_only: bool,
}

impl Default for MemoryWritePolicy {
    fn default() -> Self {
        Self {
            allow_llm_durable_writes: false,
            allow_tool_draft_writes: default_allow_tool_draft_writes(),
            episode_only: default_episode_only(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct MemoryClassificationDecision {
    #[serde(rename = "class")]
    pub class: MemoryClassificationClass,
    #[serde(default)]
    pub read_policy: MemoryReadPolicy,
    #[serde(default)]
    pub write_policy: MemoryWritePolicy,
    #[serde(default)]
    pub confidence: f32,
}

impl Default for MemoryClassificationDecision {
    fn default() -> Self {
        Self::conservative_safe_mode()
    }
}

impl MemoryClassificationDecision {
    #[must_use]
    pub(crate) fn conservative_safe_mode() -> Self {
        Self {
            class: MemoryClassificationClass::General,
            read_policy: MemoryReadPolicy::default(),
            write_policy: MemoryWritePolicy::default(),
            confidence: 0.0,
        }
    }

    #[must_use]
    pub(crate) fn normalized(mut self) -> Self {
        self.confidence = self.confidence.clamp(0.0, 1.0);
        self.read_policy.min_importance = self.read_policy.min_importance.clamp(0.0, 1.0);
        self.read_policy.top_k = self.read_policy.top_k.clamp(1, MEMORY_CLASSIFIER_MAX_TOP_K);

        if !self.read_policy.inject_prompt_memory {
            self.read_policy.search_episodes = false;
            self.read_policy.search_memories = false;
            self.read_policy.memory_type = None;
            self.read_policy.allow_vector_only_memory = false;
            self.read_policy.allow_full_thread_read = false;
        } else if !self.read_policy.search_memories {
            self.read_policy.memory_type = None;
            self.read_policy.allow_vector_only_memory = false;
        }

        if !self.write_policy.allow_llm_durable_writes {
            self.write_policy.episode_only = true;
        }

        self
    }
}

#[async_trait]
pub(crate) trait MemoryTaskClassifier: Send + Sync {
    async fn classify(&self, task: &str) -> Result<MemoryClassificationDecision>;
}

#[derive(Debug, Clone)]
struct MemoryTaskClassifierConfig {
    model: ModelInfo,
    timeout_secs: u64,
    max_attempts: usize,
    initial_backoff_ms: u64,
    max_backoff_ms: u64,
}

impl Default for MemoryTaskClassifierConfig {
    fn default() -> Self {
        Self {
            model: ModelInfo::default(),
            timeout_secs: DEFAULT_MEMORY_CLASSIFIER_TIMEOUT_SECS,
            max_attempts: DEFAULT_MEMORY_CLASSIFIER_MAX_ATTEMPTS,
            initial_backoff_ms: DEFAULT_MEMORY_CLASSIFIER_INITIAL_BACKOFF_MS,
            max_backoff_ms: DEFAULT_MEMORY_CLASSIFIER_MAX_BACKOFF_MS,
        }
    }
}

#[async_trait]
trait MemoryClassifierBackend: Send + Sync {
    async fn classify_once(&self, prompt: &str, route: &ModelInfo) -> Result<String, LlmError>;
}

#[derive(Clone)]
struct LlmMemoryClassifierBackend {
    llm_client: Arc<LlmClient>,
}

#[async_trait]
impl MemoryClassifierBackend for LlmMemoryClassifierBackend {
    async fn classify_once(&self, prompt: &str, route: &ModelInfo) -> Result<String, LlmError> {
        if !self.llm_client.is_provider_available(&route.provider) {
            return Err(LlmError::MissingConfig(format!(
                "memory classifier provider '{}' is not configured",
                route.provider
            )));
        }

        if LlmClient::supports_structured_output_for_model(route) {
            let messages = vec![Message::user(prompt)];
            let response = self
                .llm_client
                .chat_with_tools_single_attempt_for_model_info(
                    memory_classifier_system_prompt(),
                    &messages,
                    &[],
                    route,
                    None,
                    true,
                )
                .await?;

            if !response.tool_calls.is_empty() {
                return Err(LlmError::ApiError(
                    "memory classifier unexpectedly returned tool calls".to_string(),
                ));
            }

            response.content.ok_or_else(|| {
                LlmError::ApiError("memory classifier returned empty response body".to_string())
            })
        } else {
            self.llm_client
                .chat_completion_for_model_info(
                    memory_classifier_system_prompt(),
                    &[],
                    prompt,
                    route,
                )
                .await
        }
    }
}

#[derive(Clone)]
pub(crate) struct LlmMemoryTaskClassifier {
    backend: Arc<dyn MemoryClassifierBackend>,
    config: MemoryTaskClassifierConfig,
}

impl LlmMemoryTaskClassifier {
    #[must_use]
    pub(crate) fn new(llm_client: Arc<LlmClient>, model: ModelInfo) -> Self {
        Self::new_with_backend(
            Arc::new(LlmMemoryClassifierBackend { llm_client }),
            MemoryTaskClassifierConfig {
                model,
                ..MemoryTaskClassifierConfig::default()
            },
        )
    }

    #[must_use]
    fn new_with_backend(
        backend: Arc<dyn MemoryClassifierBackend>,
        config: MemoryTaskClassifierConfig,
    ) -> Self {
        Self { backend, config }
    }

    fn backoff_for_retry(&self, attempt: usize) -> Option<Duration> {
        if attempt >= self.config.max_attempts.max(1) {
            return None;
        }

        let exponent = (attempt.saturating_sub(1)).min(31) as u32;
        let multiplier = 2u64.pow(exponent);
        let backoff_ms = self
            .config
            .initial_backoff_ms
            .saturating_mul(multiplier)
            .min(self.config.max_backoff_ms);
        Some(Duration::from_millis(backoff_ms))
    }

    fn should_retry_after_failure(
        &self,
        failure: &ClassifierAttemptFailure,
        attempt: usize,
    ) -> Option<Duration> {
        if !failure.retryable {
            return None;
        }

        self.backoff_for_retry(attempt)
    }
}

struct ClassifierAttemptFailure {
    error: anyhow::Error,
    retryable: bool,
}

impl ClassifierAttemptFailure {
    fn retryable(error: anyhow::Error) -> Self {
        Self {
            error,
            retryable: true,
        }
    }

    fn permanent(error: anyhow::Error) -> Self {
        Self {
            error,
            retryable: false,
        }
    }
}

fn is_retryable_classifier_llm_error(error: &LlmError) -> bool {
    match error {
        LlmError::MissingConfig(_) => false,
        LlmError::ApiError(message) => {
            !message.contains("does not support structured output")
                && !message.contains("not implemented")
        }
        _ => true,
    }
}

#[async_trait]
impl MemoryTaskClassifier for LlmMemoryTaskClassifier {
    async fn classify(&self, task: &str) -> Result<MemoryClassificationDecision> {
        let prompt = build_memory_classifier_user_prompt(task);
        let max_attempts = self.config.max_attempts.max(1);
        let mut last_failure: Option<ClassifierAttemptFailure> = None;

        for attempt in 1..=max_attempts {
            let llm_call = self.backend.classify_once(&prompt, &self.config.model);
            let result = timeout(Duration::from_secs(self.config.timeout_secs), llm_call).await;

            match result {
                Ok(Ok(response)) => match parse_memory_classification_response(&response) {
                    Ok(parsed) => return Ok(parsed.normalized()),
                    Err(error) => last_failure = Some(ClassifierAttemptFailure::retryable(error)),
                },
                Ok(Err(error)) => {
                    let failure = if is_retryable_classifier_llm_error(&error) {
                        ClassifierAttemptFailure::retryable(anyhow::anyhow!(error.to_string()))
                    } else {
                        ClassifierAttemptFailure::permanent(anyhow::anyhow!(error.to_string()))
                    };
                    last_failure = Some(failure);
                }
                Err(_) => {
                    last_failure = Some(ClassifierAttemptFailure::retryable(anyhow::anyhow!(
                        "memory classifier timed out after {}s",
                        self.config.timeout_secs
                    )));
                }
            }

            let Some(failure) = last_failure.as_ref() else {
                break;
            };
            let Some(backoff) = self.should_retry_after_failure(failure, attempt) else {
                break;
            };

            warn!(
                model = %self.config.model.id,
                provider = %self.config.model.provider,
                attempt,
                max_attempts,
                backoff_ms = backoff.as_millis(),
                error = %failure.error,
                "Memory classifier attempt failed, retrying"
            );
            sleep(backoff).await;
        }

        Err(last_failure
            .map(|failure| failure.error)
            .unwrap_or_else(|| anyhow::anyhow!("memory classifier failed")))
    }
}

fn default_classifier_min_importance() -> f32 {
    0.55
}

const fn default_classifier_top_k() -> usize {
    3
}

const fn default_allow_tool_draft_writes() -> bool {
    true
}

const fn default_episode_only() -> bool {
    true
}

fn memory_classifier_system_prompt() -> &'static str {
    r#"You are a routing classifier for persistent memory.

Return exactly one JSON object. No markdown. No prose. No explanations.

Do not answer the user's task. Do not act like the main agent. Only decide routing policy.

Choose `class` from this taxonomy:
- `smalltalk`: greeting, thanks, acknowledgment, casual chatter.
- `episode_history`: the user wants prior thread history, incidents, regressions, or what happened before.
- `external_fresh_fact`: current or externally changing facts that should not use durable memory as truth.
- `procedure_howto`: reusable workflow, steps, runbook, or operational how-to.
- `constraint_policy`: stable rules, constraints, guardrails, or must/never requirements.
- `preference_recall`: stable user/team preferences, style, conventions, or formatting choices.
- `decision_recall`: previously made design/product/process decisions that may need recall.
- `durable_project_fact`: stable project/repo/domain facts worth reusing later.
- `general`: everything else or uncertain cases.

Read policy meaning:
- `inject_prompt_memory=false` means do not inject durable memory into the main prompt.
- `search_episodes` controls episode retrieval.
- `search_memories` controls reusable memory retrieval.
- `memory_type` must be `null` or one of `fact`, `preference`, `procedure`, `decision`, `constraint`.
- `allow_vector_only_memory` should be true only when semantic-only reusable memory hits are acceptable.
- `min_importance` is a float from 0.0 to 1.0.
- `top_k` is an integer from 1 to 5.
- `allow_full_thread_read` should be true only when later full-thread reads may be justified.

Write policy meaning:
- `allow_llm_durable_writes` controls whether post-run LLM durable memory writes are admitted.
- `allow_tool_draft_writes` controls whether deterministic tool-derived drafts may persist.
- `episode_only=true` means keep only the episode record and reject LLM durable memory writes.

Be conservative when uncertain. Prefer false on risky durable-memory writes or reads.

Schema:
{
  "class": "smalltalk|episode_history|external_fresh_fact|procedure_howto|constraint_policy|preference_recall|decision_recall|durable_project_fact|general",
  "read_policy": {
    "inject_prompt_memory": true,
    "search_episodes": false,
    "search_memories": true,
    "memory_type": "procedure",
    "allow_vector_only_memory": true,
    "min_importance": 0.55,
    "top_k": 3,
    "allow_full_thread_read": false
  },
  "write_policy": {
    "allow_llm_durable_writes": true,
    "allow_tool_draft_writes": true,
    "episode_only": false
  },
  "confidence": 0.0
}"#
}

fn build_memory_classifier_user_prompt(task: &str) -> String {
    format!(
        "Classify the following user task for persistent-memory routing.\n\nTask:\n{}",
        truncate_classifier_task(task.trim())
    )
}

fn truncate_classifier_task(task: &str) -> String {
    task.chars()
        .take(MEMORY_CLASSIFIER_MAX_TASK_CHARS)
        .collect()
}

fn parse_memory_classification_response(response: &str) -> Result<MemoryClassificationDecision> {
    serde_json::from_str::<MemoryClassificationDecision>(extract_classifier_json_payload(response))
        .map_err(|error| anyhow::anyhow!("memory classifier returned invalid JSON: {error}"))
}

fn extract_classifier_json_payload(response: &str) -> &str {
    let trimmed = response.trim();
    if let Some(stripped) = trimmed.strip_prefix("```") {
        let stripped = stripped
            .strip_prefix("json")
            .or_else(|| stripped.strip_prefix("JSON"))
            .unwrap_or(stripped)
            .trim();
        if let Some(inner) = stripped.strip_suffix("```") {
            return inner.trim();
        }
    }
    trimmed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AgentSettings;
    use crate::llm::MockLlmProvider;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeBackend {
        responses: Mutex<VecDeque<Result<String, LlmError>>>,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl MemoryClassifierBackend for FakeBackend {
        async fn classify_once(
            &self,
            _prompt: &str,
            _route: &ModelInfo,
        ) -> Result<String, LlmError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.responses
                .lock()
                .expect("backend mutex poisoned")
                .pop_front()
                .unwrap_or_else(|| Err(LlmError::ApiError("no fake responses left".to_string())))
        }
    }

    fn fake_classifier(responses: Vec<Result<String, LlmError>>) -> LlmMemoryTaskClassifier {
        LlmMemoryTaskClassifier::new_with_backend(
            Arc::new(FakeBackend {
                responses: Mutex::new(responses.into()),
                calls: AtomicUsize::new(0),
            }),
            MemoryTaskClassifierConfig {
                model: ModelInfo {
                    id: "mistral-small-2603".to_string(),
                    provider: "mistral".to_string(),
                    max_output_tokens: 512,
                    context_window_tokens: 32_000,
                    weight: 1,
                },
                timeout_secs: 1,
                max_attempts: 3,
                initial_backoff_ms: 0,
                max_backoff_ms: 0,
            },
        )
    }

    fn fake_classifier_with_backend(
        backend: Arc<FakeBackend>,
        model: ModelInfo,
    ) -> LlmMemoryTaskClassifier {
        LlmMemoryTaskClassifier::new_with_backend(
            backend,
            MemoryTaskClassifierConfig {
                model,
                timeout_secs: 1,
                max_attempts: 3,
                initial_backoff_ms: 0,
                max_backoff_ms: 0,
            },
        )
    }

    #[tokio::test]
    async fn classifier_parses_structured_response() {
        let classifier = fake_classifier(vec![Ok(
            r#"{"class":"procedure_howto","read_policy":{"inject_prompt_memory":true,"search_episodes":false,"search_memories":true,"memory_type":"procedure","allow_vector_only_memory":true,"min_importance":0.72,"top_k":4,"allow_full_thread_read":false},"write_policy":{"allow_llm_durable_writes":true,"allow_tool_draft_writes":true,"episode_only":false},"confidence":0.94}"#.to_string(),
        )]);

        let decision = classifier
            .classify("How do we deploy staging?")
            .await
            .expect("classifier should parse response");

        assert_eq!(decision.class, MemoryClassificationClass::ProcedureHowTo);
        assert!(decision.read_policy.inject_prompt_memory);
        assert_eq!(
            decision.read_policy.memory_type,
            Some(MemoryType::Procedure)
        );
        assert!(decision.write_policy.allow_llm_durable_writes);
        assert_eq!(decision.confidence, 0.94);
    }

    #[tokio::test]
    async fn classifier_retries_invalid_json() {
        let classifier = fake_classifier(vec![
            Ok("not json".to_string()),
            Ok(
                r#"{"class":"external_fresh_fact","read_policy":{"inject_prompt_memory":false,"search_episodes":true,"search_memories":true,"memory_type":"fact","allow_vector_only_memory":true,"min_importance":0.8,"top_k":9,"allow_full_thread_read":true},"write_policy":{"allow_llm_durable_writes":false,"allow_tool_draft_writes":true,"episode_only":true},"confidence":0.61}"#.to_string(),
            ),
        ]);

        let decision = classifier
            .classify("When is the next release?")
            .await
            .expect("classifier should retry and succeed");

        assert_eq!(decision.class, MemoryClassificationClass::ExternalFreshFact);
        assert!(!decision.read_policy.inject_prompt_memory);
        assert!(!decision.read_policy.search_episodes);
        assert!(!decision.read_policy.search_memories);
        assert_eq!(decision.read_policy.top_k, 5);
        assert!(decision.write_policy.episode_only);
    }

    #[tokio::test]
    async fn classifier_uses_plain_text_fallback_for_routes_without_structured_output() {
        let settings = AgentSettings {
            minimax_api_key: Some("test-minimax-key".to_string()),
            ..AgentSettings::default()
        };
        let mut llm = LlmClient::new(&settings);
        let mut provider = MockLlmProvider::new();
        provider.expect_chat_with_tools().times(0);
        provider
            .expect_chat_completion()
            .times(1)
            .return_once(|system_prompt, history, user_message, model_id, max_tokens| {
                assert!(system_prompt.contains("Return exactly one JSON object"));
                assert!(history.is_empty());
                assert!(user_message.contains("restart the staging deploy"));
                assert_eq!(model_id, "MiniMax-M2.7");
                assert_eq!(max_tokens, 512);
                Ok(r#"{"class":"procedure_howto","read_policy":{"inject_prompt_memory":true,"search_episodes":false,"search_memories":true,"memory_type":"procedure","allow_vector_only_memory":true,"min_importance":0.72,"top_k":4,"allow_full_thread_read":false},"write_policy":{"allow_llm_durable_writes":true,"allow_tool_draft_writes":true,"episode_only":false},"confidence":0.94}"#.to_string())
            });

        llm.register_provider("minimax".to_string(), Arc::new(provider));

        let classifier = LlmMemoryTaskClassifier::new(
            Arc::new(llm),
            ModelInfo {
                id: "MiniMax-M2.7".to_string(),
                provider: "minimax".to_string(),
                max_output_tokens: 512,
                context_window_tokens: 32_000,
                weight: 1,
            },
        );

        let decision = classifier
            .classify("how do we restart the staging deploy?")
            .await
            .expect("classifier should use plain-text fallback");

        assert_eq!(decision.class, MemoryClassificationClass::ProcedureHowTo);
        assert!(decision.read_policy.inject_prompt_memory);
        assert_eq!(
            decision.read_policy.memory_type,
            Some(MemoryType::Procedure)
        );
    }

    #[tokio::test]
    async fn classifier_does_not_retry_missing_config_failures() {
        let backend = Arc::new(FakeBackend {
            responses: Mutex::new(
                vec![Err(LlmError::MissingConfig(
                    "missing classifier route".to_string(),
                ))]
                .into(),
            ),
            calls: AtomicUsize::new(0),
        });
        let classifier = fake_classifier_with_backend(
            Arc::clone(&backend),
            ModelInfo {
                id: "mistral-small-2603".to_string(),
                provider: "mistral".to_string(),
                max_output_tokens: 512,
                context_window_tokens: 32_000,
                weight: 1,
            },
        );

        let error = classifier
            .classify("how do we deploy staging?")
            .await
            .expect_err("classifier should fail without retries");

        assert!(error.to_string().contains("missing classifier route"));
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    }
}
