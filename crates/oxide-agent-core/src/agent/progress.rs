use super::loop_detection::LoopType;
use super::providers::TodoList;
use super::thoughts;
use crate::agent::compaction::{BudgetState, CompactionTrigger};
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
    /// Estimated tokens represented by loaded skill context outside hot memory.
    pub loaded_skill_tokens: usize,
    /// Total estimated input tokens for the next request.
    pub total_input_tokens: usize,
    /// Reserved output tokens for the active model.
    pub reserved_output_tokens: usize,
    /// Estimated full request size including reserves.
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
        /// Tool name
        name: String,
        /// Tool input arguments
        input: String,
        /// Human-readable preview (e.g., command for execute_command)
        command_preview: Option<String>,
    },
    /// Agent received a tool result
    ToolResult {
        /// Tool name
        name: String,
        /// Tool execution output
        output: String,
    },
    /// Agent is waiting for operator approval before continuing a tool call.
    WaitingForApproval {
        /// Tool name awaiting approval.
        tool_name: String,
        /// Infra target name shown to the operator.
        target_name: String,
        /// Human-readable approval summary.
        summary: String,
    },
    /// Agent is continuing work due to incomplete todos
    Continuation {
        /// Reason for continuation
        reason: String,
        /// Number of continuations so far
        count: usize,
    },
    /// Todos list was updated
    TodosUpdated {
        /// Updated list of tasks
        todos: TodoList,
    },
    /// File to send to user
    FileToSend {
        /// Original file name
        file_name: String,
        /// Raw file content
        #[serde(with = "serde_bytes")]
        content: Vec<u8>,
    },
    /// File to send to user with delivery confirmation
    /// Used by ytdlp provider for automatic cleanup after successful delivery
    #[serde(skip)]
    FileToSendWithConfirmation {
        /// Original file name
        file_name: String,
        /// Raw file content
        content: Vec<u8>,
        /// Path in sandbox for cleanup after success
        sandbox_path: String,
        /// Channel to receive delivery confirmation
        confirmation_tx: tokio::sync::oneshot::Sender<Result<(), String>>,
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
        /// Short summary of reasoning
        summary: String,
    },
    /// Loop detected during execution
    LoopDetected {
        /// Type of loop detected
        loop_type: LoopType,
        /// Iteration when detected
        iteration: usize,
    },
    /// Narrative update from sidecar LLM
    Narrative {
        /// Short action-oriented headline
        headline: String,
        /// Detailed context explanation
        content: String,
    },
    /// Compaction pipeline started for the current context.
    CompactionStarted {
        /// Trigger that invoked compaction.
        trigger: CompactionTrigger,
    },
    /// Deterministic pruning removed stale artifacts before summary compaction.
    PruningApplied {
        /// Number of pruned hot-memory artifacts.
        pruned_count: usize,
        /// Estimated reclaimed tokens.
        reclaimed_tokens: usize,
    },
    /// Compaction pipeline completed.
    CompactionCompleted {
        /// Trigger that invoked compaction.
        trigger: CompactionTrigger,
        /// Whether hot memory changed.
        applied: bool,
        /// Number of newly externalized artifacts.
        externalized_count: usize,
        /// Number of newly pruned artifacts.
        pruned_count: usize,
        /// Total hot-memory tokens reclaimed by the full checkpoint.
        reclaimed_tokens: usize,
        /// Number of archived cold-context chunks.
        archived_chunk_count: usize,
        /// Whether a structured summary entry was refreshed.
        summary_updated: bool,
    },
    /// Compaction failed before the run could continue.
    CompactionFailed {
        /// Trigger that invoked compaction.
        trigger: CompactionTrigger,
        /// Human-readable failure message.
        error: String,
    },
    /// Warning that the same run needed multiple compaction passes.
    RepeatedCompactionWarning {
        /// Which kind of repeated maintenance triggered the warning.
        kind: RepeatedCompactionKind,
        /// Number of applied compaction passes in the current run.
        count: usize,
    },
}

/// User-facing class of repeated context maintenance activity.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RepeatedCompactionKind {
    /// Repeated deterministic cleanup of bulky artifacts.
    Cleanup,
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
    /// Narrative headline from sidecar LLM
    pub narrative_headline: Option<String>,
    /// Narrative content from sidecar LLM
    pub narrative_content: Option<String>,
    /// Latest compaction status shown to the operator.
    pub last_compaction_status: Option<String>,
    /// Warning shown when the same run keeps compacting repeatedly.
    pub repeated_compaction_warning: Option<String>,
    /// Latest request-side token budget snapshot.
    pub latest_token_snapshot: Option<TokenSnapshot>,
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

struct CompactionCompletionDetails {
    trigger: CompactionTrigger,
    applied: bool,
    externalized_count: usize,
    pruned_count: usize,
    reclaimed_tokens: usize,
    archived_chunk_count: usize,
    summary_updated: bool,
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
            AgentEvent::Thinking { snapshot } => self.handle_thinking(snapshot),
            AgentEvent::TokenSnapshotUpdated { snapshot } => {
                self.handle_token_snapshot_updated(snapshot)
            }
            AgentEvent::ToolCall {
                name,
                input,
                command_preview,
            } => self.handle_tool_call(name, input, command_preview),
            AgentEvent::ToolResult { .. } => self.complete_last_step(),
            AgentEvent::WaitingForApproval {
                tool_name,
                target_name,
                summary,
            } => self.handle_waiting_for_approval(tool_name, target_name, summary),
            AgentEvent::Continuation { reason, count } => self.handle_continuation(reason, count),
            AgentEvent::TodosUpdated { todos } => self.handle_todos_update(todos),
            AgentEvent::FileToSend { file_name, .. } => self.handle_file_send(file_name),
            AgentEvent::FileToSendWithConfirmation { file_name, .. } => {
                self.handle_file_send(file_name)
            }
            AgentEvent::Finished => self.handle_finish(),
            AgentEvent::Cancelling { tool_name } => self.handle_cancelling(tool_name),
            AgentEvent::Cancelled => self.handle_cancelled(),
            AgentEvent::Error(e) => self.handle_error(e),
            AgentEvent::Reasoning { summary } => self.handle_reasoning(summary),
            AgentEvent::LoopDetected {
                loop_type,
                iteration,
            } => self.handle_loop_detected(loop_type, iteration),
            AgentEvent::Narrative { headline, content } => self.handle_narrative(headline, content),
            AgentEvent::CompactionStarted { trigger } => self.handle_compaction_started(trigger),
            AgentEvent::PruningApplied {
                pruned_count,
                reclaimed_tokens,
            } => self.handle_pruning_applied(pruned_count, reclaimed_tokens),
            AgentEvent::CompactionCompleted {
                trigger,
                applied,
                externalized_count,
                pruned_count,
                reclaimed_tokens,
                archived_chunk_count,
                summary_updated,
            } => self.handle_compaction_completed(CompactionCompletionDetails {
                trigger,
                applied,
                externalized_count,
                pruned_count,
                reclaimed_tokens,
                archived_chunk_count,
                summary_updated,
            }),
            AgentEvent::CompactionFailed { trigger, error } => {
                self.handle_compaction_failed(trigger, error)
            }
            AgentEvent::RepeatedCompactionWarning { kind, count } => {
                self.handle_repeated_compaction_warning(kind, count)
            }
        }
    }

    /// Helper: Complete the last in-progress step
    fn complete_last_step(&mut self) {
        if let Some(last) = self.steps.last_mut() {
            if last.status == StepStatus::InProgress {
                last.status = StepStatus::Completed;
            }
        }
    }

    /// Helper: Mark the last in-progress step as failed
    fn fail_last_step(&mut self) {
        if let Some(last) = self.steps.last_mut() {
            if last.status == StepStatus::InProgress {
                last.status = StepStatus::Failed;
            }
        }
    }

    fn handle_thinking(&mut self, snapshot: TokenSnapshot) {
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
        });
    }

    fn handle_token_snapshot_updated(&mut self, snapshot: TokenSnapshot) {
        self.latest_token_snapshot = Some(snapshot.clone());
        if let Some(last) = self.steps.last_mut() {
            last.tokens = Some(snapshot.hot_memory_tokens);
        }
    }

    fn handle_tool_call(&mut self, name: String, input: String, command_preview: Option<String>) {
        self.complete_last_step();

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
        });
    }

    fn handle_continuation(&mut self, reason: String, count: usize) {
        self.complete_last_step();
        self.steps.push(Step {
            description: format!(
                "🔄 Continuation ({}/{}): {}",
                count,
                crate::config::AGENT_CONTINUATION_LIMIT,
                crate::utils::truncate_str(reason, 50)
            ),
            status: StepStatus::InProgress,
            tokens: None,
            tool_name: None,
        });
    }

    fn handle_waiting_for_approval(
        &mut self,
        tool_name: String,
        target_name: String,
        summary: String,
    ) {
        self.complete_last_step();
        self.current_thought = Some(format!("Waiting for SSH approval for {tool_name}"));
        self.steps.push(Step {
            description: format!(
                "SSH approval pending for {}: {}",
                target_name,
                crate::utils::truncate_str(&summary, 80)
            ),
            status: StepStatus::InProgress,
            tokens: None,
            tool_name: None,
        });
    }

    fn handle_todos_update(&mut self, todos: TodoList) {
        let current_task = todos.current_task().map(|t| t.description.clone());
        let completed = todos.completed_count();
        let total = todos.items.len();

        self.current_todos = Some(todos);

        if let Some(task) = current_task {
            // Update step description with current task
            if let Some(last) = self.steps.last_mut() {
                if last.status == StepStatus::InProgress {
                    last.description = format!("📋 {task} ({completed}/{total})");
                }
            }
        }
    }

    fn handle_file_send(&mut self, file_name: String) {
        self.steps.push(Step {
            description: format!("📤 File send: {file_name}"),
            status: StepStatus::Completed,
            tokens: None,
            tool_name: Some("file_send".to_string()),
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

    fn handle_narrative(&mut self, headline: String, content: String) {
        self.narrative_headline = Some(headline);
        self.narrative_content = Some(content);
    }

    fn handle_compaction_started(&mut self, trigger: CompactionTrigger) {
        self.complete_last_step();
        self.current_thought = Some("Compressing context to preserve task continuity.".to_string());
        self.steps.push(Step {
            description: format!(
                "🗜 Compacting context ({})",
                compaction_trigger_label(trigger)
            ),
            status: StepStatus::InProgress,
            tokens: None,
            tool_name: None,
        });
    }

    fn handle_pruning_applied(&mut self, pruned_count: usize, reclaimed_tokens: usize) {
        self.last_compaction_status = Some(format!(
            "Cleanup: pruned {pruned_count} {} - reclaimed ~{}.",
            pluralize(pruned_count, "old artifact", "old artifacts"),
            crate::utils::format_tokens(reclaimed_tokens)
        ));
    }

    fn handle_compaction_completed(&mut self, details: CompactionCompletionDetails) {
        self.complete_last_step();
        self.last_compaction_status = Some(if details.applied {
            if details.summary_updated {
                format!(
                    "Compaction: refreshed summary and rebuilt active context - reclaimed ~{}.",
                    crate::utils::format_tokens(details.reclaimed_tokens)
                )
            } else {
                let cleanup_label = match (details.externalized_count, details.pruned_count) {
                    (externalized, 0) if externalized > 0 => format!(
                        "externalized {externalized} {}",
                        pluralize(externalized, "large tool result", "large tool results")
                    ),
                    (0, pruned) if pruned > 0 => format!(
                        "pruned {pruned} {}",
                        pluralize(pruned, "old artifact", "old artifacts")
                    ),
                    (externalized, pruned) if externalized > 0 && pruned > 0 => format!(
                        "externalized {externalized} {} and pruned {pruned} {}",
                        pluralize(externalized, "large tool result", "large tool results"),
                        pluralize(pruned, "old artifact", "old artifacts")
                    ),
                    _ if details.archived_chunk_count > 0 => format!(
                        "archived {} {}",
                        details.archived_chunk_count,
                        pluralize(
                            details.archived_chunk_count,
                            "context chunk",
                            "context chunks"
                        )
                    ),
                    _ => "updated hot context".to_string(),
                };
                format!(
                    "Cleanup: {cleanup_label} - reclaimed ~{}.",
                    crate::utils::format_tokens(details.reclaimed_tokens)
                )
            }
        } else {
            format!(
                "Compaction checked context ({}) - no changes were needed.",
                compaction_trigger_label(details.trigger)
            )
        });
    }

    fn handle_compaction_failed(&mut self, trigger: CompactionTrigger, error: String) {
        self.last_compaction_status = Some(format!(
            "Compaction failed ({}) - {}",
            compaction_trigger_label(trigger),
            error
        ));
        self.error = Some(format!("Compaction failed: {error}"));
        self.fail_last_step();
    }

    fn handle_repeated_compaction_warning(&mut self, kind: RepeatedCompactionKind, count: usize) {
        self.repeated_compaction_warning = Some(match kind {
            RepeatedCompactionKind::Cleanup => format!("Cleanup repeated: {count}x"),
            RepeatedCompactionKind::Compaction => format!("History compaction: {count}x"),
        });
    }

    // Formatting is handled in the UI layer.
}

fn compaction_trigger_label(trigger: CompactionTrigger) -> &'static str {
    match trigger {
        CompactionTrigger::PreRun => "pre-run",
        CompactionTrigger::PreIteration => "pre-iteration",
        CompactionTrigger::Manual => "manual",
    }
}

fn pluralize<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 {
        singular
    } else {
        plural
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentEvent, ProgressState, RepeatedCompactionKind};
    use crate::agent::compaction::CompactionTrigger;

    #[test]
    fn compaction_events_update_progress_state() {
        let mut state = ProgressState::new(5);

        state.update(AgentEvent::CompactionStarted {
            trigger: CompactionTrigger::PreRun,
        });
        state.update(AgentEvent::PruningApplied {
            pruned_count: 2,
            reclaimed_tokens: 1200,
        });
        state.update(AgentEvent::CompactionCompleted {
            trigger: CompactionTrigger::PreRun,
            applied: true,
            externalized_count: 1,
            pruned_count: 2,
            reclaimed_tokens: 1800,
            archived_chunk_count: 1,
            summary_updated: true,
        });

        assert_eq!(state.steps.len(), 1);
        assert_eq!(state.steps[0].status, super::StepStatus::Completed);
        assert!(state
            .last_compaction_status
            .as_deref()
            .is_some_and(|status| status
                .contains("Compaction: refreshed summary and rebuilt active context")));
    }

    #[test]
    fn cleanup_events_render_cleanup_labels() {
        let mut state = ProgressState::new(5);

        state.update(AgentEvent::CompactionStarted {
            trigger: CompactionTrigger::PreIteration,
        });
        state.update(AgentEvent::CompactionCompleted {
            trigger: CompactionTrigger::PreIteration,
            applied: true,
            externalized_count: 1,
            pruned_count: 0,
            reclaimed_tokens: 797,
            archived_chunk_count: 0,
            summary_updated: false,
        });

        assert_eq!(
            state.last_compaction_status.as_deref(),
            Some("Cleanup: externalized 1 large tool result - reclaimed ~797.")
        );
    }

    #[test]
    fn pruning_events_render_cleanup_labels() {
        let mut state = ProgressState::new(5);

        state.update(AgentEvent::PruningApplied {
            pruned_count: 3,
            reclaimed_tokens: 2100,
        });

        assert_eq!(
            state.last_compaction_status.as_deref(),
            Some("Cleanup: pruned 3 old artifacts - reclaimed ~2.1k.")
        );
    }

    #[test]
    fn repeated_compaction_warning_is_preserved() {
        let mut state = ProgressState::new(5);
        state.update(AgentEvent::RepeatedCompactionWarning {
            kind: RepeatedCompactionKind::Cleanup,
            count: 3,
        });

        assert!(state
            .repeated_compaction_warning
            .as_deref()
            .is_some_and(|warning| warning == "Cleanup repeated: 3x"));
    }
}
