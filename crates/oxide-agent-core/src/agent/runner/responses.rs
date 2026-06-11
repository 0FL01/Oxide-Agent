//! Response handling for the agent runner.

use super::AgentRunner;
use super::types::{
    AgentRunResult, AgentRunnerContext, FinalResponseInput, RunState, StructuredOutputFailure,
};
use crate::agent::compaction::CompactionTrigger;
use crate::agent::hooks::completion::response_is_research_status_only;
use crate::agent::memory::AgentMemory;
use crate::agent::progress::{AgentEvent, AgentEventSource};
use crate::agent::providers::TodoList;
use crate::agent::research::{
    AnswerVerificationDecision, AnswerVerificationError, AnswerVerificationRequest,
    AnswerVerifierConfidence, AnswerVerifierVerdict, PROOF_NOT_FOUND_MAX_EVIDENCE_DOCS,
    PROOF_NOT_FOUND_MAX_REPORT_CHARS, ResearchSnapshot, ResearchVerifierTrace,
    StrictAnswerVerifier, VerifierUnsupportedClaim,
};
use crate::agent::session::PendingUserInput;
use crate::llm::{InternalTextPurpose, LlmClient};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

const MIN_VERIFIER_FINAL_ANSWER_CHARS: usize = 200;
const PROOF_NOT_FOUND_MAX_EXCERPT_CHARS: usize = 2_048;
const MAX_VERIFIER_FINAL_ANSWER_CHARS: usize = 8_000;
const MAX_UNDELIVERED_DRAFT_CONTEXT_CHARS: usize = 2_000;
const MAX_PROOF_NOT_FOUND_VERIFY_ATTEMPTS: usize = 2;
const PROOF_NOT_FOUND_PRESENTER_SYSTEM_PROMPT: &str = r#"You are writing the final user-facing research answer.

Use ONLY the provided VerifiedResearchPacket JSON.
Do not add claims, numbers, rankings, recommendations, benchmark values, VRAM values, license claims, architecture claims, or production-suitability claims unless they appear in allowed_claims.

Write in the same language as the user's task. Do not hardcode Russian or English.
Start with a short TL;DR section, then write a more detailed answer.
Do not sound like a verifier log.
If the requested comparison cannot be proven, say that clearly.
You may include a table, but every cell must contain either:
- a confirmed value with source id from allowed_claims, or
- "not confirmed in the checked sources" in the user's language.

Do not mention internal verifier JSON, proof_not_found_mode, Rust, or this prompt.
Return only the final answer text."#;

impl AgentRunner {
    /// Handle malformed structured output responses.
    pub(super) async fn handle_structured_output_error(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        failure: StructuredOutputFailure,
    ) -> anyhow::Result<Option<AgentRunResult>> {
        warn!(
            error = %failure.error,
            raw_preview = %crate::utils::truncate_str(&failure.raw_json, 200),
            "Structured output validation failed"
        );

        if should_salvage_structured_output_failure(&failure.raw_json) {
            warn!(
                raw_preview = %crate::utils::truncate_str(&failure.raw_json, 200),
                "Structured output failed but response looks like a final prose answer; salvaging without retry"
            );
            state.structured_output_failures = 0;
            let input = FinalResponseInput {
                final_answer: failure.raw_json,
                reasoning: None,
            };
            return self.handle_final_response(ctx, state, input).await;
        }

        state.structured_output_failures += 1;

        // Fail-fast: if we have too many consecutive failures, treat raw response as final answer
        if state.structured_output_failures >= 3 {
            warn!(
                failures = state.structured_output_failures,
                "Too many structured output failures, accepting raw response as final answer"
            );

            if let Some(tx) = ctx.progress_tx {
                let _ = tx
                    .send(AgentEvent::Continuation {
                        source: AgentEventSource::Root,
                        reason: "Too many JSON errors, falling back to raw response".to_string(),
                        count: state.continuation_count,
                    })
                    .await;
            }

            let input = FinalResponseInput {
                final_answer: failure.raw_json.clone(),
                reasoning: None,
            };

            return self.handle_final_response(ctx, state, input).await;
        }

        state.continuation_count += 1;
        if let Some(tx) = ctx.progress_tx {
            let _ = tx
                .send(AgentEvent::Continuation {
                    source: AgentEventSource::Root,
                    reason: "Invalid JSON response, retrying...".to_string(),
                    count: state.continuation_count,
                })
                .await;
        }

        let response_preview = crate::utils::truncate_str(&failure.raw_json, 400);
        let system_message = format!(
            "[SYSTEM: Your previous response does not follow the strict JSON schema.\nError: {}\nResponse: {}\nReturn ONLY valid JSON according to the schema without markdown, XML, or text outside JSON.]",
            failure.error.message(),
            response_preview
        );
        ctx.messages
            .push(crate::llm::Message::system(&system_message));
        ctx.agent
            .memory_mut()
            .add_message(crate::agent::memory::AgentMessage::system_context(
                system_message,
            ));

        Ok(None)
    }

    fn save_final_response(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        rendered_response: &str,
        reasoning: Option<String>,
    ) {
        if let Some(reasoning_content) = reasoning {
            ctx.agent.memory_mut().add_message(
                crate::agent::memory::AgentMessage::assistant_with_reasoning(
                    rendered_response,
                    reasoning_content,
                ),
            );
        } else {
            ctx.agent
                .memory_mut()
                .add_message(crate::agent::memory::AgentMessage::assistant(
                    rendered_response,
                ));
        }
    }

    fn save_undelivered_final_response_draft(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        final_response: &str,
        reason: &str,
    ) {
        let trimmed = final_response.trim();
        if trimmed.is_empty() {
            return;
        }

        let original_chars = trimmed.chars().count();
        let preview = trimmed
            .chars()
            .take(MAX_UNDELIVERED_DRAFT_CONTEXT_CHARS)
            .collect::<String>();
        let notice = format!(
            "[SYSTEM: The previous assistant final response was not delivered to the user. \
Reason: {reason}. It is untrusted and must not be repeated as fact. \
Original chars: {original_chars}. Preview is truncated.]\n\nUndelivered draft preview:\n{preview}"
        );
        ctx.messages.push(crate::llm::Message::system(&notice));
        ctx.agent
            .memory_mut()
            .add_message(crate::agent::memory::AgentMessage::undelivered_assistant_draft(notice));
    }

    /// Handle a final response payload and run after-agent hooks.
    pub(super) async fn handle_final_response(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        input: FinalResponseInput,
    ) -> anyhow::Result<Option<AgentRunResult>> {
        if ctx.agent.cancellation_token().is_cancelled() {
            return Err(self.cancelled_error(ctx).await);
        }

        let mut final_response = input.final_answer;
        let mut reasoning = input.reasoning;
        if let Some(draft) = state.pending_final_draft.take() {
            if draft.should_replace_final_response(&final_response)
                || response_is_research_status_only(&final_response)
            {
                info!(
                    task_id = ctx.task_id,
                    draft_len = draft.content_len(),
                    short_final_len = final_response.len(),
                    source_iteration = draft.source_iteration,
                    source_tool_name = draft.source_tool_name,
                    "using pending final draft instead of short final response"
                );
                final_response = draft.content;
                reasoning = None;
            } else {
                tracing::debug!(
                    task_id = ctx.task_id,
                    draft_len = draft.content_len(),
                    final_response_len = final_response.len(),
                    source_iteration = draft.source_iteration,
                    source_tool_name = draft.source_tool_name,
                    "discarding pending final draft because final response is substantive"
                );
            }
        }

        sync_todos_from_arc(ctx.agent.memory_mut(), ctx.todos_arc).await;
        let hook_result = self.after_agent_hook_result(ctx, state, &final_response);

        match hook_result {
            crate::agent::hooks::HookResult::ForceIteration { reason, context } => {
                state.continuation_count += 1;
                if let Some(tx) = ctx.progress_tx {
                    let _ = tx
                        .send(AgentEvent::Continuation {
                            source: AgentEventSource::Root,
                            reason: reason.clone(),
                            count: state.continuation_count,
                        })
                        .await;
                }
                let retry_message =
                    format!("[SYSTEM: {reason}]\n\n{}", context.unwrap_or_default());
                self.save_undelivered_final_response_draft(
                    ctx,
                    &final_response,
                    "completion hook forced another iteration",
                );
                ctx.messages
                    .push(crate::llm::Message::system(&retry_message));
                ctx.agent.memory_mut().add_message(
                    crate::agent::memory::AgentMessage::system_context(retry_message),
                );
                let snapshot = Self::build_token_snapshot(ctx, CompactionTrigger::PreIteration);
                Self::emit_token_snapshot_update(ctx.progress_tx, snapshot).await;
                return Ok(None);
            }
            crate::agent::hooks::HookResult::Block { reason } => {
                return Err(anyhow::anyhow!(reason));
            }
            crate::agent::hooks::HookResult::Finish(report) => {
                final_response = report;
                reasoning = None;
            }
            crate::agent::hooks::HookResult::Continue
            | crate::agent::hooks::HookResult::InjectContext(_)
            | crate::agent::hooks::HookResult::InjectTransientContext(_)
            | crate::agent::hooks::HookResult::RequestCompaction { .. } => {}
        }

        if ctx.agent.has_pending_runtime_context() {
            state.continuation_count += 1;
            if let Some(tx) = ctx.progress_tx {
                let _ = tx
                    .send(AgentEvent::Continuation {
                        source: AgentEventSource::Root,
                        reason: "New user context received, continuing the task.".to_string(),
                        count: state.continuation_count,
                    })
                    .await;
            }

            self.save_undelivered_final_response_draft(
                ctx,
                &final_response,
                "new user context arrived before delivery",
            );
            let snapshot = Self::build_token_snapshot(ctx, CompactionTrigger::PreIteration);
            Self::emit_token_snapshot_update(ctx.progress_tx, snapshot).await;
            return Ok(None);
        }

        match self
            .verify_final_response_before_delivery(ctx, state, &final_response)
            .await?
        {
            FinalVerificationOutcome::DeliverOriginal => {}
            FinalVerificationOutcome::DeliverOverride {
                final_response: override_response,
            } => {
                final_response = override_response;
                reasoning = None;
            }
            FinalVerificationOutcome::FailClosedNotice {
                final_response: notice,
            } => {
                final_response = notice;
                reasoning = None;
            }
            FinalVerificationOutcome::Continue { reason, context } => {
                state.continuation_count += 1;
                if let Some(tx) = ctx.progress_tx {
                    let _ = tx
                        .send(AgentEvent::Continuation {
                            source: AgentEventSource::Root,
                            reason: reason.clone(),
                            count: state.continuation_count,
                        })
                        .await;
                }
                let retry_message = format!("[SYSTEM: {reason}]\n\n{context}");
                self.save_undelivered_final_response_draft(
                    ctx,
                    &final_response,
                    "strict answer verifier required another iteration",
                );
                ctx.messages
                    .push(crate::llm::Message::system(&retry_message));
                ctx.agent.memory_mut().add_message(
                    crate::agent::memory::AgentMessage::system_context(retry_message),
                );
                let snapshot = Self::build_token_snapshot(ctx, CompactionTrigger::PreIteration);
                Self::emit_token_snapshot_update(ctx.progress_tx, snapshot).await;
                return Ok(None);
            }
        }

        self.save_final_response(ctx, &final_response, reasoning);
        let snapshot = Self::build_token_snapshot(ctx, CompactionTrigger::PreIteration);
        Self::emit_token_snapshot_update(ctx.progress_tx, snapshot).await;

        if let Some(tx) = ctx.progress_tx
            && !ctx.config.is_sub_agent
        {
            let _ = tx.send(AgentEvent::Finished).await;
        }
        Ok(Some(AgentRunResult::Final(final_response)))
    }

    async fn verify_final_response_before_delivery(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        final_response: &str,
    ) -> anyhow::Result<FinalVerificationOutcome> {
        if ctx.config.is_sub_agent {
            return Ok(FinalVerificationOutcome::DeliverOriginal);
        }

        let Some(verifier_config) = ctx.config.research_verifier_config.clone() else {
            return Ok(FinalVerificationOutcome::DeliverOriginal);
        };
        if !verifier_config.enabled {
            return Ok(FinalVerificationOutcome::DeliverOriginal);
        }

        let final_response_chars = final_response.trim().chars().count();
        let max_rounds = verifier_config.max_rounds.max(1);

        if state.proof_not_found_report_requested {
            self.save_undelivered_final_response_draft(
                ctx,
                final_response,
                "root-authored proof-not-found draft ignored in deterministic fallback",
            );
            let Some(research_runtime) = ctx.research_runtime.clone() else {
                return Ok(FinalVerificationOutcome::FailClosedNotice {
                    final_response: safe_fail_closed_notice(
                        "strict answer verifier has no research runtime",
                    ),
                });
            };
            let research_snapshot = research_runtime.snapshot();
            let source_decision = state.last_verifier_decision.clone().unwrap_or_else(|| {
                proof_not_found_source_decision(
                    "proof-not-found fallback started without a prior verifier decision",
                )
            });
            return self
                .verify_deterministic_proof_not_found_report(
                    ctx,
                    state,
                    research_runtime.as_ref(),
                    &research_snapshot,
                    &source_decision,
                    verifier_config,
                )
                .await;
        }

        if final_response_chars < MIN_VERIFIER_FINAL_ANSWER_CHARS {
            if !state.short_final_retry_used && state.verifier_rounds < max_rounds {
                state.short_final_retry_used = true;
                return Ok(FinalVerificationOutcome::Continue {
                    reason: "Strict answer verifier requires a substantive final answer"
                        .to_string(),
                    context: verifier_substantive_final_answer_context(final_response_chars),
                });
            }

            let Some(research_runtime) = ctx.research_runtime.clone() else {
                return Ok(FinalVerificationOutcome::FailClosedNotice {
                    final_response: safe_fail_closed_notice(
                        "strict answer verifier has no research runtime",
                    ),
                });
            };
            let research_snapshot = research_runtime.snapshot();
            let source_decision = state.last_verifier_decision.clone().unwrap_or_else(|| {
                proof_not_found_source_decision("root model produced repeated short final answers")
            });
            return self
                .verify_deterministic_proof_not_found_report(
                    ctx,
                    state,
                    research_runtime.as_ref(),
                    &research_snapshot,
                    &source_decision,
                    verifier_config,
                )
                .await;
        }

        if final_response_chars > MAX_VERIFIER_FINAL_ANSWER_CHARS {
            self.save_undelivered_final_response_draft(
                ctx,
                final_response,
                "final draft exceeded strict verifier input limit",
            );
            if !state.oversized_final_retry_used && state.verifier_rounds < max_rounds {
                state.oversized_final_retry_used = true;
                return Ok(FinalVerificationOutcome::Continue {
                    reason: "Strict answer verifier requires a shorter final answer".to_string(),
                    context: verifier_shorter_final_answer_context(final_response_chars),
                });
            }

            let Some(research_runtime) = ctx.research_runtime.clone() else {
                return Ok(FinalVerificationOutcome::FailClosedNotice {
                    final_response: safe_fail_closed_notice(
                        "strict answer verifier has no research runtime",
                    ),
                });
            };
            let research_snapshot = research_runtime.snapshot();
            let source_decision = state.last_verifier_decision.clone().unwrap_or_else(|| {
                proof_not_found_source_decision(
                    "root model produced repeated oversized final answers",
                )
            });
            return self
                .verify_deterministic_proof_not_found_report(
                    ctx,
                    state,
                    research_runtime.as_ref(),
                    &research_snapshot,
                    &source_decision,
                    verifier_config,
                )
                .await;
        }

        let Some(research_runtime) = ctx.research_runtime.clone() else {
            self.save_undelivered_final_response_draft(
                ctx,
                final_response,
                "strict answer verifier has no research runtime",
            );
            return Err(anyhow::anyhow!(
                "strict answer verifier failed closed: research runtime is unavailable"
            ));
        };

        let research_snapshot = research_runtime.snapshot();
        let max_evidence_docs = verifier_config.max_evidence_docs;
        let verifier = StrictAnswerVerifier::new(self.llm_client(), verifier_config.clone());
        state.verifier_rounds += 1;
        let round = state.verifier_rounds;
        let evidence_document_count = research_snapshot
            .evidence_documents
            .len()
            .min(max_evidence_docs);
        if let Some(tx) = ctx.progress_tx {
            let _ = tx
                .send(AgentEvent::FinalDraftPendingVerification {
                    source: AgentEventSource::Root,
                    content_chars: final_response_chars,
                    round,
                })
                .await;
            let _ = tx
                .send(AgentEvent::ResearchVerificationStarted {
                    round,
                    max_rounds,
                    proof_not_found_mode: false,
                    evidence_document_count,
                })
                .await;
        }
        let decision = verifier
            .verify(AnswerVerificationRequest {
                final_answer: final_response,
                research: &research_snapshot,
                round,
                proof_not_found_mode: false,
            })
            .await;

        match decision {
            Ok(decision) => {
                state.last_verifier_decision = Some(decision.clone());
                let enter_proof_not_found = should_enter_deterministic_proof_not_found(
                    &decision,
                    state.verifier_rounds,
                    max_rounds,
                );
                let outcome_result = verifier_decision_outcome(&decision, round, max_rounds);
                let trace_outcome = if enter_proof_not_found {
                    "continue"
                } else {
                    match &outcome_result {
                        Ok(FinalVerificationOutcome::DeliverOriginal)
                        | Ok(FinalVerificationOutcome::DeliverOverride { .. }) => "deliver",
                        Ok(FinalVerificationOutcome::Continue { .. }) => "continue",
                        Ok(FinalVerificationOutcome::FailClosedNotice { .. }) | Err(_) => {
                            "fail_closed"
                        }
                    }
                };
                let trace = verifier_trace_from_decision(
                    &decision,
                    trace_outcome,
                    round,
                    max_rounds,
                    false,
                    evidence_document_count,
                );
                let payload = research_runtime.record_verifier_trace(trace);
                emit_research_verification_trace(ctx.progress_tx, payload).await;

                if enter_proof_not_found {
                    self.save_undelivered_final_response_draft(
                        ctx,
                        final_response,
                        "strict answer verifier exhausted normal proof rounds",
                    );
                    return self
                        .verify_deterministic_proof_not_found_report(
                            ctx,
                            state,
                            research_runtime.as_ref(),
                            &research_snapshot,
                            &decision,
                            verifier_config,
                        )
                        .await;
                }

                Ok(outcome_result?)
            }
            Err(error) => {
                let summary = verifier_error_summary(&error);
                let transient_outcome = verifier_transient_error_outcome(
                    &error, &summary, round, max_rounds, false, false,
                );
                let payload = research_runtime.record_verifier_trace(ResearchVerifierTrace {
                    verdict: None,
                    outcome: if transient_outcome.is_some() {
                        "continue"
                    } else {
                        "fail_closed"
                    }
                    .to_string(),
                    summary: summary.clone(),
                    error: Some(summary.clone()),
                    round,
                    max_rounds,
                    proof_not_found_mode: false,
                    evidence_document_count,
                    unsupported_claims: Vec::new(),
                    contradictions: Vec::new(),
                    required_next_actions: Vec::new(),
                });
                emit_research_verification_trace(ctx.progress_tx, payload).await;

                if let Some((outcome, _used_proof_not_found_retry)) = transient_outcome {
                    return Ok(outcome);
                }

                self.save_undelivered_final_response_draft(
                    ctx,
                    final_response,
                    "strict answer verifier failed closed",
                );
                Err(anyhow::anyhow!(
                    "strict answer verifier failed closed: {}",
                    summary
                ))
            }
        }
    }

    async fn verify_deterministic_proof_not_found_report(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        research_runtime: &crate::agent::research::ResearchRuntime,
        research_snapshot: &ResearchSnapshot,
        source_decision: &AnswerVerificationDecision,
        verifier_config: crate::agent::research::ResearchVerifierConfig,
    ) -> anyhow::Result<FinalVerificationOutcome> {
        state.proof_not_found_report_requested = true;
        let verifier_config = compact_proof_not_found_verifier_config(verifier_config);
        let Some(presenter_model) = verifier_config.model.clone() else {
            return Ok(FinalVerificationOutcome::FailClosedNotice {
                final_response: safe_fail_closed_notice(
                    "strict verifier config has no presenter model route",
                ),
            });
        };
        let mut presenter_feedback = None;

        loop {
            state.proof_not_found_verify_attempts += 1;
            let round = state.proof_not_found_verify_attempts;
            let max_rounds = MAX_PROOF_NOT_FOUND_VERIFY_ATTEMPTS;

            let report = match self
                .synthesize_proof_not_found_answer(
                    &verifier_config,
                    &presenter_model,
                    ctx.task,
                    research_snapshot,
                    source_decision,
                    presenter_feedback.as_deref(),
                )
                .await
            {
                Ok(report) => report,
                Err(error) => {
                    return Ok(FinalVerificationOutcome::FailClosedNotice {
                        final_response: safe_fail_closed_notice(&format!(
                            "proof-not-found presenter failed: {error}"
                        )),
                    });
                }
            };

            if report.trim().chars().count() < MIN_VERIFIER_FINAL_ANSWER_CHARS {
                if !state.proof_not_found_repair_used {
                    state.proof_not_found_repair_used = true;
                    state.proof_not_found_repair_attempt_used = true;
                    presenter_feedback = Some(format!(
                        "Previous presenter answer was too short for verification ({} chars). Write a substantive user-facing answer from the packet.",
                        report.trim().chars().count()
                    ));
                    continue;
                }
                return Ok(FinalVerificationOutcome::FailClosedNotice {
                    final_response: safe_fail_closed_notice(
                        "proof-not-found presenter produced repeated short answers",
                    ),
                });
            }

            if report.chars().count() > MAX_VERIFIER_FINAL_ANSWER_CHARS {
                if !state.proof_not_found_repair_used {
                    state.proof_not_found_repair_used = true;
                    state.proof_not_found_repair_attempt_used = true;
                    presenter_feedback = Some(format!(
                        "Previous presenter answer was too long for strict verification ({} chars). Rewrite it more concisely without adding facts.",
                        report.chars().count()
                    ));
                    continue;
                }
                return Ok(FinalVerificationOutcome::FailClosedNotice {
                    final_response: safe_fail_closed_notice(
                        "proof-not-found presenter answer exceeded verifier input limit",
                    ),
                });
            }

            let evidence_document_count = research_snapshot
                .evidence_documents
                .len()
                .min(verifier_config.max_evidence_docs);
            if let Some(tx) = ctx.progress_tx {
                let _ = tx
                    .send(AgentEvent::FinalDraftPendingVerification {
                        source: AgentEventSource::Root,
                        content_chars: report.chars().count(),
                        round,
                    })
                    .await;
                let _ = tx
                    .send(AgentEvent::ResearchVerificationStarted {
                        round,
                        max_rounds,
                        proof_not_found_mode: true,
                        evidence_document_count,
                    })
                    .await;
            }

            let verifier = StrictAnswerVerifier::new(self.llm_client(), verifier_config.clone());
            let decision = verifier
                .verify(AnswerVerificationRequest {
                    final_answer: &report,
                    research: research_snapshot,
                    round,
                    proof_not_found_mode: true,
                })
                .await;

            match decision {
                Ok(decision) => {
                    let deliverable = matches!(
                        decision.verdict,
                        AnswerVerifierVerdict::Allow | AnswerVerifierVerdict::ProofNotFound
                    );
                    let repairable = matches!(
                        decision.verdict,
                        AnswerVerifierVerdict::Revise | AnswerVerifierVerdict::NeedMoreEvidence
                    );
                    let trace_outcome = if deliverable {
                        "deliver"
                    } else if repairable && !state.proof_not_found_repair_used {
                        "continue"
                    } else {
                        "fail_closed"
                    };
                    let trace = verifier_trace_from_decision(
                        &decision,
                        trace_outcome,
                        round,
                        max_rounds,
                        true,
                        evidence_document_count,
                    );
                    let payload = research_runtime.record_verifier_trace(trace);
                    emit_research_verification_trace(ctx.progress_tx, payload).await;

                    if deliverable {
                        return Ok(FinalVerificationOutcome::DeliverOverride {
                            final_response: report,
                        });
                    }

                    if repairable && !state.proof_not_found_repair_used {
                        state.proof_not_found_repair_used = true;
                        state.proof_not_found_repair_attempt_used = true;
                        presenter_feedback =
                            Some(proof_not_found_presenter_repair_feedback(&decision));
                        continue;
                    }

                    return Ok(FinalVerificationOutcome::FailClosedNotice {
                        final_response: safe_fail_closed_notice(
                            "deterministic proof-not-found report was not verified",
                        ),
                    });
                }
                Err(error) => {
                    let summary = verifier_error_summary(&error);
                    let payload = research_runtime.record_verifier_trace(ResearchVerifierTrace {
                        verdict: None,
                        outcome: "fail_closed".to_string(),
                        summary: summary.clone(),
                        error: Some(summary.clone()),
                        round,
                        max_rounds,
                        proof_not_found_mode: true,
                        evidence_document_count,
                        unsupported_claims: Vec::new(),
                        contradictions: Vec::new(),
                        required_next_actions: Vec::new(),
                    });
                    emit_research_verification_trace(ctx.progress_tx, payload).await;
                    return Ok(FinalVerificationOutcome::FailClosedNotice {
                        final_response: safe_fail_closed_notice(&summary),
                    });
                }
            }
        }
    }

    async fn synthesize_proof_not_found_answer(
        &self,
        verifier_config: &crate::agent::research::ResearchVerifierConfig,
        presenter_model: &crate::config::ModelInfo,
        user_task: &str,
        research_snapshot: &ResearchSnapshot,
        source_decision: &AnswerVerificationDecision,
        repair_feedback: Option<&str>,
    ) -> anyhow::Result<String> {
        let packet = verified_research_packet_json(
            user_task,
            research_snapshot,
            source_decision,
            repair_feedback,
        );
        let llm = self.llm_client();
        let result = tokio::time::timeout(
            verifier_config.timeout,
            llm.complete_internal_text(
                InternalTextPurpose::ProofNotFoundPresentation,
                PROOF_NOT_FOUND_PRESENTER_SYSTEM_PROMPT,
                &packet,
                presenter_model,
            ),
        )
        .await
        .map_err(|_| anyhow::anyhow!("presenter timed out after {:?}", verifier_config.timeout))?
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;

        let trimmed = result.trim();
        if trimmed.is_empty() {
            return Err(anyhow::anyhow!("presenter returned an empty answer"));
        }

        Ok(trimmed.to_string())
    }

    /// Handle a blocked response that requires more user input.
    pub(super) async fn handle_waiting_for_user_input(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        _raw_json: String,
        reasoning: Option<String>,
        request: PendingUserInput,
    ) -> anyhow::Result<Option<AgentRunResult>> {
        if ctx.agent.cancellation_token().is_cancelled() {
            return Err(self.cancelled_error(ctx).await);
        }

        sync_todos_from_arc(ctx.agent.memory_mut(), ctx.todos_arc).await;
        self.save_final_response(ctx, &request.prompt, reasoning);
        let _ = state;
        let snapshot = Self::build_token_snapshot(ctx, CompactionTrigger::PreIteration);
        Self::emit_token_snapshot_update(ctx.progress_tx, snapshot).await;

        Ok(Some(AgentRunResult::WaitingForUserInput(request)))
    }
}

enum FinalVerificationOutcome {
    DeliverOriginal,
    DeliverOverride { final_response: String },
    FailClosedNotice { final_response: String },
    Continue { reason: String, context: String },
}

fn compact_proof_not_found_verifier_config(
    mut config: crate::agent::research::ResearchVerifierConfig,
) -> crate::agent::research::ResearchVerifierConfig {
    config.max_evidence_docs = config
        .max_evidence_docs
        .min(PROOF_NOT_FOUND_MAX_EVIDENCE_DOCS);
    config.max_excerpt_chars = config
        .max_excerpt_chars
        .min(PROOF_NOT_FOUND_MAX_EXCERPT_CHARS);
    config
}

fn verifier_substantive_final_answer_context(final_response_chars: usize) -> String {
    [
        format!(
            "The previous final answer was too short to verify ({final_response_chars} chars; minimum is {MIN_VERIFIER_FINAL_ANSWER_CHARS})."
        ),
        "Do not send empty placeholders or status text as final_answer.".to_string(),
        "Generate a substantive final answer grounded in fetched EvidenceDocument excerpts.".to_string(),
        "If evidence is insufficient, state exactly what was confirmed and what was not confirmed instead of inventing missing claims.".to_string(),
    ]
    .join("\n")
}

fn verifier_shorter_final_answer_context(final_response_chars: usize) -> String {
    [
        format!(
            "The previous final answer was too long for strict verification ({final_response_chars} chars; maximum is {MAX_VERIFIER_FINAL_ANSWER_CHARS})."
        ),
        "Do not deliver the rejected draft.".to_string(),
        "Produce a shorter final answer with only claims directly supported by fetched EvidenceDocument excerpts.".to_string(),
        "Remove unsupported benchmark tables, speed/VRAM numbers, rankings, recommendations, license claims, and suitability/compliance claims.".to_string(),
    ]
    .join("\n")
}

fn verifier_decision_outcome(
    decision: &AnswerVerificationDecision,
    round: usize,
    max_rounds: usize,
) -> anyhow::Result<FinalVerificationOutcome> {
    match decision.verdict {
        AnswerVerifierVerdict::Allow => Ok(FinalVerificationOutcome::DeliverOriginal),
        AnswerVerifierVerdict::Revise | AnswerVerifierVerdict::NeedMoreEvidence => {
            Ok(FinalVerificationOutcome::Continue {
                reason: verifier_retry_reason(decision.verdict),
                context: verifier_retry_context(decision),
            })
        }
        AnswerVerifierVerdict::ProofNotFound => Err(anyhow::anyhow!(
            "strict answer verifier failed closed: proof_not_found verdict is only deliverable in constrained proof-not-found mode"
        )),
        AnswerVerifierVerdict::Block => {
            if round < max_rounds.max(1) && verifier_block_is_recoverable(decision) {
                return Ok(FinalVerificationOutcome::Continue {
                    reason: "Strict answer verifier requires recovery from a blocked draft"
                        .to_string(),
                    context: verifier_block_recovery_context(decision),
                });
            }

            Err(anyhow::anyhow!(
                "strict answer verifier blocked final response: {}",
                verifier_decision_summary(decision)
            ))
        }
    }
}

async fn emit_research_verification_trace(
    progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    payload: serde_json::Value,
) {
    tracing::info!(payload = %payload, "Strict answer verifier trace");
    if let Some(tx) = progress_tx {
        let _ = tx.send(AgentEvent::ResearchVerification { payload }).await;
    }
}

fn verifier_trace_from_decision(
    decision: &AnswerVerificationDecision,
    outcome: &str,
    round: usize,
    max_rounds: usize,
    proof_not_found_mode: bool,
    evidence_document_count: usize,
) -> ResearchVerifierTrace {
    ResearchVerifierTrace {
        verdict: Some(verifier_verdict_label(decision.verdict).to_string()),
        outcome: outcome.to_string(),
        summary: decision.summary.clone(),
        error: None,
        round,
        max_rounds,
        proof_not_found_mode,
        evidence_document_count,
        unsupported_claims: decision
            .unsupported_claims
            .iter()
            .map(|claim| claim.claim.clone())
            .collect(),
        contradictions: decision
            .contradictions
            .iter()
            .map(|contradiction| contradiction.claim.clone())
            .collect(),
        required_next_actions: decision.required_next_actions.clone(),
    }
}

const fn verifier_verdict_label(verdict: AnswerVerifierVerdict) -> &'static str {
    match verdict {
        AnswerVerifierVerdict::Allow => "allow",
        AnswerVerifierVerdict::Revise => "revise",
        AnswerVerifierVerdict::NeedMoreEvidence => "need_more_evidence",
        AnswerVerifierVerdict::ProofNotFound => "proof_not_found",
        AnswerVerifierVerdict::Block => "block",
    }
}

fn verifier_retry_reason(verdict: AnswerVerifierVerdict) -> String {
    match verdict {
        AnswerVerifierVerdict::Revise => {
            "Strict answer verifier requires a revised final answer".to_string()
        }
        AnswerVerifierVerdict::NeedMoreEvidence => {
            "Strict answer verifier requires more proof evidence".to_string()
        }
        _ => "Strict answer verifier requires another iteration".to_string(),
    }
}

fn verifier_retry_context(decision: &AnswerVerificationDecision) -> String {
    let mut sections = vec![
        "The strict answer verifier rejected the previous final draft.".to_string(),
        format!("Verifier summary: {}", decision.summary),
        "Do not deliver the rejected draft. Use only fetched proof documents, not memory, snippets, reasoning, or sub-agent prose.".to_string(),
    ];

    if !decision.unsupported_claims.is_empty() {
        sections.push("Unsupported claims to fix or prove:".to_string());
        sections.extend(
            decision
                .unsupported_claims
                .iter()
                .enumerate()
                .map(|(index, claim)| unsupported_claim_context_line(index, claim)),
        );
    }

    if !decision.contradictions.is_empty() {
        sections.push("Contradictions found by verifier:".to_string());
        sections.extend(decision.contradictions.iter().enumerate().map(
            |(index, contradiction)| {
                format!(
                    "{}. Claim: {} | Contradicting source: {} | Excerpt: {}",
                    index + 1,
                    contradiction.claim,
                    contradiction.source_id,
                    contradiction.source_excerpt
                )
            },
        ));
    }

    if !decision.required_next_actions.is_empty() {
        sections.push("Required next actions:".to_string());
        sections.extend(
            decision
                .required_next_actions
                .iter()
                .enumerate()
                .map(|(index, action)| format!("{}. {action}", index + 1)),
        );
        sections.push(
            "Before the next final answer, execute the required actions against the exact sources/URLs or remove every claim that depends on them."
                .to_string(),
        );
    }

    sections.push(
        "When evidence cannot be found, do not invent it; continue gathering proof until the proof-not-found flow is requested."
            .to_string(),
    );
    sections.join("\n")
}

fn verifier_block_is_recoverable(decision: &AnswerVerificationDecision) -> bool {
    !decision.required_next_actions.is_empty()
        || (!decision.unsupported_claims.is_empty() && decision.contradictions.is_empty())
}

fn should_enter_deterministic_proof_not_found(
    decision: &AnswerVerificationDecision,
    verifier_rounds: usize,
    max_rounds: usize,
) -> bool {
    if verifier_rounds < max_rounds.max(1) {
        return false;
    }

    match decision.verdict {
        AnswerVerifierVerdict::Revise | AnswerVerifierVerdict::NeedMoreEvidence => true,
        AnswerVerifierVerdict::Block => {
            verifier_block_is_recoverable(decision) && decision.contradictions.is_empty()
        }
        AnswerVerifierVerdict::Allow | AnswerVerifierVerdict::ProofNotFound => false,
    }
}

fn verifier_block_recovery_context(decision: &AnswerVerificationDecision) -> String {
    let mut sections = vec![
        "The strict answer verifier blocked the previous final draft, but reported actionable fixes.".to_string(),
        format!("Verifier summary: {}", decision.summary),
        "Do not deliver the blocked draft. Before producing another final answer, either fetch and cite the exact required sources or remove every unsupported claim.".to_string(),
        "Claims about model architecture, license, datasets, metrics, language support, rankings, and suitability must be present in fetched EvidenceDocument excerpts.".to_string(),
    ];

    if !decision.required_next_actions.is_empty() {
        sections.push("Mandatory required actions before retrying final answer:".to_string());
        sections.extend(
            decision
                .required_next_actions
                .iter()
                .enumerate()
                .map(|(index, action)| format!("{}. {action}", index + 1)),
        );
    }

    if !decision.unsupported_claims.is_empty() {
        sections.push("Unsupported claims to prove or remove:".to_string());
        sections.extend(
            decision
                .unsupported_claims
                .iter()
                .enumerate()
                .map(|(index, claim)| unsupported_claim_context_line(index, claim)),
        );
    }

    sections.push(
        "If a required source cannot be fetched or does not support the claim, state that it was not confirmed instead of asserting it."
            .to_string(),
    );
    sections.join("\n")
}

fn verifier_proof_not_found_short_report_context(summary: &str) -> String {
    [
        "The constrained proof-not-found report was not delivered.".to_string(),
        format!("Verifier failure: {summary}"),
        format!(
            "Produce one shorter proof-not-found report under {PROOF_NOT_FOUND_MAX_REPORT_CHARS} chars."
        ),
        format!("Use at most {PROOF_NOT_FOUND_MAX_EVIDENCE_DOCS} fetched sources."),
        "Start exactly with: Проверка завершена: достаточные пруфы не найдены".to_string(),
        "No benchmark table. No unsupported numbers. No inferred speed, VRAM, license, ranking, production-readiness, or suitability claims.".to_string(),
        "Only state: confirmed / not confirmed / claims removed / safe next steps.".to_string(),
    ]
    .join("\n")
}

fn unsupported_claim_context_line(index: usize, claim: &VerifierUnsupportedClaim) -> String {
    format!(
        "{}. Claim: {} | Reason: {} | Required evidence: {} | Suggested action: {}",
        index + 1,
        claim.claim,
        claim.reason,
        claim.required_evidence,
        claim.suggested_next_action
    )
}

fn verifier_decision_summary(decision: &AnswerVerificationDecision) -> String {
    let mut summary = decision.summary.clone();
    if !decision.unsupported_claims.is_empty() {
        let claims = decision
            .unsupported_claims
            .iter()
            .map(|claim| claim.claim.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        summary.push_str("; unsupported claims: ");
        summary.push_str(&claims);
    }
    summary
}

fn verifier_error_summary(error: &AnswerVerificationError) -> String {
    error.to_string()
}

fn proof_not_found_source_decision(summary: &str) -> AnswerVerificationDecision {
    AnswerVerificationDecision {
        verdict: AnswerVerifierVerdict::NeedMoreEvidence,
        confidence: AnswerVerifierConfidence::Low,
        summary: summary.to_string(),
        unsupported_claims: Vec::new(),
        contradictions: Vec::new(),
        allowed_claims: Vec::new(),
        required_next_actions: vec![
            "Fetch primary model cards, official benchmark pages, or run a reproducible local benchmark with fixed versions, GPU, precision, prompt length, batch size, and decoding settings.".to_string(),
        ],
    }
}

fn verified_research_packet_json(
    user_task: &str,
    research_snapshot: &ResearchSnapshot,
    source_decision: &AnswerVerificationDecision,
    repair_feedback: Option<&str>,
) -> String {
    let checked_sources = research_snapshot
        .evidence_documents
        .iter()
        .take(PROOF_NOT_FOUND_MAX_EVIDENCE_DOCS)
        .enumerate()
        .map(|(index, doc)| {
            json!({
                "id": format!("doc-{}", index + 1),
                "url": doc.final_url.as_deref().unwrap_or(doc.url.as_str()),
                "status_code": doc.status_code,
                "source_kind": doc.source_kind.as_deref().unwrap_or("unknown"),
                "truncated": doc.truncated,
            })
        })
        .collect::<Vec<_>>();

    let missing_dimensions = missing_dimensions_from_decision(source_decision);
    let forbidden_claims = forbidden_claims_from_decision(source_decision);

    serde_json::to_string_pretty(&json!({
        "packet_type": "VerifiedResearchPacket",
        "user_task": user_task,
        "instructions": {
            "same_language_as_user_task": true,
            "start_with_tldr": true,
            "then_write_detailed_answer": true,
            "do_not_sound_like_verifier_log": true,
            "no_character_limit_instruction": true,
            "facts_must_come_only_from_allowed_claims": true,
            "unsupported_claims_are_not_facts": true,
            "if_requested_comparison_cannot_be_proven_explain_clearly": true,
            "table_cells_must_be_confirmed_or_not_confirmed": true
        },
        "allowed_claims": source_decision.allowed_claims,
        "unsupported_claims": source_decision.unsupported_claims,
        "missing_evidence_by_dimension": missing_dimensions,
        "required_next_actions": source_decision.required_next_actions,
        "checked_sources": checked_sources,
        "forbidden_claims": forbidden_claims,
        "repair_feedback": repair_feedback,
    }))
    .unwrap_or_else(|_| {
        json!({
            "packet_type": "VerifiedResearchPacket",
            "user_task": user_task,
            "allowed_claims": [],
            "unsupported_claims": [],
            "required_next_actions": source_decision.required_next_actions,
            "repair_feedback": repair_feedback,
        })
        .to_string()
    })
}

fn missing_dimensions_from_decision(decision: &AnswerVerificationDecision) -> Vec<Value> {
    let dimensions = [
        (
            "generation_speed",
            &["tok/s", "throughput", "generation", "скорост"][..],
        ),
        (
            "prompt_eval_latency",
            &["ttft", "latency", "prompt eval", "prefill", "prompt"][..],
        ),
        ("vram_memory", &["vram", "memory", "gb", "памят"][..]),
        ("quality", &["quality", "benchmark", "качест"][..]),
        ("license", &["license", "лиценз"][..]),
        (
            "recommendation_or_winner",
            &["recommend", "winner", "better", "лучше", "рекоменд"][..],
        ),
        (
            "architecture_or_quantization",
            &["architecture", "quant", "nvfp4", "bf16", "moe", "архитект"][..],
        ),
    ];

    let unsupported_text = decision
        .unsupported_claims
        .iter()
        .map(|claim| format!("{} {}", claim.claim, claim.reason).to_lowercase())
        .collect::<Vec<_>>();

    let mut output = Vec::new();
    for (dimension, needles) in dimensions {
        let related_claims = decision
            .unsupported_claims
            .iter()
            .zip(unsupported_text.iter())
            .filter(|(_, text)| needles.iter().any(|needle| text.contains(needle)))
            .map(|(claim, _)| crate::utils::truncate_str(&claim.claim, 260))
            .collect::<Vec<_>>();

        if !related_claims.is_empty() {
            output.push(json!({
                "dimension": dimension,
                "status": "not_confirmed_in_checked_sources",
                "related_unsupported_claims": related_claims,
            }));
        }
    }

    if output.is_empty() && !decision.unsupported_claims.is_empty() {
        output.push(json!({
            "dimension": "requested_comparison",
            "status": "not_confirmed_in_checked_sources",
            "related_unsupported_claims": decision
                .unsupported_claims
                .iter()
                .map(|claim| crate::utils::truncate_str(&claim.claim, 260))
                .collect::<Vec<_>>(),
        }));
    }

    output
}

fn forbidden_claims_from_decision(decision: &AnswerVerificationDecision) -> Vec<String> {
    let mut forbidden = decision
        .unsupported_claims
        .iter()
        .map(|claim| crate::utils::truncate_str(&claim.claim, 320))
        .collect::<Vec<_>>();
    forbidden.push(
        "Do not provide a winner, ranking, recommendation, benchmark table, speed number, prompt-eval number, VRAM number, license claim, or architecture claim unless it is present in allowed_claims."
            .to_string(),
    );
    forbidden
}

fn proof_not_found_presenter_repair_feedback(decision: &AnswerVerificationDecision) -> String {
    let mut feedback = vec![
        "The strict verifier rejected the previous presented answer. Rewrite it using the original VerifiedResearchPacket only.".to_string(),
        format!("Verifier summary: {}", decision.summary),
    ];

    if !decision.unsupported_claims.is_empty() {
        feedback.push("Remove or weaken these unsupported claims:".to_string());
        feedback.extend(
            decision
                .unsupported_claims
                .iter()
                .enumerate()
                .map(|(index, claim)| unsupported_claim_context_line(index, claim)),
        );
    }

    if !decision.required_next_actions.is_empty() {
        feedback.push("Mention these only as next steps, not as completed research:".to_string());
        feedback.extend(
            decision
                .required_next_actions
                .iter()
                .enumerate()
                .map(|(index, action)| format!("{}. {action}", index + 1)),
        );
    }

    feedback.join("\n")
}

fn safe_fail_closed_notice(reason: &str) -> String {
    let reason = crate::utils::truncate_str(reason, 320);
    format!(
        "Проверка закрыта без выдачи предметного отчета.\n\n\
Verifier не подтвердил даже ограниченный отчет о ненайденных доказательствах.\n\
Неподтвержденные утверждения, benchmark-таблицы, числа производительности, VRAM, \
скорости, лицензии, compliance и рекомендации не доставлены.\n\n\
Техническая причина: {reason}"
    )
}

fn verifier_transient_error_outcome(
    error: &AnswerVerificationError,
    summary: &str,
    round: usize,
    max_rounds: usize,
    proof_not_found_mode: bool,
    proof_not_found_repair_attempt_used: bool,
) -> Option<(FinalVerificationOutcome, bool)> {
    let is_transient = match error {
        AnswerVerificationError::Timeout { .. } => true,
        AnswerVerificationError::Provider(error) => LlmClient::is_retryable_error(error),
        AnswerVerificationError::Disabled
        | AnswerVerificationError::MissingRoute
        | AnswerVerificationError::InvalidJson(_) => false,
    };

    if !is_transient {
        return None;
    }

    if proof_not_found_mode {
        if proof_not_found_repair_attempt_used {
            return None;
        }
        return Some((
            FinalVerificationOutcome::Continue {
                reason: "Strict answer verifier timed out while checking proof-not-found report"
                    .to_string(),
                context: verifier_proof_not_found_short_report_context(summary),
            },
            true,
        ));
    }

    if round >= max_rounds.max(1) {
        return None;
    }

    Some((
        FinalVerificationOutcome::Continue {
            reason: "Strict answer verifier had a transient provider failure; retrying with a shorter evidence-cited answer".to_string(),
            context: verifier_transient_error_context(summary),
        },
        false,
    ))
}

fn verifier_transient_error_context(summary: &str) -> String {
    [
        "The strict answer verifier could not complete because of a transient provider failure or timeout.".to_string(),
        "Do not deliver the rejected draft. Produce a shorter final answer that is easier to verify.".to_string(),
        "Keep only claims directly supported by fetched EvidenceDocument excerpts; remove unsupported rankings, recommendations, license claims, benchmark metrics, and suitability/compliance claims.".to_string(),
        "Prefer concise bullets with explicit source references. If evidence is insufficient, say what was not confirmed instead of inventing support.".to_string(),
        format!("Verifier failure: {summary}"),
    ]
    .join("\n")
}

fn should_salvage_structured_output_failure(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return false;
    }

    if trimmed.starts_with('{')
        || trimmed.starts_with('[')
        || trimmed.starts_with("```")
        || trimmed.starts_with("<")
        || trimmed.starts_with("[SYSTEM:")
    {
        return false;
    }

    let has_sentence_content = trimmed.chars().filter(|ch| !ch.is_whitespace()).count() >= 24
        && trimmed.chars().any(char::is_alphabetic);
    if !has_sentence_content {
        return false;
    }

    let unfinished_tail = ['{', '[', ':', ',', '-', '"']
        .iter()
        .any(|tail| trimmed.ends_with(*tail));
    !unfinished_tail
}

async fn sync_todos_from_arc(memory: &mut AgentMemory, todos_arc: &Arc<Mutex<TodoList>>) {
    let current_todos = todos_arc.lock().await;
    memory.todos = (*current_todos).clone();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::compaction::AgentMessageKind;
    use crate::agent::context::{AgentContext, EphemeralSession};
    use crate::agent::hooks::{CompletionCheckHook, Hook, HookContext, HookEvent, HookResult};
    use crate::agent::memory::AgentMessage;
    use crate::agent::providers::{TodoItem, TodoList, TodoStatus};
    use crate::agent::research::{ResearchRuntime, ResearchVerifierConfig};
    use crate::agent::runner::test_support::{build_llm_client, single_final_response_provider};
    use crate::agent::runner::types::{FinalResponseInput, PendingFinalDraft, RunState};
    use crate::agent::runner::{AgentRunnerConfig, AgentRunnerContext};
    use crate::agent::tool_runtime::{
        OutputTruncationMetadata, ToolCallId, ToolName, ToolOutput, ToolOutputIdentity,
        ToolOutputStatus,
    };
    use crate::config::{AgentSettings, ModelInfo};
    use crate::llm::{
        ChatResponse, ChatWithToolsRequest, InvocationId, LlmClient, LlmError, MockLlmProvider,
    };
    use chrono::Utc;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::{Mutex, mpsc};

    const SUBSTANTIVE_FINAL: &str = "Model X has 97% F1 on Russian PII. This draft also states that the model is suitable for production anonymization in Russia, has documented license terms, and should be recommended over alternatives. The verifier must check these factual claims against fetched EvidenceDocument excerpts before delivery.";

    struct StaticAfterAgentHook {
        result: HookResult,
    }

    impl Hook for StaticAfterAgentHook {
        fn name(&self) -> &'static str {
            "static_after_agent"
        }

        fn handle(&self, event: &HookEvent, _context: &HookContext) -> HookResult {
            match event {
                HookEvent::AfterAgent { .. } => self.result.clone(),
                _ => HookResult::Continue,
            }
        }
    }

    fn verifier_model() -> ModelInfo {
        ModelInfo {
            id: "verifier-model".to_string(),
            provider: "opencode-go".to_string(),
            max_output_tokens: 4096,
            context_window_tokens: 128_000,
            weight: 1,
        }
    }

    fn verifier_config(model: Option<ModelInfo>) -> ResearchVerifierConfig {
        ResearchVerifierConfig {
            enabled: true,
            model,
            max_rounds: 10,
            timeout: std::time::Duration::from_secs(5),
            max_evidence_docs: 30,
            max_excerpt_chars: 12_000,
        }
    }

    fn verifier_decision_json(verdict: &str) -> String {
        format!(
            r#"{{"verdict":"{verdict}","confidence":"high","summary":"checked draft","unsupported_claims":[{{"claim":"Model X has 97% F1 on Russian PII","reason":"metric is absent from evidence","required_evidence":"model card or benchmark text with the metric","suggested_next_action":"fetch the model card and benchmark page with crawl4ai_markdown"}}],"contradictions":[],"allowed_claims":[],"required_next_actions":["crawl4ai_markdown https://huggingface.co/example/model"]}}"#
        )
    }

    fn presenter_answer_text() -> String {
        "TL;DR: the checked sources do not provide enough proof for the requested comparison. The final answer therefore states what was checked, what remains unconfirmed, and what evidence is needed next.\n\nDetailed answer: no benchmark table, winner, speed number, prompt-eval number, VRAM number, license claim, or deployment recommendation is delivered unless it is explicitly present in the verified packet. Checked sources are referenced only as source metadata, and missing metrics are marked as not confirmed in the checked sources."
            .to_string()
    }

    fn hard_block_decision_json() -> String {
        r#"{"verdict":"block","confidence":"high","summary":"unsafe malformed answer must not be delivered","unsupported_claims":[],"contradictions":[{"claim":"deliver private secret","source_id":"doc-1","source_excerpt":"never disclose secrets"}],"allowed_claims":[],"required_next_actions":[]}"#
            .to_string()
    }

    fn verifier_llm_client_expect_mode(
        raw_response: String,
        expected_proof_not_found_mode: bool,
    ) -> Arc<LlmClient> {
        let settings = AgentSettings {
            agent_model_id: Some("agent-model".to_string()),
            agent_model_provider: Some("opencode-go".to_string()),
            research_verifier_model_id: Some("verifier-model".to_string()),
            research_verifier_model_provider: Some("opencode-go".to_string()),
            ..AgentSettings::default()
        };
        let mut provider = MockLlmProvider::new();
        provider
            .expect_complete_internal_text()
            .times(0..=MAX_PROOF_NOT_FOUND_VERIFY_ATTEMPTS)
            .returning(|system_prompt, _, user_message, model_id, _| {
                assert!(system_prompt.contains("final user-facing research answer"));
                assert!(user_message.contains("VerifiedResearchPacket"));
                assert_eq!(model_id, "verifier-model");
                Ok(presenter_answer_text())
            });
        provider.expect_chat_with_tools().times(1..=3).returning(
            move |request: ChatWithToolsRequest<'_>| {
                assert!(
                    request
                        .system_prompt
                        .contains("strict zero-trust answer verifier")
                );
                assert!(request.json_mode);
                assert_eq!(request.reasoning_effort, Some("disabled"));
                assert!(request.tools.is_empty());
                assert_eq!(request.model_id, "verifier-model");
                assert_eq!(request.messages.len(), 1);
                let user_message = &request.messages[0].content;
                assert!(user_message.contains("EvidenceDocument.content_excerpt"));
                assert!(user_message.contains("huggingface.co/example/model"));
                assert!(user_message.contains(&format!(
                    "\"proof_not_found_mode\": {expected_proof_not_found_mode}"
                )));
                Ok(ChatResponse {
                    content: Some(raw_response.clone()),
                    tool_calls: Vec::new(),
                    finish_reason: "stop".to_string(),
                    reasoning_content: None,
                    usage: None,
                })
            },
        );
        let mut llm = LlmClient::new(&settings);
        llm.register_provider("opencode-go".to_string(), Arc::new(provider));
        Arc::new(llm)
    }

    fn verifier_llm_client_expect_mode_sequence(responses: Vec<(String, bool)>) -> Arc<LlmClient> {
        let settings = AgentSettings {
            agent_model_id: Some("agent-model".to_string()),
            agent_model_provider: Some("opencode-go".to_string()),
            research_verifier_model_id: Some("verifier-model".to_string()),
            research_verifier_model_provider: Some("opencode-go".to_string()),
            ..AgentSettings::default()
        };
        let responses = Arc::new(responses);
        let calls = Arc::new(AtomicUsize::new(0));
        let responses_for_mock = Arc::clone(&responses);
        let calls_for_mock = Arc::clone(&calls);
        let mut provider = MockLlmProvider::new();
        provider
            .expect_complete_internal_text()
            .times(0..=MAX_PROOF_NOT_FOUND_VERIFY_ATTEMPTS)
            .returning(|system_prompt, _, user_message, model_id, _| {
                assert!(system_prompt.contains("final user-facing research answer"));
                assert!(user_message.contains("VerifiedResearchPacket"));
                assert_eq!(model_id, "verifier-model");
                Ok(presenter_answer_text())
            });
        provider
            .expect_chat_with_tools()
            .times(responses.len())
            .returning(move |request: ChatWithToolsRequest<'_>| {
                assert!(request.json_mode);
                assert_eq!(request.reasoning_effort, Some("disabled"));
                assert_eq!(request.messages.len(), 1);
                let index = calls_for_mock.fetch_add(1, Ordering::SeqCst);
                let (raw_response, expected_proof_not_found_mode) = &responses_for_mock[index];
                let user_message = &request.messages[0].content;
                assert!(user_message.contains("EvidenceDocument.content_excerpt"));
                assert!(user_message.contains(&format!(
                    "\"proof_not_found_mode\": {expected_proof_not_found_mode}"
                )));
                if *expected_proof_not_found_mode {
                    let payload: serde_json::Value =
                        serde_json::from_str(user_message).expect("verifier request JSON");
                    let final_answer = payload["final_answer"]
                        .as_str()
                        .expect("final answer string");
                    assert!(final_answer.chars().count() <= MAX_VERIFIER_FINAL_ANSWER_CHARS);
                    assert!(final_answer.starts_with("TL;DR:"));
                }
                Ok(ChatResponse {
                    content: Some(raw_response.clone()),
                    tool_calls: Vec::new(),
                    finish_reason: "stop".to_string(),
                    reasoning_content: None,
                    usage: None,
                })
            });
        let mut llm = LlmClient::new(&settings);
        llm.register_provider("opencode-go".to_string(), Arc::new(provider));
        Arc::new(llm)
    }

    fn verifier_llm_client_retryable_empty_response() -> Arc<LlmClient> {
        let settings = AgentSettings {
            agent_model_id: Some("agent-model".to_string()),
            agent_model_provider: Some("opencode-go".to_string()),
            research_verifier_model_id: Some("verifier-model".to_string()),
            research_verifier_model_provider: Some("opencode-go".to_string()),
            ..AgentSettings::default()
        };
        let mut provider = MockLlmProvider::new();
        provider.expect_complete_internal_text().times(1).returning(
            |system_prompt, _, user_message, model_id, _| {
                assert!(system_prompt.contains("final user-facing research answer"));
                assert!(user_message.contains("VerifiedResearchPacket"));
                assert_eq!(model_id, "verifier-model");
                Ok(presenter_answer_text())
            },
        );
        provider.expect_chat_with_tools().times(3).returning(
            |request: ChatWithToolsRequest<'_>| {
                assert!(request.json_mode);
                assert_eq!(request.reasoning_effort, Some("disabled"));
                Err(LlmError::EmptyResponse(
                    " verifier returned no text".to_string(),
                ))
            },
        );
        let mut llm = LlmClient::new(&settings);
        llm.register_provider("opencode-go".to_string(), Arc::new(provider));
        Arc::new(llm)
    }

    fn verifier_llm_client_expect_compact_proof_payload() -> Arc<LlmClient> {
        let settings = AgentSettings {
            agent_model_id: Some("agent-model".to_string()),
            agent_model_provider: Some("opencode-go".to_string()),
            research_verifier_model_id: Some("verifier-model".to_string()),
            research_verifier_model_provider: Some("opencode-go".to_string()),
            ..AgentSettings::default()
        };
        let mut provider = MockLlmProvider::new();
        provider.expect_complete_internal_text().times(1).returning(
            |system_prompt, _, user_message, model_id, _| {
                assert!(system_prompt.contains("final user-facing research answer"));
                assert!(user_message.contains("VerifiedResearchPacket"));
                assert_eq!(model_id, "verifier-model");
                Ok(presenter_answer_text())
            },
        );
        provider.expect_chat_with_tools().times(1).returning(
            |request: ChatWithToolsRequest<'_>| {
                assert!(request.json_mode);
                assert_eq!(request.reasoning_effort, Some("disabled"));
                let payload: serde_json::Value = serde_json::from_str(&request.messages[0].content)
                    .expect("verifier request JSON");
                assert_eq!(payload["proof_not_found_mode"], true);
                let docs = payload["evidence_documents"]
                    .as_array()
                    .expect("evidence docs array");
                assert_eq!(docs.len(), PROOF_NOT_FOUND_MAX_EVIDENCE_DOCS);
                for doc in docs {
                    let excerpt = doc["content_excerpt"]
                        .as_str()
                        .expect("content excerpt string");
                    assert!(excerpt.chars().count() <= PROOF_NOT_FOUND_MAX_EXCERPT_CHARS);
                }
                assert!(!request.messages[0].content.contains("doc-4"));
                Ok(ChatResponse {
                    content: Some(verifier_decision_json("proof_not_found")),
                    tool_calls: Vec::new(),
                    finish_reason: "stop".to_string(),
                    reasoning_content: None,
                    usage: None,
                })
            },
        );
        let mut llm = LlmClient::new(&settings);
        llm.register_provider("opencode-go".to_string(), Arc::new(provider));
        Arc::new(llm)
    }

    fn runtime_with_evidence_document() -> Arc<ResearchRuntime> {
        runtime_with_evidence_documents(1, 128)
    }

    fn runtime_with_evidence_documents(count: usize, excerpt_chars: usize) -> Arc<ResearchRuntime> {
        let runtime = Arc::new(ResearchRuntime::new());
        let now = Utc::now();
        for index in 0..count {
            let identity = ToolOutputIdentity {
                tool_call_id: ToolCallId::from(format!("call-{}", index + 1)),
                provider_tool_call_id: None,
                invocation_id: InvocationId::new(format!("invocation-{}", index + 1)),
                tool_name: ToolName::from("crawl4ai_markdown"),
                batch_index: index,
            };
            let mut output = ToolOutput::terminal(
                identity,
                ToolOutputStatus::Success,
                now,
                now,
                OutputTruncationMetadata::new(4096, 4096, 4096),
            );
            let markdown = format!(
                "# Example model {}\nLicense: Apache 2.0\nRussian PII support is documented.\n{}",
                index + 1,
                "A".repeat(excerpt_chars)
            );
            output.structured_payload = Some(json!({
                "provider": "crawl4ai_markdown",
                "kind": "fetch",
                "url": format!("https://huggingface.co/example/model-{}", index + 1),
                "final_url": format!("https://huggingface.co/example/model-{}", index + 1),
                "status_code": 200,
                "markdown": markdown,
                "source_kind": "model_card",
                "fetched_at": "2026-06-10T18:00:00Z"
            }));
            runtime.record_tool_output(&output);
        }
        runtime
    }

    async fn run_final_with_verifier_response(
        raw_response: String,
    ) -> (
        Result<Option<AgentRunResult>, String>,
        RunState,
        Vec<AgentMessage>,
        Vec<crate::llm::Message>,
    ) {
        run_final_with_verifier_response_and_state(raw_response, RunState::new(), false).await
    }

    async fn run_final_with_verifier_response_and_state(
        raw_response: String,
        state: RunState,
        expected_proof_not_found_mode: bool,
    ) -> (
        Result<Option<AgentRunResult>, String>,
        RunState,
        Vec<AgentMessage>,
        Vec<crate::llm::Message>,
    ) {
        run_final_with_verifier_response_and_state_and_answer(
            raw_response,
            state,
            expected_proof_not_found_mode,
            SUBSTANTIVE_FINAL,
        )
        .await
    }

    async fn run_final_with_verifier_response_and_state_and_answer(
        raw_response: String,
        state: RunState,
        expected_proof_not_found_mode: bool,
        final_answer: &str,
    ) -> (
        Result<Option<AgentRunResult>, String>,
        RunState,
        Vec<AgentMessage>,
        Vec<crate::llm::Message>,
    ) {
        let llm_client =
            verifier_llm_client_expect_mode(raw_response, expected_proof_not_found_mode);
        run_final_with_llm_client_state_and_answer(
            llm_client,
            state,
            final_answer,
            runtime_with_evidence_document(),
        )
        .await
    }

    async fn run_final_with_llm_client_state_and_answer(
        llm_client: Arc<LlmClient>,
        mut state: RunState,
        final_answer: &str,
        research_runtime: Arc<ResearchRuntime>,
    ) -> (
        Result<Option<AgentRunResult>, String>,
        RunState,
        Vec<AgentMessage>,
        Vec<crate::llm::Message>,
    ) {
        let mut runner = AgentRunner::new(llm_client);
        let mut session = EphemeralSession::new(4096);
        let todos_arc = Arc::new(Mutex::new(TodoList::new()));
        let tools = Vec::new();
        let mut messages = Vec::new();
        let mut ctx = AgentRunnerContext {
            task: "produce verified report",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "strict-verifier-final",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            research_runtime: Some(research_runtime),
            config: AgentRunnerConfig::new("test-model".to_string(), 8, 4, 60, 4096)
                .with_research_verifier_config(Some(verifier_config(Some(verifier_model())))),
        };
        let result = runner
            .handle_final_response(
                &mut ctx,
                &mut state,
                FinalResponseInput {
                    final_answer: final_answer.to_string(),
                    reasoning: None,
                },
            )
            .await
            .map_err(|error| error.to_string());
        let memory = ctx.agent.memory().get_messages().to_vec();
        let messages = ctx.messages.clone();
        (result, state, memory, messages)
    }

    #[test]
    fn salvage_detector_accepts_plain_final_prose() {
        assert!(should_salvage_structured_output_failure(
            "**TL;DR**\n\nВ Молдове есть официальный режим для digital nomad с минимальным доходом около 52 200 MDL в месяц."
        ));
    }

    #[test]
    fn salvage_detector_rejects_json_like_or_truncated_content() {
        assert!(!should_salvage_structured_output_failure(
            r#"{"final_answer":"hello"}"#
        ));
        assert!(!should_salvage_structured_output_failure(
            "```json\n{}\n```"
        ));
        assert!(!should_salvage_structured_output_failure("tool_call:"));
        assert!(!should_salvage_structured_output_failure("short answer"));
    }

    #[tokio::test]
    async fn strict_verifier_allow_delivers_final_response() {
        let (result, _state, memory, _messages) =
            run_final_with_verifier_response(verifier_decision_json("allow")).await;

        match result.expect("allow should succeed") {
            Some(AgentRunResult::Final(response)) => {
                assert_eq!(response, SUBSTANTIVE_FINAL);
            }
            _ => panic!("expected delivered final response"),
        }
        assert!(memory.iter().any(|message| {
            message.resolved_kind() == AgentMessageKind::AssistantResponse
                && message.content == SUBSTANTIVE_FINAL
        }));
    }

    #[tokio::test]
    async fn strict_verifier_revise_forces_iteration_with_claim_context() {
        let (result, state, memory, messages) =
            run_final_with_verifier_response(verifier_decision_json("revise")).await;

        assert!(result.expect("revise should continue").is_none());
        assert_eq!(state.continuation_count, 1);
        assert!(memory.iter().any(|message| {
            message.resolved_kind() == AgentMessageKind::UndeliveredAssistantDraft
                && message.content.contains("Model X has 97% F1")
        }));
        assert!(!memory.iter().any(|message| {
            message.resolved_kind() == AgentMessageKind::AssistantResponse
                && message.content.contains("Model X has 97% F1")
        }));
        assert!(messages.iter().any(|message| {
            message.role == "system"
                && message.content.contains("metric is absent from evidence")
                && message
                    .content
                    .contains("crawl4ai_markdown https://huggingface.co/example/model")
        }));
    }

    #[tokio::test]
    async fn strict_verifier_revise_records_visible_audit_trace() {
        let llm_client = verifier_llm_client_expect_mode(verifier_decision_json("revise"), false);
        let mut runner = AgentRunner::new(llm_client);
        let mut session = EphemeralSession::new(4096);
        let todos_arc = Arc::new(Mutex::new(TodoList::new()));
        let tools = Vec::new();
        let mut messages = Vec::new();
        let research_runtime = runtime_with_evidence_document();
        let (progress_tx, mut progress_rx) = mpsc::channel(8);
        let mut ctx = AgentRunnerContext {
            task: "produce verified report",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: Some(&progress_tx),
            todos_arc: &todos_arc,
            task_id: "strict-verifier-trace",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            research_runtime: Some(Arc::clone(&research_runtime)),
            config: AgentRunnerConfig::new("test-model".to_string(), 8, 4, 60, 4096)
                .with_research_verifier_config(Some(verifier_config(Some(verifier_model())))),
        };
        let mut state = RunState::new();

        let result = runner
            .handle_final_response(
                &mut ctx,
                &mut state,
                FinalResponseInput {
                    final_answer: SUBSTANTIVE_FINAL.to_string(),
                    reasoning: None,
                },
            )
            .await
            .expect("revise should continue");

        assert!(result.is_none());
        let audit = research_runtime.audit_payload();
        assert_eq!(audit["final_verifier_trace"]["verdict"], "revise");
        assert_eq!(audit["final_verifier_trace"]["outcome"], "continue");
        assert_eq!(audit["final_verifier_trace"]["evidence_document_count"], 1);
        assert_eq!(audit["final_verifier_trace"]["unsupported_claim_count"], 1);
        assert_eq!(
            audit["final_verifier_trace"]["required_next_actions"][0],
            "crawl4ai_markdown https://huggingface.co/example/model"
        );

        let event = progress_rx
            .recv()
            .await
            .expect("final draft event should be emitted");
        match event {
            AgentEvent::FinalDraftPendingVerification {
                content_chars,
                round,
                ..
            } => {
                assert_eq!(content_chars, SUBSTANTIVE_FINAL.chars().count());
                assert_eq!(round, 1);
            }
            other => panic!("expected final draft event, got {other:?}"),
        }

        let event = progress_rx
            .recv()
            .await
            .expect("verifier started event should be emitted");
        match event {
            AgentEvent::ResearchVerificationStarted {
                round,
                max_rounds,
                proof_not_found_mode,
                evidence_document_count,
            } => {
                assert_eq!(round, 1);
                assert_eq!(max_rounds, 10);
                assert!(!proof_not_found_mode);
                assert_eq!(evidence_document_count, 1);
            }
            other => panic!("expected verifier started event, got {other:?}"),
        }

        let event = progress_rx
            .recv()
            .await
            .expect("verifier trace event should be emitted");
        match event {
            AgentEvent::ResearchVerification { payload } => {
                assert_eq!(payload["verdict"], "revise");
                assert_eq!(payload["outcome"], "continue");
                assert_eq!(payload["unsupported_claim_count"], 1);
            }
            other => panic!("expected research verification event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn strict_verifier_need_more_evidence_forces_iteration() {
        let (result, state, memory, messages) =
            run_final_with_verifier_response(verifier_decision_json("need_more_evidence")).await;

        assert!(
            result
                .expect("need_more_evidence should continue")
                .is_none()
        );
        assert_eq!(state.continuation_count, 1);
        assert!(memory.iter().any(|message| {
            message.resolved_kind() == AgentMessageKind::UndeliveredAssistantDraft
        }));
        assert!(messages.iter().any(|message| {
            message.role == "system"
                && message
                    .content
                    .contains("Strict answer verifier requires more proof evidence")
        }));
    }

    #[tokio::test]
    async fn strict_verifier_recoverable_block_forces_iteration_with_required_actions() {
        let (result, state, memory, messages) =
            run_final_with_verifier_response(verifier_decision_json("block")).await;

        assert!(result.expect("recoverable block should continue").is_none());
        assert_eq!(state.continuation_count, 1);
        assert!(memory.iter().any(|message| {
            message.resolved_kind() == AgentMessageKind::UndeliveredAssistantDraft
                && message.content.contains("Model X has 97% F1")
        }));
        assert!(messages.iter().any(|message| {
            message.role == "system"
                && message.content.contains("recovery from a blocked draft")
                && message.content.contains("Mandatory required actions")
                && message
                    .content
                    .contains("crawl4ai_markdown https://huggingface.co/example/model")
                && message.content.contains("prove or remove")
        }));
    }

    #[tokio::test]
    async fn strict_verifier_retryable_provider_error_forces_shorter_retry_iteration() {
        let llm_client = verifier_llm_client_retryable_empty_response();
        let mut runner = AgentRunner::new(llm_client);
        let mut session = EphemeralSession::new(4096);
        let todos_arc = Arc::new(Mutex::new(TodoList::new()));
        let tools = Vec::new();
        let mut messages = Vec::new();
        let research_runtime = runtime_with_evidence_document();
        let mut ctx = AgentRunnerContext {
            task: "produce verified report",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "strict-verifier-transient-error",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            research_runtime: Some(Arc::clone(&research_runtime)),
            config: AgentRunnerConfig::new("test-model".to_string(), 8, 4, 60, 4096)
                .with_research_verifier_config(Some(verifier_config(Some(verifier_model())))),
        };
        let mut state = RunState::new();

        let result = runner
            .handle_final_response(
                &mut ctx,
                &mut state,
                FinalResponseInput {
                    final_answer: SUBSTANTIVE_FINAL.to_string(),
                    reasoning: None,
                },
            )
            .await
            .expect("retryable verifier provider error should continue");

        assert!(result.is_none());
        assert_eq!(state.continuation_count, 1);
        assert!(ctx.agent.memory().get_messages().iter().any(|message| {
            message.resolved_kind() == AgentMessageKind::UndeliveredAssistantDraft
                && message.content.contains("Model X has 97% F1")
        }));
        assert!(ctx.messages.iter().any(|message| {
            message.role == "system"
                && message.content.contains("shorter final answer")
                && message.content.contains("remove unsupported rankings")
        }));
        let audit = research_runtime.audit_payload();
        assert_eq!(audit["final_verifier_trace"]["outcome"], "continue");
        assert!(
            audit["final_verifier_trace"]["error"]
                .as_str()
                .expect("error string")
                .contains("strict answer verifier provider failed")
        );
    }

    #[tokio::test]
    async fn strict_verifier_short_final_answer_forces_substantive_iteration_without_provider_call()
    {
        let settings = AgentSettings {
            agent_model_id: Some("agent-model".to_string()),
            agent_model_provider: Some("opencode-go".to_string()),
            research_verifier_model_id: Some("verifier-model".to_string()),
            research_verifier_model_provider: Some("opencode-go".to_string()),
            ..AgentSettings::default()
        };
        let mut provider = MockLlmProvider::new();
        provider.expect_chat_with_tools().times(0);
        let mut llm = LlmClient::new(&settings);
        llm.register_provider("opencode-go".to_string(), Arc::new(provider));

        let mut runner = AgentRunner::new(Arc::new(llm));
        let mut session = EphemeralSession::new(4096);
        let todos_arc = Arc::new(Mutex::new(TodoList::new()));
        let tools = Vec::new();
        let mut messages = Vec::new();
        let mut ctx = AgentRunnerContext {
            task: "produce verified report",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "strict-verifier-short-final",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            research_runtime: Some(runtime_with_evidence_document()),
            config: AgentRunnerConfig::new("test-model".to_string(), 8, 4, 60, 4096)
                .with_research_verifier_config(Some(verifier_config(Some(verifier_model())))),
        };
        let mut state = RunState::new();

        let result = runner
            .handle_final_response(
                &mut ctx,
                &mut state,
                FinalResponseInput {
                    final_answer: "done".to_string(),
                    reasoning: None,
                },
            )
            .await
            .expect("short final should continue without verifier call");

        assert!(result.is_none());
        assert_eq!(state.continuation_count, 1);
        assert!(ctx.messages.iter().any(|message| {
            message.role == "system" && message.content.contains("substantive final answer")
        }));
    }

    #[tokio::test]
    async fn research_status_only_final_is_forced_to_continue_before_verifier() {
        let settings = AgentSettings {
            agent_model_id: Some("agent-model".to_string()),
            agent_model_provider: Some("opencode-go".to_string()),
            research_verifier_model_id: Some("verifier-model".to_string()),
            research_verifier_model_provider: Some("opencode-go".to_string()),
            ..AgentSettings::default()
        };
        let mut provider = MockLlmProvider::new();
        provider.expect_chat_with_tools().times(0);
        let mut llm = LlmClient::new(&settings);
        llm.register_provider("opencode-go".to_string(), Arc::new(provider));

        let mut runner = AgentRunner::new(Arc::new(llm));
        runner.register_hook(Box::new(CompletionCheckHook::new()));
        let mut session = EphemeralSession::new(4096);
        let mut todos = TodoList::new();
        todos.items.push(TodoItem {
            description: "Research comparison".to_string(),
            status: TodoStatus::Completed,
        });
        let todos_arc = Arc::new(Mutex::new(todos));
        let tools = Vec::new();
        let mut messages = Vec::new();
        let mut ctx = AgentRunnerContext {
            task: "compare two models with speed, prompt eval, VRAM and a table",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "research-status-only-final",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            research_runtime: Some(runtime_with_evidence_document()),
            config: AgentRunnerConfig::new("test-model".to_string(), 8, 4, 60, 4096)
                .with_research_verifier_config(Some(verifier_config(Some(verifier_model())))),
        };
        let mut state = RunState::new();

        let result = runner
            .handle_final_response(
                &mut ctx,
                &mut state,
                FinalResponseInput {
                    final_answer: "Отчёт готов. Если хотите, могу дополнительно проверить конкретные цифры на RTX 4090, разобрать DiffusionGemma и дать команду для vLLM запуска.".to_string(),
                    reasoning: None,
                },
            )
            .await
            .expect("status-only research final should continue before verifier");

        assert!(result.is_none());
        assert_eq!(state.continuation_count, 1);
        assert!(ctx.messages.iter().any(|message| {
            message.role == "system"
                && message.content.contains("status/offer")
                && message.content.contains("Start with TL;DR")
        }));
        assert!(!ctx.agent.memory().get_messages().iter().any(|message| {
            message.resolved_kind() == AgentMessageKind::AssistantResponse
                && message.content.contains("Отчёт готов")
        }));
    }

    #[tokio::test]
    async fn strict_verifier_block_does_not_deliver_final_response() {
        let (result, _state, memory, _messages) =
            run_final_with_verifier_response(hard_block_decision_json()).await;

        let error = match result {
            Ok(_) => panic!("block should fail closed"),
            Err(error) => error,
        };
        assert!(error.contains("blocked final response"));
        assert!(!memory.iter().any(|message| {
            message.resolved_kind() == AgentMessageKind::AssistantResponse
                && message.content.contains("Model X has 97% F1")
        }));
    }

    #[tokio::test]
    async fn strict_verifier_invalid_json_does_not_deliver_final_response() {
        let (result, _state, memory, _messages) =
            run_final_with_verifier_response("not json".to_string()).await;

        let error = match result {
            Ok(_) => panic!("invalid verifier JSON should fail closed"),
            Err(error) => error,
        };
        assert!(error.contains("strict answer verifier failed closed"));
        assert!(!memory.iter().any(|message| {
            message.resolved_kind() == AgentMessageKind::AssistantResponse
                && message.content.contains("Model X has 97% F1")
        }));
        assert!(memory.iter().any(|message| {
            message.resolved_kind() == AgentMessageKind::UndeliveredAssistantDraft
        }));
    }

    #[tokio::test]
    async fn strict_verifier_missing_route_fails_closed_without_delivery() {
        let settings = AgentSettings {
            agent_model_id: Some("agent-model".to_string()),
            agent_model_provider: Some("opencode-go".to_string()),
            ..AgentSettings::default()
        };
        let mut provider = MockLlmProvider::new();
        provider.expect_complete_internal_text().times(0);
        let mut llm = LlmClient::new(&settings);
        llm.register_provider("opencode-go".to_string(), Arc::new(provider));
        let mut runner = AgentRunner::new(Arc::new(llm));
        let mut session = EphemeralSession::new(4096);
        let todos_arc = Arc::new(Mutex::new(TodoList::new()));
        let tools = Vec::new();
        let mut messages = Vec::new();
        let mut ctx = AgentRunnerContext {
            task: "produce verified report",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "strict-verifier-missing-route",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            research_runtime: Some(runtime_with_evidence_document()),
            config: AgentRunnerConfig::new("test-model".to_string(), 8, 4, 60, 4096)
                .with_research_verifier_config(Some(verifier_config(None))),
        };
        let mut state = RunState::new();

        let error = match runner
            .handle_final_response(
                &mut ctx,
                &mut state,
                FinalResponseInput {
                    final_answer: SUBSTANTIVE_FINAL.to_string(),
                    reasoning: None,
                },
            )
            .await
        {
            Ok(_) => panic!("missing verifier route should fail closed"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("route is not configured"));
        assert!(
            !ctx.agent
                .memory()
                .get_messages()
                .iter()
                .any(|message| { message.resolved_kind() == AgentMessageKind::AssistantResponse })
        );
    }

    #[tokio::test]
    async fn strict_verifier_exhaustion_forces_constrained_proof_not_found_report() {
        let mut state = RunState::new();
        state.verifier_rounds = 9;
        let llm_client = verifier_llm_client_expect_mode_sequence(vec![
            (verifier_decision_json("need_more_evidence"), false),
            (verifier_decision_json("proof_not_found"), true),
        ]);
        let (result, state, memory, messages) = run_final_with_llm_client_state_and_answer(
            llm_client,
            state,
            SUBSTANTIVE_FINAL,
            runtime_with_evidence_document(),
        )
        .await;

        let final_response = match result.expect("exhaustion should deliver presenter report") {
            Some(AgentRunResult::Final(response)) => response,
            _ => panic!("expected proof-not-found presenter final response"),
        };
        assert_eq!(state.continuation_count, 0);
        assert!(state.proof_not_found_report_requested);
        assert_eq!(state.proof_not_found_verify_attempts, 1);
        assert!(final_response.starts_with("TL;DR:"));
        assert!(memory.iter().any(|message| {
            message.resolved_kind() == AgentMessageKind::UndeliveredAssistantDraft
        }));
        assert!(!messages.iter().any(|message| {
            message.role == "system" && message.content.contains("proof_not_found_mode")
        }));
    }

    #[tokio::test]
    async fn strict_verifier_proof_not_found_mode_delivers_verified_report() {
        let mut state = RunState::new();
        state.continuation_count = 10;
        state.proof_not_found_report_requested = true;
        let report = "Проверка завершена: достаточные пруфы не найдены\n\nЧто проверено: https://huggingface.co/example/model and fetched model-card excerpts.\nЧто подтверждено: лицензия Apache 2.0 is present in the fetched evidence excerpt.\nЧто не подтверждено: 97% F1 на русском PII, ranking, production suitability, and deployment recommendation were not directly confirmed.\nБезопасный следующий шаг: fetch official benchmark text before asserting numeric performance.";
        let (result, _state, memory, _messages) =
            run_final_with_verifier_response_and_state_and_answer(
                verifier_decision_json("proof_not_found"),
                state,
                true,
                report,
            )
            .await;

        match result.expect("verified proof_not_found report should deliver") {
            Some(AgentRunResult::Final(response)) => {
                assert_ne!(response, report);
                assert!(response.starts_with("TL;DR:"));
            }
            _ => panic!("expected proof-not-found final response"),
        }
        assert!(memory.iter().any(|message| {
            message.resolved_kind() == AgentMessageKind::AssistantResponse
                && message.content.starts_with("TL;DR:")
        }));
    }

    #[tokio::test]
    async fn strict_verifier_proof_not_found_mode_uses_compact_payload() {
        let llm_client = verifier_llm_client_expect_compact_proof_payload();
        let mut runner = AgentRunner::new(llm_client);
        let mut session = EphemeralSession::new(4096);
        let todos_arc = Arc::new(Mutex::new(TodoList::new()));
        let tools = Vec::new();
        let mut messages = Vec::new();
        let mut ctx = AgentRunnerContext {
            task: "produce verified proof-not-found report",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "strict-verifier-compact-proof",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            research_runtime: Some(runtime_with_evidence_documents(7, 4_000)),
            config: AgentRunnerConfig::new("test-model".to_string(), 8, 4, 60, 4096)
                .with_research_verifier_config(Some(verifier_config(Some(verifier_model())))),
        };
        let mut state = RunState::new();
        state.continuation_count = 10;
        state.proof_not_found_report_requested = true;
        let report = "Проверка завершена: достаточные пруфы не найдены\nconfirmed: license text was fetched from source 1 and model-card availability was checked from source 2.\nnot confirmed: speed, prompt eval, VRAM numbers, ranking, production readiness, and Russian suitability claims were not directly confirmed.\nclaims removed: benchmark table and unsupported numeric comparisons.";

        let result = runner
            .handle_final_response(
                &mut ctx,
                &mut state,
                FinalResponseInput {
                    final_answer: report.to_string(),
                    reasoning: None,
                },
            )
            .await
            .expect("verified compact proof report should deliver");

        assert!(matches!(result, Some(AgentRunResult::Final(_))));
    }

    #[tokio::test]
    async fn strict_verifier_proof_not_found_transient_error_allows_one_short_retry() {
        let mut state = RunState::new();
        state.continuation_count = 10;
        state.proof_not_found_report_requested = true;
        let (result, state, memory, messages) = run_final_with_llm_client_state_and_answer(
            verifier_llm_client_retryable_empty_response(),
            state,
            SUBSTANTIVE_FINAL,
            runtime_with_evidence_document(),
        )
        .await;

        match result.expect("proof-not-found verifier failure should return safe notice") {
            Some(AgentRunResult::Final(response)) => {
                assert!(response.starts_with("Проверка закрыта без выдачи предметного отчета"));
                assert!(!response.contains("Model X has 97% F1"));
            }
            _ => panic!("expected fail-closed notice"),
        }
        assert_eq!(state.proof_not_found_verify_attempts, 1);
        assert!(!state.proof_not_found_repair_attempt_used);
        assert!(memory.iter().any(|message| {
            message.resolved_kind() == AgentMessageKind::AssistantResponse
                && message
                    .content
                    .starts_with("Проверка закрыта без выдачи предметного отчета")
        }));
        assert!(!messages.iter().any(|message| {
            message.role == "system" && message.content.contains("under 3500 chars")
        }));
    }

    #[tokio::test]
    async fn strict_verifier_proof_not_found_long_report_gets_one_short_retry() {
        let mut state = RunState::new();
        state.continuation_count = 10;
        state.proof_not_found_report_requested = true;
        let long_report = format!(
            "Проверка завершена: достаточные пруфы не найдены\n{}",
            "x".repeat(PROOF_NOT_FOUND_MAX_REPORT_CHARS + 1)
        );
        let (result, state, _memory, _messages) = run_final_with_llm_client_state_and_answer(
            verifier_llm_client_expect_mode(verifier_decision_json("proof_not_found"), true),
            state,
            &long_report,
            runtime_with_evidence_document(),
        )
        .await;

        match result.expect("long root proof report should be ignored") {
            Some(AgentRunResult::Final(response)) => {
                assert!(response.starts_with("TL;DR:"));
                assert_ne!(response, long_report);
            }
            _ => panic!("expected proof-not-found presenter final response"),
        }
        assert!(!state.proof_not_found_repair_attempt_used);
    }

    #[tokio::test]
    async fn strict_verifier_proof_not_found_mode_accepts_allow_report() {
        let mut state = RunState::new();
        state.continuation_count = 10;
        state.proof_not_found_report_requested = true;
        let (result, _state, memory, _messages) = run_final_with_verifier_response_and_state(
            verifier_decision_json("allow"),
            state,
            true,
        )
        .await;

        assert!(matches!(
            result.expect("allow in proof_not_found mode should deliver"),
            Some(AgentRunResult::Final(_))
        ));
        assert!(
            memory
                .iter()
                .any(|message| { message.resolved_kind() == AgentMessageKind::AssistantResponse })
        );
    }

    #[tokio::test]
    async fn strict_verifier_proof_not_found_mode_blocks_unsupported_report() {
        let mut state = RunState::new();
        state.continuation_count = 10;
        state.proof_not_found_report_requested = true;
        let (result, _state, memory, _messages) = run_final_with_verifier_response_and_state(
            verifier_decision_json("revise"),
            state,
            true,
        )
        .await;

        match result.expect("unsupported proof report should return safe notice") {
            Some(AgentRunResult::Final(response)) => {
                assert!(response.starts_with("Проверка закрыта без выдачи предметного отчета"));
                assert!(!response.contains("Model X has 97% F1"));
            }
            _ => panic!("expected fail-closed notice"),
        }
        assert!(memory.iter().any(|message| {
            message.resolved_kind() == AgentMessageKind::AssistantResponse
                && message
                    .content
                    .starts_with("Проверка закрыта без выдачи предметного отчета")
        }));
    }

    #[tokio::test]
    async fn strict_verifier_proof_not_found_revise_triggers_one_deterministic_repair() {
        let mut state = RunState::new();
        state.proof_not_found_report_requested = true;
        let llm_client = verifier_llm_client_expect_mode_sequence(vec![
            (verifier_decision_json("revise"), true),
            (verifier_decision_json("proof_not_found"), true),
        ]);
        let (result, state, _memory, _messages) = run_final_with_llm_client_state_and_answer(
            llm_client,
            state,
            SUBSTANTIVE_FINAL,
            runtime_with_evidence_document(),
        )
        .await;

        match result.expect("repair should verify") {
            Some(AgentRunResult::Final(response)) => {
                assert!(response.starts_with("TL;DR:"));
                assert!(!response.contains("Model X has 97% F1"));
            }
            _ => panic!("expected proof-not-found presenter final response"),
        }
        assert!(state.proof_not_found_repair_used);
        assert_eq!(state.proof_not_found_verify_attempts, 2);
    }

    #[tokio::test]
    async fn strict_verifier_proof_not_found_no_evidence_docs_delivers_no_docs_report() {
        let mut state = RunState::new();
        state.proof_not_found_report_requested = true;
        let (result, _state, _memory, _messages) = run_final_with_llm_client_state_and_answer(
            verifier_llm_client_expect_mode_sequence(vec![(
                verifier_decision_json("proof_not_found"),
                true,
            )]),
            state,
            SUBSTANTIVE_FINAL,
            Arc::new(ResearchRuntime::new()),
        )
        .await;

        match result.expect("proof-not-found report should deliver") {
            Some(AgentRunResult::Final(response)) => {
                assert!(response.starts_with("TL;DR:"));
                assert!(response.contains("not confirmed in the checked sources"));
            }
            _ => panic!("expected proof-not-found presenter final response"),
        }
    }

    #[tokio::test]
    async fn forced_final_response_is_saved_as_undelivered_draft() {
        let llm_client = build_llm_client(single_final_response_provider());
        let mut runner = AgentRunner::new(llm_client);
        runner.register_hook(Box::new(CompletionCheckHook::new()));

        let mut session = EphemeralSession::new(2048);
        let mut todos = TodoList::new();
        todos.items.push(TodoItem::new("finish work"));
        session.memory_mut().todos = todos.clone();
        let todos_arc = Arc::new(Mutex::new(todos));
        let tools = Vec::new();
        let mut messages = Vec::new();
        let mut ctx = AgentRunnerContext {
            task: "produce report",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "forced-final-draft",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            research_runtime: None,
            config: AgentRunnerConfig::new("test-model".to_string(), 8, 4, 60, 4096),
        };
        let mut state = RunState::new();
        let draft = "Full report generated before todos were complete.";

        let result = runner
            .handle_final_response(
                &mut ctx,
                &mut state,
                FinalResponseInput {
                    final_answer: draft.to_string(),
                    reasoning: None,
                },
            )
            .await
            .expect("forced final response should continue");

        assert!(result.is_none());
        let memory = ctx.agent.memory().get_messages();
        assert!(
            !memory.iter().any(|message| {
                message.resolved_kind() == AgentMessageKind::AssistantResponse
                    && message.content.contains(draft)
            }),
            "forced final response must not be stored as delivered assistant prose"
        );
        let draft_message = memory
            .iter()
            .find(|message| message.resolved_kind() == AgentMessageKind::UndeliveredAssistantDraft)
            .expect("undelivered draft should be recorded");
        assert!(draft_message.content.contains("not delivered to the user"));
        assert!(draft_message.content.contains(draft));
    }

    #[tokio::test]
    async fn undelivered_draft_context_is_capped_and_marked_untrusted() {
        let llm_client = build_llm_client(single_final_response_provider());
        let mut runner = AgentRunner::new(llm_client);
        runner.register_hook(Box::new(CompletionCheckHook::new()));

        let mut session = EphemeralSession::new(2048);
        let mut todos = TodoList::new();
        todos.items.push(TodoItem::new("finish work"));
        session.memory_mut().todos = todos.clone();
        let todos_arc = Arc::new(Mutex::new(todos));
        let tools = Vec::new();
        let mut messages = Vec::new();
        let mut ctx = AgentRunnerContext {
            task: "produce report",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "capped-undelivered-draft",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            research_runtime: None,
            config: AgentRunnerConfig::new("test-model".to_string(), 8, 4, 60, 4096),
        };
        let mut state = RunState::new();
        let draft = format!("{}END_MARKER", "x".repeat(13_000));

        let result = runner
            .handle_final_response(
                &mut ctx,
                &mut state,
                FinalResponseInput {
                    final_answer: draft,
                    reasoning: None,
                },
            )
            .await
            .expect("forced final response should continue");

        assert!(result.is_none());
        let draft_message = ctx
            .agent
            .memory()
            .get_messages()
            .iter()
            .find(|message| message.resolved_kind() == AgentMessageKind::UndeliveredAssistantDraft)
            .expect("undelivered draft should be recorded")
            .content
            .clone();
        assert!(draft_message.contains("untrusted and must not be repeated as fact"));
        assert!(draft_message.contains("Original chars: 13010"));
        assert!(!draft_message.contains("END_MARKER"));
        assert!(draft_message.chars().count() < 2_500);
    }

    #[tokio::test]
    async fn after_agent_finish_overrides_final_response() {
        let llm_client = build_llm_client(single_final_response_provider());
        let mut runner = AgentRunner::new(llm_client);
        runner.register_hook(Box::new(StaticAfterAgentHook {
            result: HookResult::Finish("hook supplied report".to_string()),
        }));

        let mut session = EphemeralSession::new(2048);
        let todos_arc = Arc::new(Mutex::new(TodoList::new()));
        let tools = Vec::new();
        let mut messages = Vec::new();
        let mut ctx = AgentRunnerContext {
            task: "produce report",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "after-agent-finish",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            research_runtime: None,
            config: AgentRunnerConfig::new("test-model".to_string(), 8, 4, 60, 4096),
        };
        let mut state = RunState::new();

        let result = runner
            .handle_final_response(
                &mut ctx,
                &mut state,
                FinalResponseInput {
                    final_answer: "original final".to_string(),
                    reasoning: Some("ignored reasoning".to_string()),
                },
            )
            .await
            .expect("finish hook should complete");

        match result {
            Some(AgentRunResult::Final(response)) => assert_eq!(response, "hook supplied report"),
            _ => panic!("expected overridden final response"),
        }
        let memory = ctx.agent.memory().get_messages();
        assert!(memory.iter().any(|message| {
            message.resolved_kind() == AgentMessageKind::AssistantResponse
                && message.content == "hook supplied report"
        }));
        assert!(!memory.iter().any(|message| {
            message.resolved_kind() == AgentMessageKind::AssistantResponse
                && message.content == "original final"
        }));
    }

    #[tokio::test]
    async fn after_agent_block_returns_error_without_saving_final_response() {
        let llm_client = build_llm_client(single_final_response_provider());
        let mut runner = AgentRunner::new(llm_client);
        runner.register_hook(Box::new(StaticAfterAgentHook {
            result: HookResult::Block {
                reason: "blocked final".to_string(),
            },
        }));

        let mut session = EphemeralSession::new(2048);
        let todos_arc = Arc::new(Mutex::new(TodoList::new()));
        let tools = Vec::new();
        let mut messages = Vec::new();
        let mut ctx = AgentRunnerContext {
            task: "produce report",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "after-agent-block",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            research_runtime: None,
            config: AgentRunnerConfig::new("test-model".to_string(), 8, 4, 60, 4096),
        };
        let mut state = RunState::new();

        let error = match runner
            .handle_final_response(
                &mut ctx,
                &mut state,
                FinalResponseInput {
                    final_answer: "original final".to_string(),
                    reasoning: None,
                },
            )
            .await
        {
            Ok(_) => panic!("block hook should fail final response"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("blocked final"));
        let memory = ctx.agent.memory().get_messages();
        assert!(
            !memory
                .iter()
                .any(|message| message.resolved_kind() == AgentMessageKind::AssistantResponse)
        );
    }

    #[tokio::test]
    async fn pending_final_draft_replaces_short_final_response() {
        let llm_client = build_llm_client(single_final_response_provider());
        let mut runner = AgentRunner::new(llm_client);
        let mut session = EphemeralSession::new(4096);
        let todos_arc = Arc::new(Mutex::new(TodoList::new()));
        let tools = Vec::new();
        let mut messages = Vec::new();
        let mut ctx = AgentRunnerContext {
            task: "produce report",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "pending-final-draft-replace",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            research_runtime: None,
            config: AgentRunnerConfig::new("test-model".to_string(), 8, 4, 60, 4096),
        };
        let long_draft = format!(
            "## Итоговый отчёт\n\n{}",
            "- Модель: https://huggingface.co/example/model — годна после проверки.\n".repeat(80)
        );
        let mut state = RunState::new();
        state.pending_final_draft =
            PendingFinalDraft::from_write_todos_content(long_draft.clone(), 18);

        let result = runner
            .handle_final_response(
                &mut ctx,
                &mut state,
                FinalResponseInput {
                    final_answer: "Ресёрч завершён. Если нужна детализация — спрашивай."
                        .to_string(),
                    reasoning: None,
                },
            )
            .await
            .expect("final response should be handled");

        match result {
            Some(AgentRunResult::Final(response)) => assert_eq!(response, long_draft.trim()),
            _ => panic!("expected final response"),
        }
        assert!(state.pending_final_draft.is_none());
        let memory = ctx.agent.memory().get_messages();
        assert!(memory.iter().any(|message| {
            message.resolved_kind() == AgentMessageKind::AssistantResponse
                && message.content == long_draft.trim()
        }));
    }

    #[tokio::test]
    async fn pending_final_draft_replaces_research_status_only_final_response() {
        let llm_client = build_llm_client(single_final_response_provider());
        let mut runner = AgentRunner::new(llm_client);
        let mut session = EphemeralSession::new(4096);
        let todos_arc = Arc::new(Mutex::new(TodoList::new()));
        let tools = Vec::new();
        let mut messages = Vec::new();
        let mut ctx = AgentRunnerContext {
            task: "produce report",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "pending-final-draft-replaces-meta",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            research_runtime: None,
            config: AgentRunnerConfig::new("test-model".to_string(), 8, 4, 60, 4096),
        };
        let long_draft = format!("## Verified report\n\n{}", "confirmed line\n".repeat(120));
        let status_only = "Отчёт готов. Если хотите, могу дополнительно проверить источники, разобрать детали, сравнить соседние варианты и дать команду для запуска. ".repeat(10);
        let mut state = RunState::new();
        state.pending_final_draft =
            PendingFinalDraft::from_write_todos_content(long_draft.clone(), 18);

        let result = runner
            .handle_final_response(
                &mut ctx,
                &mut state,
                FinalResponseInput {
                    final_answer: status_only,
                    reasoning: None,
                },
            )
            .await
            .expect("final response should be handled");

        match result {
            Some(AgentRunResult::Final(response)) => assert_eq!(response, long_draft.trim()),
            _ => panic!("expected final response"),
        }
        assert!(state.pending_final_draft.is_none());
    }

    #[tokio::test]
    async fn pending_final_draft_does_not_replace_substantive_final_response() {
        let llm_client = build_llm_client(single_final_response_provider());
        let mut runner = AgentRunner::new(llm_client);
        let mut session = EphemeralSession::new(4096);
        let todos_arc = Arc::new(Mutex::new(TodoList::new()));
        let tools = Vec::new();
        let mut messages = Vec::new();
        let mut ctx = AgentRunnerContext {
            task: "produce report",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "pending-final-draft-keep-stop",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            research_runtime: None,
            config: AgentRunnerConfig::new("test-model".to_string(), 8, 4, 60, 4096),
        };
        let draft = format!("## Старый draft\n\n{}", "draft line\n".repeat(120));
        let final_answer = format!("## Новый финальный ответ\n\n{}", "final line\n".repeat(120));
        let mut state = RunState::new();
        state.pending_final_draft = PendingFinalDraft::from_write_todos_content(draft, 18);

        let result = runner
            .handle_final_response(
                &mut ctx,
                &mut state,
                FinalResponseInput {
                    final_answer: final_answer.clone(),
                    reasoning: None,
                },
            )
            .await
            .expect("final response should be handled");

        match result {
            Some(AgentRunResult::Final(response)) => assert_eq!(response, final_answer),
            _ => panic!("expected final response"),
        }
        assert!(state.pending_final_draft.is_none());
        let memory = ctx.agent.memory().get_messages();
        assert!(memory.iter().any(|message| {
            message.resolved_kind() == AgentMessageKind::AssistantResponse
                && message.content == final_answer
        }));
    }
}
