//! Browser Live security policy helpers (disabled in Yolo mode).
//! All policy gates are no-ops; the agent has full access to the browser.

use super::types::BrowserProfile;
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

/// Navigation URL policy is disabled in Yolo mode.
pub fn validate_navigation_url(_url: &str) -> Result<(), BrowserPolicyError> {
    Ok(())
}

/// Always returns an allow audit event in Yolo mode.
#[must_use]
pub fn policy_audit_event(
    action: &super::types::BrowserAction,
    _allowed: bool,
    reason: impl Into<String>,
) -> BrowserPolicyAuditEvent {
    BrowserPolicyAuditEvent {
        event: "browser_policy",
        decision: "allow",
        reason: reason.into(),
        action_kind: action_kind(action).to_string(),
        url_scheme: url_scheme(action),
        sensitive: false,
    }
}

fn action_kind(action: &super::types::BrowserAction) -> &'static str {
    match action {
        super::types::BrowserAction::ClickXy { .. } => "click_xy",
        super::types::BrowserAction::ClickSelector { .. } => "click_selector",
        super::types::BrowserAction::Fill { .. } => "fill",
        super::types::BrowserAction::TypeText { .. } => "type_text",
        super::types::BrowserAction::Press { .. } => "press",
        super::types::BrowserAction::Scroll { .. } => "scroll",
        super::types::BrowserAction::GetElementValue { .. } => "get_element_value",
        super::types::BrowserAction::ExecuteJavaScript { .. } => "execute_javascript",
        super::types::BrowserAction::Wait { .. } => "wait",
        super::types::BrowserAction::WaitForSelector { .. } => "wait_for_selector",
        super::types::BrowserAction::WaitForText { .. } => "wait_for_text",
        super::types::BrowserAction::Script { .. } => "script",
        super::types::BrowserAction::Navigate { .. } => "navigate",
    }
}

fn url_scheme(action: &super::types::BrowserAction) -> Option<String> {
    let super::types::BrowserAction::Navigate { url } = action else {
        return None;
    };
    url.trim()
        .split_once(':')
        .map(|(scheme, _)| scheme.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::providers::browser_live::types::{BrowserAction, BrowserProfile};

    #[test]
    fn policy_is_no_op_in_yolo_mode() {
        validate_navigation_url("file:///etc/passwd").expect("yolo allows any url");
        validate_session_policy(BrowserProfile::Ephemeral, true, true)
            .expect("yolo allows downloads/uploads");
        let action = BrowserAction::Fill {
            selector: "#password".to_string(),
            value: "password=hunter2".to_string(),
        };
        let audit = policy_audit_event(&action, true, "yolo");
        assert_eq!(audit.decision, "allow");
        assert!(!audit.sensitive);
    }
}
