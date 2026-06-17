#![allow(missing_docs)]

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Stable sidecar failure envelope body.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct SidecarErrorBody {
    /// Stable error code from the sidecar contract.
    pub code: String,
    /// Human-readable diagnostic.
    pub message: String,
    /// Whether the sidecar considers retry safe.
    #[serde(default)]
    pub retryable: bool,
    /// Optional recovery hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Optional structured diagnostics.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub details: Value,
}

/// Browser viewport contract.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Viewport {
    /// Viewport width in CSS pixels.
    pub width: u32,
    /// Viewport height in CSS pixels.
    pub height: u32,
    /// Browser device scale factor.
    pub device_scale_factor: f32,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            width: 1365,
            height: 768,
            device_scale_factor: 1.0,
        }
    }
}

/// Browser profile mode. MVP supports only ephemeral profiles.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserProfile {
    /// Isolated temporary profile.
    Ephemeral,
}

/// Request for `POST /sessions`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreateSessionRequest {
    /// Oxide task id that owns the browser session.
    pub task_id: String,
    /// Browser profile policy.
    pub profile: BrowserProfile,
    /// Fixed viewport used for screenshots and coordinate validation.
    pub viewport: Viewport,
    /// Session timezone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// Session locale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    /// Whether console recorder is enabled.
    #[serde(default)]
    pub record_console: bool,
    /// Whether network recorder is enabled.
    #[serde(default)]
    pub record_network: bool,
    /// Whether downloads are allowed.
    #[serde(default)]
    pub allow_downloads: bool,
    /// Whether uploads are allowed.
    #[serde(default)]
    pub allow_uploads: bool,
    /// Optional initial URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_url: Option<String>,
}

/// Browser identity metadata returned by sidecar session creation.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct BrowserDescriptor {
    /// Browser process id known to the sidecar.
    pub browser_id: String,
    /// Current page id known to the sidecar.
    pub page_id: String,
    /// Whether CDP connection is currently active.
    pub cdp_connected: bool,
}

/// Response from `POST /sessions`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreateSessionResponse {
    /// Request id assigned by the sidecar.
    pub request_id: String,
    /// Created session id.
    pub session_id: String,
    /// Success flag.
    pub ok: bool,
    /// Browser metadata.
    pub browser: BrowserDescriptor,
    /// Effective viewport.
    pub viewport: Viewport,
    /// Root artifact path for this session.
    pub artifact_root: String,
    /// Stable error envelope when `ok=false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<SidecarErrorBody>,
}

/// Request for `DELETE /sessions/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct CloseSessionRequest {
    /// Whether to purge browser profile data.
    #[serde(default = "default_true")]
    pub purge_profile: bool,
    /// Whether artifacts should be retained.
    #[serde(default = "default_true")]
    pub keep_artifacts: bool,
    /// Close reason.
    pub reason: CloseReason,
}

/// Browser session close reason.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CloseReason {
    Done,
    Cancelled,
    Error,
    Timeout,
    UserRequested,
}

/// Response from `DELETE /sessions/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct CloseSessionResponse {
    pub request_id: String,
    pub session_id: String,
    pub ok: bool,
    pub closed: bool,
    pub profile_purged: bool,
    pub artifacts_kept: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<SidecarErrorBody>,
}

/// Navigation wait policy.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WaitUntil {
    DomContentLoaded,
    NetworkIdle,
    Load,
}

/// Request for `POST /sessions/{id}/goto`.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct GotoRequest {
    pub url: String,
    pub wait_until: WaitUntil,
    pub timeout_ms: u64,
    #[serde(default)]
    pub capture_after: bool,
}

/// Response from `POST /sessions/{id}/goto`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GotoResponse {
    pub request_id: String,
    pub session_id: String,
    pub ok: bool,
    pub navigation: NavigationResult,
    pub observation: Option<BrowserObservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<SidecarErrorBody>,
}

/// Navigation result metadata.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct NavigationResult {
    pub url: String,
    pub final_url: String,
    pub status: NavigationStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(default)]
    pub redirect_count: u32,
}

/// Navigation terminal status.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum NavigationStatus {
    Loaded,
    Partial,
    Timeout,
    Blocked,
}

/// Loading state reported by the sidecar.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LoadingState {
    Idle,
    Loading,
    NetworkBusy,
    Unknown,
}

/// Query for `GET /sessions/{id}/observe`.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct ObserveQuery {
    #[serde(default)]
    pub fresh: bool,
    #[serde(default)]
    pub include_dom: bool,
    #[serde(default)]
    pub include_a11y: bool,
    #[serde(default)]
    pub include_network_summary: bool,
    #[serde(default)]
    pub include_console_summary: bool,
    pub max_debug_items: u32,
}

impl Default for ObserveQuery {
    fn default() -> Self {
        Self {
            fresh: false,
            include_dom: false,
            include_a11y: false,
            include_network_summary: true,
            include_console_summary: true,
            max_debug_items: 20,
        }
    }
}

/// Response from `GET /sessions/{id}/observe`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ObserveResponse {
    pub request_id: String,
    pub session_id: String,
    pub ok: bool,
    pub observation: BrowserObservation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<SidecarErrorBody>,
}

/// Full browser observation payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrowserObservation {
    pub observation_id: String,
    pub action_seq: u64,
    pub captured_at: String,
    pub url: String,
    pub title: String,
    pub viewport: Viewport,
    pub loading_state: LoadingState,
    pub screenshot: ScreenshotArtifact,
    #[serde(default)]
    pub a11y_summary: Vec<Value>,
    #[serde(default)]
    pub network_summary: Option<NetworkSummary>,
    #[serde(default)]
    pub console_summary: Option<ConsoleSummary>,
}

/// Screenshot metadata. No base64 bytes are part of this metadata contract.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct ScreenshotArtifact {
    pub screenshot_id: String,
    pub artifact_uri: String,
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_at: Option<String>,
    #[serde(default)]
    pub redacted: bool,
    /// Optional raw byte size of the screenshot artifact for metrics only.
    #[serde(default)]
    pub byte_size: u64,
}

/// Network summary embedded in observations.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct NetworkSummary {
    #[serde(default)]
    pub failed_count: u32,
    #[serde(default)]
    pub recent_failures: Vec<NetworkItem>,
}

/// Console summary embedded in observations.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct ConsoleSummary {
    #[serde(default)]
    pub error_count: u32,
    #[serde(default)]
    pub warning_count: u32,
    #[serde(default)]
    pub recent_errors: Vec<ConsoleItem>,
}

/// Request for `POST /sessions/{id}/action`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActionRequest {
    pub action_seq: u64,
    pub action: BrowserAction,
    pub expected_result: String,
    pub timeout_ms: u64,
    #[serde(default)]
    pub capture_after: bool,
    #[serde(default)]
    pub wait_for_stability: bool,
}

/// Browser action contract accepted by the sidecar.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserAction {
    ClickXy {
        x: u32,
        y: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_description: Option<String>,
    },
    ClickSelector {
        selector: String,
    },
    ClickTargetId {
        target_id: String,
    },
    Fill {
        selector: String,
        value: String,
    },
    TypeText {
        text: String,
    },
    Press {
        key: String,
    },
    Scroll {
        delta_x: i32,
        delta_y: i32,
    },
    GetElementValue {
        selector: String,
    },
    #[serde(rename = "execute_javascript")]
    ExecuteJavaScript {
        expression: String,
    },
    Wait {
        timeout_ms: u64,
    },
}

/// Strict MiMo browser decision returned by the Browser Live visual planner.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrowserDecision {
    pub schema_version: u8,
    pub rationale: String,
    pub action: BrowserDecisionAction,
    pub expected_result: String,
    pub confidence: f32,
    pub risk: BrowserDecisionRisk,
    pub sensitive_action: BrowserSensitiveAction,
    pub needs_debug: bool,
}

/// Browser action selected by MiMo after local validation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserDecisionAction {
    ClickXy {
        x: u32,
        y: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_description: Option<String>,
    },
    ClickSelector {
        selector: String,
    },
    ClickTargetId {
        target_id: String,
    },
    Fill {
        selector: String,
        value: String,
    },
    TypeText {
        text: String,
    },
    Press {
        key: String,
    },
    Scroll {
        delta_x: i32,
        delta_y: i32,
    },
    GetElementValue {
        selector: String,
    },
    #[serde(rename = "execute_javascript")]
    ExecuteJavaScript {
        expression: String,
    },
    Wait {
        timeout_ms: u64,
    },
    Navigate {
        url: String,
    },
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

/// Risk classification assigned by MiMo and enforced locally.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserDecisionRisk {
    Low,
    Medium,
    High,
}

/// Sensitive-action annotation assigned by MiMo.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct BrowserSensitiveAction {
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Response from `POST /sessions/{id}/action`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActionResponse {
    pub request_id: String,
    pub session_id: String,
    pub ok: bool,
    pub action_result: ActionResult,
    pub post_observation: Option<BrowserObservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<SidecarErrorBody>,
}

/// Action result reported by the sidecar.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct ActionResult {
    pub action_seq: u64,
    pub kind: String,
    pub status: ActionStatus,
    pub duration_ms: u64,
    pub technical_success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Optional string result returned by the action (e.g., DOM value or JS eval output).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

/// Technical action status.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ActionStatus {
    Executed,
    NoOp,
    Partial,
    Failed,
}

/// Query for `GET /sessions/{id}/screenshot/latest`.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct ScreenshotQuery {
    pub format: ScreenshotFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_width: Option<u32>,
    #[serde(default)]
    pub redacted: bool,
}

/// Screenshot response format.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ScreenshotFormat {
    Metadata,
    Binary,
}

/// Metadata response from latest screenshot endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct ScreenshotResponse {
    pub request_id: String,
    pub session_id: String,
    pub ok: bool,
    pub screenshot: ScreenshotArtifact,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<SidecarErrorBody>,
}

/// Query for network debug endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct NetworkDebugQuery {
    #[serde(default)]
    pub since_action_seq: u64,
    pub level: DebugLevel,
    #[serde(default)]
    pub include_bodies: bool,
    pub filter: NetworkFilter,
    pub limit: u32,
}

/// Query for console debug endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct ConsoleDebugQuery {
    #[serde(default)]
    pub since_action_seq: u64,
    pub level: DebugLevel,
    pub min_level: ConsoleLevel,
    pub limit: u32,
}

/// Debug output verbosity.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DebugLevel {
    Summary,
    Full,
}

/// Network filter.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum NetworkFilter {
    Failed,
    All,
    Xhr,
    Fetch,
    Document,
}

/// Console minimum level.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleLevel {
    Warning,
    Error,
}

/// Network debug response.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct NetworkDebugResponse {
    pub request_id: String,
    pub session_id: String,
    pub ok: bool,
    pub network: NetworkDebugPayload,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<SidecarErrorBody>,
}

/// Console debug response.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct ConsoleDebugResponse {
    pub request_id: String,
    pub session_id: String,
    pub ok: bool,
    pub console: ConsoleDebugPayload,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<SidecarErrorBody>,
}

/// Network debug payload.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct NetworkDebugPayload {
    #[serde(default)]
    pub failed_count: u32,
    #[serde(default)]
    pub items: Vec<NetworkItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_uri: Option<String>,
}

/// Console debug payload.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct ConsoleDebugPayload {
    #[serde(default)]
    pub error_count: u32,
    #[serde(default)]
    pub warning_count: u32,
    #[serde(default)]
    pub items: Vec<ConsoleItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_uri: Option<String>,
}

/// One redacted network event.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct NetworkItem {
    pub timestamp: String,
    pub method: String,
    pub url_redacted: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    pub resource_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_text: Option<String>,
}

/// One redacted console event.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct ConsoleItem {
    pub timestamp: String,
    pub level: ConsoleLevel,
    pub text_redacted: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// WebSocket subscribe message contract. CP-4 defines the wire type only.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct BrowserStreamSubscribe {
    #[serde(rename = "type")]
    pub event_type: String,
    pub session_id: String,
    pub token: String,
    #[serde(default)]
    pub include_screenshots: bool,
    pub max_fps: u32,
}

/// Browser stream event contract. CP-4 does not open a WebSocket connection.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BrowserStreamEvent {
    Observation {
        session_id: String,
        observation_id: String,
        screenshot_id: String,
        artifact_uri: String,
        url: String,
        title: String,
        loading_state: LoadingState,
        action_seq: u64,
    },
    Debug {
        session_id: String,
        network_failed_count: u32,
        console_error_count: u32,
    },
    Heartbeat {
        session_id: String,
        timestamp: String,
    },
    SessionClosed {
        session_id: String,
        reason: String,
    },
}

const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serializes_create_session_contract() {
        let request = CreateSessionRequest {
            task_id: "task-1".to_string(),
            profile: BrowserProfile::Ephemeral,
            viewport: Viewport::default(),
            timezone: Some("UTC".to_string()),
            locale: Some("en-US".to_string()),
            record_console: true,
            record_network: true,
            allow_downloads: false,
            allow_uploads: false,
            start_url: Some("https://example.com".to_string()),
        };

        let value = serde_json::to_value(request).expect("serialize");
        assert_eq!(value["profile"], "ephemeral");
        assert_eq!(value["viewport"]["width"], 1365);
        assert_eq!(value["start_url"], "https://example.com");
    }

    #[test]
    fn deserializes_observation_without_base64_image_bytes() {
        let response: ObserveResponse = serde_json::from_value(json!({
            "request_id": "req-1",
            "session_id": "br_1",
            "ok": true,
            "observation": {
                "observation_id": "obs_1",
                "action_seq": 7,
                "captured_at": "2026-06-16T10:30:00Z",
                "url": "https://example.com/dashboard",
                "title": "Dashboard",
                "viewport": {"width": 1365, "height": 768, "device_scale_factor": 1.0},
                "loading_state": "idle",
                "screenshot": {
                    "screenshot_id": "shot_1",
                    "artifact_uri": "browser/task/session/step-0007-observe.jpg",
                    "mime_type": "image/jpeg",
                    "width": 1365,
                    "height": 768,
                    "sha256": "abc"
                },
                "a11y_summary": [],
                "network_summary": {"failed_count": 0, "recent_failures": []},
                "console_summary": {"error_count": 0, "recent_errors": []}
            },
            "error": null
        }))
        .expect("deserialize observation");

        assert_eq!(
            response.observation.screenshot.artifact_uri,
            "browser/task/session/step-0007-observe.jpg"
        );
        let serialized = serde_json::to_string(&response).expect("serialize response");
        assert!(!serialized.contains("base64"));
        assert!(!serialized.contains("data:image"));
    }

    #[test]
    fn action_and_stream_events_use_snake_case_tags() {
        let action = BrowserAction::ClickXy {
            x: 10,
            y: 20,
            target_description: Some("button".to_string()),
        };
        let value = serde_json::to_value(action).expect("serialize action");
        assert_eq!(value["kind"], "click_xy");

        let get_value = BrowserAction::GetElementValue {
            selector: "input[name=secret]".to_string(),
        };
        let value = serde_json::to_value(get_value).expect("serialize action");
        assert_eq!(value["kind"], "get_element_value");
        assert_eq!(value["selector"], "input[name=secret]");

        let execute_js = BrowserAction::ExecuteJavaScript {
            expression: "document.title".to_string(),
        };
        let value = serde_json::to_value(execute_js).expect("serialize action");
        assert_eq!(value["kind"], "execute_javascript");
        assert_eq!(value["expression"], "document.title");

        let press = BrowserAction::Press {
            key: "ctrl+a".to_string(),
        };
        let value = serde_json::to_value(press).expect("serialize press");
        assert_eq!(value["kind"], "press");
        assert_eq!(value["key"], "ctrl+a");

        let event = BrowserStreamEvent::Heartbeat {
            session_id: "br_1".to_string(),
            timestamp: "2026-06-16T10:30:00Z".to_string(),
        };
        let value = serde_json::to_value(event).expect("serialize event");
        assert_eq!(value["type"], "heartbeat");
    }
}
