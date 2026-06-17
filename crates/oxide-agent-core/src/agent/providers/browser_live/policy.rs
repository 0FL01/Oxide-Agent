//! Browser Live security policy helpers (disabled in Yolo mode).
//! All policy gates are no-ops; the agent has full access to the browser.

use super::types::{BrowserDecision, BrowserProfile};
use serde::Serialize;
use thiserror::Error;

/// Policy error placeholder (no errors are produced in Yolo mode).
#[derive(Debug, Error, Eq, PartialEq)]
#[error("policy disabled in yolo mode")]
pub struct BrowserPolicyError;

/// Audit event placeholder (always allow in Yolo mode).
#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct BrowserPolicyAuditEvent {
    /// Event type (always `browser_policy`).
    pub event: &'static str,
    /// Policy decision (always `allow`).
    pub decision: &'static str,
    /// Human-readable reason for the decision.
    pub reason: String,
    /// Kind of the browser action being audited.
    pub action_kind: String,
    /// URL scheme of the action, if any.
    pub url_scheme: Option<String>,
    /// Sensitivity flag (always `false` in Yolo mode).
    pub sensitive: bool,
}

/// Session policy is disabled in Yolo mode.
pub fn validate_session_policy(
    _profile: BrowserProfile,
    _allow_downloads: bool,
    _allow_uploads: bool,
) -> Result<(), BrowserPolicyError> {
    Ok(())
}

/// Decision policy is disabled in Yolo mode.
pub fn validate_decision_policy(_decision: &BrowserDecision) -> Result<(), BrowserPolicyError> {
    Ok(())
}

/// Navigation URL policy is disabled in Yolo mode.
pub fn validate_navigation_url(_url: &str) -> Result<(), BrowserPolicyError> {
    Ok(())
}

/// Always returns an allow audit event in Yolo mode.
#[must_use]
pub fn policy_audit_event(
    decision: &BrowserDecision,
    _allowed: bool,
    reason: impl Into<String>,
) -> BrowserPolicyAuditEvent {
    BrowserPolicyAuditEvent {
        event: "browser_policy",
        decision: "allow",
        reason: reason.into(),
        action_kind: action_kind(&decision.action).to_string(),
        url_scheme: url_scheme(&decision.action),
        sensitive: false,
    }
}

fn action_kind(action: &super::types::BrowserDecisionAction) -> &'static str {
    match action {
        super::types::BrowserDecisionAction::ClickXy { .. } => "click_xy",
        super::types::BrowserDecisionAction::ClickSelector { .. } => "click_selector",
        super::types::BrowserDecisionAction::ClickTargetId { .. } => "click_target_id",
        super::types::BrowserDecisionAction::Fill { .. } => "fill",
        super::types::BrowserDecisionAction::TypeText { .. } => "type_text",
        super::types::BrowserDecisionAction::Press { .. } => "press",
        super::types::BrowserDecisionAction::Scroll { .. } => "scroll",
        super::types::BrowserDecisionAction::Wait { .. } => "wait",
        super::types::BrowserDecisionAction::Navigate { .. } => "navigate",
        super::types::BrowserDecisionAction::Debug { .. } => "debug",
        super::types::BrowserDecisionAction::AskUser { .. } => "ask_user",
        super::types::BrowserDecisionAction::Done { .. } => "done",
    }
}

fn url_scheme(action: &super::types::BrowserDecisionAction) -> Option<String> {
    let super::types::BrowserDecisionAction::Navigate { url } = action else {
        return None;
    };
    url.trim()
        .split_once(':')
        .map(|(scheme, _)| scheme.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::providers::browser_live::types::{
        BrowserDecision, BrowserDecisionAction, BrowserDecisionRisk, BrowserProfile,
        BrowserSensitiveAction,
    };

    #[test]
    fn policy_is_no_op_in_yolo_mode() {
        validate_navigation_url("file:///etc/passwd").expect("yolo allows any url");
        validate_session_policy(BrowserProfile::Ephemeral, true, true)
            .expect("yolo allows downloads/uploads");
        let decision = BrowserDecision {
            schema_version: 1,
            rationale: "yolo".to_string(),
            action: BrowserDecisionAction::Fill {
                selector: "#password".to_string(),
                value: "password=hunter2".to_string(),
            },
            expected_result: "filled".to_string(),
            confidence: 0.9,
            risk: BrowserDecisionRisk::High,
            sensitive_action: BrowserSensitiveAction {
                required: true,
                category: Some("credential".to_string()),
                reason: Some("password".to_string()),
            },
            needs_debug: false,
        };
        validate_decision_policy(&decision).expect("yolo allows sensitive action");
        let audit = policy_audit_event(&decision, true, "yolo");
        assert_eq!(audit.decision, "allow");
        assert!(!audit.sensitive);
    }
}
