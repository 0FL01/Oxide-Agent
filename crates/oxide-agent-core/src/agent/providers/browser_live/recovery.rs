#![allow(missing_docs)]

use super::types::{
    ActionResult, ActionStatus, BrowserAction, BrowserDecision, BrowserDecisionAction,
    ConsoleDebugPayload, NetworkDebugPayload,
};
use super::verification::BrowserActionVerification;
use serde::Serialize;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserRecoveryKind {
    StaleFrame,
    NoOpClick,
    CoordinateMismatch,
    ModalOverlay,
    LoadingTimeout,
    NetworkFailure,
    ConsoleFailure,
    InvalidJson,
    LowConfidence,
    VerificationFailed,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserRecoveryStatus {
    Attempted,
    SafeStopped,
    RepeatedLoopStopped,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct BrowserRecoveryDiagnostics {
    pub network_failed_count: u32,
    pub console_error_count: u32,
    pub network_artifact_uri: Option<String>,
    pub console_artifact_uri: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BrowserRecoveryReport {
    pub status: BrowserRecoveryStatus,
    pub kind: BrowserRecoveryKind,
    pub loop_signature: String,
    pub repeated: bool,
    pub attempted_steps: u32,
    pub max_steps: u32,
    pub js_click_allowed: bool,
    pub diagnostics: BrowserRecoveryDiagnostics,
    pub plan: BrowserRecoveryPlan,
    pub result: Option<BrowserRecoveryActionResult>,
    pub safe_stop_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserRecoveryPlan {
    SidecarAction { action: BrowserAction },
    FetchDebugOnly,
    SafeStop { reason: String },
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct BrowserRecoveryActionResult {
    pub action_seq: u64,
    pub status: ActionStatus,
    pub technical_success: bool,
    pub post_observation_id: String,
    pub post_screenshot_id: String,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct BrowserRecoverySettings {
    pub max_steps: u32,
    pub allow_js_click: bool,
}

impl Default for BrowserRecoverySettings {
    fn default() -> Self {
        Self {
            max_steps: 1,
            allow_js_click: false,
        }
    }
}

pub fn build_recovery_report(
    decision: &BrowserDecision,
    verification: &BrowserActionVerification,
    action_result: Option<&ActionResult>,
    network: &NetworkDebugPayload,
    console: &ConsoleDebugPayload,
    settings: BrowserRecoverySettings,
    repeated: bool,
) -> BrowserRecoveryReport {
    let kind = classify_recovery(decision, verification, action_result, network, console);
    let diagnostics = BrowserRecoveryDiagnostics {
        network_failed_count: network.failed_count,
        console_error_count: console.error_count,
        network_artifact_uri: network.artifact_uri.clone(),
        console_artifact_uri: console.artifact_uri.clone(),
    };
    let loop_signature = recovery_loop_signature(decision, kind);
    let plan = recovery_plan(decision, kind, settings);
    let status = if repeated {
        BrowserRecoveryStatus::RepeatedLoopStopped
    } else if matches!(plan, BrowserRecoveryPlan::SidecarAction { .. }) && settings.max_steps > 0 {
        BrowserRecoveryStatus::Attempted
    } else {
        BrowserRecoveryStatus::SafeStopped
    };
    let safe_stop_reason = match status {
        BrowserRecoveryStatus::Attempted => None,
        BrowserRecoveryStatus::RepeatedLoopStopped => {
            Some("same failed browser action recovery signature repeated".to_string())
        }
        BrowserRecoveryStatus::SafeStopped => Some(safe_stop_reason(kind)),
    };

    BrowserRecoveryReport {
        status,
        kind,
        loop_signature,
        repeated,
        attempted_steps: 0,
        max_steps: settings.max_steps,
        js_click_allowed: settings.allow_js_click,
        diagnostics,
        plan,
        result: None,
        safe_stop_reason,
    }
}

pub fn attach_recovery_result(
    report: &mut BrowserRecoveryReport,
    action_result: &ActionResult,
    post_observation_id: String,
    post_screenshot_id: String,
) {
    report.attempted_steps = 1;
    report.result = Some(BrowserRecoveryActionResult {
        action_seq: action_result.action_seq,
        status: action_result.status,
        technical_success: action_result.technical_success,
        post_observation_id,
        post_screenshot_id,
    });
}

pub fn recovery_loop_signature(decision: &BrowserDecision, kind: BrowserRecoveryKind) -> String {
    format!(
        "kind={kind:?};action={};expected={}",
        decision_action_signature(&decision.action),
        normalize_signature_text(&decision.expected_result)
    )
}

fn classify_recovery(
    decision: &BrowserDecision,
    verification: &BrowserActionVerification,
    action_result: Option<&ActionResult>,
    network: &NetworkDebugPayload,
    console: &ConsoleDebugPayload,
) -> BrowserRecoveryKind {
    let reason = verification.reason.to_ascii_lowercase();
    if reason.contains("invalid json") {
        return BrowserRecoveryKind::InvalidJson;
    }
    if reason.contains("low confidence") {
        return BrowserRecoveryKind::LowConfidence;
    }
    if reason.contains("stale") || reason.contains("not fresh") {
        return BrowserRecoveryKind::StaleFrame;
    }
    if reason.contains("timeout") {
        return BrowserRecoveryKind::LoadingTimeout;
    }
    if network.failed_count > 0 {
        return BrowserRecoveryKind::NetworkFailure;
    }
    if console_error_matches(console, &["modal", "overlay", "dialog"]) {
        return BrowserRecoveryKind::ModalOverlay;
    }
    if console.error_count > 0 {
        return BrowserRecoveryKind::ConsoleFailure;
    }
    if let Some(result) = action_result {
        let hint = result
            .hint
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if hint.contains("coordinate") || hint.contains("drift") || hint.contains("mismatch") {
            return BrowserRecoveryKind::CoordinateMismatch;
        }
        if result.status == ActionStatus::NoOp
            && matches!(decision.action, BrowserDecisionAction::ClickXy { .. })
        {
            return BrowserRecoveryKind::NoOpClick;
        }
    }
    BrowserRecoveryKind::VerificationFailed
}

fn recovery_plan(
    decision: &BrowserDecision,
    kind: BrowserRecoveryKind,
    settings: BrowserRecoverySettings,
) -> BrowserRecoveryPlan {
    if settings.max_steps == 0 {
        return BrowserRecoveryPlan::SafeStop {
            reason: "max recovery steps exhausted".to_string(),
        };
    }
    match kind {
        BrowserRecoveryKind::StaleFrame | BrowserRecoveryKind::LoadingTimeout => {
            BrowserRecoveryPlan::SidecarAction {
                action: BrowserAction::Wait { timeout_ms: 500 },
            }
        }
        BrowserRecoveryKind::NoOpClick | BrowserRecoveryKind::CoordinateMismatch => {
            if let BrowserDecisionAction::ClickTargetId { target_id } = &decision.action {
                BrowserRecoveryPlan::SidecarAction {
                    action: BrowserAction::ClickTargetId {
                        target_id: target_id.clone(),
                    },
                }
            } else {
                BrowserRecoveryPlan::SidecarAction {
                    action: BrowserAction::Scroll {
                        delta_x: 0,
                        delta_y: 400,
                    },
                }
            }
        }
        BrowserRecoveryKind::ModalOverlay => BrowserRecoveryPlan::SidecarAction {
            action: BrowserAction::Press {
                key: "Escape".to_string(),
            },
        },
        BrowserRecoveryKind::NetworkFailure | BrowserRecoveryKind::ConsoleFailure => {
            BrowserRecoveryPlan::FetchDebugOnly
        }
        BrowserRecoveryKind::InvalidJson
        | BrowserRecoveryKind::LowConfidence
        | BrowserRecoveryKind::VerificationFailed => BrowserRecoveryPlan::SafeStop {
            reason: safe_stop_reason(kind),
        },
    }
}

fn safe_stop_reason(kind: BrowserRecoveryKind) -> String {
    match kind {
        BrowserRecoveryKind::InvalidJson => {
            "invalid browser decision output cannot be recovered by executing an action".to_string()
        }
        BrowserRecoveryKind::LowConfidence => {
            "browser decision confidence remained too low for safe recovery".to_string()
        }
        BrowserRecoveryKind::NetworkFailure => {
            "network diagnostics required before retrying browser action".to_string()
        }
        BrowserRecoveryKind::ConsoleFailure => {
            "console diagnostics required before retrying browser action".to_string()
        }
        BrowserRecoveryKind::VerificationFailed => {
            "browser action verification failed without a deterministic recovery".to_string()
        }
        BrowserRecoveryKind::StaleFrame
        | BrowserRecoveryKind::NoOpClick
        | BrowserRecoveryKind::CoordinateMismatch
        | BrowserRecoveryKind::ModalOverlay
        | BrowserRecoveryKind::LoadingTimeout => "recovery plan was not executed".to_string(),
    }
}

fn console_error_matches(console: &ConsoleDebugPayload, needles: &[&str]) -> bool {
    console.items.iter().any(|item| {
        let text = item.text_redacted.to_ascii_lowercase();
        needles.iter().any(|needle| text.contains(needle))
    })
}

fn decision_action_signature(action: &BrowserDecisionAction) -> String {
    match action {
        BrowserDecisionAction::ClickXy { x, y, .. } => format!("click_xy:{x}:{y}"),
        BrowserDecisionAction::ClickSelector { selector } => {
            format!("click_selector:{}", normalize_signature_text(selector))
        }
        BrowserDecisionAction::ClickTargetId { target_id } => {
            format!("click_target_id:{}", normalize_signature_text(target_id))
        }
        BrowserDecisionAction::Fill { selector, .. } => {
            format!("fill:{}", normalize_signature_text(selector))
        }
        BrowserDecisionAction::TypeText { .. } => "type_text".to_string(),
        BrowserDecisionAction::Press { key } => format!("press:{}", normalize_signature_text(key)),
        BrowserDecisionAction::Scroll { delta_x, delta_y } => format!("scroll:{delta_x}:{delta_y}"),
        BrowserDecisionAction::GetElementValue { selector } => {
            format!("get_element_value:{}", normalize_signature_text(selector))
        }
        BrowserDecisionAction::ExecuteJavaScript { expression } => {
            format!(
                "execute_javascript:{}",
                normalize_signature_text(expression)
            )
        }
        BrowserDecisionAction::Wait { timeout_ms } => format!("wait:{timeout_ms}"),
        BrowserDecisionAction::WaitForSelector { selector, .. } => {
            format!("wait_for_selector:{}", normalize_signature_text(selector))
        }
        BrowserDecisionAction::WaitForText { text, .. } => {
            format!("wait_for_text:{}", normalize_signature_text(text))
        }
        BrowserDecisionAction::Navigate { url } => {
            format!("navigate:{}", normalize_signature_text(url))
        }
        BrowserDecisionAction::Debug { .. } => "debug".to_string(),
        BrowserDecisionAction::AskUser { .. } => "ask_user".to_string(),
        BrowserDecisionAction::Done { .. } => "done".to_string(),
        BrowserDecisionAction::Script { steps } => {
            let inner: Vec<String> = steps.iter().map(decision_action_signature).collect();
            format!("script:{}", inner.join("/"))
        }
    }
}

fn normalize_signature_text(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .take(8)
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::providers::browser_live::types::{
        BrowserDecisionRisk, BrowserSensitiveAction, ConsoleItem, ConsoleLevel,
    };
    use crate::agent::providers::browser_live::verification::BrowserVerificationStatus;

    #[test]
    fn no_op_click_classifies_to_scroll_recovery_with_js_disabled() {
        let decision = decision(BrowserDecisionAction::ClickXy {
            x: 10,
            y: 20,
            target_description: None,
        });
        let verification =
            verification("sidecar action status NoOp is not verified visual success");
        let action = action_result(ActionStatus::NoOp, None);

        let report = build_recovery_report(
            &decision,
            &verification,
            Some(&action),
            &network(0),
            &console(Vec::new()),
            BrowserRecoverySettings::default(),
            false,
        );

        assert_eq!(report.kind, BrowserRecoveryKind::NoOpClick);
        assert!(!report.js_click_allowed);
        assert!(matches!(
            report.plan,
            BrowserRecoveryPlan::SidecarAction {
                action: BrowserAction::Scroll { .. }
            }
        ));
    }

    #[test]
    fn modal_overlay_prefers_escape_recovery() {
        let decision = decision(BrowserDecisionAction::ClickXy {
            x: 10,
            y: 20,
            target_description: None,
        });
        let verification =
            verification("sidecar action status NoOp is not verified visual success");
        let action = action_result(ActionStatus::NoOp, None);

        let report = build_recovery_report(
            &decision,
            &verification,
            Some(&action),
            &network(0),
            &console(vec!["modal overlay blocked click"]),
            BrowserRecoverySettings::default(),
            false,
        );

        assert_eq!(report.kind, BrowserRecoveryKind::ModalOverlay);
        assert!(matches!(
            report.plan,
            BrowserRecoveryPlan::SidecarAction {
                action: BrowserAction::Press { .. }
            }
        ));
    }

    fn decision(action: BrowserDecisionAction) -> BrowserDecision {
        BrowserDecision {
            schema_version: 1,
            rationale: "test".to_string(),
            action,
            expected_result: "expected".to_string(),
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

    fn verification(reason: &str) -> BrowserActionVerification {
        BrowserActionVerification {
            status: BrowserVerificationStatus::VerificationFailed,
            task_success: false,
            reason: reason.to_string(),
            expected_result: "expected".to_string(),
            before_observation_id: "before".to_string(),
            after_observation_id: Some("after".to_string()),
            before_screenshot_id: "before-shot".to_string(),
            after_screenshot_id: Some("after-shot".to_string()),
        }
    }

    fn action_result(status: ActionStatus, hint: Option<&str>) -> ActionResult {
        ActionResult {
            action_seq: 1,
            kind: "click_xy".to_string(),
            status,
            duration_ms: 10,
            technical_success: status == ActionStatus::Executed,
            hint: hint.map(str::to_string),
            result: None,
        }
    }

    fn network(failed_count: u32) -> NetworkDebugPayload {
        NetworkDebugPayload {
            failed_count,
            items: Vec::new(),
            artifact_uri: Some("artifact://browser/net.json".to_string()),
        }
    }

    fn console(messages: Vec<&str>) -> ConsoleDebugPayload {
        ConsoleDebugPayload {
            error_count: messages.len() as u32,
            warning_count: 0,
            items: messages
                .into_iter()
                .map(|text| ConsoleItem {
                    timestamp: "2026-06-16T00:00:00Z".to_string(),
                    level: ConsoleLevel::Error,
                    text_redacted: text.to_string(),
                    source: None,
                    line: None,
                })
                .collect(),
            artifact_uri: Some("artifact://browser/console.json".to_string()),
        }
    }
}
