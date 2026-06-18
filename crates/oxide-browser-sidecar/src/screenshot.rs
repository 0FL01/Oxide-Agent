//! Screenshot capture and artifact management.
//!
//! Uses `Page.captureScreenshot` directly via CDP (no chrome-agent pipe).
//! Screenshots are saved to disk as `latest.png` in the session's artifact
//! directory. SHA-256 is computed on the saved file. On failure, a 1×1
//! transparent PNG fallback is used.

use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use oxide_browser_contracts::{ScreenshotArtifact, Viewport};
use sha2::{Digest, Sha256};

use crate::cdp::CdpClient;

/// CDP timeout for screenshot capture.
const SCREENSHOT_TIMEOUT: Duration = Duration::from_secs(30);

/// 1×1 transparent PNG used as fallback when screenshot capture fails.
const ONE_PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
    0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR chunk
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1x1
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, // RGBA, CRC
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, // IDAT chunk
    0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, // deflate data
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, // CRC
    0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, // IEND chunk
    0x42, 0x60, 0x82,
];

/// Capture a screenshot via `Page.captureScreenshot` and save to disk.
///
/// Returns a `ScreenshotArtifact` with metadata. On failure, saves the
/// 1×1 fallback PNG and returns `redacted: true`.
pub async fn capture_screenshot(
    cdp: &CdpClient,
    viewport: Viewport,
    artifact_dir: &Path,
    artifact_root: &str,
    screenshot_id: &str,
) -> ScreenshotArtifact {
    let result = cdp
        .send_command(
            "Page.captureScreenshot",
            serde_json::json!({"format": "png"}),
            SCREENSHOT_TIMEOUT,
        )
        .await;

    let (png_bytes, redacted) = match result {
        Ok(resp) => {
            let data = resp
                .get("data")
                .and_then(|v| v.as_str())
                .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok());
            match data {
                Some(bytes) if !bytes.is_empty() => (bytes, false),
                _ => (ONE_PIXEL_PNG.to_vec(), true),
            }
        }
        Err(_) => (ONE_PIXEL_PNG.to_vec(), true),
    };

    // Save to disk: {artifact_dir}/latest.png
    let dest = artifact_dir.join("latest.png");
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&dest, &png_bytes);

    let sha256 = sha256_of_bytes(&png_bytes);
    let byte_size = png_bytes.len() as u64;

    // URI: {artifact_root}latest.png
    let artifact_uri = format!("{artifact_root}latest.png");

    ScreenshotArtifact {
        screenshot_id: screenshot_id.to_string(),
        artifact_uri,
        mime_type: "image/png".to_string(),
        width: viewport.width,
        height: viewport.height,
        sha256,
        captured_at: Some(crate::capture::now_iso()),
        redacted,
        byte_size,
    }
}

/// Read the raw bytes of the latest screenshot from disk.
///
/// Returns `ONE_PIXEL_PNG` if the file does not exist.
pub fn read_latest_screenshot(artifact_dir: &Path) -> Vec<u8> {
    let path = artifact_dir.join("latest.png");
    match std::fs::read(&path) {
        Ok(data) if !data.is_empty() => data,
        _ => ONE_PIXEL_PNG.to_vec(),
    }
}

/// Compute the SHA-256 hex digest of a byte slice.
fn sha256_of_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Resolve the session artifact directory on disk.
///
/// Reads `BROWSER_AGENT_ARTIFACT_DIR` env var (default
/// `/var/lib/oxide-browser/artifacts`), then appends `safe(task_id)/safe(session_id)`.
pub fn session_artifact_dir(task_id: &str, session_id: &str) -> PathBuf {
    let root = std::env::var("BROWSER_AGENT_ARTIFACT_DIR")
        .unwrap_or_else(|_| "/var/lib/oxide-browser/artifacts".to_string());
    resolve_artifact_dir(&root, task_id, session_id)
}

/// Build the artifact directory path from a root, task_id, and session_id.
fn resolve_artifact_dir(root: &str, task_id: &str, session_id: &str) -> PathBuf {
    let dir = PathBuf::from(root)
        .join(crate::session::safe(task_id))
        .join(crate::session::safe(session_id));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_pixel_png_is_valid_png() {
        assert_eq!(
            &ONE_PIXEL_PNG[..8],
            &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]
        );
    }

    #[test]
    fn sha256_of_empty_is_known() {
        let hash = sha256_of_bytes(&[]);
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_of_known_data() {
        let hash = sha256_of_bytes(b"hello");
        assert_eq!(
            hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn resolve_artifact_dir_builds_correct_path() {
        let dir = resolve_artifact_dir("/tmp/test-artifacts", "task-1", "br-abc");
        assert!(dir.starts_with("/tmp/test-artifacts"));
        assert!(dir.to_string_lossy().contains("task-1"));
        assert!(dir.to_string_lossy().contains("br-abc"));
        // Cleanup
        let _ = std::fs::remove_dir_all("/tmp/test-artifacts");
    }
}
