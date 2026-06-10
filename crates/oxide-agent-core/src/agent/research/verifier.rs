//! Strict zero-trust final-answer verifier sidecar.

use super::{EvidenceDocument, ResearchSnapshot, source_priority_label};
use crate::config::{AgentSettings, ModelInfo};
use crate::llm::{InternalTextPurpose, LlmClient};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

const SYSTEM_PROMPT: &str = r#"You are a strict zero-trust answer verifier.

Rules:
- Trust ONLY EvidenceDocument.content_excerpt values from the user JSON.
- Do NOT trust the agent draft, sub-agent summaries, reasoning, memory, search snippets, or prior conclusions.
- Every factual claim in the final answer must be directly supported by evidence text.
- Licenses, benchmark metrics, language support, model architecture, training data, Russian/152-ФЗ suitability, recommendations, and rankings require direct source text.
- If evidence is missing, return need_more_evidence or revise.
- Return proof_not_found only for a constrained no-proof report that explicitly avoids unsupported recommendations.
- Return block for unsafe, contradictory, malformed, or unverifiable answers that should not be delivered.

Respond ONLY with strict JSON matching this schema:
{
  "verdict": "allow | revise | need_more_evidence | proof_not_found | block",
  "confidence": "high | medium | low",
  "summary": "short verifier summary",
  "unsupported_claims": [
    {
      "claim": "exact final-answer claim",
      "reason": "why unsupported by evidence documents",
      "required_evidence": "what source text is needed",
      "suggested_next_action": "specific search/fetch instruction"
    }
  ],
  "contradictions": [
    {
      "claim": "exact final-answer claim",
      "source_id": "doc-3",
      "source_excerpt": "conflicting source text"
    }
  ],
  "allowed_claims": [
    {
      "claim": "exact final-answer claim",
      "source_ids": ["doc-1"]
    }
  ],
  "required_next_actions": ["specific next action"]
}"#;

/// Runtime configuration for the strict answer verifier.
#[derive(Debug, Clone)]
pub struct ResearchVerifierConfig {
    /// Whether the verifier should run.
    pub enabled: bool,
    /// Explicit verifier model route. Missing route is a fail-closed condition when enabled.
    pub model: Option<ModelInfo>,
    /// Maximum evidence-gathering rounds before proof-not-found flow.
    pub max_rounds: usize,
    /// Provider call timeout.
    pub timeout: Duration,
    /// Maximum proof documents sent to the verifier.
    pub max_evidence_docs: usize,
    /// Maximum excerpt characters per proof document sent to the verifier.
    pub max_excerpt_chars: usize,
}

impl ResearchVerifierConfig {
    /// Build verifier config from loaded agent settings.
    #[must_use]
    pub fn from_settings(settings: &AgentSettings) -> Self {
        Self {
            enabled: settings.is_research_verifier_enabled(),
            model: settings.get_configured_research_verifier_model(),
            max_rounds: settings.get_research_verifier_max_rounds(),
            timeout: Duration::from_secs(settings.get_research_verifier_timeout_secs()),
            max_evidence_docs: settings.get_research_verifier_max_evidence_docs(),
            max_excerpt_chars: settings.get_research_verifier_max_excerpt_chars(),
        }
    }

    fn require_model(&self) -> Result<&ModelInfo, AnswerVerificationError> {
        if !self.enabled {
            return Err(AnswerVerificationError::Disabled);
        }
        self.model
            .as_ref()
            .ok_or(AnswerVerificationError::MissingRoute)
    }
}

/// Input for one strict final-answer verification request.
#[derive(Debug, Clone, Copy)]
pub struct AnswerVerificationRequest<'a> {
    /// Candidate final answer that would otherwise be delivered.
    pub final_answer: &'a str,
    /// Passive research snapshot containing bounded proof documents.
    pub research: &'a ResearchSnapshot,
    /// Current verifier-guided round number.
    pub round: usize,
    /// Whether the candidate answer is the constrained proof-not-found report.
    pub proof_not_found_mode: bool,
}

/// Strict verdict accepted from the verifier JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerVerifierVerdict {
    /// Final answer is directly supported and may be delivered.
    Allow,
    /// Final answer needs wording corrections before delivery.
    Revise,
    /// More proof documents are required before delivery.
    NeedMoreEvidence,
    /// Constrained no-proof report may be delivered.
    ProofNotFound,
    /// Final answer must not be delivered.
    Block,
}

/// Strict confidence value accepted from the verifier JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerVerifierConfidence {
    /// High-confidence verifier decision.
    High,
    /// Medium-confidence verifier decision.
    Medium,
    /// Low-confidence verifier decision.
    Low,
}

/// Unsupported final-answer claim reported by the verifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifierUnsupportedClaim {
    /// Exact unsupported final-answer claim.
    pub claim: String,
    /// Why the evidence documents do not support the claim.
    pub reason: String,
    /// Source text needed to support the claim.
    pub required_evidence: String,
    /// Concrete next search/fetch instruction.
    pub suggested_next_action: String,
}

/// Contradiction between final answer and source evidence reported by the verifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifierContradiction {
    /// Exact final-answer claim contradicted by evidence.
    pub claim: String,
    /// Evidence document ID containing contradictory text.
    pub source_id: String,
    /// Conflicting source excerpt.
    pub source_excerpt: String,
}

/// Final-answer claim accepted as directly supported by source evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifierAllowedClaim {
    /// Exact final-answer claim accepted as supported.
    pub claim: String,
    /// Evidence document IDs supporting the claim.
    pub source_ids: Vec<String>,
}

/// Parsed strict verifier decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerVerificationDecision {
    /// Delivery verdict.
    pub verdict: AnswerVerifierVerdict,
    /// Verifier confidence.
    pub confidence: AnswerVerifierConfidence,
    /// Short verifier summary.
    pub summary: String,
    /// Unsupported claims that must be corrected or researched.
    #[serde(default)]
    pub unsupported_claims: Vec<VerifierUnsupportedClaim>,
    /// Evidence contradictions found by the verifier.
    #[serde(default)]
    pub contradictions: Vec<VerifierContradiction>,
    /// Claims accepted as directly supported.
    #[serde(default)]
    pub allowed_claims: Vec<VerifierAllowedClaim>,
    /// Required next actions for another research iteration.
    #[serde(default)]
    pub required_next_actions: Vec<String>,
}

/// Fail-closed verifier errors.
#[derive(Debug, Error)]
pub enum AnswerVerificationError {
    /// Verifier was called while disabled.
    #[error("strict answer verifier is disabled")]
    Disabled,
    /// Verifier is enabled but has no explicit model/provider route.
    #[error("strict answer verifier route is not configured")]
    MissingRoute,
    /// Verifier provider call failed.
    #[error("strict answer verifier provider failed: {0}")]
    Provider(String),
    /// Verifier provider did not respond before timeout.
    #[error("strict answer verifier timed out after {timeout_secs}s")]
    Timeout {
        /// Timeout in seconds used for the verifier call.
        timeout_secs: u64,
    },
    /// Verifier returned malformed or schema-invalid JSON.
    #[error("strict answer verifier returned invalid JSON: {0}")]
    InvalidJson(String),
}

/// LLM-backed strict final-answer verifier.
pub struct StrictAnswerVerifier {
    llm_client: Arc<LlmClient>,
    config: ResearchVerifierConfig,
}

impl StrictAnswerVerifier {
    /// Create a strict verifier sidecar.
    #[must_use]
    pub fn new(llm_client: Arc<LlmClient>, config: ResearchVerifierConfig) -> Self {
        Self { llm_client, config }
    }

    /// Verify one candidate final answer. Any error is fail-closed for callers.
    pub async fn verify(
        &self,
        request: AnswerVerificationRequest<'_>,
    ) -> Result<AnswerVerificationDecision, AnswerVerificationError> {
        let model = self.config.require_model()?;
        let user_message = build_verifier_user_message(&request, &self.config);
        let llm_call = self.llm_client.complete_internal_text(
            InternalTextPurpose::AnswerVerification,
            SYSTEM_PROMPT,
            &user_message,
            model,
        );

        let raw = match tokio::time::timeout(self.config.timeout, llm_call).await {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => return Err(AnswerVerificationError::Provider(error.to_string())),
            Err(_) => {
                return Err(AnswerVerificationError::Timeout {
                    timeout_secs: self.config.timeout.as_secs().max(1),
                });
            }
        };

        parse_verifier_decision(&raw)
    }
}

/// Parse strict verifier JSON into a typed decision.
pub fn parse_verifier_decision(
    raw: &str,
) -> Result<AnswerVerificationDecision, AnswerVerificationError> {
    serde_json::from_str::<AnswerVerificationDecision>(raw.trim())
        .map_err(|error| AnswerVerificationError::InvalidJson(error.to_string()))
}

fn build_verifier_user_message(
    request: &AnswerVerificationRequest<'_>,
    config: &ResearchVerifierConfig,
) -> String {
    let evidence_documents = request
        .research
        .evidence_documents
        .iter()
        .take(config.max_evidence_docs)
        .enumerate()
        .map(|(index, document)| evidence_document_request_payload(index, document, config))
        .collect::<Vec<_>>();

    serde_json::to_string_pretty(&json!({
        "final_answer": request.final_answer,
        "round": request.round,
        "max_rounds": config.max_rounds,
        "proof_not_found_mode": request.proof_not_found_mode,
        "evidence_documents": evidence_documents,
        "instructions": {
            "trust_boundary": "Use only EvidenceDocument.content_excerpt as proof.",
            "snippets_are_not_proof": true,
            "memory_reasoning_and_sub_agents_are_not_proof": true,
            "fail_closed_on_missing_support": true
        }
    }))
    .expect("verifier request JSON should serialize")
}

fn evidence_document_request_payload(
    index: usize,
    document: &EvidenceDocument,
    config: &ResearchVerifierConfig,
) -> Value {
    json!({
        "id": format!("doc-{}", index + 1),
        "tool_name": &document.tool_name,
        "provider": &document.provider,
        "url": &document.url,
        "final_url": &document.final_url,
        "status_code": document.status_code,
        "source_priority": source_priority_label(document.source_priority),
        "content_excerpt": truncate_chars(&document.excerpt, config.max_excerpt_chars),
        "excerpt_sha256": &document.excerpt_sha256,
        "content_sha256": &document.content_sha256,
        "content_chars": document.content_chars,
        "excerpt_chars": document.excerpt_chars.min(config.max_excerpt_chars),
        "truncated": document.truncated || document.excerpt_chars > config.max_excerpt_chars,
        "source_kind": &document.source_kind,
        "fetched_at": &document.fetched_at,
    })
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    input.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::research::ResearchSourcePriority;
    use crate::config::AgentSettings;
    use crate::llm::{LlmClient, LlmError, Message, MockLlmProvider};
    use async_trait::async_trait;
    use std::sync::Arc;

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
            timeout: Duration::from_secs(5),
            max_evidence_docs: 30,
            max_excerpt_chars: 12_000,
        }
    }

    fn sample_snapshot() -> ResearchSnapshot {
        ResearchSnapshot {
            evidence_documents: vec![EvidenceDocument {
                tool_name: "crawl4ai_markdown".to_string(),
                provider: Some("crawl4ai_markdown".to_string()),
                url: "https://huggingface.co/example/model".to_string(),
                final_url: None,
                status_code: Some(200),
                source_priority: ResearchSourcePriority::Primary,
                excerpt: "License: Apache 2.0\nRussian support: yes".to_string(),
                excerpt_sha256: "excerpt-hash".to_string(),
                content_sha256: "content-hash".to_string(),
                content_chars: 39,
                excerpt_chars: 39,
                truncated: false,
                source_kind: Some("model_card".to_string()),
                fetched_at: Some("2026-06-10T18:00:00Z".to_string()),
            }],
            ..ResearchSnapshot::default()
        }
    }

    fn decision_json(verdict: &str) -> String {
        format!(
            r#"{{"verdict":"{verdict}","confidence":"high","summary":"checked","unsupported_claims":[],"contradictions":[],"allowed_claims":[],"required_next_actions":[]}}"#
        )
    }

    #[test]
    fn parses_all_strict_verifier_verdicts() {
        let cases = [
            ("allow", AnswerVerifierVerdict::Allow),
            ("revise", AnswerVerifierVerdict::Revise),
            (
                "need_more_evidence",
                AnswerVerifierVerdict::NeedMoreEvidence,
            ),
            ("proof_not_found", AnswerVerifierVerdict::ProofNotFound),
            ("block", AnswerVerifierVerdict::Block),
        ];

        for (raw, expected) in cases {
            let parsed = parse_verifier_decision(&decision_json(raw)).expect("verdict parses");
            assert_eq!(parsed.verdict, expected);
        }
    }

    #[test]
    fn invalid_verifier_json_fails_closed() {
        let error = parse_verifier_decision("not json").expect_err("invalid json should fail");
        assert!(matches!(error, AnswerVerificationError::InvalidJson(_)));

        let error = parse_verifier_decision(&decision_json("maybe"))
            .expect_err("unknown verdict should fail");
        assert!(matches!(error, AnswerVerificationError::InvalidJson(_)));
    }

    #[test]
    fn verifier_config_does_not_fallback_to_agent_route() {
        let settings = AgentSettings {
            agent_model_id: Some("agent-model".to_string()),
            agent_model_provider: Some("opencode-go".to_string()),
            research_verifier_enabled: Some(true),
            ..AgentSettings::default()
        };

        let config = ResearchVerifierConfig::from_settings(&settings);

        assert!(config.enabled);
        assert!(config.model.is_none());
    }

    #[tokio::test]
    async fn missing_verifier_route_fails_closed_without_provider_call() {
        let settings = AgentSettings {
            agent_model_id: Some("agent-model".to_string()),
            agent_model_provider: Some("opencode-go".to_string()),
            ..AgentSettings::default()
        };
        let mut provider = MockLlmProvider::new();
        provider.expect_complete_internal_text().times(0);
        let mut llm = LlmClient::new(&settings);
        llm.register_provider("opencode-go".to_string(), Arc::new(provider));
        let verifier = StrictAnswerVerifier::new(Arc::new(llm), verifier_config(None));

        let error = verifier
            .verify(AnswerVerificationRequest {
                final_answer: "Apache 2.0",
                research: &sample_snapshot(),
                round: 1,
                proof_not_found_mode: false,
            })
            .await
            .expect_err("missing route should fail closed");

        assert!(matches!(error, AnswerVerificationError::MissingRoute));
    }

    #[tokio::test]
    async fn verifier_sidecar_returns_typed_decision() {
        let settings = AgentSettings {
            agent_model_id: Some("agent-model".to_string()),
            agent_model_provider: Some("opencode-go".to_string()),
            research_verifier_model_id: Some("verifier-model".to_string()),
            research_verifier_model_provider: Some("opencode-go".to_string()),
            ..AgentSettings::default()
        };
        let mut provider = MockLlmProvider::new();
        provider.expect_complete_internal_text().times(1).returning(
            |system_prompt, _history, user_message, model_id, _max_tokens| {
                assert!(system_prompt.contains("strict zero-trust answer verifier"));
                assert!(user_message.contains("EvidenceDocument.content_excerpt"));
                assert!(user_message.contains("huggingface.co/example/model"));
                assert_eq!(model_id, "verifier-model");
                Ok(decision_json("allow"))
            },
        );
        let mut llm = LlmClient::new(&settings);
        llm.register_provider("opencode-go".to_string(), Arc::new(provider));
        let verifier = StrictAnswerVerifier::new(
            Arc::new(llm),
            ResearchVerifierConfig::from_settings(&settings),
        );

        let decision = verifier
            .verify(AnswerVerificationRequest {
                final_answer: "The model is Apache 2.0.",
                research: &sample_snapshot(),
                round: 1,
                proof_not_found_mode: false,
            })
            .await
            .expect("verifier should parse decision");

        assert_eq!(decision.verdict, AnswerVerifierVerdict::Allow);
    }

    #[tokio::test]
    async fn verifier_provider_error_fails_closed() {
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
            .times(1)
            .returning(|_, _, _, _, _| Err(LlmError::ApiError("boom".to_string())));
        let mut llm = LlmClient::new(&settings);
        llm.register_provider("opencode-go".to_string(), Arc::new(provider));
        let verifier = StrictAnswerVerifier::new(
            Arc::new(llm),
            ResearchVerifierConfig::from_settings(&settings),
        );

        let error = verifier
            .verify(AnswerVerificationRequest {
                final_answer: "The model is Apache 2.0.",
                research: &sample_snapshot(),
                round: 1,
                proof_not_found_mode: false,
            })
            .await
            .expect_err("provider errors should fail closed");

        assert!(matches!(error, AnswerVerificationError::Provider(_)));
    }

    struct SlowProvider;

    #[async_trait]
    impl crate::llm::LlmProvider for SlowProvider {
        async fn complete_internal_text(
            &self,
            _system_prompt: &str,
            _history: &[Message],
            _user_message: &str,
            _model_id: &str,
            _max_tokens: u32,
        ) -> Result<String, LlmError> {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok(decision_json("allow"))
        }

        async fn transcribe_audio(
            &self,
            _audio_bytes: Vec<u8>,
            _mime_type: &str,
            _model_id: &str,
        ) -> Result<String, LlmError> {
            Err(LlmError::Unknown("not implemented".to_string()))
        }

        async fn analyze_image(
            &self,
            _image_bytes: Vec<u8>,
            _text_prompt: &str,
            _system_prompt: &str,
            _model_id: &str,
        ) -> Result<String, LlmError> {
            Err(LlmError::Unknown("not implemented".to_string()))
        }
    }

    #[tokio::test]
    async fn verifier_timeout_fails_closed() {
        let settings = AgentSettings {
            agent_model_id: Some("agent-model".to_string()),
            agent_model_provider: Some("opencode-go".to_string()),
            research_verifier_model_id: Some("verifier-model".to_string()),
            research_verifier_model_provider: Some("opencode-go".to_string()),
            ..AgentSettings::default()
        };
        let mut llm = LlmClient::new(&settings);
        llm.register_provider("opencode-go".to_string(), Arc::new(SlowProvider));
        let mut config = ResearchVerifierConfig::from_settings(&settings);
        config.timeout = Duration::from_millis(1);
        let verifier = StrictAnswerVerifier::new(Arc::new(llm), config);

        let error = verifier
            .verify(AnswerVerificationRequest {
                final_answer: "The model is Apache 2.0.",
                research: &sample_snapshot(),
                round: 1,
                proof_not_found_mode: false,
            })
            .await
            .expect_err("timeout should fail closed");

        assert!(matches!(error, AnswerVerificationError::Timeout { .. }));
    }

    #[test]
    fn verifier_request_bounds_evidence_docs_and_excerpts() {
        let mut snapshot = sample_snapshot();
        snapshot.evidence_documents[0].excerpt = "abcdef".to_string();
        snapshot.evidence_documents[0].excerpt_chars = 6;
        snapshot
            .evidence_documents
            .push(snapshot.evidence_documents[0].clone());
        let mut config = verifier_config(Some(verifier_model()));
        config.max_evidence_docs = 1;
        config.max_excerpt_chars = 3;

        let message = build_verifier_user_message(
            &AnswerVerificationRequest {
                final_answer: "answer",
                research: &snapshot,
                round: 1,
                proof_not_found_mode: false,
            },
            &config,
        );
        let value: Value = serde_json::from_str(&message).expect("request JSON should parse");
        let docs = value["evidence_documents"]
            .as_array()
            .expect("documents should be an array");

        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0]["content_excerpt"], "abc");
        assert_eq!(docs[0]["truncated"], true);
    }
}
