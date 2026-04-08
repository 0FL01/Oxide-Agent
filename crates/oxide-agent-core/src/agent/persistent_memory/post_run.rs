use super::*;

const DEFAULT_POST_RUN_MEMORY_WRITER_TIMEOUT_SECS: u64 = 8;
const DEFAULT_POST_RUN_MEMORY_WRITER_MAX_ATTEMPTS: usize = 3;
const DEFAULT_POST_RUN_MEMORY_WRITER_INITIAL_BACKOFF_MS: u64 = 1_000;
const DEFAULT_POST_RUN_MEMORY_WRITER_MAX_BACKOFF_MS: u64 = 8_000;
const POST_RUN_MEMORY_WRITER_MAX_TRANSCRIPT_MESSAGES: usize = 32;
const POST_RUN_MEMORY_WRITER_MAX_MESSAGE_CHARS: usize = 1_200;
const POST_RUN_MEMORY_WRITER_MAX_MEMORIES: usize = 8;
const THREAD_SHORT_SUMMARY_MAX_CHARS: usize = 220;
const MEMORY_TITLE_MAX_CHARS: usize = 96;
const MEMORY_CONTENT_MAX_CHARS: usize = 320;
const MEMORY_SHORT_DESCRIPTION_MAX_CHARS: usize = 160;

#[derive(Debug, Clone, Copy)]
pub enum PersistentRunPhase<'a> {
    Completed { final_answer: &'a str },
    WaitingForUserInput,
}

pub struct PersistentRunContext<'a> {
    pub session_id: &'a str,
    pub task_id: &'a str,
    pub scope: &'a AgentMemoryScope,
    pub task: &'a str,
    pub messages: &'a [AgentMessage],
    pub explicit_remember_intent: bool,
    pub hot_token_estimate: usize,
    pub tool_memory_drafts: Vec<ToolDerivedMemoryDraft>,
    pub phase: PersistentRunPhase<'a>,
}

#[derive(Debug, Clone)]
pub(crate) struct PostRunMemoryWriterConfig {
    pub model_routes: Vec<ModelInfo>,
    pub timeout_secs: u64,
    pub max_attempts: usize,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
}

impl Default for PostRunMemoryWriterConfig {
    fn default() -> Self {
        Self {
            model_routes: Vec::new(),
            timeout_secs: DEFAULT_POST_RUN_MEMORY_WRITER_TIMEOUT_SECS,
            max_attempts: DEFAULT_POST_RUN_MEMORY_WRITER_MAX_ATTEMPTS,
            initial_backoff_ms: DEFAULT_POST_RUN_MEMORY_WRITER_INITIAL_BACKOFF_MS,
            max_backoff_ms: DEFAULT_POST_RUN_MEMORY_WRITER_MAX_BACKOFF_MS,
        }
    }
}

pub(crate) struct PostRunMemoryWriterInput<'a> {
    pub task_id: &'a str,
    pub scope: &'a AgentMemoryScope,
    pub task: &'a str,
    pub final_answer: &'a str,
    pub messages: &'a [AgentMessage],
    pub explicit_remember_intent: bool,
    pub tools_used: &'a [String],
    pub artifacts: &'a [ArtifactRef],
    pub compaction_summary: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct PostRunMemoryWriterResponse {
    pub thread_short_summary: Option<String>,
    pub episode: PostRunEpisodeDraft,
    #[serde(default)]
    pub memories: Vec<PostRunMemoryDraft>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct PostRunEpisodeDraft {
    pub summary: String,
    pub outcome: EpisodeOutcome,
    #[serde(default)]
    pub failures: Vec<String>,
    pub importance: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct PostRunMemoryDraft {
    pub memory_type: MemoryType,
    pub title: String,
    pub content: String,
    pub short_description: String,
    pub importance: f32,
    pub confidence: f32,
    #[serde(default)]
    pub tags: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ValidatedPostRunMemoryWrite {
    pub(crate) thread_short_summary: Option<String>,
    pub(crate) episode: ValidatedPostRunEpisode,
    pub(crate) memories: Vec<MemoryRecord>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ValidatedPostRunEpisode {
    pub(crate) summary: String,
    pub(crate) outcome: EpisodeOutcome,
    pub(crate) failures: Vec<String>,
    pub(crate) importance: f32,
}

#[async_trait]
pub(crate) trait PostRunMemoryWriter: Send + Sync {
    async fn write(
        &self,
        input: &PostRunMemoryWriterInput<'_>,
    ) -> Result<ValidatedPostRunMemoryWrite>;
}

#[derive(Clone)]
pub(crate) struct LlmPostRunMemoryWriter {
    llm_client: Arc<LlmClient>,
    config: PostRunMemoryWriterConfig,
}

impl LlmPostRunMemoryWriter {
    #[must_use]
    pub fn new(llm_client: Arc<LlmClient>, config: PostRunMemoryWriterConfig) -> Self {
        Self { llm_client, config }
    }

    async fn call_llm(&self, user_message: &str, route: &ModelInfo) -> Result<String, LlmError> {
        let max_attempts = self.config.max_attempts.max(1);
        for attempt in 1..=max_attempts {
            let llm_call = self.llm_client.chat_completion_for_model_info(
                post_run_memory_writer_system_prompt(),
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
                        "PostRun memory writer attempt failed, retrying route"
                    );
                    sleep(backoff).await;
                }
                Err(_) => {
                    let Some(backoff) = self.retry_backoff_for_timeout(attempt) else {
                        return Err(LlmError::Unknown(format!(
                            "PostRun memory writer timed out after {}s",
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
                        "PostRun memory writer attempt timed out, retrying route"
                    );
                    sleep(backoff).await;
                }
            }
        }

        Err(LlmError::ApiError(
            "PostRun memory writer retry attempts exhausted".to_string(),
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

#[async_trait]
impl PostRunMemoryWriter for LlmPostRunMemoryWriter {
    async fn write(
        &self,
        input: &PostRunMemoryWriterInput<'_>,
    ) -> Result<ValidatedPostRunMemoryWrite> {
        let user_message = build_post_run_memory_writer_user_message(input);
        let mut last_error = None;

        for route in self
            .config
            .model_routes
            .iter()
            .filter(|route| !route.id.trim().is_empty() && !route.provider.trim().is_empty())
        {
            if !self.llm_client.is_provider_available(&route.provider) {
                continue;
            }

            match self.call_llm(&user_message, route).await {
                Ok(response) => match parse_post_run_memory_writer_response(&response) {
                    Some(parsed) => return validate_post_run_memory_writer_response(input, parsed),
                    None => {
                        last_error = Some(anyhow::anyhow!(
                            "PostRun memory writer returned invalid JSON"
                        ));
                    }
                },
                Err(error) => {
                    last_error = Some(anyhow::anyhow!(error.to_string()));
                }
            }
        }

        Err(last_error
            .unwrap_or_else(|| anyhow::anyhow!("No available PostRun memory writer routes")))
    }
}

fn post_run_memory_writer_system_prompt() -> &'static str {
    r#"You write durable memory records for an autonomous software agent.

Return JSON only. No markdown. No prose outside JSON.

Your job:
1. Produce one compact episode summary for the completed task.
2. Produce only reusable long-term memories worth retrieving later.
3. Ignore transient noise, raw tool payloads, progress chatter, and duplicated facts.

Rules:
- Only write memories that would still matter in a future session.
- Prefer project facts, user preferences, procedures, decisions, and constraints.
- If `explicit_remember_intent` is true, preserve the explicitly requested durable information when it is grounded in the transcript and reusable later.
- Do not invent facts not grounded in the provided conversation.
- Keep summaries concise and specific.
- `importance` and `confidence` must be floats between 0.0 and 1.0.
- `outcome` must be one of: success, partial, failure, cancelled.
- `memory_type` must be one of: fact, preference, procedure, decision, constraint.
- Output at most 8 memories.

Schema:
{
  "thread_short_summary": "optional short thread summary",
  "episode": {
    "summary": "what happened and what matters",
    "outcome": "success|partial|failure|cancelled",
    "failures": ["notable failures if any"],
    "importance": 0.0
  },
  "memories": [
    {
      "memory_type": "fact|preference|procedure|decision|constraint",
      "title": "short title",
      "content": "full reusable memory text",
      "short_description": "compact preview",
      "importance": 0.0,
      "confidence": 0.0,
      "tags": ["tag"],
      "reason": "why this should be remembered"
    }
  ]
}"#
}

fn build_post_run_memory_writer_user_message(input: &PostRunMemoryWriterInput<'_>) -> String {
    let mut sections = vec![
        format!("Task ID: {}", input.task_id),
        format!("Context key: {}", input.scope.context_key),
        format!(
            "Explicit remember intent: {}",
            if input.explicit_remember_intent {
                "true"
            } else {
                "false"
            }
        ),
        format!("User task:\n{}", input.task.trim()),
        format!("Final answer:\n{}", input.final_answer.trim()),
    ];

    if let Some(summary) = input
        .compaction_summary
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        sections.push(format!("Compaction summary:\n{summary}"));
    }

    if !input.tools_used.is_empty() {
        sections.push(format!("Tools used:\n- {}", input.tools_used.join("\n- ")));
    }

    if !input.artifacts.is_empty() {
        let artifacts = input
            .artifacts
            .iter()
            .map(|artifact| {
                format!(
                    "- {} | {} | {}",
                    artifact.storage_key,
                    artifact.description,
                    artifact.content_type.as_deref().unwrap_or("unknown")
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!("Artifacts:\n{artifacts}"));
    }

    let transcript = input
        .messages
        .iter()
        .rev()
        .take(POST_RUN_MEMORY_WRITER_MAX_TRANSCRIPT_MESSAGES)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(render_post_run_message)
        .collect::<Vec<_>>()
        .join("\n\n");
    sections.push(format!("Transcript excerpt:\n{transcript}"));
    sections.join("\n\n")
}

fn render_post_run_message(message: &AgentMessage) -> String {
    let role = match message.role {
        crate::agent::memory::MessageRole::System => "system",
        crate::agent::memory::MessageRole::User => "user",
        crate::agent::memory::MessageRole::Assistant => "assistant",
        crate::agent::memory::MessageRole::Tool => "tool",
    };
    let kind = format!("{:?}", message.resolved_kind());
    let content = truncate_chars(
        message.content.trim(),
        POST_RUN_MEMORY_WRITER_MAX_MESSAGE_CHARS,
    );
    if let Some(tool_name) = message.tool_name.as_deref() {
        format!("[{role}/{kind}/{tool_name}]\n{content}")
    } else {
        format!("[{role}/{kind}]\n{content}")
    }
}

fn parse_post_run_memory_writer_response(response: &str) -> Option<PostRunMemoryWriterResponse> {
    serde_json::from_str(extract_json_payload(response)).ok()
}

fn extract_json_payload(response: &str) -> &str {
    lazy_regex!(r"(?s)```(?:json)?\s*(\{.*\})\s*```")
        .captures(response)
        .and_then(|captures| captures.get(1))
        .map_or_else(|| response.trim(), |json| json.as_str().trim())
}

fn validate_post_run_memory_writer_response(
    input: &PostRunMemoryWriterInput<'_>,
    parsed: PostRunMemoryWriterResponse,
) -> Result<ValidatedPostRunMemoryWrite> {
    let now = Utc::now();
    let episode_summary = truncate_chars(parsed.episode.summary.trim(), 2_000);
    if episode_summary.is_empty() {
        return Err(anyhow::anyhow!(
            "PostRun memory writer produced empty episode summary"
        ));
    }

    let failures = parsed
        .episode
        .failures
        .into_iter()
        .map(|item| truncate_chars(item.trim(), 240))
        .filter(|item| !item.is_empty())
        .take(6)
        .collect::<Vec<_>>();

    let mut seen_hashes = HashSet::new();
    let mut memories = Vec::new();
    for draft in parsed
        .memories
        .into_iter()
        .take(POST_RUN_MEMORY_WRITER_MAX_MEMORIES)
    {
        let content = truncate_chars(draft.content.trim(), MEMORY_CONTENT_MAX_CHARS);
        if content.is_empty() {
            continue;
        }
        let content_hash = stable_memory_content_hash(draft.memory_type, &content);
        if !seen_hashes.insert(content_hash.clone()) {
            continue;
        }

        let mut tags = draft
            .tags
            .into_iter()
            .map(|tag| truncate_chars(tag.trim(), 32))
            .filter(|tag| !tag.is_empty())
            .collect::<Vec<_>>();
        tags.push("llm_post_run".to_string());
        tags.push(memory_type_label(draft.memory_type).to_string());
        tags.sort();
        tags.dedup();

        memories.push(MemoryRecord {
            memory_id: format!(
                "llm-post-run:{}:{}:{}",
                input.task_id,
                memory_type_label(draft.memory_type),
                &content_hash[..12.min(content_hash.len())]
            ),
            context_key: input.scope.context_key.clone(),
            source_episode_id: Some(input.task_id.to_string()),
            memory_type: draft.memory_type,
            title: truncate_chars(draft.title.trim(), MEMORY_TITLE_MAX_CHARS),
            content,
            short_description: truncate_chars(
                draft.short_description.trim(),
                MEMORY_SHORT_DESCRIPTION_MAX_CHARS,
            ),
            importance: draft.importance.clamp(0.0, 1.0),
            confidence: draft.confidence.clamp(0.0, 1.0),
            source: Some("llm_post_run_writer".to_string()),
            content_hash: Some(content_hash),
            reason: Some(truncate_chars(draft.reason.trim(), 240)),
            tags,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        });
    }

    Ok(ValidatedPostRunMemoryWrite {
        thread_short_summary: parsed
            .thread_short_summary
            .map(|value| truncate_chars(value.trim(), THREAD_SHORT_SUMMARY_MAX_CHARS))
            .filter(|value| !value.is_empty()),
        episode: ValidatedPostRunEpisode {
            summary: episode_summary,
            outcome: parsed.episode.outcome,
            failures,
            importance: parsed.episode.importance.clamp(0.0, 1.0),
        },
        memories,
    })
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}
