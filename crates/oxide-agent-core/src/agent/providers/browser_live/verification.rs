#![allow(missing_docs)]

use super::types::{
    ActionResult, ActionStatus, BrowserAction, BrowserObservation, NavigationResult,
    NavigationStatus,
};
use serde::Serialize;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserVerificationStatus {
    ActionVerified,
    VerificationFailed,
    Done,
    Timeout,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct BrowserActionVerification {
    pub status: BrowserVerificationStatus,
    pub reason: String,
    pub expected_result: String,
    pub before_observation_id: String,
    pub after_observation_id: Option<String>,
    pub before_screenshot_id: String,
    pub after_screenshot_id: Option<String>,
}

pub fn verify_sidecar_action(
    expected_result: &str,
    action: &BrowserAction,
    before: &BrowserObservation,
    action_result: &ActionResult,
    after: &BrowserObservation,
) -> BrowserActionVerification {
    if !action_result.technical_success || action_result.status != ActionStatus::Executed {
        return failed(
            expected_result,
            before,
            Some(after),
            format!(
                "sidecar action status {:?} is not verified visual success",
                action_result.status
            ),
        );
    }
    if action_result.action_seq > after.action_seq {
        return failed(
            expected_result,
            before,
            Some(after),
            "post-action observation action_seq is stale".to_string(),
        );
    }
    verify_fresh_visual_evidence(
        expected_result,
        &action_verified_reason(action),
        before,
        after,
    )
}

/// Verifies a pure sidecar action (e.g. `get_element_value`, `execute_javascript`,
/// `wait`) that intentionally does not produce a post-action screenshot.
pub fn verify_by_result(
    expected_result: &str,
    before: &BrowserObservation,
    action_result: &ActionResult,
) -> BrowserActionVerification {
    if !action_result.technical_success {
        return failed(
            expected_result,
            before,
            None,
            format!(
                "pure action status {:?} is not technically successful",
                action_result.status
            ),
        );
    }
    BrowserActionVerification {
        status: BrowserVerificationStatus::ActionVerified,
        reason: "action completed successfully".to_string(),
        expected_result: expected_result.to_string(),
        before_observation_id: before.observation_id.clone(),
        after_observation_id: None,
        before_screenshot_id: before.screenshot.screenshot_id.clone(),
        after_screenshot_id: None,
    }
}

pub fn verify_navigation(
    expected_result: &str,
    before: &BrowserObservation,
    navigation: &NavigationResult,
    after: &BrowserObservation,
) -> BrowserActionVerification {
    if !matches!(
        navigation.status,
        NavigationStatus::Loaded | NavigationStatus::Partial
    ) {
        return failed(
            expected_result,
            before,
            Some(after),
            format!(
                "navigation status {:?} is not visually verified",
                navigation.status
            ),
        );
    }
    verify_fresh_visual_evidence(
        expected_result,
        "fresh post-action screenshot captured after navigation",
        before,
        after,
    )
}

pub fn terminal_done(
    expected_result: &str,
    observation: &BrowserObservation,
    reason: String,
) -> BrowserActionVerification {
    BrowserActionVerification {
        status: BrowserVerificationStatus::Done,
        reason,
        expected_result: expected_result.to_string(),
        before_observation_id: observation.observation_id.clone(),
        after_observation_id: Some(observation.observation_id.clone()),
        before_screenshot_id: observation.screenshot.screenshot_id.clone(),
        after_screenshot_id: Some(observation.screenshot.screenshot_id.clone()),
    }
}

pub fn timeout_report(
    expected_result: &str,
    observation: &BrowserObservation,
    reason: String,
) -> BrowserActionVerification {
    terminal(
        expected_result,
        observation,
        BrowserVerificationStatus::Timeout,
        reason,
    )
}

fn verify_fresh_visual_evidence(
    expected_result: &str,
    reason: &str,
    before: &BrowserObservation,
    after: &BrowserObservation,
) -> BrowserActionVerification {
    if before.observation_id == after.observation_id
        || before.screenshot.screenshot_id == after.screenshot.screenshot_id
    {
        return failed(
            expected_result,
            before,
            Some(after),
            "post-action screenshot is not fresh".to_string(),
        );
    }
    BrowserActionVerification {
        status: BrowserVerificationStatus::ActionVerified,
        reason: reason.to_string(),
        expected_result: expected_result.to_string(),
        before_observation_id: before.observation_id.clone(),
        after_observation_id: Some(after.observation_id.clone()),
        before_screenshot_id: before.screenshot.screenshot_id.clone(),
        after_screenshot_id: Some(after.screenshot.screenshot_id.clone()),
    }
}

fn terminal(
    expected_result: &str,
    observation: &BrowserObservation,
    status: BrowserVerificationStatus,
    reason: String,
) -> BrowserActionVerification {
    BrowserActionVerification {
        status,
        reason,
        expected_result: expected_result.to_string(),
        before_observation_id: observation.observation_id.clone(),
        after_observation_id: None,
        before_screenshot_id: observation.screenshot.screenshot_id.clone(),
        after_screenshot_id: None,
    }
}

fn failed(
    expected_result: &str,
    before: &BrowserObservation,
    after: Option<&BrowserObservation>,
    reason: String,
) -> BrowserActionVerification {
    BrowserActionVerification {
        status: BrowserVerificationStatus::VerificationFailed,
        reason,
        expected_result: expected_result.to_string(),
        before_observation_id: before.observation_id.clone(),
        after_observation_id: after.map(|observation| observation.observation_id.clone()),
        before_screenshot_id: before.screenshot.screenshot_id.clone(),
        after_screenshot_id: after.map(|observation| observation.screenshot.screenshot_id.clone()),
    }
}

/// Produce a deterministic, action-specific reason for a visually-verified action.
///
/// Uses structured action parameters (not `expected_result` free-text) so the
/// reason reflects what the sidecar actually did, not what the LLM hoped for.
fn action_verified_reason(action: &BrowserAction) -> String {
    match action {
        BrowserAction::WaitForText { text, .. } => {
            format!("wait condition met: text '{text}' found on page")
        }
        BrowserAction::WaitForSelector { selector, .. } => {
            format!("wait condition met: selector '{selector}' present on page")
        }
        BrowserAction::Script { steps } => format!(
            "script with {} step(s) executed, fresh post-action screenshot captured",
            steps.len()
        ),
        _ => "fresh post-action screenshot captured".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::providers::browser_live::types::{
        LoadingState, ScreenshotArtifact, Viewport,
    };

    #[test]
    fn technical_success_requires_fresh_post_action_screenshot() {
        let before = observation("obs-1", "shot-1", 0);
        let after = observation("obs-2", "shot-2", 1);
        let result = ActionResult {
            action_seq: 1,
            kind: "click_xy".to_string(),
            status: ActionStatus::Executed,
            duration_ms: 10,
            technical_success: true,
            hint: None,
            result: None,
        };
        let action = BrowserAction::ClickXy {
            x: 10,
            y: 20,
            target_description: None,
        };

        let verification = verify_sidecar_action("expected", &action, &before, &result, &after);

        assert_eq!(
            verification.status,
            BrowserVerificationStatus::ActionVerified
        );
        assert_eq!(verification.reason, "fresh post-action screenshot captured");
    }

    #[test]
    fn wait_for_text_action_has_specific_reason() {
        let before = observation("obs-1", "shot-1", 0);
        let after = observation("obs-2", "shot-2", 1);
        let result = ActionResult {
            action_seq: 1,
            kind: "wait_for_text".to_string(),
            status: ActionStatus::Executed,
            duration_ms: 10,
            technical_success: true,
            hint: None,
            result: None,
        };
        let action = BrowserAction::WaitForText {
            text: "Welcome".to_string(),
            timeout_ms: 5000,
        };

        let verification = verify_sidecar_action("expected", &action, &before, &result, &after);

        assert_eq!(
            verification.status,
            BrowserVerificationStatus::ActionVerified
        );
        assert_eq!(
            verification.reason,
            "wait condition met: text 'Welcome' found on page"
        );
    }

    #[test]
    fn wait_for_selector_action_has_specific_reason() {
        let before = observation("obs-1", "shot-1", 0);
        let after = observation("obs-2", "shot-2", 1);
        let result = ActionResult {
            action_seq: 1,
            kind: "wait_for_selector".to_string(),
            status: ActionStatus::Executed,
            duration_ms: 10,
            technical_success: true,
            hint: None,
            result: None,
        };
        let action = BrowserAction::WaitForSelector {
            selector: "#submit".to_string(),
            timeout_ms: 5000,
        };

        let verification = verify_sidecar_action("expected", &action, &before, &result, &after);

        assert_eq!(
            verification.reason,
            "wait condition met: selector '#submit' present on page"
        );
    }

    #[test]
    fn script_action_has_step_count_in_reason() {
        let before = observation("obs-1", "shot-1", 0);
        let after = observation("obs-2", "shot-2", 1);
        let result = ActionResult {
            action_seq: 1,
            kind: "script".to_string(),
            status: ActionStatus::Executed,
            duration_ms: 10,
            technical_success: true,
            hint: None,
            result: None,
        };
        let action = BrowserAction::Script {
            steps: vec![
                BrowserAction::ClickXy {
                    x: 1,
                    y: 2,
                    target_description: None,
                },
                BrowserAction::Fill {
                    selector: "#email".to_string(),
                    value: "a@b.com".to_string(),
                },
            ],
        };

        let verification = verify_sidecar_action("expected", &action, &before, &result, &after);

        assert_eq!(
            verification.reason,
            "script with 2 step(s) executed, fresh post-action screenshot captured"
        );
    }

    #[test]
    fn noop_action_is_verification_failure() {
        let before = observation("obs-1", "shot-1", 0);
        let after = observation("obs-2", "shot-2", 1);
        let result = ActionResult {
            action_seq: 1,
            kind: "click_xy".to_string(),
            status: ActionStatus::NoOp,
            duration_ms: 10,
            technical_success: false,
            hint: Some("no visible change".to_string()),
            result: None,
        };
        let action = BrowserAction::ClickXy {
            x: 10,
            y: 20,
            target_description: None,
        };

        let verification = verify_sidecar_action("expected", &action, &before, &result, &after);

        assert_eq!(
            verification.status,
            BrowserVerificationStatus::VerificationFailed
        );
    }

    #[test]
    fn pure_action_verified_by_result_without_post_action_screenshot() {
        let before = observation("obs-1", "shot-1", 0);
        let result = ActionResult {
            action_seq: 1,
            kind: "get_element_value".to_string(),
            status: ActionStatus::Executed,
            duration_ms: 10,
            technical_success: true,
            hint: None,
            result: Some("secret-value".to_string()),
        };

        let verification = verify_by_result("expected", &before, &result);

        assert_eq!(
            verification.status,
            BrowserVerificationStatus::ActionVerified
        );
        assert_eq!(verification.reason, "action completed successfully");
        assert!(verification.after_observation_id.is_none());
        assert!(verification.after_screenshot_id.is_none());
    }

    #[test]
    fn noop_pure_action_with_technical_success_is_verified() {
        let before = observation("obs-1", "shot-1", 0);
        let result = ActionResult {
            action_seq: 1,
            kind: "wait".to_string(),
            status: ActionStatus::NoOp,
            duration_ms: 3_000,
            technical_success: true,
            hint: None,
            result: Some("waited 3000ms".to_string()),
        };

        let verification = verify_by_result("expected", &before, &result);

        assert_eq!(
            verification.status,
            BrowserVerificationStatus::ActionVerified
        );
        assert_eq!(verification.reason, "action completed successfully");
    }

    #[test]
    fn terminal_done_reports_success() {
        let observation = observation("obs-1", "shot-1", 0);
        let verification = terminal_done("expected", &observation, "done".to_string());
        assert_eq!(verification.status, BrowserVerificationStatus::Done);
    }

    fn observation(
        observation_id: &str,
        screenshot_id: &str,
        action_seq: u64,
    ) -> BrowserObservation {
        BrowserObservation {
            observation_id: observation_id.to_string(),
            action_seq,
            captured_at: "2026-06-16T00:00:00Z".to_string(),
            url: "https://example.test".to_string(),
            title: "Example".to_string(),
            viewport: Viewport::default(),
            loading_state: LoadingState::Idle,
            screenshot: ScreenshotArtifact {
                screenshot_id: screenshot_id.to_string(),
                artifact_uri: format!("artifact://browser/task/br/{screenshot_id}.jpg"),
                mime_type: "image/jpeg".to_string(),
                width: 1365,
                height: 768,
                sha256: screenshot_id.to_string(),
                captured_at: Some("2026-06-16T00:00:00Z".to_string()),
                redacted: false,
                byte_size: 0,
            },
            a11y_summary: Vec::new(),
            dom_snapshot: Vec::new(),
            dom_snapshot_error: None,
            network_summary: None,
            console_summary: None,
        }
    }
}
