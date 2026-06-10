use super::loop_detection::LoopType;
use super::providers::TodoList;
use super::thoughts;
use crate::agent::compaction::{BudgetState, CompactionBackend, CompactionPhase, CompactionReason};
use crate::llm::TokenUsage;
use serde::{Deserialize, Serialize};

/// Snapshot of the agent's current request-side token budget.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenSnapshot {
    /// Estimated tokens represented by the current hot memory only.
    pub hot_memory_tokens: usize,
    /// Estimated tokens represented by the rendered system prompt.
    pub system_prompt_tokens: usize,
    /// Estimated tokens represented by serialized tool schemas.
    pub tool_schema_tokens: usize,
    /// Total estimated input tokens for the next request.
    pub total_input_tokens: usize,
    /// Output tokens pre-reserved from the input window. Kept for API compatibility; currently zero.
    pub reserved_output_tokens: usize,
    /// Additional hard safety buffer kept free outside model completion reserve.
    #[serde(default)]
    pub hard_reserve_tokens: usize,
    /// Estimated input-side request size including the hard reserve.
    pub projected_total_tokens: usize,
    /// Effective model context window configured for the session.
    pub context_window_tokens: usize,
    /// Remaining headroom in the configured context window.
    pub headroom_tokens: usize,
    /// High-level request budget state.
    pub budget_state: BudgetState,
    /// Last request-scoped token usage reported by the API.
    pub last_api_usage: Option<TokenUsage>,
}

/// Preferred delivery kind for a file emitted by the agent.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FileDeliveryKind {
    /// Let the transport infer the best delivery method from the file itself.
    #[default]
    Auto,
    /// Deliver the file as a regular audio attachment when possible.
    Audio,
    /// Deliver the file as a Telegram voice note when possible.
    VoiceNote,
    /// Deliver the file as a plain document.
    Document,
}

/// Optional transport receipt for a delivered file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub struct FileDeliveryReceipt {
    /// Transport-scoped identifier for the delivered file, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
    /// Auth-protected download URL that the user can open, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_url: Option<String>,
}

/// Execution source for events that can be relayed from delegated sub-agents.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentEventSource {
    /// Event came from the root agent task.
    #[default]
    Root,
    /// Event came from a delegated sub-agent.
    SubAgent,
}

impl AgentEventSource {
    /// Stable string value used in transport payloads.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::SubAgent => "sub_agent",
        }
    }
}

/// Events that can occur during agent execution
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEvent {
    /// Agent is thinking about the next step
    Thinking {
        /// Current request-side token snapshot
        snapshot: TokenSnapshot,
    },
    /// Token snapshot was refreshed without starting a new iteration.
    TokenSnapshotUpdated {
        /// Current request-side token snapshot.
        snapshot: TokenSnapshot,
    },
    /// Agent is calling a tool
    ToolCall {
        /// Stable runtime invocation identifier for pairing call/result events.
        #[serde(default)]
        id: String,
        /// Root or delegated sub-agent source.
        #[serde(default)]
        source: AgentEventSource,
        /// Tool name
        name: String,
        /// Tool input arguments
        input: String,
        /// Human-readable preview (e.g., command for execute_command)
        command_preview: Option<String>,
    },
    /// Agent received a tool result
    ToolResult {
        /// Stable runtime invocation identifier for pairing call/result events.
        #[serde(default)]
        id: String,
        /// Root or delegated sub-agent source.
        #[serde(default)]
        source: AgentEventSource,
        /// Tool name
        name: String,
        /// Tool execution output
        output: String,
        /// Whether the tool finished successfully.
        success: bool,
    },
    /// Agent is continuing work due to incomplete todos
    Continuation {
        /// Root or delegated sub-agent source.
        #[serde(default)]
        source: AgentEventSource,
        /// Reason for continuation
        reason: String,
        /// Number of continuations so far
        count: usize,
    },
    /// Todos list was updated
    TodosUpdated {
        /// Root or delegated sub-agent source.
        #[serde(default)]
        source: AgentEventSource,
        /// Updated list of tasks
        todos: TodoList,
    },
    /// File to send to user
    FileToSend {
        /// Preferred delivery kind for the file.
        #[serde(default)]
        kind: FileDeliveryKind,
        /// Original file name
        file_name: String,
        /// Raw file content
        content: Vec<u8>,
    },
    /// File to send to user with delivery confirmation
    /// Used by ytdlp provider for automatic cleanup after successful delivery
    #[serde(skip)]
    FileToSendWithConfirmation {
        /// Preferred delivery kind for the file.
        kind: FileDeliveryKind,
        /// Original file name
        file_name: String,
        /// Raw file content
        content: Vec<u8>,
        /// Source path for diagnostics and cleanup logging
        source_path: String,
        /// Channel to receive delivery confirmation
        confirmation_tx: tokio::sync::oneshot::Sender<Result<FileDeliveryReceipt, String>>,
    },
    /// Agent has finished the task
    Finished,
    /// Agent is being cancelled (cleanup in progress)
    Cancelling {
        /// Tool that was interrupted
        tool_name: String,
    },
    /// Agent was cancelled by user
    Cancelled,
    /// Agent encountered an error
    Error(String),
    /// Agent's reasoning/thinking process (for models that support it)
    Reasoning {
        /// Root or delegated sub-agent source.
        #[serde(default)]
        source: AgentEventSource,
        /// Short summary of reasoning
        summary: String,
    },
    /// Strict research verifier decision/audit trace.
    ResearchVerification {
        /// Compact structured verifier trace payload.
        payload: serde_json::Value,
    },
    /// Candidate final draft was generated and is about to be checked before delivery.
    FinalDraftPendingVerification {
        /// Root or delegated sub-agent source.
        #[serde(default)]
        source: AgentEventSource,
        /// Character count of the withheld draft. The draft text itself is not exposed.
        content_chars: usize,
        /// Current verifier-guided round number.
        round: usize,
    },
    /// Strict research verifier sidecar started checking a candidate final draft.
    ResearchVerificationStarted {
        /// Current verifier-guided round number.
        round: usize,
        /// Maximum verifier-guided rounds before proof-not-found mode.
        max_rounds: usize,
        /// Whether the verifier is checking a constrained proof-not-found report.
        proof_not_found_mode: bool,
        /// Number of evidence documents sent to the verifier.
        evidence_document_count: usize,
    },
    /// Event relayed from a named delegated sub-agent.
    SubAgent {
        /// Stable sub-agent job id.
        sub_agent_id: String,
        /// Human-readable sub-agent display name.
        sub_agent_name: String,
        /// Original sub-agent event.
        event: Box<AgentEvent>,
    },
    /// Loop detected during execution
    LoopDetected {
        /// Type of loop detected
        loop_type: LoopType,
        /// Iteration when detected
        iteration: usize,
    },
    /// Runtime/session-level compaction started.
    RuntimeCompactionStarted {
        /// Why compaction was requested.
        reason: CompactionReason,
        /// Runtime phase where compaction is running.
        phase: CompactionPhase,
        /// Summary backend used.
        backend: CompactionBackend,
        /// Provider selected when known.
        provider: Option<String>,
        /// Model/route selected when known.
        route: Option<String>,
        /// Approximate hot-memory tokens before compaction.
        token_before: usize,
        /// Hot-memory item count before compaction.
        history_items_before: usize,
    },
    /// Runtime/session-level compaction completed.
    RuntimeCompactionCompleted {
        /// Why compaction was requested.
        reason: CompactionReason,
        /// Runtime phase where compaction ran.
        phase: CompactionPhase,
        /// Summary backend used.
        backend: CompactionBackend,
        /// Provider used for summary generation.
        provider: String,
        /// Model/route used for summary generation.
        route: String,
        /// Approximate hot-memory tokens before compaction.
        token_before: usize,
        /// Approximate hot-memory tokens after replacement.
        token_after: usize,
        /// Hot-memory item count before compaction.
        history_items_before: usize,
        /// Hot-memory item count after replacement.
        history_items_after: usize,
        /// Compacted-summary generation.
        generation: u32,
        /// Whether history repair changed replacement output.
        repair_applied: bool,
    },
    /// Runtime/session-level compaction failed before mutation or continuation.
    RuntimeCompactionFailed {
        /// Why compaction was requested.
        reason: CompactionReason,
        /// Runtime phase where compaction ran.
        phase: CompactionPhase,
        /// Summary backend used.
        backend: CompactionBackend,
        /// Provider selected when known.
        provider: Option<String>,
        /// Model/route selected when known.
        route: Option<String>,
        /// Human-readable failure message.
        error: String,
    },
    /// Runtime/session-level compaction was skipped.
    RuntimeCompactionSkipped {
        /// Why compaction was considered.
        reason: CompactionReason,
        /// Runtime phase where compaction was considered.
        phase: CompactionPhase,
        /// Human-readable skipped reason.
        skipped_reason: String,
    },
    /// Warning that the same run needed multiple compaction passes.
    RepeatedCompactionWarning {
        /// Which kind of repeated maintenance triggered the warning.
        kind: RepeatedCompactionKind,
        /// Number of applied compaction passes in the current run.
        count: usize,
    },
    /// Local tool-history repair rewrote invalid messages before retrying.
    HistoryRepairApplied {
        /// Provider handling the current request.
        provider: String,
        /// Whether the provider requires strict tool-call/result matching.
        strict_tool_history: bool,
        /// Number of invalid tool result messages removed.
        dropped_tool_results: usize,
        /// Number of tool calls trimmed out of assistant batches.
        trimmed_tool_calls: usize,
        /// Number of assistant tool-call messages converted to plain assistant text.
        converted_tool_call_messages: usize,
        /// Number of assistant tool-call messages dropped entirely.
        dropped_tool_call_messages: usize,
    },
    /// Rate limit hit, retrying with backoff.
    RateLimitRetrying {
        /// Current attempt number (starts at 1)
        attempt: usize,
        /// Maximum number of retry attempts
        max_attempts: usize,
        /// Whether retries are intentionally unbounded for this provider/error.
        #[serde(default)]
        unbounded: bool,
        /// Wait time in seconds before next attempt (if known)
        wait_secs: Option<u64>,
        /// Provider name for display
        provider: String,
    },
    /// Non-rate-limit retryable error, retrying with backoff.
    LlmRetrying {
        /// Current attempt number (starts at 1)
        attempt: usize,
        /// Maximum number of retry attempts
        max_attempts: usize,
        /// Whether retries are intentionally unbounded for this provider/error.
        #[serde(default)]
        unbounded: bool,
        /// Wait time in seconds before next attempt
        wait_secs: Option<u64>,
        /// Provider name for display
        provider: String,
        /// Error class (e.g. "network", "timeout", "server_error")
        error_class: String,
    },
    /// LLM routing switched to a fallback provider after persistent rate limits.
    ProviderFailoverActivated {
        /// Previous provider name.
        from_provider: String,
        /// Previous model identifier.
        from_model: String,
        /// New provider name.
        to_provider: String,
        /// New model identifier.
        to_model: String,
    },
    /// Execution milestone for latency tracking.
    Milestone {
        /// Milestone name (e.g., "executor_lock_acquired", "thinking_sent", "llm_call_started")
        name: String,
        /// Timestamp when milestone was reached (Unix timestamp in milliseconds)
        timestamp_ms: i64,
    },
}

impl AgentEvent {
    /// Mark visible delegated progress before relaying it into the parent stream.
    #[must_use]
    pub fn with_sub_agent_source(self, sub_agent_id: String, sub_agent_name: String) -> Self {
        Self::SubAgent {
            sub_agent_id,
            sub_agent_name,
            event: Box::new(self.with_sub_agent_source_marker()),
        }
    }

    fn with_sub_agent_source_marker(self) -> Self {
        match self {
            Self::ToolCall {
                id,
                name,
                input,
                command_preview,
                ..
            } => Self::ToolCall {
                id,
                source: AgentEventSource::SubAgent,
                name,
                input,
                command_preview,
            },
            Self::ToolResult {
                id,
                name,
                output,
                success,
                ..
            } => Self::ToolResult {
                id,
                source: AgentEventSource::SubAgent,
                name,
                output,
                success,
            },
            Self::Continuation { reason, count, .. } => Self::Continuation {
                source: AgentEventSource::SubAgent,
                reason,
                count,
            },
            Self::TodosUpdated { todos, .. } => Self::TodosUpdated {
                source: AgentEventSource::SubAgent,
                todos,
            },
            Self::Reasoning { summary, .. } => Self::Reasoning {
                source: AgentEventSource::SubAgent,
                summary,
            },
            Self::FinalDraftPendingVerification {
                content_chars,
                round,
                ..
            } => Self::FinalDraftPendingVerification {
                source: AgentEventSource::SubAgent,
                content_chars,
                round,
            },
            other => other,
        }
    }
}

/// User-facing class of repeated context maintenance activity.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RepeatedCompactionKind {
    /// Repeated summary/rebuild passes over older history.
    Compaction,
}

/// Current state of the agent's progress
#[derive(Debug, Clone, Default)]
pub struct ProgressState {
    /// Index of current iteration
    pub current_iteration: usize,
    /// Maximum allowed iterations
    pub max_iterations: usize,
    /// List of steps executed so far
    pub steps: Vec<Step>,
    /// Optional list of todos/tasks
    pub current_todos: Option<TodoList>,
    /// Whether the agent has finished
    pub is_finished: bool,
    /// Optional error message
    pub error: Option<String>,
    /// Current agent thought/reasoning
    pub current_thought: Option<String>,
    /// Latest compaction status shown to the operator.
    pub last_compaction_status: Option<String>,
    /// Warning shown when the same run keeps compacting repeatedly.
    pub repeated_compaction_warning: Option<String>,
    /// Latest request-side token budget snapshot.
    pub latest_token_snapshot: Option<TokenSnapshot>,
    /// Latest status for automatic tool-history repair.
    pub last_history_repair_status: Option<String>,
    /// Current LLM retry status (cleared on success or final error)
    pub llm_retry: Option<LlmRetryState>,
    /// Latest provider failover notice for the current run.
    pub provider_failover_notice: Option<String>,
    /// Whether the loop-detected modal was already surfaced for this run.
    pub loop_notification_sent: bool,
}

/// State for LLM retry display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmRetryState {
    /// Current attempt number (starts at 1)
    pub attempt: usize,
    /// Maximum number of retry attempts
    pub max_attempts: usize,
    /// Whether retries are intentionally unbounded for this provider/error.
    #[serde(default)]
    pub unbounded: bool,
    /// Wait time in seconds before next attempt (if known)
    pub wait_secs: Option<u64>,
    /// Provider name for display
    pub provider: String,
    /// Error class for non-rate-limit retryable LLM errors.
    #[serde(default)]
    pub error_class: Option<String>,
}

/// A single step in the agent's execution process
#[derive(Debug, Clone)]
pub struct Step {
    /// Human-readable description of the step
    pub description: String,
    /// Current status of the step
    pub status: StepStatus,
    /// Optional token count at this step
    pub tokens: Option<usize>,
    /// Tool name for grouping (None for non-tool steps like Thinking)
    pub tool_name: Option<String>,
    /// Tool invocation id for pairing concurrent calls/results.
    pub tool_id: Option<String>,
}

/// Possible statuses for an execution step
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepStatus {
    /// Step is waiting to be executed
    Pending,
    /// Step is currently being executed
    InProgress,
    /// Step was completed successfully
    Completed,
    /// Step failed
    Failed,
}

struct RuntimeCompactionStartedDetails {
    reason: CompactionReason,
    phase: CompactionPhase,
    backend: CompactionBackend,
    provider: Option<String>,
    route: Option<String>,
    token_before: usize,
    history_items_before: usize,
}

struct RuntimeCompactionCompletedDetails {
    reason: CompactionReason,
    phase: CompactionPhase,
    backend: CompactionBackend,
    provider: String,
    route: String,
    token_before: usize,
    token_after: usize,
    history_items_before: usize,
    history_items_after: usize,
    generation: u32,
    repair_applied: bool,
}

struct RuntimeCompactionFailedDetails {
    reason: CompactionReason,
    phase: CompactionPhase,
    backend: CompactionBackend,
    provider: Option<String>,
    route: Option<String>,
    error: String,
}

impl ProgressState {
    /// Creates a new empty progress state
    #[must_use]
    pub fn new(max_iterations: usize) -> Self {
        Self {
            max_iterations,
            ..Default::default()
        }
    }

    /// Updates the progress state based on an agent event
    pub fn update(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::SubAgent { event, .. } => self.update(*event),
            AgentEvent::Thinking { snapshot } => self.handle_thinking(snapshot),
            AgentEvent::TokenSnapshotUpdated { snapshot } => {
                self.handle_token_snapshot_updated(snapshot)
            }
            AgentEvent::ToolCall {
                id,
                name,
                input,
                command_preview,
                ..
            } => self.handle_tool_call(id, name, input, command_preview),
            AgentEvent::ToolResult { id, success, .. } => self.handle_tool_result(&id, success),
            AgentEvent::Continuation { reason, count, .. } => {
                self.handle_continuation(reason, count)
            }
            AgentEvent::TodosUpdated { todos, .. } => self.handle_todos_update(todos),
            AgentEvent::FileToSend { file_name, .. } => self.handle_file_send(file_name),
            AgentEvent::FileToSendWithConfirmation { file_name, .. } => {
                self.handle_file_send(file_name)
            }
            AgentEvent::Finished => self.handle_finish(),
            AgentEvent::Cancelling { tool_name } => self.handle_cancelling(tool_name),
            AgentEvent::Cancelled => self.handle_cancelled(),
            AgentEvent::Error(e) => self.handle_error(e),
            AgentEvent::Reasoning { summary, .. } => self.handle_reasoning(summary),
            AgentEvent::ResearchVerification { payload } => {
                tracing::info!(payload = %payload, "Research verification trace")
            }
            AgentEvent::FinalDraftPendingVerification { .. }
            | AgentEvent::ResearchVerificationStarted { .. } => {}
            AgentEvent::LoopDetected {
                loop_type,
                iteration,
            } => self.handle_loop_detected(loop_type, iteration),
            AgentEvent::RuntimeCompactionStarted {
                reason,
                phase,
                backend,
                provider,
                route,
                token_before,
                history_items_before,
            } => self.handle_runtime_compaction_started(RuntimeCompactionStartedDetails {
                reason,
                phase,
                backend,
                provider,
                route,
                token_before,
                history_items_before,
            }),
            AgentEvent::RuntimeCompactionCompleted {
                reason,
                phase,
                backend,
                provider,
                route,
                token_before,
                token_after,
                history_items_before,
                history_items_after,
                generation,
                repair_applied,
            } => self.handle_runtime_compaction_completed(RuntimeCompactionCompletedDetails {
                reason,
                phase,
                backend,
                provider,
                route,
                token_before,
                token_after,
                history_items_before,
                history_items_after,
                generation,
                repair_applied,
            }),
            AgentEvent::RuntimeCompactionFailed {
                reason,
                phase,
                backend,
                provider,
                route,
                error,
            } => self.handle_runtime_compaction_failed(RuntimeCompactionFailedDetails {
                reason,
                phase,
                backend,
                provider,
                route,
                error,
            }),
            AgentEvent::RuntimeCompactionSkipped {
                reason,
                phase,
                skipped_reason,
            } => self.handle_runtime_compaction_skipped(reason, phase, skipped_reason),
            AgentEvent::RepeatedCompactionWarning { kind, count } => {
                self.handle_repeated_compaction_warning(kind, count)
            }
            AgentEvent::HistoryRepairApplied {
                provider,
                strict_tool_history,
                dropped_tool_results,
                trimmed_tool_calls,
                converted_tool_call_messages,
                dropped_tool_call_messages,
            } => self.handle_history_repair_applied(
                provider,
                strict_tool_history,
                dropped_tool_results,
                trimmed_tool_calls,
                converted_tool_call_messages,
                dropped_tool_call_messages,
            ),
            AgentEvent::RateLimitRetrying {
                attempt,
                max_attempts,
                unbounded,
                wait_secs,
                provider,
            } => self.handle_rate_limit_retrying(
                attempt,
                max_attempts,
                unbounded,
                wait_secs,
                provider,
            ),
            AgentEvent::LlmRetrying {
                attempt,
                max_attempts,
                unbounded,
                wait_secs,
                provider,
                error_class,
            } => self.handle_llm_retrying(
                attempt,
                max_attempts,
                unbounded,
                wait_secs,
                provider,
                error_class,
            ),
            AgentEvent::ProviderFailoverActivated {
                from_provider,
                from_model,
                to_provider,
                to_model,
            } => self.handle_provider_failover(from_provider, from_model, to_provider, to_model),
            AgentEvent::Milestone { name, timestamp_ms } => {
                tracing::debug!(milestone = %name, timestamp_ms, "Execution milestone reached");
            }
        }
    }

    /// Helper: Complete the last in-progress step
    fn complete_last_step(&mut self) {
        if let Some(last) = self.steps.last_mut()
            && last.status == StepStatus::InProgress
        {
            last.status = StepStatus::Completed;
        }
    }

    /// Helper: Mark the last in-progress step as failed
    fn fail_last_step(&mut self) {
        if let Some(last) = self.steps.last_mut()
            && last.status == StepStatus::InProgress
        {
            last.status = StepStatus::Failed;
        }
    }

    fn handle_thinking(&mut self, snapshot: TokenSnapshot) {
        // Clear any active LLM retry display: the agent is back to work,
        // so the user should no longer see the "retrying" banner.
        self.llm_retry = None;
        self.provider_failover_notice = None;
        self.current_iteration += 1;
        self.complete_last_step();
        self.latest_token_snapshot = Some(snapshot.clone());
        self.steps.push(Step {
            description: format!(
                "Task analysis (iteration {}/{})",
                self.current_iteration, self.max_iterations
            ),
            status: StepStatus::InProgress,
            tokens: Some(snapshot.hot_memory_tokens),
            tool_name: None,
            tool_id: None,
        });
    }

    fn handle_token_snapshot_updated(&mut self, snapshot: TokenSnapshot) {
        self.latest_token_snapshot = Some(snapshot.clone());
        if let Some(last) = self.steps.last_mut() {
            last.tokens = Some(snapshot.hot_memory_tokens);
        }
    }

    fn handle_tool_call(
        &mut self,
        id: String,
        name: String,
        input: String,
        command_preview: Option<String>,
    ) {
        if !self
            .steps
            .last()
            .is_some_and(|step| step.status == StepStatus::InProgress && step.tool_name.is_some())
        {
            self.complete_last_step();
        }

        // Try to infer a human-readable thought from tool call
        let inferred_thought = thoughts::infer_thought(&name, &input);

        // Update current thought with inferred thought or command preview
        if let Some(ref thought) = inferred_thought {
            self.current_thought = Some(thought.clone());
        } else if let Some(ref preview) = command_preview {
            self.current_thought = Some(thoughts::infer_thought_from_command(preview));
        }

        // Use command preview if available, otherwise show tool name
        let description = command_preview
            .map(|preview| format!("🔧 {}", crate::utils::truncate_str(preview, 60)))
            .unwrap_or_else(|| format!("Execution: {}", &name));

        self.steps.push(Step {
            description,
            status: StepStatus::InProgress,
            tokens: None,
            tool_name: Some(name),
            tool_id: Some(id),
        });
    }

    fn handle_tool_result(&mut self, id: &str, success: bool) {
        if !id.is_empty()
            && let Some(step) = self.steps.iter_mut().rev().find(|step| {
                step.status == StepStatus::InProgress && step.tool_id.as_deref() == Some(id)
            })
        {
            step.status = if success {
                StepStatus::Completed
            } else {
                StepStatus::Failed
            };
            return;
        }

        if success {
            self.complete_last_step();
        } else {
            self.fail_last_step();
        }
    }

    fn handle_continuation(&mut self, reason: String, count: usize) {
        self.complete_last_step();
        self.steps.push(Step {
            description: format!(
                "🔄 Continuation ({}/{}): {}",
                count,
                crate::config::get_agent_continuation_limit(),
                crate::utils::truncate_str(reason, 50)
            ),
            status: StepStatus::InProgress,
            tokens: None,
            tool_name: None,
            tool_id: None,
        });
    }

    fn handle_todos_update(&mut self, todos: TodoList) {
        let current_task = todos
            .current_task()
            .map(|t| (t.description.clone(), false))
            .or_else(|| todos.blocked_task().map(|t| (t.description.clone(), true)));
        let completed = todos.completed_count();
        let total = todos.items.len();

        self.current_todos = Some(todos);

        if let Some((task, blocked_on_user)) = current_task {
            // Update step description with current task
            if let Some(last) = self.steps.last_mut()
                && last.status == StepStatus::InProgress
            {
                let prefix = if blocked_on_user {
                    "📋 Waiting on user"
                } else {
                    "📋"
                };
                last.description = format!("{prefix} {task} ({completed}/{total})");
            }
        }
    }

    fn handle_file_send(&mut self, file_name: String) {
        self.steps.push(Step {
            description: format!("📤 File send: {file_name}"),
            status: StepStatus::Completed,
            tokens: None,
            tool_name: Some("file_send".to_string()),
            tool_id: None,
        });
    }

    fn handle_finish(&mut self) {
        self.is_finished = true;
        self.current_thought = None; // Clear thought on finish
        for step in &mut self.steps {
            if step.status == StepStatus::InProgress {
                step.status = StepStatus::Completed;
            }
        }
    }

    fn handle_reasoning(&mut self, summary: String) {
        // Only update if reasoning is meaningful (>20 chars)
        // Otherwise keep the previous inferred thought from tool call
        if summary.len() >= 20 {
            self.current_thought = Some(summary);
        }
    }

    fn handle_cancelling(&mut self, tool_name: String) {
        // Add a step showing cancellation is in progress
        if let Some(last) = self.steps.last_mut() {
            if last.status == StepStatus::InProgress {
                last.description = format!("⏹ Cancellation: {tool_name}...");
            }
        } else {
            self.steps.push(Step {
                description: format!("⏹ Cancellation: {tool_name}..."),
                status: StepStatus::InProgress,
                tokens: None,
                tool_name: None,
                tool_id: None,
            });
        }
    }

    fn handle_cancelled(&mut self) {
        self.error = Some("Task cancelled by user".to_string());
        self.fail_last_step();
    }

    fn handle_error(&mut self, e: String) {
        self.error = Some(e);
        self.fail_last_step();
    }

    fn handle_loop_detected(&mut self, loop_type: LoopType, iteration: usize) {
        let label = match loop_type {
            LoopType::ToolCallLoop => "Recurring calls",
            LoopType::ContentLoop => "Recurring text",
            LoopType::CognitiveLoop => "Stuck",
        };
        self.error = Some(format!("Loop detected: {label} (iteration {iteration})"));
        self.fail_last_step();
    }

    fn handle_runtime_compaction_started(&mut self, details: RuntimeCompactionStartedDetails) {
        self.complete_last_step();
        self.current_thought =
            Some("Compacting session history with a local LLM summary.".to_string());
        let route = format_optional_route(details.provider.as_deref(), details.route.as_deref());
        self.steps.push(Step {
            description: format!(
                "🗜 Compacting context ({}/{}, {}, {} items, ~{})",
                compaction_reason_label(details.reason),
                compaction_phase_label(details.phase),
                compaction_backend_label(details.backend),
                details.history_items_before,
                crate::utils::format_tokens(details.token_before)
            ),
            status: StepStatus::InProgress,
            tokens: None,
            tool_name: None,
            tool_id: None,
        });
        self.last_compaction_status = Some(format!(
            "Compaction: running {} ({}/{}){}.",
            compaction_backend_label(details.backend),
            compaction_reason_label(details.reason),
            compaction_phase_label(details.phase),
            route
        ));
    }

    fn handle_runtime_compaction_completed(&mut self, details: RuntimeCompactionCompletedDetails) {
        self.complete_last_step();
        let reclaimed = details.token_before.saturating_sub(details.token_after);
        let repair_note = if details.repair_applied {
            "; history repair applied"
        } else {
            ""
        };
        self.last_compaction_status = Some(format!(
            "Compaction: compacted history ({}/{}, {}, {}/{}) - {} -> {}, {} -> {} items, reclaimed ~{}; generation {}{}.",
            compaction_reason_label(details.reason),
            compaction_phase_label(details.phase),
            compaction_backend_label(details.backend),
            details.provider,
            details.route,
            crate::utils::format_tokens(details.token_before),
            crate::utils::format_tokens(details.token_after),
            details.history_items_before,
            details.history_items_after,
            crate::utils::format_tokens(reclaimed),
            details.generation,
            repair_note
        ));
        self.last_history_repair_status = details
            .repair_applied
            .then(|| "History repair applied after compaction.".to_string());
    }

    fn handle_runtime_compaction_failed(&mut self, details: RuntimeCompactionFailedDetails) {
        let route = format_optional_route(details.provider.as_deref(), details.route.as_deref());
        self.last_compaction_status = Some(format!(
            "Compaction failed ({}/{}, {}){} - {}",
            compaction_reason_label(details.reason),
            compaction_phase_label(details.phase),
            compaction_backend_label(details.backend),
            route,
            details.error
        ));
        self.error = Some(format!("Compaction failed: {}", details.error));
        self.fail_last_step();
    }

    fn handle_runtime_compaction_skipped(
        &mut self,
        reason: CompactionReason,
        phase: CompactionPhase,
        skipped_reason: String,
    ) {
        self.last_compaction_status = Some(format!(
            "Compaction skipped ({}/{}) - {}",
            compaction_reason_label(reason),
            compaction_phase_label(phase),
            skipped_reason
        ));
    }

    fn handle_repeated_compaction_warning(&mut self, kind: RepeatedCompactionKind, count: usize) {
        self.repeated_compaction_warning = Some(match kind {
            RepeatedCompactionKind::Compaction => format!("History compaction: {count}x"),
        });
    }

    fn handle_history_repair_applied(
        &mut self,
        provider: String,
        strict_tool_history: bool,
        dropped_tool_results: usize,
        trimmed_tool_calls: usize,
        converted_tool_call_messages: usize,
        dropped_tool_call_messages: usize,
    ) {
        let mode = if strict_tool_history {
            "strict"
        } else {
            "best-effort"
        };
        self.last_history_repair_status = Some(format!(
            "History repair ({provider}, {mode}): removed {dropped_tool_results} tool results, trimmed {trimmed_tool_calls} tool calls, converted {converted_tool_call_messages}, dropped {dropped_tool_call_messages}."
        ));
        self.error = None;
    }

    fn handle_rate_limit_retrying(
        &mut self,
        attempt: usize,
        max_attempts: usize,
        unbounded: bool,
        wait_secs: Option<u64>,
        provider: String,
    ) {
        self.llm_retry = Some(LlmRetryState {
            attempt,
            max_attempts,
            unbounded,
            wait_secs,
            provider,
            error_class: None,
        });
        // Clear any previous error since we're retrying
        self.error = None;
    }

    fn handle_llm_retrying(
        &mut self,
        attempt: usize,
        max_attempts: usize,
        unbounded: bool,
        wait_secs: Option<u64>,
        provider: String,
        error_class: String,
    ) {
        self.llm_retry = Some(LlmRetryState {
            attempt,
            max_attempts,
            unbounded,
            wait_secs,
            provider,
            error_class: Some(error_class),
        });
        self.error = None;
    }

    fn handle_provider_failover(
        &mut self,
        from_provider: String,
        from_model: String,
        to_provider: String,
        to_model: String,
    ) {
        self.llm_retry = None;
        self.provider_failover_notice = Some(format!(
            "Failover: {}:{} -> {}:{}",
            from_provider, from_model, to_provider, to_model
        ));
        self.error = None;
    }

    // Formatting is handled in the UI layer.
}

fn compaction_reason_label(reason: CompactionReason) -> &'static str {
    match reason {
        CompactionReason::PreTurn => "pre-turn",
        CompactionReason::MidTurn => "mid-turn",
        CompactionReason::Manual => "manual",
        CompactionReason::ContextLimit => "context-limit",
        CompactionReason::ModelDownshift => "model-downshift",
    }
}

fn compaction_phase_label(phase: CompactionPhase) -> &'static str {
    match phase {
        CompactionPhase::PreSampling => "pre-sampling",
        CompactionPhase::MidTurn => "mid-turn",
        CompactionPhase::Manual => "manual",
        CompactionPhase::ModelSwitch => "model-switch",
    }
}

fn compaction_backend_label(backend: CompactionBackend) -> &'static str {
    backend.as_str()
}

fn format_optional_route(provider: Option<&str>, route: Option<&str>) -> String {
    match (
        provider.filter(|value| !value.is_empty()),
        route.filter(|value| !value.is_empty()),
    ) {
        (Some(provider), Some(route)) => format!(" via {provider}/{route}"),
        (Some(provider), None) => format!(" via {provider}"),
        (None, Some(route)) => format!(" via {route}"),
        (None, None) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentEvent, ProgressState, RepeatedCompactionKind};
    use crate::agent::compaction::{CompactionBackend, CompactionPhase, CompactionReason};

    #[test]
    fn runtime_compaction_events_update_progress_state() {
        let mut state = ProgressState::new(5);

        state.update(AgentEvent::RuntimeCompactionStarted {
            reason: CompactionReason::Manual,
            phase: CompactionPhase::Manual,
            backend: CompactionBackend::LocalLlmSummary,
            provider: None,
            route: None,
            token_before: 2_000,
            history_items_before: 10,
        });
        state.update(AgentEvent::RuntimeCompactionCompleted {
            reason: CompactionReason::Manual,
            phase: CompactionPhase::Manual,
            backend: CompactionBackend::LocalLlmSummary,
            provider: "mock".to_string(),
            route: "compact".to_string(),
            token_before: 2_000,
            token_after: 900,
            history_items_before: 10,
            history_items_after: 3,
            generation: 2,
            repair_applied: false,
        });

        assert_eq!(state.steps.len(), 1);
        assert_eq!(state.steps[0].status, super::StepStatus::Completed);
        assert!(
            state
                .last_compaction_status
                .as_deref()
                .is_some_and(|status| status.contains("Compaction: compacted history"))
        );
        assert!(
            state
                .last_compaction_status
                .as_deref()
                .is_some_and(|status| status.contains("manual/manual"))
        );
        assert!(
            state
                .last_compaction_status
                .as_deref()
                .is_some_and(|status| status.contains("mock/compact"))
        );
    }

    #[test]
    fn repeated_compaction_warning_is_preserved() {
        let mut state = ProgressState::new(5);
        state.update(AgentEvent::RepeatedCompactionWarning {
            kind: RepeatedCompactionKind::Compaction,
            count: 3,
        });

        assert!(
            state
                .repeated_compaction_warning
                .as_deref()
                .is_some_and(|warning| warning == "History compaction: 3x")
        );
    }
}
