#![allow(missing_docs)]

use super::types::{
    ActionResult, ActionStatus, BrowserDecision, BrowserObservation, NavigationResult,
    NavigationStatus,
};
use serde::Serialize;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserVerificationStatus {
    ActionVerified,
    VerificationFailed,
    Done,
    NeedsUser,
    DebugRequested,
    Timeout,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct BrowserActionVerification {
    pub status: BrowserVerificationStatus,
    pub task_success: bool,
    pub reason: String,
    pub expected_result: String,
    pub before_observation_id: String,
    pub after_observation_id: Option<String>,
    pub before_screenshot_id: String,
    pub after_screenshot_id: Option<String>,
}

pub fn verify_sidecar_action(
    decision: &BrowserDecision,
    before: &BrowserObservation,
    action_result: &ActionResult,
    after: &BrowserObservation,
) -> BrowserActionVerification {
    if !action_result.technical_success || action_result.status != ActionStatus::Executed {
        return failed(
            decision,
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
            decision,
            before,
            Some(after),
            "post-action observation action_seq is stale".to_string(),
        );
    }
    verify_fresh_visual_evidence(decision, before, after)
}

pub fn verify_navigation(
    decision: &BrowserDecision,
    before: &BrowserObservation,
    navigation: &NavigationResult,
    after: &BrowserObservation,
) -> BrowserActionVerification {
    if !matches!(
        navigation.status,
        NavigationStatus::Loaded | NavigationStatus::Partial
    ) {
        return failed(
            decision,
            before,
            Some(after),
            format!(
                "navigation status {:?} is not visually verified",
                navigation.status
            ),
        );
    }
    verify_fresh_visual_evidence(decision, before, after)
}

pub fn terminal_done(
    decision: &BrowserDecision,
    observation: &BrowserObservation,
    reason: String,
) -> BrowserActionVerification {
    BrowserActionVerification {
        status: BrowserVerificationStatus::Done,
        task_success: true,
        reason,
        expected_result: decision.expected_result.clone(),
        before_observation_id: observation.observation_id.clone(),
        after_observation_id: Some(observation.observation_id.clone()),
        before_screenshot_id: observation.screenshot.screenshot_id.clone(),
        after_screenshot_id: Some(observation.screenshot.screenshot_id.clone()),
    }
}

pub fn terminal_needs_user(
    decision: &BrowserDecision,
    observation: &BrowserObservation,
    reason: String,
) -> BrowserActionVerification {
    terminal(
        decision,
        observation,
        BrowserVerificationStatus::NeedsUser,
        reason,
    )
}

pub fn terminal_debug(
    decision: &BrowserDecision,
    observation: &BrowserObservation,
    reason: String,
) -> BrowserActionVerification {
    terminal(
        decision,
        observation,
        BrowserVerificationStatus::DebugRequested,
        reason,
    )
}

pub fn timeout_report(
    decision: &BrowserDecision,
    observation: &BrowserObservation,
    reason: String,
) -> BrowserActionVerification {
    terminal(
        decision,
        observation,
        BrowserVerificationStatus::Timeout,
        reason,
    )
}

fn verify_fresh_visual_evidence(
    decision: &BrowserDecision,
    before: &BrowserObservation,
    after: &BrowserObservation,
) -> BrowserActionVerification {
    if before.observation_id == after.observation_id
        || before.screenshot.screenshot_id == after.screenshot.screenshot_id
    {
        return failed(
            decision,
            before,
            Some(after),
            "post-action screenshot is not fresh".to_string(),
        );
    }
    BrowserActionVerification {
        status: BrowserVerificationStatus::ActionVerified,
        task_success: false,
        reason: "fresh post-action screenshot captured; task success still requires a later done decision"
            .to_string(),
        expected_result: decision.expected_result.clone(),
        before_observation_id: before.observation_id.clone(),
        after_observation_id: Some(after.observation_id.clone()),
        before_screenshot_id: before.screenshot.screenshot_id.clone(),
        after_screenshot_id: Some(after.screenshot.screenshot_id.clone()),
    }
}

fn terminal(
    decision: &BrowserDecision,
    observation: &BrowserObservation,
    status: BrowserVerificationStatus,
    reason: String,
) -> BrowserActionVerification {
    BrowserActionVerification {
        status,
        task_success: false,
        reason,
        expected_result: decision.expected_result.clone(),
        before_observation_id: observation.observation_id.clone(),
        after_observation_id: None,
        before_screenshot_id: observation.screenshot.screenshot_id.clone(),
        after_screenshot_id: None,
    }
}

fn failed(
    decision: &BrowserDecision,
    before: &BrowserObservation,
    after: Option<&BrowserObservation>,
    reason: String,
) -> BrowserActionVerification {
    BrowserActionVerification {
        status: BrowserVerificationStatus::VerificationFailed,
        task_success: false,
        reason,
        expected_result: decision.expected_result.clone(),
        before_observation_id: before.observation_id.clone(),
        after_observation_id: after.map(|observation| observation.observation_id.clone()),
        before_screenshot_id: before.screenshot.screenshot_id.clone(),
        after_screenshot_id: after.map(|observation| observation.screenshot.screenshot_id.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::providers::browser_live::types::{
        BrowserDecisionAction, BrowserDecisionRisk, BrowserSensitiveAction, LoadingState,
        ScreenshotArtifact, Viewport,
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
        };

        let verification = verify_sidecar_action(&decision(), &before, &result, &after);

        assert_eq!(
            verification.status,
            BrowserVerificationStatus::ActionVerified
        );
        assert!(!verification.task_success);
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
        };

        let verification = verify_sidecar_action(&decision(), &before, &result, &after);

        assert_eq!(
            verification.status,
            BrowserVerificationStatus::VerificationFailed
        );
    }

    fn decision() -> BrowserDecision {
        BrowserDecision {
            schema_version: 1,
            rationale: "test".to_string(),
            action: BrowserDecisionAction::ClickXy {
                x: 10,
                y: 20,
                target_description: None,
            },
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
            network_summary: None,
            console_summary: None,
        }
    }
}
