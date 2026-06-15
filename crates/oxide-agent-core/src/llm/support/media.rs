//! Low-level media encoding helpers.
//!
//! Capability decisions and provider-specific content part shapes stay with
//! provider profiles and request builders.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

#[must_use]
pub(crate) fn image_data_url(image_bytes: &[u8]) -> String {
    image_data_url_with_mime(image_bytes, infer_image_mime_type(image_bytes))
}

#[must_use]
pub(crate) fn image_data_url_with_mime(image_bytes: &[u8], mime_type: &str) -> String {
    let mime_type = normalized_image_mime_type(mime_type, image_bytes);
    data_url(&mime_type, image_bytes)
}

#[must_use]
pub(crate) fn normalized_image_mime_type(mime_type: &str, image_bytes: &[u8]) -> String {
    let trimmed = mime_type.trim();
    if trimmed.starts_with("image/") {
        trimmed.to_string()
    } else {
        infer_image_mime_type(image_bytes).to_string()
    }
}

#[must_use]
pub(crate) fn infer_image_mime_type(image_bytes: &[u8]) -> &'static str {
    if image_bytes.starts_with(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n']) {
        return "image/png";
    }
    if image_bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return "image/jpeg";
    }
    if image_bytes.starts_with(b"GIF87a") || image_bytes.starts_with(b"GIF89a") {
        return "image/gif";
    }
    if image_bytes.starts_with(b"RIFF") && image_bytes.get(8..12) == Some(b"WEBP") {
        return "image/webp";
    }
    "image/jpeg"
}

#[must_use]
pub(crate) fn data_url(mime_type: &str, bytes: &[u8]) -> String {
    format!("data:{mime_type};base64,{}", base64_data(bytes))
}

#[must_use]
pub(crate) fn base64_data(bytes: &[u8]) -> String {
    BASE64.encode(bytes)
}

#[must_use]
pub(crate) fn audio_input_format(mime_type: &str) -> &'static str {
    let normalized = mime_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();

    match normalized.as_str() {
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/ogg" | "audio/opus" | "audio/vorbis" => "ogg",
        "audio/flac" => "flac",
        "audio/mp4" | "audio/x-m4a" => "m4a",
        "audio/webm" => "webm",
        _ => "wav",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_media_infers_png_jpeg_webp_gif_and_defaults_safely() {
        assert_eq!(
            infer_image_mime_type(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n']),
            "image/png"
        );
        assert_eq!(infer_image_mime_type(&[0xFF, 0xD8, 0xFF]), "image/jpeg");
        assert_eq!(infer_image_mime_type(b"GIF89a"), "image/gif");
        assert_eq!(infer_image_mime_type(b"RIFFxxxxWEBPpayload"), "image/webp");
        assert_eq!(infer_image_mime_type(b"unknown"), "image/jpeg");
    }

    #[test]
    fn support_media_builds_data_url_compatible_with_legacy_requests() {
        assert_eq!(
            image_data_url_with_mime(b"png", " image/png "),
            "data:image/png;base64,cG5n"
        );
        assert_eq!(
            image_data_url_with_mime(&[0xFF, 0xD8, 0xFF], "application/octet-stream"),
            "data:image/jpeg;base64,/9j/"
        );
    }

    #[test]
    fn support_media_maps_audio_input_formats_without_content_shape() {
        assert_eq!(audio_input_format("audio/mpeg; codecs=mp3"), "mp3");
        assert_eq!(audio_input_format("audio/webm"), "webm");
        assert_eq!(audio_input_format("application/octet-stream"), "wav");
    }
}
