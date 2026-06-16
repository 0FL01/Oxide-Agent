#![allow(missing_docs)]

use super::types::{BrowserObservation, Viewport};
use serde_json::{Value, json};

pub const BROWSER_DECISION_SCHEMA_VERSION: u8 = 1;

const STABLE_SYSTEM_PROMPT: &str = r#"You are the Browser Live visual decision planner for Oxide Agent.
Return exactly one JSON object matching the BrowserDecision schema. Do not use markdown.
Use the attached screenshot as the visual source. The text prompt contains compact state only.
Never reveal or request raw secrets. If a step may submit credentials, payment data, 2FA, CAPTCHA, irreversible purchase, deletion, external message, or other sensitive action, set sensitive_action.required=true and choose ask_user or debug instead of an executable action.
Prefer low-risk, observable actions. If confidence is low, choose wait, debug, or ask_user. Do not claim done unless visible evidence supports completion.
Valid executable visual actions are click_xy, click_selector, click_target_id, fill, type_text, press, scroll, wait, and navigate. Use navigate only for http/https URLs. Debug, ask_user, and done are terminal/non-mutating decisions for the next layer.
"#;

pub struct BrowserDecisionPromptContext<'a> {
    pub task: &'a str,
    pub session_id: &'a str,
    pub observation: &'a BrowserObservation,
    pub history_summary: Option<&'a str>,
}

#[must_use]
pub const fn stable_system_prompt() -> &'static str {
    STABLE_SYSTEM_PROMPT
}

#[must_use]
pub fn browser_decision_json_schema() -> Value {
    json!({
        "type": "object",
        "required": ["schema_version", "rationale", "action", "expected_result", "confidence", "risk", "sensitive_action", "needs_debug"],
        "additionalProperties": false,
        "properties": {
            "schema_version": {"type": "integer", "const": BROWSER_DECISION_SCHEMA_VERSION},
            "rationale": {"type": "string", "minLength": 1, "maxLength": 1200},
            "action": {
                "oneOf": [
                    {"type": "object", "required": ["kind", "x", "y"], "additionalProperties": false, "properties": {"kind": {"const": "click_xy"}, "x": {"type": "integer", "minimum": 0}, "y": {"type": "integer", "minimum": 0}, "target_description": {"type": "string"}}},
                    {"type": "object", "required": ["kind", "selector"], "additionalProperties": false, "properties": {"kind": {"const": "click_selector"}, "selector": {"type": "string", "minLength": 1}}},
                    {"type": "object", "required": ["kind", "target_id"], "additionalProperties": false, "properties": {"kind": {"const": "click_target_id"}, "target_id": {"type": "string", "minLength": 1}}},
                    {"type": "object", "required": ["kind", "selector", "value"], "additionalProperties": false, "properties": {"kind": {"const": "fill"}, "selector": {"type": "string", "minLength": 1}, "value": {"type": "string"}}},
                    {"type": "object", "required": ["kind", "text"], "additionalProperties": false, "properties": {"kind": {"const": "type_text"}, "text": {"type": "string", "minLength": 1}}},
                    {"type": "object", "required": ["kind", "key"], "additionalProperties": false, "properties": {"kind": {"const": "press"}, "key": {"type": "string", "minLength": 1}}},
                    {"type": "object", "required": ["kind", "delta_x", "delta_y"], "additionalProperties": false, "properties": {"kind": {"const": "scroll"}, "delta_x": {"type": "integer"}, "delta_y": {"type": "integer"}}},
                    {"type": "object", "required": ["kind", "timeout_ms"], "additionalProperties": false, "properties": {"kind": {"const": "wait"}, "timeout_ms": {"type": "integer", "minimum": 100, "maximum": 10000}}},
                    {"type": "object", "required": ["kind", "url"], "additionalProperties": false, "properties": {"kind": {"const": "navigate"}, "url": {"type": "string", "minLength": 1}}},
                    {"type": "object", "required": ["kind", "reason"], "additionalProperties": false, "properties": {"kind": {"const": "debug"}, "reason": {"type": "string", "minLength": 1}}},
                    {"type": "object", "required": ["kind", "question"], "additionalProperties": false, "properties": {"kind": {"const": "ask_user"}, "question": {"type": "string", "minLength": 1}}},
                    {"type": "object", "required": ["kind", "final_answer", "evidence"], "additionalProperties": false, "properties": {"kind": {"const": "done"}, "final_answer": {"type": "string", "minLength": 1}, "evidence": {"type": "string", "minLength": 1}}}
                ]
            },
            "expected_result": {"type": "string", "minLength": 1, "maxLength": 1000},
            "confidence": {"type": "number", "minimum": 0, "maximum": 1},
            "risk": {"type": "string", "enum": ["low", "medium", "high"]},
            "sensitive_action": {
                "type": "object",
                "required": ["required"],
                "additionalProperties": false,
                "properties": {"required": {"type": "boolean"}, "category": {"type": "string"}, "reason": {"type": "string"}}
            },
            "needs_debug": {"type": "boolean"}
        }
    })
}

#[must_use]
pub fn build_dynamic_state_prompt(context: &BrowserDecisionPromptContext<'_>) -> String {
    let obs = context.observation;
    let network_failed = obs
        .network_summary
        .as_ref()
        .map(|summary| summary.failed_count)
        .unwrap_or_default();
    let console_errors = obs
        .console_summary
        .as_ref()
        .map(|summary| summary.error_count)
        .unwrap_or_default();
    let history = context.history_summary.unwrap_or("none");
    format!(
        "Task: {task}\nSession: {session_id}\nObservation: id={observation_id} action_seq={action_seq} captured_at={captured_at}\nPage: url={url} title={title} loading_state={loading_state:?}\nViewport: {width}x{height} dsf={dsf}\nScreenshot: artifact_ref={artifact_uri} screenshot_id={screenshot_id} sha256={sha256} redacted={redacted}. The image bytes are attached separately, not in this text.\nA11y items: {a11y_count}\nNetwork failed count: {network_failed}\nConsole error count: {console_errors}\nCompact browser history: {history}\nReturn BrowserDecision JSON only. Schema: {schema}",
        task = sanitize_prompt_text(context.task),
        session_id = sanitize_prompt_text(context.session_id),
        observation_id = sanitize_prompt_text(&obs.observation_id),
        action_seq = obs.action_seq,
        captured_at = sanitize_prompt_text(&obs.captured_at),
        url = sanitize_prompt_text(&obs.url),
        title = sanitize_prompt_text(&obs.title),
        loading_state = obs.loading_state,
        width = obs.viewport.width,
        height = obs.viewport.height,
        dsf = obs.viewport.device_scale_factor,
        artifact_uri = sanitize_prompt_text(&obs.screenshot.artifact_uri),
        screenshot_id = sanitize_prompt_text(&obs.screenshot.screenshot_id),
        sha256 = sanitize_prompt_text(&obs.screenshot.sha256),
        redacted = obs.screenshot.redacted,
        a11y_count = obs.a11y_summary.len(),
        schema = browser_decision_json_schema(),
    )
}

#[must_use]
pub fn build_repair_prompt(
    context: &BrowserDecisionPromptContext<'_>,
    previous_output: &str,
    parser_error: &str,
) -> String {
    format!(
        "{}\n\nPrevious output was invalid and no browser action was executed. Parser error: {}\nPrevious output excerpt: {}\nRepair once by returning exactly one valid BrowserDecision JSON object only.",
        build_dynamic_state_prompt(context),
        sanitize_prompt_text(parser_error),
        sanitize_prompt_text(&truncate(previous_output, 1200))
    )
}

#[must_use]
pub const fn executable_confidence_threshold() -> f32 {
    0.55
}

#[must_use]
pub const fn sensitive_confidence_threshold() -> f32 {
    0.90
}

#[must_use]
pub const fn viewport_from_observation(observation: &BrowserObservation) -> Viewport {
    observation.viewport
}

fn sanitize_prompt_text(input: &str) -> String {
    input
        .replace("data:image", "data:[redacted-image]")
        .replace("base64", "[redacted-base64]")
        .replace('\n', " ")
}

fn truncate(input: &str, max_chars: usize) -> String {
    let mut out = input.chars().take(max_chars).collect::<String>();
    if input.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::providers::browser_live::types::{
        BrowserObservation, LoadingState, ScreenshotArtifact, Viewport,
    };

    #[test]
    fn dynamic_prompt_keeps_volatile_state_out_of_stable_prompt() {
        let observation = observation();
        let ctx = BrowserDecisionPromptContext {
            task: "click the login button",
            session_id: "br-1",
            observation: &observation,
            history_summary: Some("browser_session session_id=br-1 latest_screenshot_id=shot-1"),
        };

        let stable = stable_system_prompt();
        let dynamic = build_dynamic_state_prompt(&ctx);

        assert!(!stable.contains("https://example.test"));
        assert!(!stable.contains("shot-1"));
        assert!(dynamic.contains("https://example.test"));
        assert!(dynamic.contains("artifact://browser/task/br-1/live.jpg"));
        assert!(!dynamic.contains("data:image"));
        assert!(!dynamic.contains("base64"));
    }

    fn observation() -> BrowserObservation {
        BrowserObservation {
            observation_id: "obs-1".to_string(),
            action_seq: 1,
            captured_at: "2026-06-16T10:00:00Z".to_string(),
            url: "https://example.test".to_string(),
            title: "Example".to_string(),
            viewport: Viewport::default(),
            loading_state: LoadingState::Idle,
            screenshot: ScreenshotArtifact {
                screenshot_id: "shot-1".to_string(),
                artifact_uri: "artifact://browser/task/br-1/live.jpg".to_string(),
                mime_type: "image/jpeg".to_string(),
                width: 1365,
                height: 768,
                sha256: "abc".to_string(),
                captured_at: Some("2026-06-16T10:00:00Z".to_string()),
                redacted: true,
            },
            a11y_summary: Vec::new(),
            network_summary: None,
            console_summary: None,
        }
    }
}
