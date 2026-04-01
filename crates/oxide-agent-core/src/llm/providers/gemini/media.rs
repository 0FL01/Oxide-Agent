use gemini_rust::safety::{HarmBlockThreshold, HarmCategory, SafetySetting};

use super::GeminiProvider;

impl GeminiProvider {
    pub(super) fn safety_settings() -> Vec<SafetySetting> {
        vec![
            SafetySetting {
                category: HarmCategory::Harassment,
                threshold: HarmBlockThreshold::BlockNone,
            },
            SafetySetting {
                category: HarmCategory::HateSpeech,
                threshold: HarmBlockThreshold::BlockNone,
            },
            SafetySetting {
                category: HarmCategory::SexuallyExplicit,
                threshold: HarmBlockThreshold::BlockNone,
            },
            SafetySetting {
                category: HarmCategory::DangerousContent,
                threshold: HarmBlockThreshold::BlockNone,
            },
        ]
    }

    pub(super) fn max_output_tokens(max_tokens: u32) -> i32 {
        i32::try_from(max_tokens).unwrap_or(i32::MAX)
    }

    pub(super) fn infer_image_mime_type(image_bytes: &[u8]) -> &'static str {
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
}
