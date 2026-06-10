//! Response handling for the agent runner.

use super::AgentRunner;
use super::types::{
    AgentRunResult, AgentRunnerContext, FinalResponseInput, RunState, StructuredOutputFailure,
};
use crate::agent::compaction::CompactionTrigger;
use crate::agent::memory::AgentMemory;
use crate::agent::progress::{AgentEvent, AgentEventSource};
use crate::agent::providers::TodoList;
use crate::agent::research::{
    AnswerVerificationDecision, AnswerVerificationError, AnswerVerificationRequest,
    AnswerVerifierVerdict, ResearchVerifierTrace, StrictAnswerVerifier, VerifierUnsupportedClaim,
};
use crate::agent::session::PendingUserInput;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

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

        let notice = format!(
            "[SYSTEM: The previous assistant final response was not delivered to the user. \
Reason: {reason}. Use it only as internal context; if any of it is needed for the user, \
include it explicitly in a later final_answer.]\n\nUndelivered draft:\n{trimmed}"
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
            if draft.should_replace_final_response(&final_response) {
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
            FinalVerificationOutcome::Deliver => {}
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
            return Ok(FinalVerificationOutcome::Deliver);
        }

        let Some(verifier_config) = ctx.config.research_verifier_config.clone() else {
            return Ok(FinalVerificationOutcome::Deliver);
        };
        if !verifier_config.enabled {
            return Ok(FinalVerificationOutcome::Deliver);
        }

        let Some(research_runtime) = ctx.research_runtime.as_deref() else {
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
        let max_rounds = verifier_config.max_rounds;
        let verifier = StrictAnswerVerifier::new(self.llm_client(), verifier_config);
        let round = state.continuation_count.saturating_add(1);
        let proof_not_found_mode = state.proof_not_found_report_requested;
        let evidence_document_count = research_snapshot.evidence_documents.len();
        let decision = verifier
            .verify(AnswerVerificationRequest {
                final_answer: final_response,
                research: &research_snapshot,
                round,
                proof_not_found_mode,
            })
            .await;

        match decision {
            Ok(decision) => {
                let outcome_result =
                    verifier_decision_outcome(&decision, proof_not_found_mode, round, max_rounds);
                let trace_outcome = match &outcome_result {
                    Ok((FinalVerificationOutcome::Deliver, _)) => "deliver",
                    Ok((FinalVerificationOutcome::Continue { .. }, _)) => "continue",
                    Err(_) => "fail_closed",
                };
                let trace = verifier_trace_from_decision(
                    &decision,
                    trace_outcome,
                    round,
                    max_rounds,
                    proof_not_found_mode,
                    evidence_document_count,
                );
                let payload = research_runtime.record_verifier_trace(trace);
                emit_research_verification_trace(ctx.progress_tx, payload).await;
                let (outcome, proof_not_found_requested) = outcome_result?;
                if proof_not_found_requested {
                    state.proof_not_found_report_requested = true;
                }
                Ok(outcome)
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
                    proof_not_found_mode,
                    evidence_document_count,
                    unsupported_claims: Vec::new(),
                    contradictions: Vec::new(),
                    required_next_actions: Vec::new(),
                });
                emit_research_verification_trace(ctx.progress_tx, payload).await;
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
    Deliver,
    Continue { reason: String, context: String },
}

fn verifier_decision_outcome(
    decision: &AnswerVerificationDecision,
    proof_not_found_mode: bool,
    round: usize,
    max_rounds: usize,
) -> anyhow::Result<(FinalVerificationOutcome, bool)> {
    if proof_not_found_mode {
        return verifier_proof_not_found_mode_outcome(decision).map(|outcome| (outcome, false));
    }

    match decision.verdict {
        AnswerVerifierVerdict::Allow => Ok((FinalVerificationOutcome::Deliver, false)),
        AnswerVerifierVerdict::Revise | AnswerVerifierVerdict::NeedMoreEvidence => {
            if round >= max_rounds.max(1) {
                return Ok((
                    FinalVerificationOutcome::Continue {
                        reason: "Strict answer verifier exhausted proof search".to_string(),
                        context: verifier_proof_not_found_context(decision, max_rounds.max(1)),
                    },
                    true,
                ));
            }
            Ok((
                FinalVerificationOutcome::Continue {
                    reason: verifier_retry_reason(decision.verdict),
                    context: verifier_retry_context(decision),
                },
                false,
            ))
        }
        AnswerVerifierVerdict::ProofNotFound => Err(anyhow::anyhow!(
            "strict answer verifier failed closed: proof_not_found verdict is only deliverable in constrained proof-not-found mode"
        )),
        AnswerVerifierVerdict::Block => Err(anyhow::anyhow!(
            "strict answer verifier blocked final response: {}",
            verifier_decision_summary(decision)
        )),
    }
}

fn verifier_proof_not_found_mode_outcome(
    decision: &AnswerVerificationDecision,
) -> anyhow::Result<FinalVerificationOutcome> {
    match decision.verdict {
        AnswerVerifierVerdict::Allow | AnswerVerifierVerdict::ProofNotFound => {
            Ok(FinalVerificationOutcome::Deliver)
        }
        AnswerVerifierVerdict::Revise
        | AnswerVerifierVerdict::NeedMoreEvidence
        | AnswerVerifierVerdict::Block => Err(anyhow::anyhow!(
            "strict answer verifier failed closed: constrained proof-not-found report was not verified: {}",
            verifier_decision_summary(decision)
        )),
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
    }

    sections.push(
        "When evidence cannot be found, do not invent it; continue gathering proof until the proof-not-found flow is requested."
            .to_string(),
    );
    sections.join("\n")
}

fn verifier_proof_not_found_context(
    decision: &AnswerVerificationDecision,
    max_rounds: usize,
) -> String {
    let mut sections = vec![
        format!(
            "The strict answer verifier reached RESEARCH_VERIFIER_MAX_ROUNDS={max_rounds} without enough proof for the previous draft."
        ),
        format!("Verifier summary: {}", decision.summary),
        "Do not try to deliver or prove the rejected draft anymore. Produce exactly one constrained proof-not-found final report.".to_string(),
        "The report must start exactly with: Проверка завершена: достаточные пруфы не найдены".to_string(),
        "The report must include: what was checked, what was confirmed by fetched proof documents, what was not confirmed, claims that cannot be asserted, and safe next steps.".to_string(),
        "The report must not recommend a model as usable, best, suitable, compliant, licensed, Russian-capable, or benchmarked unless that exact claim is directly supported by EvidenceDocument excerpts.".to_string(),
        "The next final answer will be verified in proof_not_found_mode and will be delivered only if the verifier returns proof_not_found or allow.".to_string(),
    ];

    if !decision.unsupported_claims.is_empty() {
        sections.push("Claims that exhausted proof search:".to_string());
        sections.extend(
            decision
                .unsupported_claims
                .iter()
                .enumerate()
                .map(|(index, claim)| unsupported_claim_context_line(index, claim)),
        );
    }

    if !decision.required_next_actions.is_empty() {
        sections.push("Actions already requested by verifier before exhaustion:".to_string());
        sections.extend(
            decision
                .required_next_actions
                .iter()
                .enumerate()
                .map(|(index, action)| format!("{}. {action}", index + 1)),
        );
    }

    sections.join("\n")
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
    use crate::agent::providers::{TodoItem, TodoList};
    use crate::agent::research::{ResearchRuntime, ResearchVerifierConfig};
    use crate::agent::runner::test_support::{build_llm_client, single_final_response_provider};
    use crate::agent::runner::types::{FinalResponseInput, PendingFinalDraft, RunState};
    use crate::agent::runner::{AgentRunnerConfig, AgentRunnerContext};
    use crate::agent::tool_runtime::{
        OutputTruncationMetadata, ToolCallId, ToolName, ToolOutput, ToolOutputIdentity,
        ToolOutputStatus,
    };
    use crate::config::{AgentSettings, ModelInfo};
    use crate::llm::{InvocationId, LlmClient, MockLlmProvider};
    use chrono::Utc;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::{Mutex, mpsc};

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
            .times(1..=3)
            .returning(
                move |system_prompt, _history, user_message, model_id, _max_tokens| {
                    assert!(system_prompt.contains("strict zero-trust answer verifier"));
                    assert!(user_message.contains("EvidenceDocument.content_excerpt"));
                    assert!(user_message.contains("huggingface.co/example/model"));
                    assert!(user_message.contains(&format!(
                        "\"proof_not_found_mode\": {expected_proof_not_found_mode}"
                    )));
                    assert_eq!(model_id, "verifier-model");
                    Ok(raw_response.clone())
                },
            );
        let mut llm = LlmClient::new(&settings);
        llm.register_provider("opencode-go".to_string(), Arc::new(provider));
        Arc::new(llm)
    }

    fn runtime_with_evidence_document() -> Arc<ResearchRuntime> {
        let runtime = Arc::new(ResearchRuntime::new());
        let now = Utc::now();
        let identity = ToolOutputIdentity {
            tool_call_id: ToolCallId::from("call-1"),
            provider_tool_call_id: None,
            invocation_id: InvocationId::new("invocation-1"),
            tool_name: ToolName::from("crawl4ai_markdown"),
            batch_index: 0,
        };
        let mut output = ToolOutput::terminal(
            identity,
            ToolOutputStatus::Success,
            now,
            now,
            OutputTruncationMetadata::new(4096, 4096, 4096),
        );
        output.structured_payload = Some(json!({
            "provider": "crawl4ai_markdown",
            "kind": "fetch",
            "url": "https://huggingface.co/example/model",
            "final_url": "https://huggingface.co/example/model",
            "status_code": 200,
            "markdown": "# Example model\nLicense: Apache 2.0\nRussian PII support is documented.",
            "source_kind": "model_card",
            "fetched_at": "2026-06-10T18:00:00Z"
        }));
        runtime.record_tool_output(&output);
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
            "Model X has 97% F1 on Russian PII.",
        )
        .await
    }

    async fn run_final_with_verifier_response_and_state_and_answer(
        raw_response: String,
        mut state: RunState,
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
            research_runtime: Some(runtime_with_evidence_document()),
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
                assert_eq!(response, "Model X has 97% F1 on Russian PII.");
            }
            _ => panic!("expected delivered final response"),
        }
        assert!(memory.iter().any(|message| {
            message.resolved_kind() == AgentMessageKind::AssistantResponse
                && message.content == "Model X has 97% F1 on Russian PII."
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
                    final_answer: "Model X has 97% F1 on Russian PII.".to_string(),
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
    async fn strict_verifier_block_does_not_deliver_final_response() {
        let (result, _state, memory, _messages) =
            run_final_with_verifier_response(verifier_decision_json("block")).await;

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
                    final_answer: "Model X has 97% F1 on Russian PII.".to_string(),
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
        state.continuation_count = 9;
        let (result, state, memory, messages) = run_final_with_verifier_response_and_state(
            verifier_decision_json("need_more_evidence"),
            state,
            false,
        )
        .await;

        assert!(result.expect("exhaustion should continue").is_none());
        assert_eq!(state.continuation_count, 10);
        assert!(state.proof_not_found_report_requested);
        assert!(memory.iter().any(|message| {
            message.resolved_kind() == AgentMessageKind::UndeliveredAssistantDraft
        }));
        assert!(messages.iter().any(|message| {
            message.role == "system"
                && message
                    .content
                    .contains("Проверка завершена: достаточные пруфы не найдены")
                && message.content.contains("proof_not_found_mode")
                && message.content.contains("Model X has 97% F1")
        }));
    }

    #[tokio::test]
    async fn strict_verifier_proof_not_found_mode_delivers_verified_report() {
        let mut state = RunState::new();
        state.continuation_count = 10;
        state.proof_not_found_report_requested = true;
        let report = "Проверка завершена: достаточные пруфы не найдены\n\nЧто проверено: https://huggingface.co/example/model\nЧто подтверждено: лицензия Apache 2.0.\nЧто не подтверждено: 97% F1 на русском PII.";
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
                assert_eq!(response, report);
            }
            _ => panic!("expected proof-not-found final response"),
        }
        assert!(memory.iter().any(|message| {
            message.resolved_kind() == AgentMessageKind::AssistantResponse
                && message.content == report
        }));
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

        let error = match result {
            Ok(_) => panic!("unsupported proof-not-found report should fail closed"),
            Err(error) => error,
        };
        assert!(error.contains("constrained proof-not-found report was not verified"));
        assert!(
            !memory
                .iter()
                .any(|message| { message.resolved_kind() == AgentMessageKind::AssistantResponse })
        );
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
