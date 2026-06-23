use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

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
    /// Action sequence number from the core, used to stamp the post-navigation
    /// observation so it can be correlated with the originating action.
    pub action_seq: u64,
    #[serde(default)]
    pub capture_after: bool,
    /// Force a full page reload before navigating to the target URL.
    ///
    /// Useful for SPA hash-based routes that cache state in memory and do not
    /// re-initialize when only the hash changes.
    #[serde(default)]
    pub force_reload: bool,
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
    /// Whether the sidecar performed a forced reload before navigating.
    #[serde(default)]
    pub force_reload: bool,
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
    #[serde(default = "default_limit")]
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

/// Snapshot of a DOM element included in observations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DomSnapshotNode {
    /// CSS selector hint for the element.
    pub selector: String,
    /// Element tag name.
    pub tag: String,
    /// Visible text (truncated).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Form element value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Resolved absolute URL for anchors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    /// Data-* attributes that are often used by SPAs to store state.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, String>,
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
    pub dom_snapshot: Vec<DomSnapshotNode>,
    /// Structured failure when a DOM snapshot was requested but could not be captured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dom_snapshot_error: Option<SidecarErrorBody>,
    #[serde(default)]
    pub network_summary: Option<NetworkSummary>,
    #[serde(default)]
    pub console_summary: Option<ConsoleSummary>,
}

/// Default number of DOM root matches returned by extract endpoint.
pub const DOM_EXTRACT_DEFAULT_MAX_RESULTS: u32 = 10;
/// Hard maximum number of DOM root matches returned by extract endpoint.
pub const DOM_EXTRACT_MAX_RESULTS_LIMIT: u32 = 50;
/// Maximum number of fields evaluated for each DOM root match.
pub const DOM_EXTRACT_MAX_FIELDS_LIMIT: u32 = 20;
/// Default maximum characters returned for one extracted field value.
pub const DOM_EXTRACT_DEFAULT_MAX_VALUE_CHARS: u32 = 512;
/// Hard maximum characters returned for one extracted field value.
pub const DOM_EXTRACT_MAX_VALUE_CHARS_LIMIT: u32 = 2_000;
/// Default maximum aggregate characters returned across all extracted values.
pub const DOM_EXTRACT_DEFAULT_MAX_TOTAL_CHARS: u32 = 16_000;
/// Hard maximum aggregate characters returned across all extracted values.
pub const DOM_EXTRACT_MAX_TOTAL_CHARS_LIMIT: u32 = 24_000;

/// Request for `POST /sessions/{id}/extract/dom`.
///
/// `selector` selects root rows. `fields` are evaluated relative to each root
/// row; when omitted, the request behaves as the legacy single-field extractor
/// and returns one field named `value` using `attribute` or `innerText`.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct DomExtractRequest {
    /// CSS selector for root rows.
    pub selector: String,
    /// Legacy single-field attribute/property shorthand used when `fields` is empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribute: Option<String>,
    /// Structured fields to extract from each root row.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<DomExtractField>,
    /// Maximum root rows to return. Sidecar clamps this to
    /// [`DOM_EXTRACT_MAX_RESULTS_LIMIT`].
    #[serde(default = "default_dom_extract_max_results")]
    pub max_results: u32,
    /// Default per-field character cap. Sidecar clamps this to
    /// [`DOM_EXTRACT_MAX_VALUE_CHARS_LIMIT`].
    #[serde(default = "default_dom_extract_max_value_chars")]
    pub max_value_chars: u32,
    /// Aggregate character cap across all returned field values. Sidecar clamps
    /// this to [`DOM_EXTRACT_MAX_TOTAL_CHARS_LIMIT`].
    #[serde(default = "default_dom_extract_max_total_chars")]
    pub max_total_chars: u32,
}

/// One field projected by [`DomExtractRequest`].
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct DomExtractField {
    /// Stable output field name.
    pub name: String,
    /// CSS selector evaluated relative to each root row. Omit to read the root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    /// DOM property or attribute to read. Defaults to request attribute or `innerText`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribute: Option<String>,
    /// Per-field character cap. Sidecar clamps this to
    /// [`DOM_EXTRACT_MAX_VALUE_CHARS_LIMIT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_chars: Option<u32>,
}

/// Response from `POST /sessions/{id}/extract/dom`.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct DomExtractResponse {
    pub request_id: String,
    pub session_id: String,
    pub ok: bool,
    pub extraction: DomExtractPayload,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<SidecarErrorBody>,
}

/// Bounded DOM extraction payload.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct DomExtractPayload {
    pub selector: String,
    pub total_matches: u32,
    pub returned_matches: u32,
    pub truncated: bool,
    pub limits: DomExtractLimits,
    #[serde(default)]
    pub matches: Vec<DomExtractMatch>,
}

/// Effective limits applied by the sidecar.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct DomExtractLimits {
    pub max_results: u32,
    pub max_fields: u32,
    pub max_value_chars: u32,
    pub max_total_chars: u32,
}

/// One extracted root row.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct DomExtractMatch {
    pub index: u32,
    pub tag: String,
    #[serde(default)]
    pub fields: BTreeMap<String, DomExtractValue>,
}

/// One extracted field value with bounded diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct DomExtractValue {
    pub attribute: String,
    pub source: DomExtractValueSource,
    pub found: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub truncated: bool,
    pub original_chars: u32,
}

/// Where a DOM extraction value came from.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DomExtractValueSource {
    Property,
    Attribute,
    Computed,
    Missing,
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

/// Diagnostic scope classification for a network/console item.
///
/// Computed at capture time (where page identity — the top-level URL — is
/// known) and stored on each item, so downstream summary/debug builders
/// classify by a typed enum instead of re-deriving from URL strings after the
/// page has navigated. Drives summary surfacing: `SiteRelated` and `FirstParty`
/// are surfaced into `recent_*`; `BrowserInternal`, `ThirdPartySubresource`,
/// and `Benign` are suppressed into `ScopeCounts`.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, Eq, PartialEq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticScope {
    /// Top-level document / navigation of the page under test (surfaced).
    #[default]
    SiteRelated,
    /// Same-site subresource / XHR / fetch (surfaced).
    FirstParty,
    /// Subresource from an unrelated third-party host (suppressed, counted).
    ThirdPartySubresource,
    /// Browser-internal scheme: chrome, chrome-untrusted, devtools,
    /// chrome-extension, edge, about (suppressed, counted).
    BrowserInternal,
    /// Benign noise: data:/blob: URLs, favicon, canceled aborts (suppressed,
    /// counted).
    Benign,
}

impl DiagnosticScope {
    /// Whether this scope is surfaced in compact summaries (vs suppressed).
    #[must_use]
    pub const fn is_surfaced(self) -> bool {
        matches!(self, Self::SiteRelated | Self::FirstParty)
    }
}

/// Counts of items suppressed from compact summaries, bucketed by scope.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, Eq, PartialEq)]
pub struct ScopeCounts {
    #[serde(default)]
    pub browser_internal: u32,
    #[serde(default)]
    pub third_party: u32,
    #[serde(default)]
    pub benign: u32,
}

impl ScopeCounts {
    /// Add one suppressed item to the appropriate bucket. Surfaced scopes are
    /// ignored (they belong in `recent_*`, not the suppressed counts).
    pub fn record(&mut self, scope: DiagnosticScope) {
        match scope {
            DiagnosticScope::BrowserInternal => self.browser_internal += 1,
            DiagnosticScope::ThirdPartySubresource => self.third_party += 1,
            DiagnosticScope::Benign => self.benign += 1,
            DiagnosticScope::SiteRelated | DiagnosticScope::FirstParty => {}
        }
    }

    /// Total suppressed items across all buckets.
    #[must_use]
    pub const fn total(&self) -> u32 {
        self.browser_internal + self.third_party + self.benign
    }
}

/// Network summary embedded in observations.
///
/// Scoped to the current action/page: `recent_*` carry only surfaced
/// (`SiteRelated`/`FirstParty`) failures/requests, latest first. Internal,
/// third-party, and benign items are tallied in `suppressed`.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct NetworkSummary {
    #[serde(default)]
    pub failed_count: u32,
    #[serde(default)]
    pub recent_failures: Vec<NetworkItem>,
    #[serde(default)]
    pub request_count: u32,
    #[serde(default)]
    pub recent_requests: Vec<NetworkItem>,
    #[serde(default)]
    pub suppressed: ScopeCounts,
}

/// Console summary embedded in observations.
///
/// Scoped to the current action/page: `recent_errors` carry only surfaced
/// (`SiteRelated`/`FirstParty`) errors, latest first. Internal, third-party,
/// and benign items are tallied in `suppressed`.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct ConsoleSummary {
    #[serde(default)]
    pub error_count: u32,
    #[serde(default)]
    pub warning_count: u32,
    #[serde(default)]
    pub recent_errors: Vec<ConsoleItem>,
    #[serde(default)]
    pub suppressed: ScopeCounts,
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
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
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
    Fill {
        selector: String,
        value: String,
    },
    TypeText {
        selector: String,
        value: String,
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
    WaitForSelector {
        selector: String,
        timeout_ms: u64,
    },
    WaitForText {
        text: String,
        timeout_ms: u64,
    },
    Script {
        steps: Vec<BrowserAction>,
    },
    Navigate {
        url: String,
        #[serde(default)]
        force_reload: bool,
    },
}

/// All valid action kind strings, comma-separated for error messages.
const VALID_ACTION_KINDS: &str = "click_xy, click_selector, fill, type_text, press, scroll, get_element_value, execute_javascript, wait, wait_for_selector, wait_for_text, script, navigate";

/// Required fields for each action kind (excluding `kind` itself).
fn required_fields(kind: &str) -> Option<&'static [&'static str]> {
    match kind {
        "click_xy" => Some(&["x", "y"]),
        "click_selector" => Some(&["selector"]),
        "fill" => Some(&["selector", "value"]),
        "type_text" => Some(&["selector", "value"]),
        "press" => Some(&["key"]),
        "scroll" => Some(&["delta_x", "delta_y"]),
        "get_element_value" => Some(&["selector"]),
        "execute_javascript" => Some(&["expression"]),
        "wait" => Some(&["timeout_ms"]),
        "wait_for_selector" => Some(&["selector", "timeout_ms"]),
        "wait_for_text" => Some(&["text", "timeout_ms"]),
        "script" => Some(&["steps"]),
        "navigate" => Some(&["url"]),
        _ => None,
    }
}

/// Validates a raw JSON `action` value for human-readable error messages before
/// serde deserialization.
///
/// Returns `Ok(())` if the action has a known `kind` and all required fields are
/// present. Recurses into `script` steps. Does **not** check field types or
/// unknown fields — serde's `deny_unknown_fields` and type checking remain the
/// final authority.
///
/// # Errors
///
/// Returns a human-readable diagnostic string suitable for LLM consumption.
pub fn validate_action_fields(action: &Value) -> Result<(), String> {
    validate_action_fields_inner(action, None)
}

fn validate_action_fields_inner(action: &Value, script_path: Option<usize>) -> Result<(), String> {
    let prefix = match script_path {
        Some(idx) => format!("script step {idx}: "),
        None => String::new(),
    };

    let Some(obj) = action.as_object() else {
        return Err(format!("{prefix}action must be a JSON object"));
    };

    let Some(kind) = obj.get("kind").and_then(Value::as_str) else {
        return Err(format!(
            "{prefix}action is missing \"kind\" field or it is not a string.\n\
             Valid kinds: {VALID_ACTION_KINDS}."
        ));
    };

    let Some(required) = required_fields(kind) else {
        return Err(format!(
            "{prefix}unknown action kind \"{kind}\".\n\
             Valid kinds: {VALID_ACTION_KINDS}."
        ));
    };

    // Collect missing required fields.
    let missing: Vec<&str> = required
        .iter()
        .copied()
        .filter(|field| !obj.contains_key(*field))
        .collect();

    if missing.is_empty() {
        // Recurse into script steps.
        if kind == "script"
            && let Some(steps) = obj.get("steps").and_then(Value::as_array)
        {
            for (idx, step) in steps.iter().enumerate() {
                validate_action_fields_inner(step, Some(idx))?;
            }
        }
        return Ok(());
    }

    let fields_list = required.to_vec().join(", ");
    let missing_list = missing
        .iter()
        .copied()
        .map(|f| format!("\"{f}\""))
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "{prefix}action \"{kind}\" is missing required field{s} {missing_list}.\n\
         Required fields for \"{kind}\": {fields_list}.",
        s = if missing.len() == 1 { "" } else { "s" },
    ))
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
#[derive(Debug, Clone, Default, Serialize, Deserialize, Eq, PartialEq)]
pub struct ScreenshotQuery {
    #[serde(default)]
    pub format: ScreenshotFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_width: Option<u32>,
    #[serde(default)]
    pub redacted: bool,
}

/// Screenshot response format.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ScreenshotFormat {
    #[default]
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
    #[serde(default)]
    pub level: DebugLevel,
    #[serde(default)]
    pub include_bodies: bool,
    #[serde(default)]
    pub filter: NetworkFilter,
    #[serde(default = "default_limit")]
    pub limit: u32,
    /// Include suppressed scopes (browser-internal, third-party, benign). When
    /// false (default), only surfaced site-related/first-party items return.
    #[serde(default)]
    pub include_suppressed: bool,
}

/// Query for console debug endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct ConsoleDebugQuery {
    #[serde(default)]
    pub since_action_seq: u64,
    #[serde(default)]
    pub level: DebugLevel,
    #[serde(default)]
    pub min_level: ConsoleLevel,
    #[serde(default = "default_limit")]
    pub limit: u32,
    /// Include suppressed scopes (browser-internal, third-party, benign). When
    /// false (default), only surfaced site-related/first-party items return.
    #[serde(default)]
    pub include_suppressed: bool,
}

/// Debug output verbosity.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DebugLevel {
    #[default]
    Summary,
    Full,
}

/// Network filter.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum NetworkFilter {
    #[default]
    Failed,
    All,
    Xhr,
    Fetch,
    Document,
}

/// Console minimum level.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleLevel {
    Warning,
    #[default]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Diagnostic scope, classified at capture time.
    #[serde(default)]
    pub scope: DiagnosticScope,
    /// Number of fingerprint-identical occurrences of this item within the
    /// current action (per-observation), not cumulative across the session.
    #[serde(default = "default_occurrences", skip_serializing_if = "is_one")]
    pub occurrences: u32,
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
    /// Diagnostic scope, classified at capture time.
    #[serde(default)]
    pub scope: DiagnosticScope,
    /// Number of fingerprint-identical occurrences of this item within the
    /// current action (per-observation), not cumulative across the session.
    #[serde(default = "default_occurrences", skip_serializing_if = "is_one")]
    pub occurrences: u32,
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

/// Default for `ObserveQuery::max_debug_items` and `*DebugQuery::limit`.
const fn default_limit() -> u32 {
    20
}

const fn default_dom_extract_max_results() -> u32 {
    DOM_EXTRACT_DEFAULT_MAX_RESULTS
}

const fn default_dom_extract_max_value_chars() -> u32 {
    DOM_EXTRACT_DEFAULT_MAX_VALUE_CHARS
}

const fn default_dom_extract_max_total_chars() -> u32 {
    DOM_EXTRACT_DEFAULT_MAX_TOTAL_CHARS
}

/// Default occurrence count for a freshly captured network/console item.
const fn default_occurrences() -> u32 {
    1
}

/// Serde skip predicate: omit `occurrences` from the wire when it is the
/// default of 1 (single occurrence).
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_one(value: &u32) -> bool {
    *value == 1
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

        let wait_selector = BrowserAction::WaitForSelector {
            selector: "#ready".to_string(),
            timeout_ms: 3_000,
        };
        let value = serde_json::to_value(wait_selector).expect("serialize wait_for_selector");
        assert_eq!(value["kind"], "wait_for_selector");
        assert_eq!(value["selector"], "#ready");
        assert_eq!(value["timeout_ms"], 3_000);

        let wait_text = BrowserAction::WaitForText {
            text: "Loaded".to_string(),
            timeout_ms: 5_000,
        };
        let value = serde_json::to_value(wait_text).expect("serialize wait_for_text");
        assert_eq!(value["kind"], "wait_for_text");
        assert_eq!(value["text"], "Loaded");
        assert_eq!(value["timeout_ms"], 5_000);

        let navigate = BrowserAction::Navigate {
            url: "https://example.com/login".to_string(),
            force_reload: false,
        };
        let value = serde_json::to_value(navigate).expect("serialize navigate");
        assert_eq!(value["kind"], "navigate");
        assert_eq!(value["url"], "https://example.com/login");
        assert_eq!(value["force_reload"], false);

        let event = BrowserStreamEvent::Heartbeat {
            session_id: "br_1".to_string(),
            timestamp: "2026-06-16T10:30:00Z".to_string(),
        };
        let value = serde_json::to_value(event).expect("serialize event");
        assert_eq!(value["type"], "heartbeat");
    }

    #[test]
    fn dom_extract_contract_serializes_structured_fields_and_defaults() {
        let request: DomExtractRequest = serde_json::from_value(json!({
            "selector": "[data-marker='item']",
            "fields": [
                {"name": "title", "selector": "[data-marker='item-title']"},
                {"name": "url", "selector": "a", "attribute": "href", "max_chars": 500}
            ]
        }))
        .expect("deserialize request");

        assert_eq!(request.max_results, DOM_EXTRACT_DEFAULT_MAX_RESULTS);
        assert_eq!(request.max_value_chars, DOM_EXTRACT_DEFAULT_MAX_VALUE_CHARS);
        assert_eq!(request.max_total_chars, DOM_EXTRACT_DEFAULT_MAX_TOTAL_CHARS);
        assert_eq!(request.fields.len(), 2);
        assert_eq!(request.fields[0].name, "title");
        assert_eq!(request.fields[1].attribute.as_deref(), Some("href"));

        let response = DomExtractResponse {
            request_id: "req-1".to_string(),
            session_id: "br_1".to_string(),
            ok: true,
            extraction: DomExtractPayload {
                selector: request.selector,
                total_matches: 1,
                returned_matches: 1,
                truncated: false,
                limits: DomExtractLimits {
                    max_results: 10,
                    max_fields: 20,
                    max_value_chars: 512,
                    max_total_chars: 16_000,
                },
                matches: vec![DomExtractMatch {
                    index: 0,
                    tag: "div".to_string(),
                    fields: BTreeMap::from([(
                        "title".to_string(),
                        DomExtractValue {
                            attribute: "innerText".to_string(),
                            source: DomExtractValueSource::Property,
                            found: true,
                            value: Some("ThinkPad".to_string()),
                            truncated: false,
                            original_chars: 8,
                        },
                    )]),
                }],
            },
            error: None,
        };
        let value = serde_json::to_value(response).expect("serialize response");
        assert_eq!(
            value["extraction"]["matches"][0]["fields"]["title"]["source"],
            "property"
        );
        assert_eq!(
            value["extraction"]["matches"][0]["fields"]["title"]["value"],
            "ThinkPad"
        );
    }

    #[test]
    fn browser_action_rejects_unknown_variant_fields() {
        let wait_with_alias = serde_json::from_value::<BrowserAction>(json!({
            "kind": "wait",
            "timeout_ms": 1_000,
            "ms": 1_000
        }));
        assert!(wait_with_alias.is_err());

        let fill_with_extra = serde_json::from_value::<BrowserAction>(json!({
            "kind": "fill",
            "selector": "#secret",
            "value": "hello",
            "unexpected": true
        }));
        assert!(fill_with_extra.is_err());
    }

    #[test]
    fn validate_action_fields_accepts_valid_fill() {
        let action = json!({"kind": "fill", "selector": "#email", "value": "hi"});
        assert!(validate_action_fields(&action).is_ok());
    }

    #[test]
    fn validate_action_fields_rejects_missing_value() {
        let action = json!({"kind": "fill", "selector": "#email"});
        let err = validate_action_fields(&action).expect_err("validation should fail");
        assert!(
            err.contains(r#"action "fill" is missing required field "value""#),
            "got: {err}"
        );
        assert!(err.contains(r#"Required fields for "fill": selector, value"#));
    }

    #[test]
    fn validate_action_fields_rejects_missing_selector() {
        let action = json!({"kind": "click_selector"});
        let err = validate_action_fields(&action).expect_err("validation should fail");
        assert!(err.contains(r#"action "click_selector" is missing required field "selector""#));
        assert!(err.contains(r#"Required fields for "click_selector": selector"#));
    }

    #[test]
    fn validate_action_fields_rejects_unknown_kind() {
        let action = json!({"kind": "type", "selector": "#q", "value": "hi"});
        let err = validate_action_fields(&action).expect_err("validation should fail");
        assert!(err.contains(r#"unknown action kind "type""#));
        assert!(err.contains("click_xy"));
        assert!(err.contains("navigate"));
    }

    #[test]
    fn validate_action_fields_rejects_missing_kind() {
        let action = json!({"selector": "#q", "value": "hi"});
        let err = validate_action_fields(&action).expect_err("validation should fail");
        assert!(err.contains(r#"action is missing "kind" field"#));
    }

    #[test]
    fn validate_action_fields_rejects_non_object() {
        let action = json!("not an object");
        let err = validate_action_fields(&action).expect_err("validation should fail");
        assert!(err.contains("action must be a JSON object"));
    }

    #[test]
    fn validate_action_fields_reports_multiple_missing_fields() {
        let action = json!({"kind": "wait_for_selector"});
        let err = validate_action_fields(&action).expect_err("validation should fail");
        assert!(err.contains(r#""selector""#));
        assert!(err.contains(r#""timeout_ms""#));
        assert!(err.contains(r#"Required fields for "wait_for_selector": selector, timeout_ms"#));
    }

    #[test]
    fn validate_action_fields_recurses_into_script_steps() {
        let action = json!({
            "kind": "script",
            "steps": [
                {"kind": "fill", "selector": "#q"}
            ]
        });
        let err = validate_action_fields(&action).expect_err("validation should fail");
        assert!(err.contains("script step 0:"), "got: {err}");
        assert!(err.contains(r#"action "fill" is missing required field "value""#));
    }

    #[test]
    fn validate_action_fields_accepts_valid_script() {
        let action = json!({
            "kind": "script",
            "steps": [
                {"kind": "click_selector", "selector": "#btn"},
                {"kind": "wait", "timeout_ms": 500}
            ]
        });
        assert!(validate_action_fields(&action).is_ok());
    }

    #[test]
    fn validate_action_fields_accepts_all_variants() {
        let cases = [
            json!({"kind": "click_xy", "x": 10, "y": 20}),
            json!({"kind": "click_selector", "selector": "#a"}),
            json!({"kind": "fill", "selector": "#a", "value": "v"}),
            json!({"kind": "type_text", "selector": "#a", "value": "v"}),
            json!({"kind": "press", "key": "Enter"}),
            json!({"kind": "scroll", "delta_x": 0, "delta_y": 100}),
            json!({"kind": "get_element_value", "selector": "#a"}),
            json!({"kind": "execute_javascript", "expression": "1+1"}),
            json!({"kind": "wait", "timeout_ms": 100}),
            json!({"kind": "wait_for_selector", "selector": "#a", "timeout_ms": 100}),
            json!({"kind": "wait_for_text", "text": "hi", "timeout_ms": 100}),
            json!({"kind": "navigate", "url": "https://example.com"}),
        ];
        for case in &cases {
            assert!(
                validate_action_fields(case).is_ok(),
                "expected ok for {case}"
            );
        }
    }
}
