//! Native browser sidecar library — shared between the binary and tests.
//!
//! Replaces `chrome-agent-sidecar.py` + external `chrome-agent` subprocess
//! with a single Rust binary speaking CDP directly.

pub mod actions;
pub mod browser;
pub mod capture;
pub mod cdp;
pub mod dom;
pub mod observe;
pub mod screenshot;
pub mod session;
pub mod snapshot;
pub mod stealth;

use std::sync::Arc;
use std::time::Duration;

use axum::{
    Router,
    extract::{Path, Query, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Json, Response},
    routing::{delete, get, post},
};
use oxide_browser_contracts::{
    ActionRequest, ActionResponse, ActionStatus, BrowserObservation, CloseSessionRequest,
    CloseSessionResponse, ConsoleDebugPayload, ConsoleDebugQuery, ConsoleDebugResponse,
    CreateSessionRequest, CreateSessionResponse, GotoRequest, GotoResponse, NavigationResult,
    NavigationStatus, NetworkDebugPayload, NetworkDebugQuery, NetworkDebugResponse, ObserveQuery,
    ObserveResponse, ScreenshotFormat, ScreenshotQuery, ScreenshotResponse, SidecarErrorBody,
    WaitUntil,
};
use serde_json::json;
use tracing::warn;

use session::SessionManager;

/// Pause after mutating actions before draining capture (matches Python's 200ms).
const POST_ACTION_DRAIN_DELAY: Duration = Duration::from_millis(200);

/// Network idle polling timeout for `wait_until=networkidle`.
const NETWORK_IDLE_TIMEOUT: Duration = Duration::from_secs(2);

/// Network idle polling interval.
const NETWORK_IDLE_POLL: Duration = Duration::from_millis(50);

/// Shared state passed to all route handlers.
#[derive(Clone)]
pub struct AppState {
    pub sessions: Arc<SessionManager>,
}

/// Build the axum router with all routes (healthz + authed session routes).
pub fn create_app(state: AppState, token: String) -> Router {
    let authed_routes = Router::new()
        .route("/sessions", post(create_session))
        .route("/sessions/:id", delete(close_session))
        .route("/sessions/:id/goto", post(goto))
        .route("/sessions/:id/observe", get(observe))
        .route("/sessions/:id/action", post(action))
        .route("/sessions/:id/screenshot/latest", get(screenshot_latest))
        .route("/sessions/:id/debug/network", get(debug_network))
        .route("/sessions/:id/debug/console", get(debug_console))
        .route_layer(axum::middleware::from_fn_with_state(token, bearer_auth))
        .with_state(state);

    Router::new()
        .route("/healthz", get(healthz))
        .merge(authed_routes)
}

// ── Route handlers ──────────────────────────────────────────────────────

async fn healthz() -> Json<serde_json::Value> {
    Json(json!({"ok": true, "native": true}))
}

async fn create_session(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> Json<CreateSessionResponse> {
    Json(state.sessions.create(req).await)
}

async fn close_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<CloseSessionRequest>,
) -> Json<CloseSessionResponse> {
    Json(state.sessions.close(&id, req).await)
}

async fn goto(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<GotoRequest>,
) -> Json<GotoResponse> {
    let request_id = session::new_request_id();
    let session_id = id.clone();

    let Some(session) = state.sessions.get(&id).await else {
        return Json(GotoResponse {
            request_id,
            session_id,
            ok: false,
            navigation: NavigationResult {
                url: req.url.clone(),
                final_url: String::new(),
                status: NavigationStatus::Blocked,
                http_status: None,
                redirect_count: 0,
                force_reload: req.force_reload,
            },
            observation: None,
            error: Some(not_found_error()),
        });
    };

    let url = req.url.clone();
    let timeout = Duration::from_millis(req.timeout_ms.max(100));

    // Force reload: close browser, relaunch, navigate.
    if req.force_reload {
        if let Err(e) = session.force_reload(&url).await {
            warn!("goto force_reload error: {e:#}");
            return Json(GotoResponse {
                request_id,
                session_id,
                ok: false,
                navigation: NavigationResult {
                    url: url.clone(),
                    final_url: session.url(),
                    status: NavigationStatus::Blocked,
                    http_status: None,
                    redirect_count: 0,
                    force_reload: true,
                },
                observation: None,
                error: Some(SidecarErrorBody {
                    code: "fresh_navigation_failed".to_string(),
                    message: format!("failed to reload browser: {e:#}"),
                    retryable: true,
                    hint: Some("start a new browser session".to_string()),
                    details: serde_json::Value::Null,
                }),
            });
        }
    } else {
        // Check for SPA hash navigation (same origin, same path, only hash changed).
        let current_url = session.url();
        if is_same_origin_path_hash_navigation(&current_url, &url) {
            // Set hash via JS eval.
            let cdp = session.cdp().await;
            let hash = reqwest::Url::parse(&url)
                .ok()
                .and_then(|u| u.fragment().map(|f| format!("#{f}")))
                .unwrap_or_default();
            let script = format!(
                "window.location.hash = {}; true",
                serde_json::to_string(&hash).unwrap_or_else(|_| "\"\"".to_string())
            );
            let _ = cdp
                .send_command(
                    "Runtime.evaluate",
                    json!({"expression": script, "returnByValue": true}),
                    Duration::from_secs(15),
                )
                .await;
            // Brief wait for SPA to process the hash change.
            tokio::time::sleep(Duration::from_millis(500)).await;
        } else {
            // Normal navigation.
            if let Err(e) = session.navigate(&url, timeout).await {
                return Json(GotoResponse {
                    request_id,
                    session_id,
                    ok: false,
                    navigation: NavigationResult {
                        url: url.clone(),
                        final_url: session.url(),
                        status: NavigationStatus::Blocked,
                        http_status: None,
                        redirect_count: 0,
                        force_reload: false,
                    },
                    observation: None,
                    error: Some(SidecarErrorBody {
                        code: "navigation_failed".to_string(),
                        message: format!("navigation error: {e:#}"),
                        retryable: true,
                        hint: Some("check the URL and retry".to_string()),
                        details: serde_json::Value::Null,
                    }),
                });
            }
        }

        // Wait for network idle if requested.
        if req.wait_until == WaitUntil::NetworkIdle {
            let capture = session.capture().await;
            wait_for_network_idle(&capture).await;
        }
    }

    // Build observation if capture_after is true.
    let observation = if req.capture_after {
        Some(observe::build_observation(&session, 0, true, true, true, true, 20).await)
    } else {
        None
    };

    // Get final URL from session (may have been updated by navigation).
    let final_url = session.url();

    Json(GotoResponse {
        request_id,
        session_id,
        ok: true,
        navigation: NavigationResult {
            url: url.clone(),
            final_url,
            status: NavigationStatus::Loaded,
            http_status: None,
            redirect_count: 0,
            force_reload: req.force_reload,
        },
        observation,
        error: None,
    })
}

async fn observe(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<ObserveQuery>,
) -> Json<ObserveResponse> {
    let request_id = session::new_request_id();
    let session_id = id.clone();

    let Some(session) = state.sessions.get(&id).await else {
        return Json(ObserveResponse {
            request_id,
            session_id,
            ok: false,
            observation: empty_observation(),
            error: Some(not_found_error()),
        });
    };

    let action_seq = session.next_action_seq();
    let observation = observe::build_observation(
        &session,
        action_seq,
        query.include_dom,
        query.include_network_summary,
        query.include_console_summary,
        query.fresh,
        query.max_debug_items,
    )
    .await;

    Json(ObserveResponse {
        request_id,
        session_id,
        ok: true,
        observation,
        error: None,
    })
}

async fn action(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ActionRequest>,
) -> Json<ActionResponse> {
    let request_id = session::new_request_id();
    let session_id = id.clone();

    let Some(session) = state.sessions.get(&id).await else {
        return Json(ActionResponse {
            request_id,
            session_id,
            ok: false,
            action_result: oxide_browser_contracts::ActionResult {
                action_seq: req.action_seq,
                kind: action_kind_str(&req.action),
                status: ActionStatus::Failed,
                duration_ms: 0,
                technical_success: false,
                hint: None,
                result: None,
            },
            post_observation: None,
            error: Some(not_found_error()),
        });
    };

    let cdp = session.cdp().await;
    let timeout = Duration::from_millis(req.timeout_ms.max(100));

    let mut action_result = actions::execute_action(&cdp, &req.action, timeout).await;
    action_result.action_seq = req.action_seq;

    let ok = action_result.technical_success;

    // Build post-observation if capture_after and action succeeded.
    let post_observation = if ok && req.capture_after {
        // Brief pause for in-flight network requests to complete.
        tokio::time::sleep(POST_ACTION_DRAIN_DELAY).await;
        Some(observe::build_observation(&session, req.action_seq, true, true, true, true, 20).await)
    } else {
        None
    };

    let error = if ok {
        None
    } else {
        Some(SidecarErrorBody {
            code: "action_failed".to_string(),
            message: action_result
                .hint
                .clone()
                .unwrap_or_else(|| "action failed".to_string()),
            retryable: false,
            hint: None,
            details: serde_json::Value::Null,
        })
    };

    Json(ActionResponse {
        request_id,
        session_id,
        ok,
        action_result,
        post_observation,
        error,
    })
}

async fn screenshot_latest(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<ScreenshotQuery>,
) -> Response {
    let request_id = session::new_request_id();
    let session_id = id.clone();

    let Some(session) = state.sessions.get(&id).await else {
        return Json(ScreenshotResponse {
            request_id,
            session_id,
            ok: false,
            screenshot: empty_screenshot(),
            error: Some(not_found_error()),
        })
        .into_response();
    };

    // Get or capture screenshot.
    let mut screenshot = if let Some(last) = session.last_screenshot() {
        last
    } else {
        let cdp = session.cdp().await;
        let screenshot_id = format!("shot-{}-{}", session.id, session.next_screenshot_seq());
        crate::screenshot::capture_screenshot(
            &cdp,
            session.viewport,
            &session.artifact_dir,
            &session.artifact_root,
            &screenshot_id,
        )
        .await
    };

    if query.redacted {
        screenshot.redacted = true;
    }

    if query.format == ScreenshotFormat::Binary {
        let bytes = crate::screenshot::read_latest_screenshot(&session.artifact_dir);
        let mut headers = HeaderMap::new();
        headers.insert(
            "content-type",
            "image/jpeg".parse().expect("valid header value"),
        );
        return (StatusCode::OK, headers, bytes).into_response();
    }

    Json(ScreenshotResponse {
        request_id,
        session_id,
        ok: true,
        screenshot,
        error: None,
    })
    .into_response()
}

async fn debug_network(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<NetworkDebugQuery>,
) -> Json<NetworkDebugResponse> {
    let request_id = session::new_request_id();
    let session_id = id.clone();

    let Some(session) = state.sessions.get(&id).await else {
        return Json(NetworkDebugResponse {
            request_id,
            session_id,
            ok: false,
            network: NetworkDebugPayload {
                failed_count: 0,
                items: Vec::new(),
                artifact_uri: None,
            },
            error: Some(not_found_error()),
        });
    };

    let history = session.network_history();
    let payload = capture::build_network_debug_payload(
        &history,
        query.since_action_seq,
        query.filter,
        query.include_bodies,
        query.limit as usize,
    );

    Json(NetworkDebugResponse {
        request_id,
        session_id,
        ok: true,
        network: payload,
        error: None,
    })
}

async fn debug_console(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<ConsoleDebugQuery>,
) -> Json<ConsoleDebugResponse> {
    let request_id = session::new_request_id();
    let session_id = id.clone();

    let Some(session) = state.sessions.get(&id).await else {
        return Json(ConsoleDebugResponse {
            request_id,
            session_id,
            ok: false,
            console: ConsoleDebugPayload {
                error_count: 0,
                warning_count: 0,
                items: Vec::new(),
                artifact_uri: None,
            },
            error: Some(not_found_error()),
        });
    };

    let history = session.console_history();
    let payload = capture::build_console_debug_payload(
        &history,
        query.since_action_seq,
        query.min_level,
        query.limit as usize,
    );

    Json(ConsoleDebugResponse {
        request_id,
        session_id,
        ok: true,
        console: payload,
        error: None,
    })
}

// ── Auth middleware ─────────────────────────────────────────────────────

async fn bearer_auth(
    axum::extract::State(expected_token): axum::extract::State<String>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Response {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if auth == format!("Bearer {expected_token}") {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({"ok": false, "code": "unauthorized", "message": "invalid or missing bearer token"})),
        )
            .into_response()
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Build a `not_found` error body for missing sessions.
fn not_found_error() -> SidecarErrorBody {
    SidecarErrorBody {
        code: "not_found".to_string(),
        message: "browser session not found".to_string(),
        retryable: false,
        hint: Some("start a new session".to_string()),
        details: serde_json::Value::Null,
    }
}

/// Check if a navigation is a same-origin, same-path, hash-only change (SPA).
fn is_same_origin_path_hash_navigation(current: &str, target: &str) -> bool {
    if current.is_empty() {
        return false;
    }
    let Ok(c) = reqwest::Url::parse(current) else {
        return false;
    };
    let Ok(t) = reqwest::Url::parse(target) else {
        return false;
    };
    // Must have a scheme (not a relative URL).
    if c.scheme().is_empty() {
        return false;
    }
    c.scheme() == t.scheme()
        && c.host_str() == t.host_str()
        && c.port() == t.port()
        && c.path() == t.path()
        && (c.fragment() != t.fragment() || t.fragment().is_some())
}

/// Wait for network idle by polling the capture collector's pending count.
async fn wait_for_network_idle(capture: &Arc<crate::capture::CaptureCollector>) {
    let deadline = tokio::time::Instant::now() + NETWORK_IDLE_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if capture.pending_request_count() == 0 {
            break;
        }
        tokio::time::sleep(NETWORK_IDLE_POLL).await;
    }
}

/// Get the string representation of a `BrowserAction` kind.
fn action_kind_str(action: &oxide_browser_contracts::BrowserAction) -> String {
    let value = serde_json::to_value(action).ok();
    value
        .as_ref()
        .and_then(|v| v.get("kind"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string()
}

/// Build an empty observation for error responses.
fn empty_observation() -> BrowserObservation {
    BrowserObservation {
        observation_id: String::new(),
        action_seq: 0,
        captured_at: String::new(),
        url: String::new(),
        title: String::new(),
        viewport: oxide_browser_contracts::Viewport::default(),
        loading_state: oxide_browser_contracts::LoadingState::Unknown,
        screenshot: empty_screenshot(),
        a11y_summary: Vec::new(),
        dom_snapshot: Vec::new(),
        dom_snapshot_error: None,
        network_summary: None,
        console_summary: None,
    }
}

/// Build an empty screenshot artifact for error responses.
fn empty_screenshot() -> oxide_browser_contracts::ScreenshotArtifact {
    oxide_browser_contracts::ScreenshotArtifact {
        screenshot_id: String::new(),
        artifact_uri: String::new(),
        mime_type: "image/jpeg".to_string(),
        width: 0,
        height: 0,
        sha256: String::new(),
        captured_at: None,
        redacted: true,
        byte_size: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_nav_detects_same_origin_hash_change() {
        assert!(is_same_origin_path_hash_navigation(
            "https://example.com/page",
            "https://example.com/page#section2"
        ));
    }

    #[test]
    fn hash_nav_rejects_different_path() {
        assert!(!is_same_origin_path_hash_navigation(
            "https://example.com/page",
            "https://example.com/other#hash"
        ));
    }

    #[test]
    fn hash_nav_rejects_different_host() {
        assert!(!is_same_origin_path_hash_navigation(
            "https://example.com/page",
            "https://other.com/page#hash"
        ));
    }

    #[test]
    fn hash_nav_rejects_empty_current() {
        assert!(!is_same_origin_path_hash_navigation(
            "",
            "https://example.com/page#hash"
        ));
    }

    #[test]
    fn hash_nav_accepts_hash_only_change() {
        assert!(is_same_origin_path_hash_navigation(
            "https://example.com/page#section1",
            "https://example.com/page#section2"
        ));
    }

    #[test]
    fn action_kind_str_returns_correct_kind() {
        let action = oxide_browser_contracts::BrowserAction::ClickSelector {
            selector: "#btn".to_string(),
        };
        assert_eq!(action_kind_str(&action), "click_selector");
    }
}
