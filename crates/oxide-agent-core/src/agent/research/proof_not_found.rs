//! Deterministic proof-not-found final report builder.

use super::{EvidenceDocument, VerifierAllowedClaim, VerifierUnsupportedClaim};

/// Maximum user-visible proof-not-found report size.
pub const PROOF_NOT_FOUND_MAX_REPORT_CHARS: usize = 3_500;
/// Maximum evidence documents referenced by compact proof-not-found verification.
pub const PROOF_NOT_FOUND_MAX_EVIDENCE_DOCS: usize = 3;
const PROOF_NOT_FOUND_MAX_ITEM_CHARS: usize = 240;
const PROOF_NOT_FOUND_MAX_CONFIRMED_ITEMS: usize = 4;
const PROOF_NOT_FOUND_MAX_UNCONFIRMED_ITEMS: usize = 6;
const PROOF_NOT_FOUND_MAX_NEXT_ACTIONS: usize = 5;

/// Inputs for deterministic proof-not-found report construction.
#[derive(Debug, Clone, Copy)]
pub struct ProofNotFoundReportInput<'a> {
    /// Original user task.
    pub user_task: &'a str,
    /// Claims accepted by the verifier as directly supported.
    pub allowed_claims: &'a [VerifierAllowedClaim],
    /// Claims rejected by the verifier as unsupported.
    pub unsupported_claims: &'a [VerifierUnsupportedClaim],
    /// Concrete follow-up actions requested by the verifier.
    pub required_next_actions: &'a [String],
    /// Proof-grade evidence documents available to the verifier.
    pub evidence_documents: &'a [EvidenceDocument],
    /// Whether this is the stricter deterministic repair report.
    pub repair_mode: bool,
}

/// Bounded deterministic report and omission counters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofNotFoundReport {
    /// Report text guaranteed to be at most [`PROOF_NOT_FOUND_MAX_REPORT_CHARS`].
    pub text: String,
    /// Supported claims omitted due to report limits.
    pub omitted_confirmed: usize,
    /// Unsupported claims omitted due to report limits.
    pub omitted_unconfirmed: usize,
    /// Follow-up actions omitted due to report limits.
    pub omitted_next_actions: usize,
    /// Evidence documents omitted due to report limits.
    pub omitted_evidence_docs: usize,
}

/// Build a short proof-not-found report without asking the root model to author it.
#[must_use]
pub fn build_proof_not_found_report(input: ProofNotFoundReportInput<'_>) -> ProofNotFoundReport {
    let mut out = BudgetedText::new(PROOF_NOT_FOUND_MAX_REPORT_CHARS);

    out.line("Проверка завершена: достаточные пруфы не найдены");
    out.blank();

    out.line("Запрос:");
    out.item(&clip_clean(input.user_task, 280));
    out.blank();

    out.line("Подтверждено:");
    let confirmed_items = if input.repair_mode {
        Vec::new()
    } else {
        input
            .allowed_claims
            .iter()
            .take(PROOF_NOT_FOUND_MAX_CONFIRMED_ITEMS)
            .map(|claim| {
                let sources = if claim.source_ids.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", claim.source_ids.join(", "))
                };
                format!(
                    "{}{}",
                    clip_clean(&claim.claim, PROOF_NOT_FOUND_MAX_ITEM_CHARS),
                    sources
                )
            })
            .collect::<Vec<_>>()
    };
    if confirmed_items.is_empty() {
        out.item("Нет предметных утверждений, которые можно безопасно подтвердить по доступным доказательствам.");
    } else {
        for item in &confirmed_items {
            out.item(item);
        }
    }
    out.blank();

    out.line("Не подтверждено:");
    let mut unconfirmed_written = 0usize;
    for claim in input
        .unsupported_claims
        .iter()
        .take(PROOF_NOT_FOUND_MAX_UNCONFIRMED_ITEMS)
    {
        let line = format!(
            "{} — {}",
            clip_clean(&claim.claim, PROOF_NOT_FOUND_MAX_ITEM_CHARS),
            clip_clean(&claim.reason, 180)
        );
        if out.item(&line) {
            unconfirmed_written += 1;
        }
    }
    if unconfirmed_written == 0 {
        out.item(
            "Запрошенные показатели/сравнения не подтверждены доступными evidence-фрагментами.",
        );
    }
    out.blank();

    out.line("Удалены утверждения:");
    out.item("Не выдана неподтвержденная benchmark-таблица.");
    out.item("Не выданы неподтвержденные числа по скорости, VRAM, latency, throughput, качеству, лицензии, compliance или production-suitability.");
    out.item(
        "Не выдана рекомендация выбирать одну модель вместо другой без прямого доказательства.",
    );
    out.blank();

    out.line("Проверенные документы-доказательства:");
    let docs_written = push_evidence_doc_metadata(&mut out, input.evidence_documents);
    if docs_written == 0 {
        out.item("Документы-доказательства отсутствуют.");
    }
    out.blank();

    out.line("Дальнейшие шаги:");
    let mut actions_written = 0usize;
    for action in input
        .required_next_actions
        .iter()
        .take(PROOF_NOT_FOUND_MAX_NEXT_ACTIONS)
    {
        if out.item(&clip_clean(action, PROOF_NOT_FOUND_MAX_ITEM_CHARS)) {
            actions_written += 1;
        }
    }
    if actions_written == 0 {
        out.item("Получить первичные model cards, официальные benchmark-страницы или воспроизводимый локальный запуск с фиксированными версиями, GPU, precision, batch size и длиной prompt.");
    }

    let text = out.finish();
    debug_assert!(text.chars().count() <= PROOF_NOT_FOUND_MAX_REPORT_CHARS);

    ProofNotFoundReport {
        text,
        omitted_confirmed: input
            .allowed_claims
            .len()
            .saturating_sub(PROOF_NOT_FOUND_MAX_CONFIRMED_ITEMS),
        omitted_unconfirmed: input
            .unsupported_claims
            .len()
            .saturating_sub(PROOF_NOT_FOUND_MAX_UNCONFIRMED_ITEMS),
        omitted_next_actions: input
            .required_next_actions
            .len()
            .saturating_sub(PROOF_NOT_FOUND_MAX_NEXT_ACTIONS),
        omitted_evidence_docs: input
            .evidence_documents
            .len()
            .saturating_sub(PROOF_NOT_FOUND_MAX_EVIDENCE_DOCS),
    }
}

fn push_evidence_doc_metadata(out: &mut BudgetedText, docs: &[EvidenceDocument]) -> usize {
    let mut written = 0usize;
    for (idx, doc) in docs
        .iter()
        .take(PROOF_NOT_FOUND_MAX_EVIDENCE_DOCS)
        .enumerate()
    {
        let url = doc.final_url.as_deref().unwrap_or(doc.url.as_str());
        let kind = doc.source_kind.as_deref().unwrap_or("unknown");
        let status = doc
            .status_code
            .map(|status| status.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let line = format!(
            "doc-{}: {}, status={}, kind={}, truncated={}",
            idx + 1,
            clip_clean(url, 180),
            status,
            kind,
            doc.truncated
        );
        if out.item(&line) {
            written += 1;
        }
    }
    written
}

struct BudgetedText {
    max_chars: usize,
    text: String,
}

impl BudgetedText {
    fn new(max_chars: usize) -> Self {
        Self {
            max_chars,
            text: String::new(),
        }
    }

    fn line(&mut self, line: &str) -> bool {
        self.push(line)
    }

    fn item(&mut self, line: &str) -> bool {
        self.push(&format!("- {line}"))
    }

    fn blank(&mut self) -> bool {
        self.push("")
    }

    fn push(&mut self, line: &str) -> bool {
        let sep = if self.text.is_empty() { "" } else { "\n" };
        let addition = format!("{sep}{line}");
        let next_len = self.text.chars().count() + addition.chars().count();
        if next_len <= self.max_chars {
            self.text.push_str(&addition);
            return true;
        }

        let marker = if self.text.ends_with('\n') {
            "…"
        } else {
            "\n…"
        };
        if self.text.chars().count() + marker.chars().count() <= self.max_chars {
            self.text.push_str(marker);
        }
        false
    }

    fn finish(self) -> String {
        truncate_chars(&self.text, self.max_chars)
    }
}

fn clip_clean(input: &str, max_chars: usize) -> String {
    truncate_chars(&collapse_ws(input), max_chars)
}

fn collapse_ws(input: &str) -> String {
    let mut out = String::new();
    let mut last_was_ws = false;
    for ch in input.chars() {
        if ch.is_control() && ch != '\n' && ch != '\t' {
            continue;
        }
        if ch.is_whitespace() {
            if !last_was_ws {
                out.push(' ');
                last_was_ws = true;
            }
        } else {
            out.push(ch);
            last_was_ws = false;
        }
    }
    out.trim().to_string()
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    input.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::research::ResearchSourcePriority;

    fn unsupported(index: usize) -> VerifierUnsupportedClaim {
        VerifierUnsupportedClaim {
            claim: format!("97% F1 benchmark claim number {index} with very long unsupported tail"),
            reason: "metric is absent from evidence documents and must not be asserted".to_string(),
            required_evidence: "official benchmark text".to_string(),
            suggested_next_action: "fetch official benchmark page".to_string(),
        }
    }

    fn evidence(index: usize) -> EvidenceDocument {
        EvidenceDocument {
            tool_name: "crawl4ai_markdown".to_string(),
            provider: Some("crawl4ai_markdown".to_string()),
            url: format!("https://example.com/model/{index}/{}", "x".repeat(300)),
            final_url: None,
            status_code: Some(200),
            source_priority: ResearchSourcePriority::Primary,
            excerpt: "License: Apache 2.0".to_string(),
            excerpt_sha256: "excerpt".to_string(),
            content_sha256: "content".to_string(),
            content_chars: 128,
            excerpt_chars: 19,
            truncated: false,
            source_kind: Some("model_card".to_string()),
            fetched_at: Some("2026-06-11T00:00:00Z".to_string()),
        }
    }

    #[test]
    fn proof_not_found_report_is_bounded_to_3500_chars() {
        let unsupported_claims = (0..50).map(unsupported).collect::<Vec<_>>();
        let actions = (0..50)
            .map(|index| format!("fetch source {index} with {}", "y".repeat(300)))
            .collect::<Vec<_>>();
        let docs = (0..10).map(evidence).collect::<Vec<_>>();
        let report = build_proof_not_found_report(ProofNotFoundReportInput {
            user_task: &"Сравнить модели ".repeat(200),
            allowed_claims: &[],
            unsupported_claims: &unsupported_claims,
            required_next_actions: &actions,
            evidence_documents: &docs,
            repair_mode: false,
        });

        assert!(report.text.chars().count() <= PROOF_NOT_FOUND_MAX_REPORT_CHARS);
        assert!(
            report
                .text
                .starts_with("Проверка завершена: достаточные пруфы не найдены")
        );
        assert!(!report.text.contains('|'));
    }

    #[test]
    fn proof_not_found_report_uses_allowed_claims_only_in_confirmed() {
        let allowed = vec![VerifierAllowedClaim {
            claim: "License is Apache 2.0".to_string(),
            source_ids: vec!["doc-1".to_string()],
        }];
        let unsupported_claims = vec![unsupported(1)];
        let report = build_proof_not_found_report(ProofNotFoundReportInput {
            user_task: "compare models",
            allowed_claims: &allowed,
            unsupported_claims: &unsupported_claims,
            required_next_actions: &[],
            evidence_documents: &[],
            repair_mode: false,
        });

        assert!(report.text.contains("License is Apache 2.0 [doc-1]"));
        assert!(report.text.contains("97% F1 benchmark claim"));
        assert!(!report.text.contains("Подтверждено:\n- 97% F1"));
    }

    #[test]
    fn proof_not_found_report_with_no_evidence_documents() {
        let report = build_proof_not_found_report(ProofNotFoundReportInput {
            user_task: "compare models",
            allowed_claims: &[],
            unsupported_claims: &[],
            required_next_actions: &[],
            evidence_documents: &[],
            repair_mode: false,
        });

        assert!(report.text.contains("Документы-доказательства отсутствуют"));
        assert!(report.text.contains("Нет предметных утверждений"));
    }

    #[test]
    fn proof_not_found_report_with_docs_but_no_allowed_claims() {
        let docs = vec![evidence(1), evidence(2)];
        let report = build_proof_not_found_report(ProofNotFoundReportInput {
            user_task: "compare models",
            allowed_claims: &[],
            unsupported_claims: &[unsupported(1)],
            required_next_actions: &[],
            evidence_documents: &docs,
            repair_mode: false,
        });

        assert!(report.text.contains("doc-1:"));
        assert!(report.text.contains("doc-2:"));
        assert!(report.text.contains("Нет предметных утверждений"));
        assert!(!report.text.contains('|'));
    }

    #[test]
    fn proof_not_found_report_repair_mode_removes_confirmed_claims() {
        let allowed = vec![VerifierAllowedClaim {
            claim: "License is Apache 2.0".to_string(),
            source_ids: vec!["doc-1".to_string()],
        }];
        let report = build_proof_not_found_report(ProofNotFoundReportInput {
            user_task: "compare models",
            allowed_claims: &allowed,
            unsupported_claims: &[],
            required_next_actions: &[],
            evidence_documents: &[],
            repair_mode: true,
        });

        assert!(report.text.contains("Нет предметных утверждений"));
        assert!(!report.text.contains("License is Apache 2.0"));
    }

    #[test]
    fn proof_not_found_report_handles_cyrillic_char_limit() {
        let unsupported_claims = (0..100).map(unsupported).collect::<Vec<_>>();
        let report = build_proof_not_found_report(ProofNotFoundReportInput {
            user_task: &"Очень длинный кириллический запрос ".repeat(300),
            allowed_claims: &[],
            unsupported_claims: &unsupported_claims,
            required_next_actions: &[],
            evidence_documents: &[],
            repair_mode: false,
        });

        assert!(report.text.chars().count() <= PROOF_NOT_FOUND_MAX_REPORT_CHARS);
        assert!(std::str::from_utf8(report.text.as_bytes()).is_ok());
    }
}
