//! Screenshot capture via CDP `Page.captureScreenshot`.
//!
//! Screenshots are captured as JPEG (quality 80) for ~4-10x smaller size vs
//! PNG on photographic content. The bytes are returned in-memory — no disk
//! I/O. The caller is responsible for persisting them (Postgres BYTEA via
//! the core provider). On failure, a 1×1 white JPEG fallback is used.

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
pub const ONE_PIXEL_JPEG: &[u8] = &[
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

/// Capture a screenshot via `Page.captureScreenshot` and return bytes in-memory.
///
/// Returns `(ScreenshotArtifact, Vec<u8>)` — metadata + raw JPEG bytes.
/// No disk I/O. On failure, returns the 1×1 fallback JPEG and `redacted: true`.
pub async fn capture_screenshot(
    cdp: &CdpClient,
    viewport: Viewport,
    artifact_root: &str,
    screenshot_id: &str,
) -> (ScreenshotArtifact, Vec<u8>) {
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

    let sha256 = sha256_of_bytes(&jpeg_bytes);
    let byte_size = jpeg_bytes.len() as u64;

    // URI: {artifact_root}latest.jpg
    let artifact_uri = format!("{artifact_root}latest.jpg");

    let artifact = ScreenshotArtifact {
        screenshot_id: screenshot_id.to_string(),
        artifact_uri,
        mime_type: "image/jpeg".to_string(),
        width: viewport.width,
        height: viewport.height,
        sha256,
        captured_at: Some(crate::capture::now_iso()),
        redacted,
        byte_size,
    };

    (artifact, jpeg_bytes)
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
}
