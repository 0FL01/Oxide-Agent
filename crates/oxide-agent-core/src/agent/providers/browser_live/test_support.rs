#![allow(missing_docs)]

use super::client::{BrowserSidecar, IdempotencyKey};
use super::error::BrowserSidecarError;
use super::types::{
    ActionRequest, ActionResponse, ActionResult, ActionStatus, BrowserDescriptor,
    BrowserObservation, CloseSessionRequest, CloseSessionResponse, ConsoleDebugPayload,
    ConsoleDebugQuery, ConsoleDebugResponse, ConsoleItem, ConsoleLevel, ConsoleSummary,
    CreateSessionRequest, CreateSessionResponse, DebugLevel, GotoRequest, GotoResponse,
    LoadingState, NavigationResult, NavigationStatus, NetworkDebugPayload, NetworkDebugQuery,
    NetworkDebugResponse, NetworkFilter, NetworkItem, NetworkSummary, ObservationSummary,
    ObserveQuery, ObserveResponse, ScreenshotArtifact, ScreenshotFormat, ScreenshotQuery,
    ScreenshotResponse, Viewport,
};
use async_trait::async_trait;
use serde_json::json;
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum FakeActionOutcome {
    Success,
    DelaySuccess(Duration),
    NoOp,
    CoordinateDrift,
    Failure,
    StaleFrame,
}

#[derive(Debug, Clone)]
pub(crate) struct FakeBrowserSidecar {
    state: Arc<Mutex<FakeState>>,
}

#[derive(Debug)]
struct FakeState {
    next_session: u64,
    next_request: u64,
    sessions: BTreeMap<String, FakeSession>,
    action_outcomes: VecDeque<FakeActionOutcome>,
    network_items: Vec<NetworkItem>,
    console_items: Vec<ConsoleItem>,
    crash_next_request: bool,
}

#[derive(Debug, Clone)]
struct FakeSession {
    task_id: String,
    viewport: Viewport,
    action_seq: u64,
    observation_seq: u64,
    url: String,
    title: String,
    closed: bool,
}

impl Default for FakeBrowserSidecar {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeBrowserSidecar {
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(FakeState {
                next_session: 1,
                next_request: 1,
                sessions: BTreeMap::new(),
                action_outcomes: VecDeque::new(),
                network_items: Vec::new(),
                console_items: Vec::new(),
                crash_next_request: false,
            })),
        }
    }

    pub(crate) fn with_action_script(self, outcomes: Vec<FakeActionOutcome>) -> Self {
        self.state().action_outcomes = outcomes.into();
        self
    }

    pub(crate) fn add_network_failure(&self, url_redacted: &str, error_text: &str) {
        self.state().network_items.push(NetworkItem {
            timestamp: fixed_timestamp(),
            method: "GET".to_string(),
            url_redacted: url_redacted.to_string(),
            status: None,
            resource_type: "xhr".to_string(),
            error_text: Some(error_text.to_string()),
        });
    }

    pub(crate) fn add_console_error(&self, text_redacted: &str) {
        self.state().console_items.push(ConsoleItem {
            timestamp: fixed_timestamp(),
            level: ConsoleLevel::Error,
            text_redacted: text_redacted.to_string(),
            source: Some("app.js".to_string()),
            line: Some(42),
        });
    }

    pub(crate) fn crash_next_request(&self) {
        self.state().crash_next_request = true;
    }

    fn state(&self) -> MutexGuard<'_, FakeState> {
        self.state.lock().expect("fake sidecar state lock")
    }
}

#[async_trait]
impl BrowserSidecar for FakeBrowserSidecar {
    async fn healthz(&self) -> Result<serde_json::Value, BrowserSidecarError> {
        self.maybe_crash()?;
        Ok(json!({"ok": true, "fake": true}))
    }

    async fn create_session(
        &self,
        request: &CreateSessionRequest,
        _key: &IdempotencyKey,
    ) -> Result<CreateSessionResponse, BrowserSidecarError> {
        self.maybe_crash()?;
        let mut state = self.state();
        let session_id = format!("fake-br-{}", state.next_session);
        state.next_session += 1;
        let url = request
            .start_url
            .clone()
            .unwrap_or_else(|| "about:blank".to_string());
        let title = title_for_url(&url);
        let session = FakeSession {
            task_id: request.task_id.clone(),
            viewport: request.viewport,
            action_seq: 0,
            observation_seq: 1,
            url,
            title,
            closed: false,
        };
        state.sessions.insert(session_id.clone(), session);

        Ok(CreateSessionResponse {
            request_id: state.next_request_id(),
            session_id: session_id.clone(),
            ok: true,
            browser: BrowserDescriptor {
                browser_id: format!("fake-chromium-{session_id}"),
                page_id: "fake-page-1".to_string(),
                cdp_connected: true,
            },
            viewport: request.viewport,
            artifact_root: format!("browser/{}/{session_id}/", request.task_id),
            error: None,
        })
    }

    async fn close_session(
        &self,
        session_id: &str,
        request: &CloseSessionRequest,
        _key: &IdempotencyKey,
    ) -> Result<CloseSessionResponse, BrowserSidecarError> {
        self.maybe_crash()?;
        let mut state = self.state();
        state.session_mut(session_id)?.closed = true;

        Ok(CloseSessionResponse {
            request_id: state.next_request_id(),
            session_id: session_id.to_string(),
            ok: true,
            closed: true,
            profile_purged: request.purge_profile,
            artifacts_kept: request.keep_artifacts,
            error: None,
        })
    }

    async fn goto(
        &self,
        session_id: &str,
        request: &GotoRequest,
        _key: &IdempotencyKey,
    ) -> Result<GotoResponse, BrowserSidecarError> {
        self.maybe_crash()?;
        let mut state = self.state();
        let summary = {
            let session = state.session_mut(session_id)?;
            session.url.clone_from(&request.url);
            session.title = title_for_url(&request.url);
            session.observation_seq += 1;
            session.summary(session_id)
        };

        Ok(GotoResponse {
            request_id: state.next_request_id(),
            session_id: session_id.to_string(),
            ok: true,
            navigation: NavigationResult {
                url: request.url.clone(),
                final_url: request.url.clone(),
                status: NavigationStatus::Loaded,
                http_status: Some(200),
                redirect_count: 0,
            },
            observation: summary,
            error: None,
        })
    }

    async fn observe(
        &self,
        session_id: &str,
        _query: &ObserveQuery,
    ) -> Result<ObserveResponse, BrowserSidecarError> {
        self.maybe_crash()?;
        let mut state = self.state();
        let request_id = state.next_request_id();
        let observation = {
            let network_items = state.network_items.clone();
            let console_items = state.console_items.clone();
            let session = state.session_mut(session_id)?;
            session.observation(session_id, &network_items, &console_items)
        };

        Ok(ObserveResponse {
            request_id,
            session_id: session_id.to_string(),
            ok: true,
            observation,
            error: None,
        })
    }

    async fn execute_action(
        &self,
        session_id: &str,
        request: &ActionRequest,
        _key: &IdempotencyKey,
    ) -> Result<ActionResponse, BrowserSidecarError> {
        self.maybe_crash()?;
        let mut outcome = {
            let mut state = self.state();
            state
                .action_outcomes
                .pop_front()
                .unwrap_or(FakeActionOutcome::Success)
        };
        if let FakeActionOutcome::DelaySuccess(duration) = outcome {
            tokio::time::sleep(duration).await;
            outcome = FakeActionOutcome::Success;
        }
        if outcome == FakeActionOutcome::Failure {
            return Err(api_failure(
                "invalid_action",
                "fake action failure",
                false,
                Some("inspect debug output before retrying"),
            ));
        }

        let mut state = self.state();
        let summary = {
            let session = state.session_mut(session_id)?;
            session.action_seq = request.action_seq;
            if outcome != FakeActionOutcome::StaleFrame {
                session.observation_seq += 1;
            }
            if outcome == FakeActionOutcome::Success && session.url == "about:blank" {
                session.url = "https://example.test/success".to_string();
                session.title = "Success".to_string();
            }
            session.summary(session_id)
        };
        let status = match outcome {
            FakeActionOutcome::Success
            | FakeActionOutcome::DelaySuccess(_)
            | FakeActionOutcome::StaleFrame => ActionStatus::Executed,
            FakeActionOutcome::NoOp | FakeActionOutcome::CoordinateDrift => ActionStatus::NoOp,
            FakeActionOutcome::Failure => unreachable!("failure returned above"),
        };
        let technical_success = status == ActionStatus::Executed;
        let hint = match outcome {
            FakeActionOutcome::CoordinateDrift => Some("coordinate mismatch detected".to_string()),
            _ => (status == ActionStatus::NoOp)
                .then(|| "action produced no visible change".to_string()),
        };

        Ok(ActionResponse {
            request_id: state.next_request_id(),
            session_id: session_id.to_string(),
            ok: true,
            action_result: ActionResult {
                action_seq: request.action_seq,
                kind: action_kind(request),
                status,
                duration_ms: 25,
                technical_success,
                hint,
            },
            post_observation: summary,
            error: None,
        })
    }

    async fn latest_screenshot(
        &self,
        session_id: &str,
        query: &ScreenshotQuery,
    ) -> Result<ScreenshotResponse, BrowserSidecarError> {
        self.maybe_crash()?;
        if query.format != ScreenshotFormat::Metadata {
            return Err(api_failure(
                "policy_denied",
                "fake sidecar exposes metadata only",
                false,
                None,
            ));
        }
        let mut state = self.state();
        let request_id = state.next_request_id();
        let screenshot = state.session_mut(session_id)?.screenshot(session_id);

        Ok(ScreenshotResponse {
            request_id,
            session_id: session_id.to_string(),
            ok: true,
            screenshot,
            error: None,
        })
    }

    async fn latest_screenshot_bytes(
        &self,
        session_id: &str,
        query: &ScreenshotQuery,
    ) -> Result<Vec<u8>, BrowserSidecarError> {
        self.maybe_crash()?;
        if query.format != ScreenshotFormat::Binary {
            return Err(api_failure(
                "policy_denied",
                "fake sidecar binary endpoint requires binary format",
                false,
                None,
            ));
        }
        let mut state = self.state();
        state.session_mut(session_id)?;
        Ok(b"fake-browser-screenshot-bytes".to_vec())
    }

    async fn debug_network(
        &self,
        session_id: &str,
        query: &NetworkDebugQuery,
    ) -> Result<NetworkDebugResponse, BrowserSidecarError> {
        self.maybe_crash()?;
        let mut state = self.state();
        state.session_mut(session_id)?;
        let mut items = state.network_items.clone();
        if query.filter == NetworkFilter::Failed {
            items.retain(|item| {
                item.error_text.is_some() || item.status.is_some_and(|status| status >= 400)
            });
        }
        items.truncate(query.limit as usize);
        Ok(NetworkDebugResponse {
            request_id: state.next_request_id(),
            session_id: session_id.to_string(),
            ok: true,
            network: NetworkDebugPayload {
                failed_count: items.len() as u32,
                items,
                artifact_uri: Some(format!("browser/fake/{session_id}/network.json")),
            },
            error: None,
        })
    }

    async fn debug_console(
        &self,
        session_id: &str,
        query: &ConsoleDebugQuery,
    ) -> Result<ConsoleDebugResponse, BrowserSidecarError> {
        self.maybe_crash()?;
        let mut state = self.state();
        state.session_mut(session_id)?;
        let mut items = state.console_items.clone();
        if query.min_level == ConsoleLevel::Error {
            items.retain(|item| item.level == ConsoleLevel::Error);
        }
        items.truncate(query.limit as usize);
        let warning_count = items
            .iter()
            .filter(|item| item.level == ConsoleLevel::Warning)
            .count() as u32;
        let error_count = items
            .iter()
            .filter(|item| item.level == ConsoleLevel::Error)
            .count() as u32;
        Ok(ConsoleDebugResponse {
            request_id: state.next_request_id(),
            session_id: session_id.to_string(),
            ok: true,
            console: ConsoleDebugPayload {
                error_count,
                warning_count,
                items,
                artifact_uri: Some(format!("browser/fake/{session_id}/console.json")),
            },
            error: None,
        })
    }
}

impl FakeBrowserSidecar {
    fn maybe_crash(&self) -> Result<(), BrowserSidecarError> {
        let mut state = self.state();
        if state.crash_next_request {
            state.crash_next_request = false;
            return Err(api_failure(
                "browser_crashed",
                "fake browser crashed",
                true,
                Some("restart browser session"),
            ));
        }
        Ok(())
    }
}

impl FakeState {
    fn next_request_id(&mut self) -> String {
        let request_id = format!("fake-req-{}", self.next_request);
        self.next_request += 1;
        request_id
    }

    fn session_mut(&mut self, session_id: &str) -> Result<&mut FakeSession, BrowserSidecarError> {
        let session = self.sessions.get_mut(session_id).ok_or_else(|| {
            api_failure(
                "not_found",
                "fake browser session not found",
                false,
                Some("start a browser session first"),
            )
        })?;
        if session.closed {
            return Err(api_failure(
                "stale_session",
                "fake browser session is closed",
                false,
                Some("start a new browser session"),
            ));
        }
        Ok(session)
    }
}

impl FakeSession {
    fn summary(&self, _session_id: &str) -> ObservationSummary {
        ObservationSummary {
            observation_id: self.observation_id(),
            screenshot_id: self.screenshot_id(),
            url: self.url.clone(),
            title: self.title.clone(),
            loading_state: LoadingState::Idle,
        }
    }

    fn observation(
        &self,
        session_id: &str,
        network_items: &[NetworkItem],
        console_items: &[ConsoleItem],
    ) -> BrowserObservation {
        let failed_count = network_items
            .iter()
            .filter(|item| {
                item.error_text.is_some() || item.status.is_some_and(|status| status >= 400)
            })
            .count() as u32;
        let error_count = console_items
            .iter()
            .filter(|item| item.level == ConsoleLevel::Error)
            .count() as u32;
        let warning_count = console_items
            .iter()
            .filter(|item| item.level == ConsoleLevel::Warning)
            .count() as u32;

        BrowserObservation {
            observation_id: self.observation_id(),
            action_seq: self.action_seq,
            captured_at: fixed_timestamp(),
            url: self.url.clone(),
            title: self.title.clone(),
            viewport: self.viewport,
            loading_state: LoadingState::Idle,
            screenshot: self.screenshot(session_id),
            a11y_summary: Vec::new(),
            network_summary: Some(NetworkSummary {
                failed_count,
                recent_failures: network_items.to_vec(),
            }),
            console_summary: Some(ConsoleSummary {
                error_count,
                warning_count,
                recent_errors: console_items.to_vec(),
            }),
        }
    }

    fn screenshot(&self, session_id: &str) -> ScreenshotArtifact {
        ScreenshotArtifact {
            screenshot_id: self.screenshot_id(),
            artifact_uri: format!(
                "browser/{}/{session_id}/step-{seq:04}.jpg",
                self.task_id,
                seq = self.observation_seq
            ),
            mime_type: "image/jpeg".to_string(),
            width: self.viewport.width,
            height: self.viewport.height,
            sha256: format!("fake-sha256-{}-{}", self.action_seq, self.observation_seq),
            captured_at: Some(fixed_timestamp()),
            redacted: false,
            byte_size: 1024,
        }
    }

    fn observation_id(&self) -> String {
        format!("fake-obs-{}", self.observation_seq)
    }

    fn screenshot_id(&self) -> String {
        format!("fake-shot-{}", self.observation_seq)
    }
}

fn title_for_url(url: &str) -> String {
    if url == "about:blank" {
        "Blank".to_string()
    } else {
        format!("Fake page for {url}")
    }
}

fn action_kind(request: &ActionRequest) -> String {
    serde_json::to_value(&request.action)
        .ok()
        .and_then(|value| {
            value
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn fixed_timestamp() -> String {
    "2026-06-16T00:00:00Z".to_string()
}

fn api_failure(
    code: &str,
    message: &str,
    retryable: bool,
    hint: Option<&str>,
) -> BrowserSidecarError {
    BrowserSidecarError::ApiFailure {
        code: code.to_string(),
        message: message.to_string(),
        retryable,
        hint: hint.map(str::to_string),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::providers::browser_live::types::{
        BrowserAction, BrowserProfile, CloseReason, WaitUntil,
    };

    #[tokio::test]
    async fn fake_session_create_goto_observe_action_close_is_deterministic() {
        let fake = FakeBrowserSidecar::new()
            .with_action_script(vec![FakeActionOutcome::Success, FakeActionOutcome::NoOp]);
        let key = key();
        let session = fake
            .create_session(&create_request(Some("https://example.test")), &key)
            .await
            .expect("create fake session");

        let first_observation = fake
            .observe(&session.session_id, &ObserveQuery::default())
            .await
            .expect("first observe");
        let goto = fake
            .goto(
                &session.session_id,
                &GotoRequest {
                    url: "https://example.test/dashboard".to_string(),
                    wait_until: WaitUntil::DomContentLoaded,
                    timeout_ms: 5_000,
                    capture_after: true,
                },
                &key,
            )
            .await
            .expect("goto");
        let click = fake
            .execute_action(&session.session_id, &click_request(1), &key)
            .await
            .expect("click success");
        let noop = fake
            .execute_action(&session.session_id, &click_request(2), &key)
            .await
            .expect("click noop");
        let screenshot = fake
            .latest_screenshot(
                &session.session_id,
                &ScreenshotQuery {
                    format: ScreenshotFormat::Metadata,
                    max_width: None,
                    redacted: false,
                },
            )
            .await
            .expect("screenshot metadata");
        let closed = fake
            .close_session(
                &session.session_id,
                &CloseSessionRequest {
                    purge_profile: true,
                    keep_artifacts: true,
                    reason: CloseReason::Done,
                },
                &key,
            )
            .await
            .expect("close fake session");

        assert!(session.ok);
        assert_eq!(first_observation.observation.observation_id, "fake-obs-1");
        assert_eq!(goto.observation.observation_id, "fake-obs-2");
        assert_eq!(click.post_observation.observation_id, "fake-obs-3");
        assert_eq!(noop.action_result.status, ActionStatus::NoOp);
        assert!(!noop.action_result.technical_success);
        assert_eq!(noop.post_observation.observation_id, "fake-obs-4");
        assert_eq!(screenshot.screenshot.screenshot_id, "fake-shot-4");
        assert!(closed.closed);
    }

    #[tokio::test]
    async fn fake_stale_frame_preserves_screenshot_after_action() {
        let fake =
            FakeBrowserSidecar::new().with_action_script(vec![FakeActionOutcome::StaleFrame]);
        let key = key();
        let session = fake
            .create_session(&create_request(None), &key)
            .await
            .expect("create fake session");
        let before = fake
            .observe(&session.session_id, &ObserveQuery::default())
            .await
            .expect("before observe");

        let action = fake
            .execute_action(&session.session_id, &click_request(1), &key)
            .await
            .expect("stale action response");

        assert_eq!(
            action.post_observation.screenshot_id,
            before.observation.screenshot.screenshot_id
        );
        assert_eq!(
            action.post_observation.observation_id,
            before.observation.observation_id
        );
    }

    #[tokio::test]
    async fn fake_error_envelope_covers_failure_and_browser_crash() {
        let fake = FakeBrowserSidecar::new().with_action_script(vec![FakeActionOutcome::Failure]);
        let key = key();
        let session = fake
            .create_session(&create_request(None), &key)
            .await
            .expect("create fake session");

        let failure = fake
            .execute_action(&session.session_id, &click_request(1), &key)
            .await
            .expect_err("fake action failure");
        assert_eq!(failure.kind(), "browser_sidecar_invalid_action");
        assert!(!failure.is_retryable());

        fake.crash_next_request();
        let crash = fake
            .observe(&session.session_id, &ObserveQuery::default())
            .await
            .expect_err("fake browser crash");
        assert_eq!(crash.kind(), "browser_sidecar_browser_crashed");
        assert!(crash.is_retryable());
    }

    #[tokio::test]
    async fn fake_debug_endpoints_return_network_and_console_diagnostics() {
        let fake = FakeBrowserSidecar::new();
        fake.add_network_failure("https://example.test/api", "connection reset");
        fake.add_console_error("Uncaught fake error");
        let key = key();
        let session = fake
            .create_session(&create_request(None), &key)
            .await
            .expect("create fake session");

        let network = fake
            .debug_network(
                &session.session_id,
                &NetworkDebugQuery {
                    since_action_seq: 0,
                    level: DebugLevel::Summary,
                    include_bodies: false,
                    filter: NetworkFilter::Failed,
                    limit: 10,
                },
            )
            .await
            .expect("network debug");
        let console = fake
            .debug_console(
                &session.session_id,
                &ConsoleDebugQuery {
                    since_action_seq: 0,
                    level: DebugLevel::Summary,
                    min_level: ConsoleLevel::Error,
                    limit: 10,
                },
            )
            .await
            .expect("console debug");
        let observation = fake
            .observe(&session.session_id, &ObserveQuery::default())
            .await
            .expect("observe with debug summary");

        assert_eq!(network.network.failed_count, 1);
        assert_eq!(
            network.network.items[0].error_text.as_deref(),
            Some("connection reset")
        );
        assert_eq!(console.console.error_count, 1);
        assert_eq!(
            console.console.items[0].text_redacted,
            "Uncaught fake error"
        );
        assert_eq!(
            observation
                .observation
                .network_summary
                .expect("network summary")
                .failed_count,
            1
        );
        assert_eq!(
            observation
                .observation
                .console_summary
                .expect("console summary")
                .error_count,
            1
        );
    }

    fn create_request(start_url: Option<&str>) -> CreateSessionRequest {
        CreateSessionRequest {
            task_id: "task-fake".to_string(),
            profile: BrowserProfile::Ephemeral,
            viewport: Viewport::default(),
            timezone: Some("UTC".to_string()),
            locale: Some("en-US".to_string()),
            record_console: true,
            record_network: true,
            allow_downloads: false,
            allow_uploads: false,
            start_url: start_url.map(str::to_string),
        }
    }

    fn click_request(action_seq: u64) -> ActionRequest {
        ActionRequest {
            action_seq,
            action: BrowserAction::ClickXy {
                x: 10,
                y: 20,
                target_description: Some("button".to_string()),
            },
            expected_result: "button clicked".to_string(),
            timeout_ms: 1_000,
            capture_after: true,
            wait_for_stability: true,
        }
    }

    fn key() -> IdempotencyKey {
        IdempotencyKey::new("fake-key").expect("idempotency key")
    }
}
