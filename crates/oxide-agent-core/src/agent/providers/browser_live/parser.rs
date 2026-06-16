#![allow(missing_docs)]

use super::prompt::{
    BROWSER_DECISION_SCHEMA_VERSION, executable_confidence_threshold,
    sensitive_confidence_threshold,
};
use super::types::{
    BrowserDecision, BrowserDecisionAction, BrowserDecisionRisk, BrowserSensitiveAction, Viewport,
};
use thiserror::Error;

const MAX_TEXT_INPUT_CHARS: usize = 4096;
const MAX_WAIT_MS: u64 = 10_000;
const MAX_SCROLL_DELTA: i32 = 3_000;

#[derive(Debug, Error)]
pub enum BrowserDecisionParseError {
    #[error("browser decision output does not contain exactly one JSON object")]
    JsonObjectExtraction,
    #[error("browser decision JSON is malformed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("browser decision schema_version must be 1")]
    SchemaVersion,
    #[error("browser decision confidence must be between 0 and 1")]
    ConfidenceRange,
    #[error("browser decision confidence {confidence:.2} is below threshold {threshold:.2}")]
    LowConfidence { confidence: f32, threshold: f32 },
    #[error("browser decision action is invalid: {0}")]
    InvalidAction(String),
    #[error("browser decision marks a sensitive action that requires approval")]
    SensitiveActionRequiresApproval,
    #[error("browser decision high-risk executable action is not allowed")]
    HighRiskExecutableAction,
}

#[derive(Debug, Clone, Copy)]
pub struct BrowserDecisionValidation {
    pub viewport: Viewport,
    pub executable_confidence_threshold: f32,
    pub sensitive_confidence_threshold: f32,
}

impl BrowserDecisionValidation {
    #[must_use]
    pub fn for_viewport(viewport: Viewport) -> Self {
        Self {
            viewport,
            executable_confidence_threshold: executable_confidence_threshold(),
            sensitive_confidence_threshold: sensitive_confidence_threshold(),
        }
    }
}

pub fn parse_browser_decision(
    output: &str,
    validation: BrowserDecisionValidation,
) -> Result<BrowserDecision, BrowserDecisionParseError> {
    let json = extract_single_json_object(output)?;
    let decision = serde_json::from_str::<BrowserDecision>(json)?;
    validate_browser_decision(&decision, validation)?;
    Ok(decision)
}

pub fn validate_browser_decision(
    decision: &BrowserDecision,
    validation: BrowserDecisionValidation,
) -> Result<(), BrowserDecisionParseError> {
    if decision.schema_version != BROWSER_DECISION_SCHEMA_VERSION {
        return Err(BrowserDecisionParseError::SchemaVersion);
    }
    if !decision.confidence.is_finite() || !(0.0..=1.0).contains(&decision.confidence) {
        return Err(BrowserDecisionParseError::ConfidenceRange);
    }
    non_empty("rationale", &decision.rationale)?;
    non_empty("expected_result", &decision.expected_result)?;
    validate_action(&decision.action, validation.viewport)?;

    if is_executable_action(&decision.action)
        && decision.confidence < validation.executable_confidence_threshold
    {
        return Err(BrowserDecisionParseError::LowConfidence {
            confidence: decision.confidence,
            threshold: validation.executable_confidence_threshold,
        });
    }

    validate_sensitive_action(&decision.sensitive_action)?;
    if decision.sensitive_action.required && is_executable_action(&decision.action) {
        return Err(BrowserDecisionParseError::SensitiveActionRequiresApproval);
    }
    if decision.sensitive_action.required
        && decision.confidence < validation.sensitive_confidence_threshold
    {
        return Err(BrowserDecisionParseError::LowConfidence {
            confidence: decision.confidence,
            threshold: validation.sensitive_confidence_threshold,
        });
    }
    if decision.risk == BrowserDecisionRisk::High && is_executable_action(&decision.action) {
        return Err(BrowserDecisionParseError::HighRiskExecutableAction);
    }
    Ok(())
}

fn validate_action(
    action: &BrowserDecisionAction,
    viewport: Viewport,
) -> Result<(), BrowserDecisionParseError> {
    match action {
        BrowserDecisionAction::ClickXy {
            x,
            y,
            target_description,
        } => {
            if *x >= viewport.width || *y >= viewport.height {
                return invalid_action(format!(
                    "click_xy coordinates ({x},{y}) outside viewport {}x{}",
                    viewport.width, viewport.height
                ));
            }
            if let Some(description) = target_description {
                non_empty("target_description", description)?;
            }
        }
        BrowserDecisionAction::ClickSelector { selector } => non_empty("selector", selector)?,
        BrowserDecisionAction::Fill { selector, value } => {
            non_empty("selector", selector)?;
            if value.chars().count() > MAX_TEXT_INPUT_CHARS {
                return invalid_action("fill value is too long");
            }
        }
        BrowserDecisionAction::TypeText { text } => {
            non_empty("text", text)?;
            if text.chars().count() > MAX_TEXT_INPUT_CHARS {
                return invalid_action("type_text value is too long");
            }
        }
        BrowserDecisionAction::Press { key } => non_empty("key", key)?,
        BrowserDecisionAction::Scroll { delta_x, delta_y } => {
            if *delta_x == 0 && *delta_y == 0 {
                return invalid_action("scroll delta must not be zero");
            }
            if delta_x.unsigned_abs() > MAX_SCROLL_DELTA as u32
                || delta_y.unsigned_abs() > MAX_SCROLL_DELTA as u32
            {
                return invalid_action("scroll delta is too large");
            }
        }
        BrowserDecisionAction::Wait { timeout_ms } => {
            if !(100..=MAX_WAIT_MS).contains(timeout_ms) {
                return invalid_action("wait timeout_ms must be between 100 and 10000");
            }
        }
        BrowserDecisionAction::Debug { reason } => non_empty("reason", reason)?,
        BrowserDecisionAction::AskUser { question } => non_empty("question", question)?,
        BrowserDecisionAction::Done {
            final_answer,
            evidence,
        } => {
            non_empty("final_answer", final_answer)?;
            non_empty("evidence", evidence)?;
        }
    }
    Ok(())
}

fn validate_sensitive_action(
    sensitive: &BrowserSensitiveAction,
) -> Result<(), BrowserDecisionParseError> {
    if sensitive.required {
        if let Some(category) = &sensitive.category {
            non_empty("sensitive_action.category", category)?;
        }
        if let Some(reason) = &sensitive.reason {
            non_empty("sensitive_action.reason", reason)?;
        }
    }
    Ok(())
}

fn is_executable_action(action: &BrowserDecisionAction) -> bool {
    matches!(
        action,
        BrowserDecisionAction::ClickXy { .. }
            | BrowserDecisionAction::ClickSelector { .. }
            | BrowserDecisionAction::Fill { .. }
            | BrowserDecisionAction::TypeText { .. }
            | BrowserDecisionAction::Press { .. }
            | BrowserDecisionAction::Scroll { .. }
            | BrowserDecisionAction::Wait { .. }
    )
}

fn non_empty(field: &str, value: &str) -> Result<(), BrowserDecisionParseError> {
    if value.trim().is_empty() {
        return invalid_action(format!("{field} must not be empty"));
    }
    Ok(())
}

fn invalid_action<T>(message: impl Into<String>) -> Result<T, BrowserDecisionParseError> {
    Err(BrowserDecisionParseError::InvalidAction(message.into()))
}

fn extract_single_json_object(output: &str) -> Result<&str, BrowserDecisionParseError> {
    let trimmed = output.trim();
    if trimmed.starts_with('{')
        && trimmed.ends_with('}')
        && serde_json::from_str::<serde_json::Value>(trimmed).is_ok()
    {
        return Ok(trimmed);
    }

    let mut ranges = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    let mut in_string = false;
    let mut escaped = false;

    for (idx, ch) in output.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    start = Some(idx);
                }
                depth += 1;
            }
            '}' => {
                if depth == 0 {
                    return Err(BrowserDecisionParseError::JsonObjectExtraction);
                }
                depth -= 1;
                if depth == 0 {
                    let end = idx + ch.len_utf8();
                    ranges.push((start.take().unwrap_or(idx), end));
                }
            }
            _ => {}
        }
    }

    if depth != 0 || ranges.len() != 1 {
        return Err(BrowserDecisionParseError::JsonObjectExtraction);
    }
    let (start, end) = ranges[0];
    Ok(&output[start..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_golden_valid_decision() {
        let decision = parse_browser_decision(valid_click(), validation()).expect("valid decision");

        assert_eq!(decision.schema_version, 1);
        assert!(matches!(
            decision.action,
            BrowserDecisionAction::ClickXy { .. }
        ));
    }

    #[test]
    fn extracts_exactly_one_json_object_from_prose() {
        let output = format!("Here is the plan: {}", valid_click());

        let decision = parse_browser_decision(&output, validation()).expect("one object");

        assert!(matches!(
            decision.action,
            BrowserDecisionAction::ClickXy { .. }
        ));
    }

    #[test]
    fn rejects_multiple_json_objects() {
        let output = format!("{}\n{}", valid_click(), valid_click());

        let error = parse_browser_decision(&output, validation()).expect_err("two objects reject");

        assert!(matches!(
            error,
            BrowserDecisionParseError::JsonObjectExtraction
        ));
    }

    #[test]
    fn rejects_coordinate_out_of_bounds() {
        let output = valid_click().replace("\"x\": 10", "\"x\": 2000");

        let error = parse_browser_decision(&output, validation()).expect_err("outside viewport");

        assert!(matches!(error, BrowserDecisionParseError::InvalidAction(_)));
    }

    #[test]
    fn rejects_low_confidence_executable_action() {
        let output = valid_click().replace("\"confidence\": 0.91", "\"confidence\": 0.2");

        let error = parse_browser_decision(&output, validation()).expect_err("low confidence");

        assert!(matches!(
            error,
            BrowserDecisionParseError::LowConfidence { .. }
        ));
    }

    #[test]
    fn rejects_sensitive_executable_action() {
        let output = valid_click().replace(
            "\"required\": false",
            "\"required\": true, \"category\": \"credentials\", \"reason\": \"password entry\"",
        );

        let error = parse_browser_decision(&output, validation()).expect_err("sensitive action");

        assert!(matches!(
            error,
            BrowserDecisionParseError::SensitiveActionRequiresApproval
        ));
    }

    fn validation() -> BrowserDecisionValidation {
        BrowserDecisionValidation::for_viewport(Viewport::default())
    }

    fn valid_click() -> &'static str {
        r#"{
          "schema_version": 1,
          "rationale": "The button is visible.",
          "action": {"kind": "click_xy", "x": 10, "y": 20, "target_description": "Login"},
          "expected_result": "Login form opens",
          "confidence": 0.91,
          "risk": "low",
          "sensitive_action": {"required": false},
          "needs_debug": false
        }"#
    }
}
