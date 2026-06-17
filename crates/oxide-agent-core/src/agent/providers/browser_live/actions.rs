#![allow(missing_docs)]

use super::types::{ActionRequest, BrowserAction, GotoRequest, WaitUntil};

const MIN_ACTION_TIMEOUT_MS: u64 = 100;
const MAX_ACTION_TIMEOUT_MS: u64 = 60_000;

/// Result of planning a direct `browser_execute` action.
#[derive(Debug, Clone, PartialEq)]
pub enum BrowserExecutePlan {
    /// Send the action to the sidecar `/action` endpoint.
    SidecarAction(ActionRequest),
    /// Send the action to the sidecar `/goto` endpoint.
    Navigate(GotoRequest),
}

/// Maps a direct `BrowserAction` from the main agent into the sidecar request shape.
///
/// The `request_timeout_ms` is the overall timeout for the sidecar call; per-action
/// wait timeouts inside scripts are clamped to this value so they never exceed the
/// enclosing request timeout.
pub fn plan_browser_action(
    action: BrowserAction,
    action_seq: u64,
    request_timeout_ms: u64,
    expected_result: String,
) -> BrowserExecutePlan {
    let request_timeout_ms = bounded_timeout_ms(request_timeout_ms);
    match action {
        BrowserAction::Navigate { url, force_reload } => {
            BrowserExecutePlan::Navigate(GotoRequest {
                url,
                wait_until: WaitUntil::DomContentLoaded,
                timeout_ms: request_timeout_ms,
                capture_after: true,
                force_reload,
            })
        }
        action => BrowserExecutePlan::SidecarAction(build_action_request(
            action,
            action_seq,
            request_timeout_ms,
            expected_result,
        )),
    }
}

fn build_action_request(
    action: BrowserAction,
    action_seq: u64,
    request_timeout_ms: u64,
    expected_result: String,
) -> ActionRequest {
    let (capture_after, wait_for_stability) = action_metadata(&action);
    let action = clamp_action_timeouts(action, request_timeout_ms);
    ActionRequest {
        action_seq,
        action,
        expected_result,
        timeout_ms: request_timeout_ms,
        capture_after,
        wait_for_stability,
    }
}

fn action_metadata(action: &BrowserAction) -> (bool, bool) {
    let capture_after = needs_post_action_observation(action);
    (capture_after, false)
}

fn needs_post_action_observation(action: &BrowserAction) -> bool {
    match action {
        BrowserAction::GetElementValue { .. } | BrowserAction::Wait { .. } => false,
        BrowserAction::Script { steps } => steps.iter().any(needs_post_action_observation),
        // `execute_javascript` is intentionally treated as observable/mutating:
        // the receiver cannot prove an arbitrary expression is read-only.
        _ => true,
    }
}

fn clamp_action_timeouts(action: BrowserAction, request_timeout_ms: u64) -> BrowserAction {
    match action {
        BrowserAction::Script { steps } => BrowserAction::Script {
            steps: steps
                .into_iter()
                .map(|step| clamp_action_timeouts(step, request_timeout_ms))
                .collect(),
        },
        BrowserAction::Wait { timeout_ms } => BrowserAction::Wait {
            timeout_ms: timeout_ms.min(request_timeout_ms),
        },
        BrowserAction::WaitForSelector {
            selector,
            timeout_ms,
        } => BrowserAction::WaitForSelector {
            selector,
            timeout_ms: timeout_ms.min(request_timeout_ms),
        },
        BrowserAction::WaitForText { text, timeout_ms } => BrowserAction::WaitForText {
            text,
            timeout_ms: timeout_ms.min(request_timeout_ms),
        },
        other => other,
    }
}

fn bounded_timeout_ms(value: u64) -> u64 {
    value.clamp(MIN_ACTION_TIMEOUT_MS, MAX_ACTION_TIMEOUT_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click_action() -> BrowserAction {
        BrowserAction::ClickXy {
            x: 10,
            y: 20,
            target_description: Some("login".to_string()),
        }
    }

    #[test]
    fn maps_click_action_to_sidecar_action_request() {
        let plan = plan_browser_action(click_action(), 7, 30_000, "button clicked".to_string());
        let BrowserExecutePlan::SidecarAction(request) = plan else {
            panic!("expected sidecar action");
        };
        assert_eq!(request.action_seq, 7);
        assert!(request.capture_after);
        assert!(!request.wait_for_stability);
        assert_eq!(request.expected_result, "button clicked");
    }

    #[test]
    fn maps_get_element_value_to_result_only_sidecar_action_request() {
        let plan = plan_browser_action(
            BrowserAction::GetElementValue {
                selector: "input[name=secret]".to_string(),
            },
            3,
            10_000,
            "value read".to_string(),
        );
        let BrowserExecutePlan::SidecarAction(request) = plan else {
            panic!("expected sidecar action");
        };
        assert_eq!(request.action_seq, 3);
        assert!(matches!(
            request.action,
            BrowserAction::GetElementValue { ref selector } if selector == "input[name=secret]"
        ));
        assert!(!request.capture_after);
        assert!(!request.wait_for_stability);
    }

    #[test]
    fn maps_execute_javascript_to_sidecar_action_request() {
        let plan = plan_browser_action(
            BrowserAction::ExecuteJavaScript {
                expression: "document.querySelector('input').value".to_string(),
            },
            4,
            10_000,
            "value extracted".to_string(),
        );
        let BrowserExecutePlan::SidecarAction(request) = plan else {
            panic!("expected sidecar action");
        };
        assert_eq!(request.action_seq, 4);
        assert!(matches!(
            request.action,
            BrowserAction::ExecuteJavaScript { ref expression } if expression == "document.querySelector('input').value"
        ));
        assert!(request.capture_after);
        assert!(!request.wait_for_stability);
    }

    #[test]
    fn maps_wait_action_to_sidecar_action_request() {
        let plan = plan_browser_action(
            BrowserAction::Wait { timeout_ms: 1_500 },
            6,
            10_000,
            "waited".to_string(),
        );
        let BrowserExecutePlan::SidecarAction(request) = plan else {
            panic!("expected sidecar action");
        };
        assert_eq!(request.action_seq, 6);
        assert!(matches!(
            request.action,
            BrowserAction::Wait { timeout_ms: 1_500 }
        ));
        assert!(!request.capture_after);
        assert!(!request.wait_for_stability);
    }

    #[test]
    fn clamps_wait_timeout_to_request_timeout() {
        let plan = plan_browser_action(
            BrowserAction::Wait { timeout_ms: 20_000 },
            6,
            10_000,
            "waited".to_string(),
        );
        let BrowserExecutePlan::SidecarAction(request) = plan else {
            panic!("expected sidecar action");
        };
        assert!(matches!(
            request.action,
            BrowserAction::Wait { timeout_ms: 10_000 }
        ));
    }

    #[test]
    fn maps_wait_for_selector_action_to_sidecar_action_request() {
        let plan = plan_browser_action(
            BrowserAction::WaitForSelector {
                selector: "#success".to_string(),
                timeout_ms: 5_000,
            },
            6,
            10_000,
            "selector appeared".to_string(),
        );
        let BrowserExecutePlan::SidecarAction(request) = plan else {
            panic!("expected sidecar action");
        };
        assert_eq!(request.action_seq, 6);
        assert!(matches!(
            request.action,
            BrowserAction::WaitForSelector {
                selector: ref s,
                timeout_ms: 5_000,
            } if s == "#success"
        ));
        assert!(request.capture_after);
        assert!(!request.wait_for_stability);
    }

    #[test]
    fn maps_wait_for_text_action_to_sidecar_action_request() {
        let plan = plan_browser_action(
            BrowserAction::WaitForText {
                text: "Secret created".to_string(),
                timeout_ms: 7_000,
            },
            6,
            10_000,
            "text appeared".to_string(),
        );
        let BrowserExecutePlan::SidecarAction(request) = plan else {
            panic!("expected sidecar action");
        };
        assert_eq!(request.action_seq, 6);
        assert!(matches!(
            request.action,
            BrowserAction::WaitForText {
                text: ref t,
                timeout_ms: 7_000,
            } if t == "Secret created"
        ));
        assert!(request.capture_after);
        assert!(!request.wait_for_stability);
    }

    #[test]
    fn maps_script_action_to_sidecar_action_request() {
        let plan = plan_browser_action(
            BrowserAction::Script {
                steps: vec![
                    BrowserAction::Fill {
                        selector: "#secret".to_string(),
                        value: "hello".to_string(),
                    },
                    BrowserAction::ClickSelector {
                        selector: "button[type=submit]".to_string(),
                    },
                ],
            },
            9,
            20_000,
            "form submitted".to_string(),
        );
        let BrowserExecutePlan::SidecarAction(request) = plan else {
            panic!("expected sidecar action");
        };
        assert_eq!(request.action_seq, 9);
        assert!(request.capture_after);
        assert!(!request.wait_for_stability);
        let BrowserAction::Script { ref steps } = request.action else {
            panic!("expected script action");
        };
        assert_eq!(steps.len(), 2);
        assert!(matches!(
            steps[0],
            BrowserAction::Fill {
                selector: ref s,
                value: ref v,
            } if s == "#secret" && v == "hello"
        ));
        assert!(matches!(
            steps[1],
            BrowserAction::ClickSelector { selector: ref s } if s == "button[type=submit]"
        ));
    }

    #[test]
    fn result_only_script_skips_post_action_observation() {
        let plan = plan_browser_action(
            BrowserAction::Script {
                steps: vec![BrowserAction::GetElementValue {
                    selector: "#secret".to_string(),
                }],
            },
            9,
            20_000,
            "value read".to_string(),
        );
        let BrowserExecutePlan::SidecarAction(request) = plan else {
            panic!("expected sidecar action");
        };
        assert!(!request.capture_after);
        assert!(!request.wait_for_stability);
    }

    #[test]
    fn maps_press_combo_action_to_sidecar_action_request() {
        let plan = plan_browser_action(
            BrowserAction::Press {
                key: "ctrl+a".to_string(),
            },
            5,
            10_000,
            "pressed".to_string(),
        );
        let BrowserExecutePlan::SidecarAction(request) = plan else {
            panic!("expected sidecar action");
        };
        assert_eq!(request.action_seq, 5);
        assert!(matches!(request.action, BrowserAction::Press { ref key } if key == "ctrl+a"));
        assert!(request.capture_after);
        assert!(!request.wait_for_stability);
    }

    #[test]
    fn maps_http_navigation_to_goto_request() {
        let plan = plan_browser_action(
            BrowserAction::Navigate {
                url: "https://example.test/dashboard".to_string(),
                force_reload: false,
            },
            1,
            10_000,
            "navigated".to_string(),
        );
        let BrowserExecutePlan::Navigate(request) = plan else {
            panic!("expected navigation");
        };
        assert_eq!(request.url, "https://example.test/dashboard");
        assert!(request.capture_after);
        assert!(!request.force_reload);
    }

    #[test]
    fn maps_forced_reload_navigation_to_goto_request() {
        let plan = plan_browser_action(
            BrowserAction::Navigate {
                url: "https://example.test/#hash".to_string(),
                force_reload: true,
            },
            1,
            10_000,
            "navigated".to_string(),
        );
        let BrowserExecutePlan::Navigate(request) = plan else {
            panic!("expected navigation");
        };
        assert_eq!(request.url, "https://example.test/#hash");
        assert!(request.force_reload);
    }

    #[test]
    fn yolo_allows_non_web_navigation_url() {
        let plan = plan_browser_action(
            BrowserAction::Navigate {
                url: "file:///etc/passwd".to_string(),
                force_reload: false,
            },
            1,
            10_000,
            "navigated".to_string(),
        );
        assert!(matches!(plan, BrowserExecutePlan::Navigate(_)));
    }
}
