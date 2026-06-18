use super::artifacts::{
    BrowserArtifactPurpose, BrowserArtifactSettings, build_browser_artifact_ref,
};
use super::types::{
    BrowserObservation, ConsoleSummary, DomSnapshotNode, LoadingState, NetworkSummary,
    ScreenshotArtifact, Viewport,
};
use crate::agent::tool_runtime::artifacts::ArtifactRef;
use chrono::{DateTime, Utc};
use std::collections::VecDeque;

const DEFAULT_RING_BUFFER_FRAMES: usize = 8;

/// One screenshot frame tracked outside model history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserFrame {
    /// Sidecar observation id.
    pub observation_id: String,
    /// Browser action sequence associated with this frame.
    pub action_seq: u64,
    /// Screenshot metadata without image bytes.
    pub screenshot: ScreenshotArtifact,
    /// Page URL captured with this frame.
    pub url: String,
    /// Page title captured with this frame.
    pub title: String,
    /// Page loading state captured with this frame.
    pub loading_state: LoadingState,
    /// Compact network failure summary for UI badges.
    pub network_summary: Option<NetworkSummary>,
    /// Compact console error/warning summary for UI badges.
    pub console_summary: Option<ConsoleSummary>,
    /// DOM snapshot of interactive elements (links, buttons, inputs, data-*).
    pub dom_snapshot: Vec<DomSnapshotNode>,
    /// Structured DOM snapshot capture failure, when the sidecar requested a snapshot.
    pub dom_snapshot_error: Option<super::types::SidecarErrorBody>,
    /// Internal artifact reference resolvable by UI/reporting layers.
    pub artifact: ArtifactRef,
    /// Whether this artifact must survive ring-buffer eviction.
    pub retained: bool,
}

/// Compact browser state kept task-local and outside durable LLM history.
#[derive(Debug, Clone)]
pub struct BrowserSessionState {
    task_id: String,
    session_id: String,
    viewport: Viewport,
    max_ring_frames: usize,
    artifact_settings: BrowserArtifactSettings,
    action_seq: u64,
    latest: Option<BrowserFrame>,
    ring: VecDeque<BrowserFrame>,
    retained_artifacts: Vec<ArtifactRef>,
    live_bytes: u64,
}

impl BrowserSessionState {
    /// Create task-local browser state with default ring-buffer size.
    #[must_use]
    pub fn new(
        task_id: impl Into<String>,
        session_id: impl Into<String>,
        viewport: Viewport,
        artifact_settings: BrowserArtifactSettings,
    ) -> Self {
        Self::with_ring_buffer_frames(
            task_id,
            session_id,
            viewport,
            artifact_settings,
            DEFAULT_RING_BUFFER_FRAMES,
        )
    }

    /// Create task-local browser state with an explicit ring-buffer size.
    #[must_use]
    pub fn with_ring_buffer_frames(
        task_id: impl Into<String>,
        session_id: impl Into<String>,
        viewport: Viewport,
        artifact_settings: BrowserArtifactSettings,
        max_ring_frames: usize,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            session_id: session_id.into(),
            viewport,
            max_ring_frames: max_ring_frames.max(1),
            artifact_settings,
            action_seq: 0,
            latest: None,
            ring: VecDeque::new(),
            retained_artifacts: Vec::new(),
            live_bytes: 0,
        }
    }

    /// Current action sequence.
    #[must_use]
    pub const fn action_seq(&self) -> u64 {
        self.action_seq
    }

    /// Latest screenshot frame, if any.
    #[must_use]
    pub const fn latest(&self) -> Option<&BrowserFrame> {
        self.latest.as_ref()
    }

    /// Current ring-buffer frames.
    pub fn ring_frames(&self) -> impl Iterator<Item = &BrowserFrame> {
        self.ring.iter()
    }

    /// Retained final/milestone/debug artifacts.
    #[must_use]
    pub fn retained_artifacts(&self) -> &[ArtifactRef] {
        &self.retained_artifacts
    }

    /// Current live frame bytes tracked by the ring-buffer.
    #[must_use]
    pub const fn live_bytes(&self) -> u64 {
        self.live_bytes
    }

    /// Record a sidecar observation as an artifact-backed frame.
    ///
    /// # Errors
    /// Returns a validation error when sidecar screenshot metadata is unsafe or
    /// inconsistent with the session viewport.
    pub fn record_observation(
        &mut self,
        observation: &BrowserObservation,
        purpose: BrowserArtifactPurpose,
        bytes: u64,
    ) -> Result<&BrowserFrame, BrowserStateError> {
        self.validate_observation(observation)?;
        self.action_seq = self.action_seq.max(observation.action_seq);
        let artifact = build_browser_artifact_ref(
            &self.artifact_settings,
            &self.task_id,
            &self.session_id,
            observation.action_seq,
            purpose,
            bytes,
            Some(observation.screenshot.sha256.clone()),
            parse_timestamp(observation.screenshot.captured_at.as_deref()),
        );
        let frame = BrowserFrame {
            observation_id: observation.observation_id.clone(),
            action_seq: observation.action_seq,
            screenshot: observation.screenshot.clone(),
            url: observation.url.clone(),
            title: observation.title.clone(),
            loading_state: observation.loading_state,
            network_summary: observation.network_summary.clone(),
            console_summary: observation.console_summary.clone(),
            dom_snapshot: observation.dom_snapshot.clone(),
            dom_snapshot_error: observation.dom_snapshot_error.clone(),
            retained: purpose.is_retained(),
            artifact,
        };

        if frame.retained {
            if let Some(pos) = self
                .retained_artifacts
                .iter()
                .position(|artifact| artifact.uri == frame.artifact.uri)
            {
                self.retained_artifacts[pos] = frame.artifact.clone();
            } else {
                self.retained_artifacts.push(frame.artifact.clone());
            }
        } else {
            self.live_bytes = self.live_bytes.saturating_add(frame.artifact.bytes);
        }
        self.latest = Some(frame.clone());
        self.ring.push_back(frame);
        self.evict_ring();
        self.latest.as_ref().ok_or(BrowserStateError::EmptyState)
    }

    /// Update the latest frame's screenshot byte size and hash after the
    /// corresponding artifact file has been written to disk.
    pub fn update_latest_artifact_bytes(
        &mut self,
        bytes: &[u8],
        sha256: String,
    ) -> Result<(), BrowserStateError> {
        let byte_size = bytes.len() as u64;
        let latest = self.latest.as_mut().ok_or(BrowserStateError::EmptyState)?;
        latest.screenshot.byte_size = byte_size;
        latest.screenshot.sha256 = sha256.clone();
        latest.artifact.bytes = byte_size;
        latest.artifact.sha256 = Some(sha256.clone());
        if let Some(frame) = self
            .ring
            .iter_mut()
            .find(|frame| frame.observation_id == latest.observation_id)
        {
            frame.screenshot.byte_size = byte_size;
            frame.screenshot.sha256 = sha256.clone();
            frame.artifact.bytes = byte_size;
            frame.artifact.sha256 = Some(sha256.clone());
        }
        if let Some(pos) = self
            .retained_artifacts
            .iter()
            .position(|artifact| artifact.uri == latest.artifact.uri)
        {
            let retained = &mut self.retained_artifacts[pos];
            retained.bytes = byte_size;
            retained.sha256 = Some(sha256);
        }
        Ok(())
    }

    /// Emit a compact summary safe for durable text history.
    #[must_use]
    pub fn compact_history_summary(&self) -> String {
        let Some(latest) = &self.latest else {
            return format!(
                "browser_session session_id={} state=empty ring_frames=0",
                self.session_id
            );
        };
        format!(
            "browser_session session_id={} action_seq={} url_screenshot_ref={} latest_screenshot_id={} viewport={}x{} dsf={} ring_frames={} retained_artifacts={}",
            self.session_id,
            latest.action_seq,
            latest.artifact.uri,
            latest.screenshot.screenshot_id,
            latest.screenshot.width,
            latest.screenshot.height,
            self.viewport.device_scale_factor,
            self.ring.len(),
            self.retained_artifacts.len()
        )
    }

    fn validate_observation(
        &self,
        observation: &BrowserObservation,
    ) -> Result<(), BrowserStateError> {
        let screenshot = &observation.screenshot;
        if screenshot.artifact_uri.starts_with("data:")
            || screenshot.artifact_uri.contains("base64")
            || screenshot.mime_type.contains("base64")
        {
            return Err(BrowserStateError::ImageBytesInMetadata);
        }
        if screenshot.width != self.viewport.width || screenshot.height != self.viewport.height {
            return Err(BrowserStateError::ViewportMismatch {
                expected_width: self.viewport.width,
                expected_height: self.viewport.height,
                actual_width: screenshot.width,
                actual_height: screenshot.height,
            });
        }
        if screenshot.sha256.trim().is_empty() {
            return Err(BrowserStateError::MissingHash);
        }
        Ok(())
    }

    fn evict_ring(&mut self) {
        while self.ring.len() > self.max_ring_frames {
            if let Some(evicted) = self.ring.pop_front()
                && !evicted.retained
            {
                self.live_bytes = self.live_bytes.saturating_sub(evicted.artifact.bytes);
            }
        }

        while self.live_bytes > self.artifact_settings.max_total_bytes {
            let Some(position) = self.ring.iter().position(|frame| !frame.retained) else {
                break;
            };
            if let Some(evicted) = self.ring.remove(position) {
                self.live_bytes = self.live_bytes.saturating_sub(evicted.artifact.bytes);
            }
        }
    }
}

/// Browser session state validation errors.
#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum BrowserStateError {
    /// Screenshot metadata attempted to carry raw image bytes.
    #[error("browser screenshot metadata must not contain image bytes")]
    ImageBytesInMetadata,
    /// Screenshot dimensions do not match the session viewport.
    #[error(
        "browser screenshot viewport mismatch: expected {expected_width}x{expected_height}, got {actual_width}x{actual_height}"
    )]
    ViewportMismatch {
        /// Expected width.
        expected_width: u32,
        /// Expected height.
        expected_height: u32,
        /// Actual width.
        actual_width: u32,
        /// Actual height.
        actual_height: u32,
    },
    /// Screenshot hash is required for artifact integrity.
    #[error("browser screenshot metadata is missing sha256")]
    MissingHash,
    /// Internal invariant: state has no latest frame after record.
    #[error("browser session state has no latest frame")]
    EmptyState,
}

fn parse_timestamp(value: Option<&str>) -> Option<DateTime<Utc>> {
    value.and_then(|value| value.parse::<DateTime<Utc>>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::providers::browser_live::types::LoadingState;

    #[test]
    fn ring_buffer_evicts_old_live_frames_without_deleting_retained_artifacts() {
        let mut state = test_state(3, 10_000);
        for seq in 1..=3 {
            state
                .record_observation(&observation(seq), BrowserArtifactPurpose::LiveFrame, 100)
                .expect("record live frame");
        }
        state
            .record_observation(&observation(4), BrowserArtifactPurpose::Final, 100)
            .expect("record final frame");

        let frame_ids = state
            .ring_frames()
            .map(|frame| frame.screenshot.screenshot_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(frame_ids, ["shot-2", "shot-3", "shot-4"]);
        assert_eq!(state.retained_artifacts().len(), 1);
        assert_eq!(
            state.retained_artifacts()[0].uri,
            "artifact://browser/task-1/session-1/step-0004-final.jpg"
        );
        assert_eq!(state.live_bytes(), 200);
    }

    #[test]
    fn artifact_size_cap_evicts_old_live_frames() {
        let mut state = test_state(8, 250);
        for seq in 1..=4 {
            state
                .record_observation(&observation(seq), BrowserArtifactPurpose::LiveFrame, 100)
                .expect("record live frame");
        }

        let frame_ids = state
            .ring_frames()
            .map(|frame| frame.screenshot.screenshot_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(frame_ids, ["shot-3", "shot-4"]);
        assert_eq!(state.live_bytes(), 200);
    }

    #[test]
    fn validates_screenshot_metadata_and_viewport() {
        let mut state = test_state(3, 10_000);
        let mut with_data_url = observation(1);
        with_data_url.screenshot.artifact_uri = "data:image/jpeg;base64,abc".to_string();
        assert_eq!(
            state
                .record_observation(&with_data_url, BrowserArtifactPurpose::LiveFrame, 10)
                .expect_err("data urls rejected"),
            BrowserStateError::ImageBytesInMetadata
        );

        let mut wrong_viewport = observation(2);
        wrong_viewport.screenshot.width = 1;
        assert!(matches!(
            state.record_observation(&wrong_viewport, BrowserArtifactPurpose::LiveFrame, 10),
            Err(BrowserStateError::ViewportMismatch { .. })
        ));

        let mut missing_hash = observation(3);
        missing_hash.screenshot.sha256.clear();
        assert_eq!(
            state
                .record_observation(&missing_hash, BrowserArtifactPurpose::LiveFrame, 10)
                .expect_err("hash required"),
            BrowserStateError::MissingHash
        );
    }

    #[test]
    fn compact_history_summary_contains_refs_not_image_bytes() {
        let mut state = test_state(3, 10_000);
        state
            .record_observation(&observation(1), BrowserArtifactPurpose::Milestone, 100)
            .expect("record milestone");

        let summary = state.compact_history_summary();
        assert!(summary.contains("artifact://browser/task-1/session-1/step-0001-milestone.jpg"));
        assert!(summary.contains("latest_screenshot_id=shot-1"));
        assert!(!summary.contains("base64"));
        assert!(!summary.contains("data:image"));
    }

    #[test]
    fn retained_artifacts_deduplicate_by_uri() {
        let mut state = test_state(3, 10_000);
        let mut first = observation(1);
        first.screenshot.sha256 = "sha-first".to_string();
        state
            .record_observation(&first, BrowserArtifactPurpose::Milestone, 100)
            .expect("record first milestone");

        let mut second = observation(1);
        second.screenshot.sha256 = "sha-second".to_string();
        state
            .record_observation(&second, BrowserArtifactPurpose::Milestone, 150)
            .expect("record second milestone with same URI");

        assert_eq!(state.retained_artifacts().len(), 1);
        assert_eq!(
            state.retained_artifacts()[0].uri,
            "artifact://browser/task-1/session-1/step-0001-milestone.jpg"
        );
        assert_eq!(
            state.retained_artifacts()[0].sha256.as_deref(),
            Some("sha-second")
        );
        assert_eq!(state.retained_artifacts()[0].bytes, 150);
    }

    #[test]
    fn update_latest_artifact_bytes_also_updates_retained_artifact() {
        let mut state = test_state(3, 10_000);
        state
            .record_observation(&observation(1), BrowserArtifactPurpose::Milestone, 0)
            .expect("record milestone");

        state
            .update_latest_artifact_bytes(b"new bytes", "sha-updated".to_string())
            .expect("update latest bytes");

        assert_eq!(state.retained_artifacts().len(), 1);
        assert_eq!(
            state.retained_artifacts()[0].sha256.as_deref(),
            Some("sha-updated")
        );
        assert_eq!(state.retained_artifacts()[0].bytes, 9);
    }

    fn test_state(max_ring_frames: usize, max_total_bytes: u64) -> BrowserSessionState {
        BrowserSessionState::with_ring_buffer_frames(
            "task-1",
            "session-1",
            Viewport::default(),
            BrowserArtifactSettings {
                max_total_bytes,
                ..BrowserArtifactSettings::default()
            },
            max_ring_frames,
        )
    }

    fn observation(seq: u64) -> BrowserObservation {
        BrowserObservation {
            observation_id: format!("obs-{seq}"),
            action_seq: seq,
            captured_at: "2026-06-16T00:00:00Z".to_string(),
            url: "https://example.test".to_string(),
            title: "Example".to_string(),
            viewport: Viewport::default(),
            loading_state: LoadingState::Idle,
            screenshot: ScreenshotArtifact {
                screenshot_id: format!("shot-{seq}"),
                artifact_uri: format!("browser/task/session/shot-{seq}.jpg"),
                mime_type: "image/jpeg".to_string(),
                width: Viewport::default().width,
                height: Viewport::default().height,
                sha256: format!("sha-{seq}"),
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
