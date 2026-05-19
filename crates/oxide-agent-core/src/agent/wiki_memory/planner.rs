use super::scope::wiki_slug;
use super::signals::{WikiSignal, WikiSignalBuffer, WikiSignalBufferConfig, WikiSignalKind};
use super::store::wiki_content_hash;
use super::{WikiPatchOperation, WikiPatchSet};
use crate::agent::memory_behavior::{ToolDerivedMemoryDraft, ToolDerivedMemoryKind};
use chrono::{DateTime, Utc};

const TASK_SIGNAL_MAX_CHARS: usize = 700;
const DRAFT_CONTENT_MAX_CHARS: usize = 900;
const TITLE_MAX_CHARS: usize = 96;

/// Conservative deterministic patch planner for one completed agent run.
///
/// This planner intentionally stays conservative: explicit remember requests and
/// high-confidence procedure/preference drafts become scoped `pages/*.md`, while
/// low-confidence facts stay in `inbox/`.
#[derive(Debug, Clone)]
pub struct WikiPatchPlanner {
    config: WikiPatchPlannerConfig,
}

/// Runtime limits for deterministic wiki patch planning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WikiPatchPlannerConfig {
    /// Maximum inbox operations produced for one completed run.
    pub max_inbox_items: usize,
    /// Maximum retained bytes across candidate signals.
    pub max_signal_bytes: usize,
}

impl Default for WikiPatchPlannerConfig {
    fn default() -> Self {
        Self {
            max_inbox_items: 8,
            max_signal_bytes: 24 * 1024,
        }
    }
}

impl Default for WikiPatchPlanner {
    fn default() -> Self {
        Self::new(WikiPatchPlannerConfig::default())
    }
}

impl WikiPatchPlanner {
    /// Create a deterministic wiki patch planner.
    #[must_use]
    pub const fn new(config: WikiPatchPlannerConfig) -> Self {
        Self { config }
    }

    /// Plan a validated-patch input from explicit remember intent and hook drafts.
    #[must_use]
    pub fn plan_run_patch(
        &self,
        context_id: &str,
        task_id: &str,
        task: &str,
        drafts: &[ToolDerivedMemoryDraft],
        now: DateTime<Utc>,
    ) -> Option<WikiPatchSet> {
        let mut buffer = WikiSignalBuffer::new(WikiSignalBufferConfig {
            max_candidates: self.config.max_inbox_items,
            max_bytes: self.config.max_signal_bytes,
        });
        let source_ref = run_source_ref(task_id, now);

        if has_explicit_remember_intent(task) && !drafts.iter().any(is_explicit_remember_draft) {
            buffer.push(WikiSignal {
                kind: WikiSignalKind::ExplicitRemember,
                content: truncate_chars(task, TASK_SIGNAL_MAX_CHARS),
                source_refs: vec![source_ref.clone()],
                explicit: true,
            });
        }

        for draft in drafts {
            buffer.push(WikiSignal {
                kind: signal_kind_for_draft(draft),
                content: draft.content.clone(),
                source_refs: vec![source_ref.clone(), format!("hook:{}", draft.source)],
                explicit: false,
            });
        }

        if buffer.signals().is_empty() {
            return None;
        }

        let mut operations = Vec::with_capacity(buffer.signals().len());
        for (index, signal) in buffer.signals().iter().enumerate() {
            let draft = drafts
                .iter()
                .find(|draft| draft.content.trim() == signal.content.trim());
            if should_create_page(signal, draft) {
                operations.push(WikiPatchOperation::CreatePage {
                    path: page_path(context_id, task_id, index, signal, draft, now),
                    content: page_content(task, signal, draft, now),
                });
            } else {
                operations.push(WikiPatchOperation::CreateInboxItem {
                    path: inbox_path(context_id, task_id, index, signal, draft, now),
                    content: inbox_content(task, signal, draft, now),
                });
            }
        }

        Some(WikiPatchSet {
            reason: "post-run wiki memory candidate capture".to_string(),
            source_refs: vec![source_ref],
            operations,
        })
    }
}

pub(crate) fn has_explicit_remember_intent(task: &str) -> bool {
    let normalized = task.to_lowercase();
    if normalized.contains("do not remember")
        || normalized.contains("don't remember")
        || normalized.contains("dont remember")
        || normalized.contains("не запоминай")
    {
        return false;
    }

    [
        "remember this",
        "remember that",
        "remember:",
        "memorize",
        "save this",
        "save that",
        "use this next time",
        "запомни",
        "сохрани",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

pub(crate) fn extract_explicit_remember_payload(task: &str) -> Option<String> {
    let trimmed = task.trim();
    if trimmed.is_empty() || !has_explicit_remember_intent(trimmed) {
        return None;
    }

    if let Some((prefix, suffix)) = trimmed.split_once(':') {
        if has_explicit_remember_intent(prefix) {
            let cleaned = cleanup_explicit_payload(suffix);
            if !cleaned.is_empty() {
                return Some(cleaned);
            }
        }
    }

    for prefix in [
        "Remember this",
        "Remember that",
        "Remember",
        "remember this",
        "remember that",
        "remember",
        "Save this",
        "Save that",
        "Save",
        "save this",
        "save that",
        "save",
        "Memorize",
        "memorize",
        "Use this next time",
        "use this next time",
        "Сохрани",
        "сохрани",
        "Запомни",
        "запомни",
    ] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            let cleaned = cleanup_explicit_payload(rest);
            if !cleaned.is_empty() {
                return Some(cleaned);
            }
        }
    }

    None
}

fn signal_kind_for_draft(draft: &ToolDerivedMemoryDraft) -> WikiSignalKind {
    match draft.kind {
        ToolDerivedMemoryKind::Fact => {
            if draft.confidence < 0.75 {
                WikiSignalKind::LowConfidence
            } else {
                WikiSignalKind::Decision
            }
        }
        ToolDerivedMemoryKind::Preference => WikiSignalKind::Preference,
        ToolDerivedMemoryKind::Procedure => WikiSignalKind::Procedure,
    }
}

fn run_source_ref(task_id: &str, now: DateTime<Utc>) -> String {
    let short_task_id: String = task_id
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
        .take(12)
        .collect();
    let short_task_id = if short_task_id.is_empty() {
        "unknown".to_string()
    } else {
        short_task_id
    };
    format!("run:{}:{short_task_id}", now.format("%Y-%m-%d"))
}

fn inbox_path(
    context_id: &str,
    task_id: &str,
    index: usize,
    signal: &WikiSignal,
    draft: Option<&ToolDerivedMemoryDraft>,
    now: DateTime<Utc>,
) -> String {
    let title = draft
        .map(|draft| draft.title.as_str())
        .unwrap_or_else(|| signal_kind_label(signal.kind));
    let hash = wiki_content_hash(&signal.content);
    let task_slug = wiki_slug(task_id, 12);
    let title_slug = wiki_slug(title, 36);
    let item_slug = format!(
        "{}-{}-{:02}-{}-{}",
        now.format("%Y-%m-%d"),
        task_slug,
        index + 1,
        title_slug,
        &hash[..8]
    );
    format!("contexts/{context_id}/inbox/{item_slug}.md")
}

fn page_path(
    context_id: &str,
    task_id: &str,
    index: usize,
    signal: &WikiSignal,
    draft: Option<&ToolDerivedMemoryDraft>,
    now: DateTime<Utc>,
) -> String {
    let title = draft
        .map(|draft| draft.title.as_str())
        .unwrap_or_else(|| signal_kind_label(signal.kind));
    let hash = wiki_content_hash(&signal.content);
    let task_slug = wiki_slug(task_id, 12);
    let title_slug = wiki_slug(title, 36);
    let page_slug = format!(
        "{}-{}-{:02}-{}-{}",
        now.format("%Y-%m-%d"),
        task_slug,
        index + 1,
        title_slug,
        &hash[..8]
    );
    format!("contexts/{context_id}/pages/{page_slug}.md")
}

fn should_create_page(signal: &WikiSignal, draft: Option<&ToolDerivedMemoryDraft>) -> bool {
    if signal.explicit {
        return true;
    }
    let Some(draft) = draft else {
        return false;
    };

    match draft.kind {
        ToolDerivedMemoryKind::Preference => draft.confidence >= 0.7,
        ToolDerivedMemoryKind::Procedure => draft.confidence >= 0.75,
        ToolDerivedMemoryKind::Fact => draft.confidence >= 0.85,
    }
}

fn page_content(
    task: &str,
    signal: &WikiSignal,
    draft: Option<&ToolDerivedMemoryDraft>,
    now: DateTime<Utc>,
) -> String {
    let title = draft
        .map(|draft| draft.title.as_str())
        .unwrap_or_else(|| signal_kind_label(signal.kind));
    let confidence = draft
        .map(|draft| confidence_label(draft.confidence))
        .unwrap_or(if signal.explicit { "medium" } else { "low" });
    let page_type = draft
        .map(page_type_for_draft)
        .unwrap_or_else(|| page_type_for_signal(signal.kind));
    let mut tags = draft
        .map(|draft| draft.tags.clone())
        .unwrap_or_else(|| vec![signal_kind_label(signal.kind).to_string()]);
    tags.push(page_type.to_string());
    tags.sort();
    tags.dedup();
    let source_refs = if signal.source_refs.is_empty() {
        vec!["run:unknown".to_string()]
    } else {
        signal.source_refs.clone()
    };
    let reason = draft
        .map(|draft| draft.reason.as_str())
        .unwrap_or("explicit user memory intent");
    let body = draft
        .map(|draft| draft.content.as_str())
        .unwrap_or(signal.content.as_str());
    let evidence = draft.map(render_evidence_section).unwrap_or_default();

    format!(
        "---\ntitle: {}\ntype: {}\nupdated_at: {}\nconfidence: {}\ntags:\n{}\nsources:\n{}\n---\n\n# {}\n\n{}{}\n\n## Capture\n\n- Kind: {}\n- Reason: {}\n- Source: {}\n\n## Source Task\n\n{}\n",
        yaml_string(&truncate_chars(title, TITLE_MAX_CHARS)),
        page_type,
        now.to_rfc3339(),
        confidence,
        yaml_list(&tags),
        yaml_list(&source_refs),
        truncate_chars(title, TITLE_MAX_CHARS),
        truncate_chars(body, DRAFT_CONTENT_MAX_CHARS),
        evidence,
        signal_kind_label(signal.kind),
        truncate_chars(reason, 240),
        draft.map(|draft| draft.source.as_str()).unwrap_or("user_task"),
        truncate_chars(task, TASK_SIGNAL_MAX_CHARS),
    )
}

fn inbox_content(
    task: &str,
    signal: &WikiSignal,
    draft: Option<&ToolDerivedMemoryDraft>,
    now: DateTime<Utc>,
) -> String {
    let title = draft
        .map(|draft| draft.title.as_str())
        .unwrap_or_else(|| signal_kind_label(signal.kind));
    let confidence = draft
        .map(|draft| confidence_label(draft.confidence))
        .unwrap_or(if signal.explicit { "medium" } else { "low" });
    let mut tags = draft
        .map(|draft| draft.tags.clone())
        .unwrap_or_else(|| vec![signal_kind_label(signal.kind).to_string()]);
    tags.push("inbox".to_string());
    tags.sort();
    tags.dedup();

    let source_refs = if signal.source_refs.is_empty() {
        vec!["run:unknown".to_string()]
    } else {
        signal.source_refs.clone()
    };
    let reason = draft
        .map(|draft| draft.reason.as_str())
        .unwrap_or("explicit user memory intent");
    let body = draft
        .map(|draft| draft.content.as_str())
        .unwrap_or(signal.content.as_str());
    let evidence = draft.map(render_evidence_section).unwrap_or_default();

    format!(
        "---\ntitle: {}\ntype: inbox\nupdated_at: {}\nconfidence: {}\ntags:\n{}\nsources:\n{}\n---\n\n# {}\n\n{}{}\n\n## Capture\n\n- Kind: {}\n- Explicit: {}\n- Reason: {}\n- Source: {}\n\n## Task\n\n{}\n",
        yaml_string(&truncate_chars(title, TITLE_MAX_CHARS)),
        now.to_rfc3339(),
        confidence,
        yaml_list(&tags),
        yaml_list(&source_refs),
        truncate_chars(title, TITLE_MAX_CHARS),
        truncate_chars(body, DRAFT_CONTENT_MAX_CHARS),
        evidence,
        signal_kind_label(signal.kind),
        signal.explicit,
        truncate_chars(reason, 240),
        draft.map(|draft| draft.source.as_str()).unwrap_or("user_task"),
        truncate_chars(task, TASK_SIGNAL_MAX_CHARS),
    )
}

fn page_type_for_draft(draft: &ToolDerivedMemoryDraft) -> &'static str {
    match draft.kind {
        ToolDerivedMemoryKind::Fact => "note",
        ToolDerivedMemoryKind::Preference => "preference",
        ToolDerivedMemoryKind::Procedure => "procedure",
    }
}

fn is_explicit_remember_draft(draft: &ToolDerivedMemoryDraft) -> bool {
    draft.source == "explicit_remember_capture"
        || draft.tags.iter().any(|tag| tag == "explicit-remember")
}

fn cleanup_explicit_payload(value: &str) -> String {
    let mut cleaned = value
        .trim()
        .trim_start_matches([':', '-', '—', '–', ' '])
        .trim_start_matches("this")
        .trim_start_matches("that")
        .trim_start_matches("это")
        .trim();
    for suffix in ["in memory", "в память"] {
        if let Some(stripped) = cleaned.strip_suffix(suffix) {
            cleaned = stripped.trim();
        }
    }
    cleaned.trim_matches([' ', '.', ':']).to_string()
}

fn render_evidence_section(draft: &ToolDerivedMemoryDraft) -> String {
    if draft.evidence.is_empty() {
        return String::new();
    }
    format!("\n\n## Evidence\n\n{}", markdown_bullets(&draft.evidence))
}

fn markdown_bullets(items: &[String]) -> String {
    items
        .iter()
        .map(|item| format!("- {}", truncate_chars(item, DRAFT_CONTENT_MAX_CHARS)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn page_type_for_signal(kind: WikiSignalKind) -> &'static str {
    match kind {
        WikiSignalKind::ExplicitRemember => "note",
        WikiSignalKind::Decision => "decision",
        WikiSignalKind::Procedure => "procedure",
        WikiSignalKind::Constraint => "constraint",
        WikiSignalKind::Preference => "preference",
        WikiSignalKind::LowConfidence => "note",
    }
}

fn signal_kind_label(kind: WikiSignalKind) -> &'static str {
    match kind {
        WikiSignalKind::ExplicitRemember => "explicit-remember",
        WikiSignalKind::Decision => "decision",
        WikiSignalKind::Procedure => "procedure",
        WikiSignalKind::Constraint => "constraint",
        WikiSignalKind::Preference => "preference",
        WikiSignalKind::LowConfidence => "low-confidence",
    }
}

fn confidence_label(confidence: f32) -> &'static str {
    if confidence >= 0.85 {
        "high"
    } else if confidence >= 0.7 {
        "medium"
    } else {
        "low"
    }
}

fn yaml_list(values: &[String]) -> String {
    values
        .iter()
        .map(|value| format!("  - {}", yaml_string(value)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn yaml_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::memory_behavior::{ToolDerivedMemoryDraft, ToolDerivedMemoryKind};
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 19, 10, 0, 0)
            .single()
            .expect("valid test time")
    }

    #[test]
    fn planner_skips_trivial_runs_without_signals() {
        let planner = WikiPatchPlanner::default();

        assert!(planner
            .plan_run_patch("ctx-12345678", "task-1", "what time is it?", &[], now())
            .is_none());
    }

    #[test]
    fn planner_routes_explicit_remember_to_page() {
        let planner = WikiPatchPlanner::default();
        let patch = planner
            .plan_run_patch(
                "ctx-12345678",
                "task-abc123",
                "Remember this: use staging before prod deploys.",
                &[],
                now(),
            )
            .expect("explicit remember should create patch");

        assert_eq!(patch.operations.len(), 1);
        match &patch.operations[0] {
            WikiPatchOperation::CreatePage { path, content } => {
                assert!(path.starts_with("contexts/ctx-12345678/pages/2026-05-19-"));
                assert!(content.contains("type: note"));
                assert!(content.contains("use staging before prod deploys"));
            }
            other => panic!("unexpected op: {other:?}"),
        }
    }

    #[test]
    fn planner_extracts_explicit_payload_without_memory_suffix() {
        assert_eq!(
            extract_explicit_remember_payload("Сохрани текущий курс BTC USDT в память").as_deref(),
            Some("текущий курс BTC USDT")
        );
    }

    #[test]
    fn planner_routes_russian_save_intent_to_page() {
        let planner = WikiPatchPlanner::default();
        let patch = planner
            .plan_run_patch(
                "ctx-12345678",
                "task-abc123",
                "Сохрани это в память: перед деплоем запускать smoke tests.",
                &[],
                now(),
            )
            .expect("Russian save intent should create patch");

        assert_eq!(patch.operations.len(), 1);
        match &patch.operations[0] {
            WikiPatchOperation::CreatePage { path, content } => {
                assert!(path.starts_with("contexts/ctx-12345678/pages/2026-05-19-"));
                assert!(content.contains("type: note"));
                assert!(content.contains("перед деплоем запускать smoke tests"));
            }
            other => panic!("unexpected op: {other:?}"),
        }
    }

    #[test]
    fn planner_routes_confident_tool_drafts_to_page() {
        let planner = WikiPatchPlanner::default();
        let draft = ToolDerivedMemoryDraft {
            kind: ToolDerivedMemoryKind::Procedure,
            title: "Deploy workflow".to_string(),
            content: "Run cargo test before deployment.".to_string(),
            short_description: "Test before deploy".to_string(),
            importance: 0.8,
            confidence: 0.76,
            source: "test_hook".to_string(),
            reason: "successful deploy flow observed".to_string(),
            tags: vec!["procedure".to_string(), "deploy".to_string()],
            evidence: vec!["Tool: write_file. Path: Cargo.toml.".to_string()],
            captured_at: now(),
        };

        let patch = planner
            .plan_run_patch("ctx-12345678", "task-abc123", "deploy", &[draft], now())
            .expect("draft should create patch");

        assert_eq!(patch.operations.len(), 1);
        match &patch.operations[0] {
            WikiPatchOperation::CreatePage { path, content } => {
                assert!(path.contains("deploy-workflow"));
                assert!(content.contains("Run cargo test before deployment."));
                assert!(content.contains("## Evidence"));
                assert!(content.contains("hook:test_hook"));
            }
            other => panic!("unexpected op: {other:?}"),
        }
    }

    #[test]
    fn planner_routes_low_confidence_facts_to_inbox() {
        let planner = WikiPatchPlanner::default();
        let draft = ToolDerivedMemoryDraft {
            kind: ToolDerivedMemoryKind::Fact,
            title: "Command failure".to_string(),
            content: "Command failed with a transient timeout.".to_string(),
            short_description: "Transient timeout".to_string(),
            importance: 0.7,
            confidence: 0.6,
            source: "test_hook".to_string(),
            reason: "failure observed once".to_string(),
            tags: vec!["failure".to_string()],
            evidence: vec![],
            captured_at: now(),
        };

        let patch = planner
            .plan_run_patch("ctx-12345678", "task-abc123", "debug", &[draft], now())
            .expect("low confidence draft should create inbox patch");

        assert_eq!(patch.operations.len(), 1);
        match &patch.operations[0] {
            WikiPatchOperation::CreateInboxItem { path, content } => {
                assert!(path.starts_with("contexts/ctx-12345678/inbox/"));
                assert!(content.contains("type: inbox"));
            }
            other => panic!("unexpected op: {other:?}"),
        }
    }

    #[test]
    fn planner_suppresses_raw_explicit_signal_when_explicit_draft_exists() {
        let planner = WikiPatchPlanner::default();
        let draft = ToolDerivedMemoryDraft {
            kind: ToolDerivedMemoryKind::Fact,
            title: "BTC/USDT current rate".to_string(),
            content: "BTC/USDT = 80 104.40".to_string(),
            short_description: "Verified BTC/USDT rate".to_string(),
            importance: 0.9,
            confidence: 0.45,
            source: "explicit_remember_capture".to_string(),
            reason: "unverified volatile fact".to_string(),
            tags: vec!["explicit-remember".to_string(), "volatile".to_string()],
            evidence: vec![],
            captured_at: now(),
        };

        let patch = planner
            .plan_run_patch(
                "ctx-12345678",
                "task-abc123",
                "Сохрани текущий курс BTC USDT в память",
                &[draft],
                now(),
            )
            .expect("explicit draft should create one operation");

        assert_eq!(patch.operations.len(), 1);
        match &patch.operations[0] {
            WikiPatchOperation::CreateInboxItem { content, .. } => {
                assert!(content.contains("BTC/USDT = 80 104.40"));
            }
            other => panic!("unexpected op: {other:?}"),
        }
    }
}
