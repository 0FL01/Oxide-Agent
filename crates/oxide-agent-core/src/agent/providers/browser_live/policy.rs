//! Browser Live MVP security policy helpers.

use super::types::{BrowserDecision, BrowserDecisionAction, BrowserDecisionRisk, BrowserProfile};
use serde::Serialize;
use thiserror::Error;

/// Policy error reported before a browser action reaches the sidecar.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum BrowserPolicyError {
    /// Only HTTP(S) navigation is allowed in the MVP.
    #[error("browser navigation URL must use http or https")]
    InvalidNavigationUrl,
    /// MiMo selected a sensitive executable action without routing to approval.
    #[error("browser sensitive action requires approval before execution")]
    SensitiveActionRequiresApproval,
    /// Raw credentials/secrets must not be typed into the browser action payload.
    #[error("browser action contains a raw credential; use a secret reference and approval flow")]
    RawCredentialValue,
    /// Downloads and uploads are disabled for the MVP.
    #[error("browser downloads/uploads are disabled for the MVP")]
    DownloadUploadDisabled,
    /// Real Chrome profiles/cookies are disabled for the MVP.
    #[error("browser real profile/cookie attachment is disabled for the MVP")]
    RealProfileDisabled,
}

/// Redacted audit event emitted/stored around browser policy decisions.
#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct BrowserPolicyAuditEvent {
    /// Stable event name for log/event filters.
    pub event: &'static str,
    /// Policy decision: `allow` or `block`.
    pub decision: &'static str,
    /// Redacted human-readable reason.
    pub reason: String,
    /// Browser action kind without action values.
    pub action_kind: String,
    /// URL scheme only, never the full URL.
    pub url_scheme: Option<String>,
    /// Whether the local classifier considered the decision sensitive.
    pub sensitive: bool,
}

/// Validates MVP session policy: ephemeral profile, no downloads/uploads.
pub fn validate_session_policy(
    profile: BrowserProfile,
    allow_downloads: bool,
    allow_uploads: bool,
) -> Result<(), BrowserPolicyError> {
    if !matches!(profile, BrowserProfile::Ephemeral) {
        return Err(BrowserPolicyError::RealProfileDisabled);
    }
    if allow_downloads || allow_uploads {
        return Err(BrowserPolicyError::DownloadUploadDisabled);
    }
    Ok(())
}

/// Validates one BrowserDecision against local MVP policy gates.
pub fn validate_decision_policy(decision: &BrowserDecision) -> Result<(), BrowserPolicyError> {
    if let BrowserDecisionAction::Navigate { url } = &decision.action {
        validate_navigation_url(url)?;
    }

    if contains_raw_credential_value(&decision.action) {
        return Err(BrowserPolicyError::RawCredentialValue);
    }

    if decision.sensitive_action.required && is_executable_action(&decision.action) {
        return Err(BrowserPolicyError::SensitiveActionRequiresApproval);
    }
    if decision.risk == BrowserDecisionRisk::High && is_executable_action(&decision.action) {
        return Err(BrowserPolicyError::SensitiveActionRequiresApproval);
    }
    if classifier_requires_approval(decision) && is_executable_action(&decision.action) {
        return Err(BrowserPolicyError::SensitiveActionRequiresApproval);
    }
    Ok(())
}

/// Validates allow-by-default HTTP/HTTPS URL policy for MVP navigation.
pub fn validate_navigation_url(url: &str) -> Result<(), BrowserPolicyError> {
    let trimmed = url.trim();
    let Some((scheme, _)) = trimmed.split_once(':') else {
        return Err(BrowserPolicyError::InvalidNavigationUrl);
    };
    if matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        Ok(())
    } else {
        Err(BrowserPolicyError::InvalidNavigationUrl)
    }
}

/// Builds a compact redacted audit event. Never includes typed values or raw URLs.
#[must_use]
pub fn policy_audit_event(
    decision: &BrowserDecision,
    allowed: bool,
    reason: impl Into<String>,
) -> BrowserPolicyAuditEvent {
    BrowserPolicyAuditEvent {
        event: "browser_policy",
        decision: if allowed { "allow" } else { "block" },
        reason: reason.into(),
        action_kind: action_kind(&decision.action).to_string(),
        url_scheme: url_scheme(&decision.action),
        sensitive: decision.sensitive_action.required || classifier_requires_approval(decision),
    }
}

fn classifier_requires_approval(decision: &BrowserDecision) -> bool {
    let haystack = format!(
        "{} {} {}",
        decision.rationale,
        decision.expected_result,
        action_policy_text(&decision.action)
    )
    .to_ascii_lowercase();
    [
        "password",
        "credential",
        "token",
        "secret",
        "2fa",
        "two-factor",
        "otp",
        "captcha",
        "payment",
        "credit card",
        "purchase",
        "delete account",
        "send message",
    ]
    .iter()
    .any(|needle| haystack.contains(needle))
}

fn contains_raw_credential_value(action: &BrowserDecisionAction) -> bool {
    let value = match action {
        BrowserDecisionAction::Fill { value, .. }
        | BrowserDecisionAction::TypeText { text: value } => value.trim(),
        _ => return false,
    };
    if value.starts_with("env:") || value.starts_with("storage:") {
        return false;
    }
    let lowered = value.to_ascii_lowercase();
    lowered.contains("password=")
        || lowered.contains("token=")
        || lowered.contains("secret=")
        || looks_like_secret(value)
}

fn looks_like_secret(value: &str) -> bool {
    let alnum = value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .count();
    alnum >= 24 && value.chars().any(|ch| ch.is_ascii_digit())
}

fn is_executable_action(action: &BrowserDecisionAction) -> bool {
    matches!(
        action,
        BrowserDecisionAction::ClickXy { .. }
            | BrowserDecisionAction::ClickSelector { .. }
            | BrowserDecisionAction::ClickTargetId { .. }
            | BrowserDecisionAction::Fill { .. }
            | BrowserDecisionAction::TypeText { .. }
            | BrowserDecisionAction::Press { .. }
            | BrowserDecisionAction::Scroll { .. }
            | BrowserDecisionAction::Wait { .. }
            | BrowserDecisionAction::Navigate { .. }
    )
}

fn action_kind(action: &BrowserDecisionAction) -> &'static str {
    match action {
        BrowserDecisionAction::ClickXy { .. } => "click_xy",
        BrowserDecisionAction::ClickSelector { .. } => "click_selector",
        BrowserDecisionAction::ClickTargetId { .. } => "click_target_id",
        BrowserDecisionAction::Fill { .. } => "fill",
        BrowserDecisionAction::TypeText { .. } => "type_text",
        BrowserDecisionAction::Press { .. } => "press",
        BrowserDecisionAction::Scroll { .. } => "scroll",
        BrowserDecisionAction::Wait { .. } => "wait",
        BrowserDecisionAction::Navigate { .. } => "navigate",
        BrowserDecisionAction::Debug { .. } => "debug",
        BrowserDecisionAction::AskUser { .. } => "ask_user",
        BrowserDecisionAction::Done { .. } => "done",
    }
}

fn action_policy_text(action: &BrowserDecisionAction) -> &str {
    match action {
        BrowserDecisionAction::ClickXy {
            target_description: Some(value),
            ..
        } => value,
        BrowserDecisionAction::ClickSelector { selector }
        | BrowserDecisionAction::ClickTargetId {
            target_id: selector,
        }
        | BrowserDecisionAction::Fill { selector, .. }
        | BrowserDecisionAction::TypeText { text: selector }
        | BrowserDecisionAction::Press { key: selector }
        | BrowserDecisionAction::Navigate { url: selector }
        | BrowserDecisionAction::Debug { reason: selector }
        | BrowserDecisionAction::AskUser { question: selector }
        | BrowserDecisionAction::Done {
            final_answer: selector,
            ..
        } => selector,
        BrowserDecisionAction::ClickXy { .. } | BrowserDecisionAction::Scroll { .. } => "",
        BrowserDecisionAction::Wait { .. } => "wait",
    }
}

fn url_scheme(action: &BrowserDecisionAction) -> Option<String> {
    let BrowserDecisionAction::Navigate { url } = action else {
        return None;
    };
    url.trim()
        .split_once(':')
        .map(|(scheme, _)| scheme.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::providers::browser_live::types::BrowserSensitiveAction;

    #[test]
    fn url_policy_allows_http_https_without_domain_allowlist() {
        validate_navigation_url("https://any.example/path").expect("https allowed");
        validate_navigation_url("http://localhost:3000").expect("http allowed");
    }

    #[test]
    fn url_policy_rejects_non_web_schemes() {
        for url in [
            "file:///etc/passwd",
            "chrome://settings",
            "devtools://devtools/bundled/inspector.html",
            "data:text/html;base64,abc",
        ] {
            assert_eq!(
                validate_navigation_url(url),
                Err(BrowserPolicyError::InvalidNavigationUrl)
            );
        }
    }

    #[test]
    fn sensitive_classifier_blocks_executable_captcha_or_payment_action() {
        let mut decision = decision(BrowserDecisionAction::ClickXy {
            x: 1,
            y: 1,
            target_description: Some("captcha checkbox".to_string()),
        });
        decision.expected_result = "CAPTCHA solved".to_string();

        assert_eq!(
            validate_decision_policy(&decision),
            Err(BrowserPolicyError::SensitiveActionRequiresApproval)
        );
    }

    #[test]
    fn credential_policy_rejects_raw_secret_values_but_allows_refs() {
        let raw = decision(BrowserDecisionAction::Fill {
            selector: "#password".to_string(),
            value: "password=hunter2".to_string(),
        });
        assert_eq!(
            validate_decision_policy(&raw),
            Err(BrowserPolicyError::RawCredentialValue)
        );

        let reference = decision(BrowserDecisionAction::TypeText {
            text: "env:LOGIN_PASSWORD".to_string(),
        });
        assert_eq!(
            validate_decision_policy(&reference),
            Err(BrowserPolicyError::SensitiveActionRequiresApproval)
        );
    }

    #[test]
    fn session_policy_disables_download_upload_and_real_profile() {
        validate_session_policy(BrowserProfile::Ephemeral, false, false).expect("safe defaults");
        assert_eq!(
            validate_session_policy(BrowserProfile::Ephemeral, true, false),
            Err(BrowserPolicyError::DownloadUploadDisabled)
        );
    }

    #[test]
    fn audit_event_is_redacted_and_structured() {
        let decision = decision(BrowserDecisionAction::Fill {
            selector: "#password".to_string(),
            value: "password=hunter2".to_string(),
        });
        let audit = policy_audit_event(&decision, false, "blocked raw credential");
        let json = serde_json::to_string(&audit).expect("audit json");

        assert!(json.contains("browser_policy"));
        assert!(json.contains("fill"));
        assert!(!json.contains("hunter2"));
        assert!(!json.contains("password=hunter2"));
    }

    fn decision(action: BrowserDecisionAction) -> BrowserDecision {
        BrowserDecision {
            schema_version: 1,
            rationale: "continue safely".to_string(),
            action,
            expected_result: "visible state changes".to_string(),
            confidence: 0.9,
            risk: BrowserDecisionRisk::Low,
            sensitive_action: BrowserSensitiveAction {
                required: false,
                category: None,
                reason: None,
            },
            needs_debug: false,
        }
    }
}
