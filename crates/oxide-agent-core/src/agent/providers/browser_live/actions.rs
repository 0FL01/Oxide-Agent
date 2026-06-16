#![allow(missing_docs)]

use super::policy::validate_navigation_url;
use super::types::{
    ActionRequest, BrowserAction, BrowserDecision, BrowserDecisionAction, GotoRequest, WaitUntil,
};
use thiserror::Error;

const MIN_ACTION_TIMEOUT_MS: u64 = 100;
const MAX_ACTION_TIMEOUT_MS: u64 = 60_000;

#[derive(Debug, Clone, PartialEq)]
pub enum BrowserActionPlan {
    SidecarAction(ActionRequest),
    Navigate(GotoRequest),
    Debug {
        reason: String,
    },
    AskUser {
        question: String,
    },
    Done {
        final_answer: String,
        evidence: String,
    },
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum BrowserActionPlanError {
    #[error("browser navigation URL must use http or https")]
    InvalidNavigationUrl,
}

pub fn plan_browser_action(
    decision: &BrowserDecision,
    action_seq: u64,
    action_timeout_ms: u64,
) -> Result<BrowserActionPlan, BrowserActionPlanError> {
    let timeout_ms = bounded_timeout_ms(action_timeout_ms);
    match &decision.action {
        BrowserDecisionAction::ClickXy {
            x,
            y,
            target_description,
        } => Ok(BrowserActionPlan::SidecarAction(ActionRequest {
            action_seq,
            action: BrowserAction::ClickXy {
                x: *x,
                y: *y,
                target_description: target_description.clone(),
            },
            expected_result: decision.expected_result.clone(),
            timeout_ms,
            capture_after: true,
            wait_for_stability: true,
        })),
        BrowserDecisionAction::ClickSelector { selector } => {
            Ok(BrowserActionPlan::SidecarAction(ActionRequest {
                action_seq,
                action: BrowserAction::ClickSelector {
                    selector: selector.clone(),
                },
                expected_result: decision.expected_result.clone(),
                timeout_ms,
                capture_after: true,
                wait_for_stability: true,
            }))
        }
        BrowserDecisionAction::ClickTargetId { target_id } => {
            Ok(BrowserActionPlan::SidecarAction(ActionRequest {
                action_seq,
                action: BrowserAction::ClickTargetId {
                    target_id: target_id.clone(),
                },
                expected_result: decision.expected_result.clone(),
                timeout_ms,
                capture_after: true,
                wait_for_stability: true,
            }))
        }
        BrowserDecisionAction::Fill { selector, value } => {
            Ok(BrowserActionPlan::SidecarAction(ActionRequest {
                action_seq,
                action: BrowserAction::Fill {
                    selector: selector.clone(),
                    value: value.clone(),
                },
                expected_result: decision.expected_result.clone(),
                timeout_ms,
                capture_after: true,
                wait_for_stability: true,
            }))
        }
        BrowserDecisionAction::TypeText { text } => {
            Ok(BrowserActionPlan::SidecarAction(ActionRequest {
                action_seq,
                action: BrowserAction::TypeText { text: text.clone() },
                expected_result: decision.expected_result.clone(),
                timeout_ms,
                capture_after: true,
                wait_for_stability: true,
            }))
        }
        BrowserDecisionAction::Press { key } => {
            Ok(BrowserActionPlan::SidecarAction(ActionRequest {
                action_seq,
                action: BrowserAction::Press { key: key.clone() },
                expected_result: decision.expected_result.clone(),
                timeout_ms,
                capture_after: true,
                wait_for_stability: true,
            }))
        }
        BrowserDecisionAction::Scroll { delta_x, delta_y } => {
            Ok(BrowserActionPlan::SidecarAction(ActionRequest {
                action_seq,
                action: BrowserAction::Scroll {
                    delta_x: *delta_x,
                    delta_y: *delta_y,
                },
                expected_result: decision.expected_result.clone(),
                timeout_ms,
                capture_after: true,
                wait_for_stability: true,
            }))
        }
        BrowserDecisionAction::Wait {
            timeout_ms: wait_ms,
        } => Ok(BrowserActionPlan::SidecarAction(ActionRequest {
            action_seq,
            action: BrowserAction::Wait {
                timeout_ms: (*wait_ms).min(timeout_ms),
            },
            expected_result: decision.expected_result.clone(),
            timeout_ms: (*wait_ms).min(timeout_ms),
            capture_after: true,
            wait_for_stability: true,
        })),
        BrowserDecisionAction::Navigate { url } => {
            if validate_navigation_url(url).is_err() {
                return Err(BrowserActionPlanError::InvalidNavigationUrl);
            }
            Ok(BrowserActionPlan::Navigate(GotoRequest {
                url: url.clone(),
                wait_until: WaitUntil::DomContentLoaded,
                timeout_ms,
                capture_after: true,
            }))
        }
        BrowserDecisionAction::Debug { reason } => Ok(BrowserActionPlan::Debug {
            reason: reason.clone(),
        }),
        BrowserDecisionAction::AskUser { question } => Ok(BrowserActionPlan::AskUser {
            question: question.clone(),
        }),
        BrowserDecisionAction::Done {
            final_answer,
            evidence,
        } => Ok(BrowserActionPlan::Done {
            final_answer: final_answer.clone(),
            evidence: evidence.clone(),
        }),
    }
}

fn bounded_timeout_ms(value: u64) -> u64 {
    value.clamp(MIN_ACTION_TIMEOUT_MS, MAX_ACTION_TIMEOUT_MS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::providers::browser_live::types::{
        BrowserDecisionRisk, BrowserSensitiveAction,
    };

    #[test]
    fn maps_click_decision_to_sidecar_action_request() {
        let decision = decision(BrowserDecisionAction::ClickXy {
            x: 10,
            y: 20,
            target_description: Some("login".to_string()),
        });

        let plan = plan_browser_action(&decision, 7, 30_000).expect("plan");

        let BrowserActionPlan::SidecarAction(request) = plan else {
            panic!("expected sidecar action");
        };
        assert_eq!(request.action_seq, 7);
        assert!(request.capture_after);
        assert!(request.wait_for_stability);
    }

    #[test]
    fn maps_http_navigation_to_goto_request() {
        let decision = decision(BrowserDecisionAction::Navigate {
            url: "https://example.test/dashboard".to_string(),
        });

        let plan = plan_browser_action(&decision, 1, 10_000).expect("plan");

        let BrowserActionPlan::Navigate(request) = plan else {
            panic!("expected navigation");
        };
        assert_eq!(request.url, "https://example.test/dashboard");
        assert!(request.capture_after);
    }

    #[test]
    fn rejects_non_web_navigation_url() {
        let decision = decision(BrowserDecisionAction::Navigate {
            url: "file:///etc/passwd".to_string(),
        });

        let error = plan_browser_action(&decision, 1, 10_000).expect_err("invalid url");

        assert_eq!(error, BrowserActionPlanError::InvalidNavigationUrl);
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
}
