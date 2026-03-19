use super::*;

#[test]
fn normalize_topic_prompt_payload_normalizes_line_endings_and_trailing_spaces() {
    let normalized = normalize_topic_prompt_payload("  line 1  \r\nline 2\t\r\n\r\n");
    assert_eq!(normalized, "line 1\nline 2");
}

#[test]
fn validate_topic_context_rejects_markdown_documents() {
    let error = validate_topic_context_content("# AGENTS\nDo the thing")
        .expect_err("markdown document must be rejected");
    assert!(error
        .to_string()
        .contains("store AGENTS.md-style documents in topic_agents_md"));
}

#[test]
fn validate_topic_context_rejects_oversized_payload() {
    let oversized = vec!["line"; TOPIC_CONTEXT_MAX_LINES + 1].join("\n");
    let error = validate_topic_context_content(&oversized)
        .expect_err("oversized topic context must be rejected");
    assert!(error.to_string().contains(&format!(
        "context must not exceed {TOPIC_CONTEXT_MAX_LINES} lines"
    )));
}

#[test]
fn validate_topic_context_rejects_too_many_characters() {
    let oversized = "x".repeat(TOPIC_CONTEXT_MAX_CHARS + 1);
    let error = validate_topic_context_content(&oversized)
        .expect_err("oversized topic context must be rejected");
    assert!(error.to_string().contains(&format!(
        "context must not exceed {TOPIC_CONTEXT_MAX_CHARS} characters"
    )));
}

#[test]
fn validate_topic_agents_md_normalizes_payload() {
    let normalized = validate_topic_agents_md_content("\r\n# Topic AGENTS  \r\nUse checklist\r\n")
        .expect("agents md must normalize");
    assert_eq!(normalized, "# Topic AGENTS\nUse checklist");
}

#[test]
fn validate_topic_agents_md_rejects_oversized_payload() {
    let oversized = vec!["line"; TOPIC_AGENTS_MD_MAX_LINES + 1].join("\n");
    let error = validate_topic_agents_md_content(&oversized)
        .expect_err("oversized agents md must be rejected");
    assert!(error.to_string().contains(&format!(
        "agents_md must not exceed {TOPIC_AGENTS_MD_MAX_LINES} lines"
    )));
}
