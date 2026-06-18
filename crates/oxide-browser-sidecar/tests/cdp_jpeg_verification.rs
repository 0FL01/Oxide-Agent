//! CP0 P0.5 verification: CDP `Page.captureScreenshot` JPEG format support,
//! size comparison vs PNG, and JPEG magic-byte validation on real Chromium.
//!
//! Run with:
//!
//! ```sh
//! cargo test -p oxide-browser-sidecar --test cdp_jpeg_verification -- --ignored --nocapture
//! ```

use std::time::Duration;

use base64::Engine;
use oxide_browser_contracts::Viewport;
use oxide_browser_sidecar::browser::ChromiumProcess;
use oxide_browser_sidecar::cdp::CdpClient;

const SCREENSHOT_TIMEOUT: Duration = Duration::from_secs(30);

const PAGE_HTML: &str = "data:text/html,\
<html><head><title>JPEG vs PNG</title>\
<style>body{font-family:sans-serif;background:linear-gradient(135deg,#1a1a2e,#16213e,#0f3460);\
color:#e94560;margin:0;padding:20px}\
h1{font-size:48px;text-shadow:2px 2px 4px rgba(0,0,0,0.5)}\
p{font-size:18px;line-height:1.6;max-width:800px}\
.grid{display:grid;grid-template-columns:repeat(3,1fr);gap:10px;margin:20px 0}\
.box{height:100px;border-radius:8px;display:flex;align-items:center;justify-content:center;\
color:white;font-weight:bold}\
.box:nth-child(1){background:#e94560}.box:nth-child(2){background:#0f3460}\
.box:nth-child(3){background:#16213e}.box:nth-child(4){background:#533483}\
.box:nth-child(5){background:#e94560}.box:nth-child(6){background:#0f3460}\
</style></head><body>\
<h1>JPEG vs PNG Quality Test</h1>\
<p>This page has gradients, text, colors, and layout complexity to produce a \
realistic screenshot size comparison between PNG and JPEG encoding via CDP.</p>\
<div class=\"grid\">\
<div class=\"box\">Box 1</div><div class=\"box\">Box 2</div><div class=\"box\">Box 3</div>\
<div class=\"box\">Box 4</div><div class=\"box\">Box 5</div><div class=\"box\">Box 6</div>\
</div>\
<p>Paragraph with lots of text to fill the viewport. Lorem ipsum dolor sit amet, \
consectetur adipiscing elit. Sed do eiusmod tempor incididunt ut labore et dolore \
magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris \
nisi ut aliquip ex ea commodo consequat.</p>\
</body></html>";

/// JPEG magic bytes: SOI marker `\xff\xd8` followed by a JFIF/EXIF marker `\xff\xe0` or `\xff\xe1`.
const JPEG_MAGIC: &[u8] = &[0xff, 0xd8, 0xff];

#[tokio::test]
#[ignore = "requires Chromium binary"]
async fn cdp_jpeg_screenshot_verification() {
    let viewport = Viewport::default(); // 1365x768 @ 1x
    let (mut chromium, cdp) = ChromiumProcess::launch(&viewport)
        .await
        .expect("failed to launch Chromium");

    // Enable Page domain.
    cdp.send_command("Page.enable", serde_json::json!({}), SCREENSHOT_TIMEOUT)
        .await
        .expect("Page.enable failed");

    // Navigate to a real-world page with photographic content, complex layout,
    // gradients, and text — the kind of pages the browser agent actually screenshots.
    // Solid-color pages compress better as PNG; real pages with photos/noise
    // compress 4-10x better as JPEG.
    let nav_result = cdp
        .send_command(
            "Page.navigate",
            serde_json::json!({"url": "https://en.wikipedia.org/wiki/Chromium_(web_browser)"}),
            SCREENSHOT_TIMEOUT,
        )
        .await
        .expect("Page.navigate failed");
    println!("Navigate result: {nav_result}");

    // Wait for the page to fully render (real network + images).
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    // --- Capture PNG ---
    let png_result = cdp
        .send_command(
            "Page.captureScreenshot",
            serde_json::json!({"format": "png"}),
            SCREENSHOT_TIMEOUT,
        )
        .await
        .expect("PNG captureScreenshot failed");

    let png_b64 = png_result
        .get("data")
        .and_then(|v| v.as_str())
        .expect("PNG response missing data field");
    let png_bytes = base64::engine::general_purpose::STANDARD
        .decode(png_b64)
        .expect("PNG base64 decode failed");
    let png_size = png_bytes.len();

    // Verify PNG magic bytes (\x89PNG).
    assert_eq!(
        &png_bytes[0..4],
        &[0x89, 0x50, 0x4e, 0x47],
        "PNG magic bytes mismatch"
    );

    // --- Capture JPEG q=80 ---
    let jpeg_result = cdp
        .send_command(
            "Page.captureScreenshot",
            serde_json::json!({"format": "jpeg", "quality": 80}),
            SCREENSHOT_TIMEOUT,
        )
        .await
        .expect("JPEG captureScreenshot failed");

    let jpeg_b64 = jpeg_result
        .get("data")
        .and_then(|v| v.as_str())
        .expect("JPEG response missing data field");
    let jpeg_bytes = base64::engine::general_purpose::STANDARD
        .decode(jpeg_b64)
        .expect("JPEG base64 decode failed");
    let jpeg_size = jpeg_bytes.len();

    // Verify JPEG magic bytes (\xff\xd8\xff).
    assert_eq!(
        &jpeg_bytes[0..3],
        JPEG_MAGIC,
        "JPEG magic bytes mismatch — not a valid JPEG"
    );

    // --- Capture JPEG q=90 (for comparison) ---
    let jpeg90_result = cdp
        .send_command(
            "Page.captureScreenshot",
            serde_json::json!({"format": "jpeg", "quality": 90}),
            SCREENSHOT_TIMEOUT,
        )
        .await
        .expect("JPEG q90 captureScreenshot failed");

    let jpeg90_b64 = jpeg90_result
        .get("data")
        .and_then(|v| v.as_str())
        .expect("JPEG q90 response missing data field");
    let jpeg90_bytes = base64::engine::general_purpose::STANDARD
        .decode(jpeg90_b64)
        .expect("JPEG q90 base64 decode failed");
    let jpeg90_size = jpeg90_bytes.len();

    assert_eq!(
        &jpeg90_bytes[0..3],
        JPEG_MAGIC,
        "JPEG q90 magic bytes mismatch"
    );

    // --- Report ---
    let ratio = (jpeg_size as f64 / png_size as f64) * 100.0;
    println!("╔══════════════════════════════════════════════════╗");
    println!("║  CP0: CDP JPEG vs PNG Screenshot Verification    ║");
    println!("╠══════════════════════════════════════════════════╣");
    println!("║  Viewport:    {}x{} @ {}x                        ║", viewport.width, viewport.height, viewport.device_scale_factor);
    println!("║  PNG size:    {:>8} bytes ({:>6.1} KB)           ║", png_size, png_size as f64 / 1024.0);
    println!("║  JPEG q80:    {:>8} bytes ({:>6.1} KB)           ║", jpeg_size, jpeg_size as f64 / 1024.0);
    println!("║  JPEG q90:    {:>8} bytes ({:>6.1} KB)           ║", jpeg90_size, jpeg90_size as f64 / 1024.0);
    println!("║  JPEG/PNG:    {:>6.1}% (q80)                     ║", ratio);
    println!("║  Savings:     {:>6.1}% smaller (q80)             ║", 100.0 - ratio);
    println!("║  JPEG magic:  {}                            ║", if jpeg_bytes.starts_with(JPEG_MAGIC) { "VERIFIED" } else { "FAILED" });
    println!("╚══════════════════════════════════════════════════╝");

    // --- Assertions (V1 evidence) ---
    // JPEG must be valid (magic bytes checked above).
    // JPEG q80 must be smaller than PNG.
    assert!(
        jpeg_size < png_size,
        "JPEG q80 ({jpeg_size} bytes) must be smaller than PNG ({png_size} bytes)"
    );

    // Quality parameter must be effective (q80 produces smaller output than q90).
    assert!(
        jpeg_size < jpeg90_size,
        "JPEG q80 ({jpeg_size}) must be smaller than q90 ({jpeg90_size}) — quality parameter must work"
    );

    // JPEG q80 should be under 200KB for 1365x768 (Q1 acceptance).
    assert!(
        jpeg_size < 200_000,
        "JPEG q80 ({jpeg_size} bytes) should be under 200KB for 1365x768 viewport"
    );

    chromium.shutdown().await.expect("Chromium shutdown");

    println!("\nCP0 V1: CDP Page.captureScreenshot with format=jpeg,quality=80 — VERIFIED");
    println!("CP0 Q1: JPEG q80 size {} bytes ({:.1} KB) — under 200KB target — VERIFIED", jpeg_size, jpeg_size as f64 / 1024.0);
}
