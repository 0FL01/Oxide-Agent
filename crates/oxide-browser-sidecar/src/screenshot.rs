//! Screenshot capture and artifact management.
//!
//! Uses `Page.captureScreenshot` directly via CDP (no chrome-agent pipe).
//! Screenshots are captured as JPEG (quality 80) for ~4-10x smaller size vs
//! PNG on photographic content. SHA-256 is computed on the captured bytes.
//! On failure, a 1×1 white JPEG fallback is used.

use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use oxide_browser_contracts::{ScreenshotArtifact, Viewport};
use sha2::{Digest, Sha256};

use crate::cdp::CdpClient;

/// CDP timeout for screenshot capture.
const SCREENSHOT_TIMEOUT: Duration = Duration::from_secs(30);

/// JPEG quality for CDP capture (0-100). 80 balances size and visual quality.
const JPEG_QUALITY: u64 = 80;

/// 1×1 white JPEG used as fallback when screenshot capture fails.
/// Generated via ImageMagick `convert -size 1x1 xc:white`.
const ONE_PIXEL_JPEG: &[u8] = &[
    0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, // SOI + APP0 JFIF
    0x49, 0x46, 0x00, 0x01, 0x01, 0x00, 0x00, 0x01, // JFIF data
    0x00, 0x01, 0x00, 0x00, 0xff, 0xdb, 0x00, 0x43, // DQT marker
    0x00, 0x03, 0x02, 0x02, 0x02, 0x02, 0x02, 0x03, // Quantization table
    0x02, 0x02, 0x02, 0x03, 0x03, 0x03, 0x03, 0x04, 0x06, 0x04, 0x04, 0x04, 0x04, 0x04, 0x08, 0x06,
    0x06, 0x05, 0x06, 0x09, 0x08, 0x0a, 0x0a, 0x09, 0x08, 0x09, 0x09, 0x0a, 0x0c, 0x0f, 0x0c, 0x0a,
    0x0b, 0x0e, 0x0b, 0x09, 0x09, 0x0d, 0x11, 0x0d, 0x0e, 0x0f, 0x10, 0x10, 0x11, 0x10, 0x0a, 0x0c,
    0x12, 0x13, 0x12, 0x10, 0x13, 0x0f, 0x10, 0x10, 0x10, 0xff, 0xc0, 0x00, 0x0b, 0x08, 0x00,
    0x01, // SOF0: 8-bit, 1x1
    0x00, 0x01, 0x01, 0x01, 0x00, 0xff, 0xc4, 0x00, // DHT DC
    0x14, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x09, 0xff, 0xc4, 0x00, 0x14, 0x10, // DHT AC
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0xff, 0xda, 0x00, 0x08, 0x01, 0x01, 0x00, // SOS
    0x00, 0x3f, 0x00, 0x54, 0xdf, 0xff, 0xd9, // Compressed data + EOI
];

/// Capture a screenshot via `Page.captureScreenshot` and save to disk.
///
/// Returns a `ScreenshotArtifact` with metadata. On failure, saves the
/// 1×1 fallback JPEG and returns `redacted: true`.
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
            serde_json::json!({"format": "jpeg", "quality": JPEG_QUALITY}),
            SCREENSHOT_TIMEOUT,
        )
        .await;

    let (jpeg_bytes, redacted) = match result {
        Ok(resp) => {
            let data = resp
                .get("data")
                .and_then(|v| v.as_str())
                .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok());
            match data {
                Some(bytes) if !bytes.is_empty() => (bytes, false),
                _ => (ONE_PIXEL_JPEG.to_vec(), true),
            }
        }
        Err(_) => (ONE_PIXEL_JPEG.to_vec(), true),
    };

    // Save to disk: {artifact_dir}/latest.jpg
    let dest = artifact_dir.join("latest.jpg");
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&dest, &jpeg_bytes);

    let sha256 = sha256_of_bytes(&jpeg_bytes);
    let byte_size = jpeg_bytes.len() as u64;

    // URI: {artifact_root}latest.jpg
    let artifact_uri = format!("{artifact_root}latest.jpg");

    ScreenshotArtifact {
        screenshot_id: screenshot_id.to_string(),
        artifact_uri,
        mime_type: "image/jpeg".to_string(),
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
/// Returns `ONE_PIXEL_JPEG` if the file does not exist.
pub fn read_latest_screenshot(artifact_dir: &Path) -> Vec<u8> {
    let path = artifact_dir.join("latest.jpg");
    match std::fs::read(&path) {
        Ok(data) if !data.is_empty() => data,
        _ => ONE_PIXEL_JPEG.to_vec(),
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
    fn one_pixel_jpeg_is_valid_jpeg() {
        // JPEG SOI marker: FF D8 FF
        assert_eq!(&ONE_PIXEL_JPEG[..3], &[0xff, 0xd8, 0xff]);
        // JPEG EOI marker: FF D9 (last 2 bytes)
        let len = ONE_PIXEL_JPEG.len();
        assert_eq!(&ONE_PIXEL_JPEG[len - 2..], &[0xff, 0xd9]);
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
