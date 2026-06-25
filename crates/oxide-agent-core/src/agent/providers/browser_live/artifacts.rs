use crate::agent::tool_runtime::artifacts::{ArtifactKind, ArtifactRef};
use crate::agent::tool_runtime::config::ToolRuntimeConfig;
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Browser artifact retention and storage settings.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BrowserArtifactSettings {
    /// Root directory shared with the tool runtime artifact store.
    pub root_dir: PathBuf,
    /// Best-effort retention for non-final browser artifacts.
    pub retention: Duration,
    /// Soft cap for browser artifact bytes kept in task-local state.
    pub max_total_bytes: u64,
}

impl BrowserArtifactSettings {
    /// Build browser artifact settings from the existing tool runtime config.
    #[must_use]
    pub fn from_tool_runtime(config: &ToolRuntimeConfig) -> Self {
        Self {
            root_dir: config.artifact_dir.clone(),
            retention: config.artifact_retention,
            max_total_bytes: config.storage_soft_cap_bytes.unwrap_or(1_073_741_824),
        }
    }
}

impl Default for BrowserArtifactSettings {
    fn default() -> Self {
        Self::from_tool_runtime(&ToolRuntimeConfig::default())
    }
}

/// Browser artifact purpose. Final and milestone artifacts are retained even
/// when live-frame ring-buffer entries are evicted.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum BrowserArtifactPurpose {
    /// Non-final live frame used for recovery/verification.
    LiveFrame,
    /// User-visible milestone frame.
    Milestone,
    /// Final evidence frame.
    Final,
    /// Structured observe debug payload.
    ObserveJson,
    /// Structured network debug payload.
    NetworkJson,
    /// Structured console debug payload.
    ConsoleJson,
}

impl BrowserArtifactPurpose {
    /// Whether artifact must be retained independently of ring-buffer eviction.
    #[must_use]
    pub const fn is_retained(self) -> bool {
        matches!(
            self,
            Self::Milestone
                | Self::Final
                | Self::ObserveJson
                | Self::NetworkJson
                | Self::ConsoleJson
        )
    }

    const fn stem(self) -> &'static str {
        match self {
            Self::LiveFrame => "live",
            Self::Milestone => "milestone",
            Self::Final => "final",
            Self::ObserveJson => "observe",
            Self::NetworkJson => "network",
            Self::ConsoleJson => "console",
        }
    }

    const fn extension(self) -> &'static str {
        match self {
            Self::LiveFrame | Self::Milestone | Self::Final => "jpg",
            Self::ObserveJson | Self::NetworkJson | Self::ConsoleJson => "json",
        }
    }
}

/// Build a stable browser artifact reference under the tool artifact root.
#[must_use]
pub fn build_browser_artifact_ref(
    settings: &BrowserArtifactSettings,
    task_id: &str,
    session_id: &str,
    action_seq: u64,
    purpose: BrowserArtifactPurpose,
    bytes: u64,
    sha256: Option<String>,
    captured_at: Option<DateTime<Utc>>,
) -> ArtifactRef {
    let uri = browser_artifact_uri(task_id, session_id, action_seq, purpose);
    let local_path = settings
        .root_dir
        .join(uri.strip_prefix("artifact://").unwrap_or(&uri));
    let mut artifact = ArtifactRef::internal(uri, local_path, ArtifactKind::File, bytes);
    artifact.sha256 = sha256;
    if !purpose.is_retained() {
        let captured_at = captured_at.unwrap_or_else(Utc::now);
        artifact.expires_at = chrono::Duration::from_std(settings.retention)
            .ok()
            .map(|retention| captured_at + retention);
    }
    artifact
}

/// Build a browser artifact URI suitable for Web UI/Telegram references.
#[must_use]
pub fn browser_artifact_uri(
    task_id: &str,
    session_id: &str,
    action_seq: u64,
    purpose: BrowserArtifactPurpose,
) -> String {
    format!(
        "artifact://browser/{}/{}/step-{action_seq:04}-{}.{}",
        safe_segment(task_id),
        safe_segment(session_id),
        purpose.stem(),
        purpose.extension()
    )
}

fn safe_segment(value: &str) -> String {
    let sanitized: String = value
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

/// Returns true when `path` is inside the configured artifact root.
#[must_use]
pub fn path_is_under_root(root: &Path, path: &Path) -> bool {
    path.starts_with(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_artifact_names_are_stable_and_sanitized() {
        let settings = BrowserArtifactSettings::default();
        let artifact = build_browser_artifact_ref(
            &settings,
            "task/1",
            "session#1",
            8,
            BrowserArtifactPurpose::Milestone,
            123,
            Some("abc".to_string()),
            None,
        );

        assert_eq!(
            artifact.uri,
            "artifact://browser/task-1/session-1/step-0008-milestone.jpg"
        );
        assert_eq!(artifact.sha256.as_deref(), Some("abc"));
        assert!(path_is_under_root(&settings.root_dir, &artifact.local_path));
        assert!(artifact.expires_at.is_none());
    }

    #[test]
    fn live_frame_artifact_gets_retention_expiry() {
        let settings = BrowserArtifactSettings {
            retention: Duration::from_secs(60),
            ..BrowserArtifactSettings::default()
        };
        let captured_at = "2026-06-16T00:00:00Z"
            .parse::<DateTime<Utc>>()
            .expect("timestamp");
        let artifact = build_browser_artifact_ref(
            &settings,
            "task",
            "session",
            1,
            BrowserArtifactPurpose::LiveFrame,
            10,
            None,
            Some(captured_at),
        );

        assert_eq!(
            artifact.expires_at.expect("expiry").to_rfc3339(),
            "2026-06-16T00:01:00+00:00"
        );
    }
}
