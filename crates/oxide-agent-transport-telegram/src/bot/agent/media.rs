//! Media extraction from Telegram messages
//!
//! Converts Telegram message types to `AgentInput`, preserving uploaded files when needed.

use anyhow::Result;
use oxide_agent_core::agent::preprocessor::AgentInput;
use teloxide::net::Download;
use teloxide::prelude::*;
use tracing::info;

/// Maximum inline media size for multimodal uploads (20 MB)
pub(crate) const MAX_INLINE_MEDIA_SIZE: u32 = 20 * 1024 * 1024;

fn mime_extension<'a>(mime_type: &str, fallback: &'a str) -> &'a str {
    match mime_type {
        "audio/ogg" => "ogg",
        "audio/mpeg" => "mp3",
        "audio/wav" | "audio/x-wav" => "wav",
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/quicktime" => "mov",
        _ => fallback,
    }
}

async fn download_telegram_file(bot: &Bot, file_id: teloxide::types::FileId) -> Result<Vec<u8>> {
    oxide_agent_core::utils::retry_transport_operation(|| async {
        let file = bot.get_file(file_id.clone()).await?;
        let mut buf = Vec::new();
        bot.download_file(&file.path, &mut buf).await?;
        Ok(buf)
    })
    .await
}

fn build_uploaded_file_input(
    bytes: Vec<u8>,
    file_name: String,
    mime_type: Option<String>,
    caption: Option<String>,
) -> AgentInput {
    AgentInput::Document {
        bytes,
        file_name,
        mime_type,
        caption,
    }
}

async fn extract_agent_input_inner(
    bot: &Bot,
    msg: &Message,
    preserve_binary_uploads: bool,
) -> Result<AgentInput> {
    let caption = msg.caption().map(ToString::to_string);

    // Voice message
    if let Some(voice) = msg.voice() {
        let buffer = download_telegram_file(bot, voice.file.id.clone()).await?;
        let mime_type = voice
            .mime_type
            .as_ref()
            .map_or_else(|| "audio/ogg".to_string(), ToString::to_string);

        if preserve_binary_uploads {
            let ext = mime_extension(&mime_type, "ogg");
            return Ok(build_uploaded_file_input(
                buffer,
                format!("voice_{}.{}", msg.id.0, ext),
                Some(mime_type),
                caption,
            ));
        }

        return Ok(AgentInput::Voice {
            bytes: buffer,
            mime_type,
        });
    }

    // Photo
    if let Some(photos) = msg.photo() {
        if let Some(photo) = photos.last() {
            let buffer = download_telegram_file(bot, photo.file.id.clone()).await?;
            if preserve_binary_uploads {
                return Ok(build_uploaded_file_input(
                    buffer,
                    format!("photo_{}.jpg", msg.id.0),
                    Some("image/jpeg".to_string()),
                    caption,
                ));
            }

            return Ok(AgentInput::Image {
                bytes: buffer,
                context: caption,
            });
        }
    }

    // Video
    if let Some(video) = msg.video() {
        if video.file.size > MAX_INLINE_MEDIA_SIZE {
            anyhow::bail!(
                "Video too large: {:.1} MB (max 20 MB)",
                f64::from(video.file.size) / 1024.0 / 1024.0
            );
        }

        let buffer = download_telegram_file(bot, video.file.id.clone()).await?;
        let mime_type = video
            .mime_type
            .as_ref()
            .map_or_else(|| "video/mp4".to_string(), ToString::to_string);
        let ext = mime_extension(&mime_type, "mp4");

        return Ok(build_uploaded_file_input(
            buffer,
            format!("video_{}.{}", msg.id.0, ext),
            Some(mime_type),
            caption,
        ));
    }

    // Document
    if let Some(doc) = msg.document() {
        if doc.file.size > MAX_INLINE_MEDIA_SIZE {
            anyhow::bail!(
                "File too large: {:.1} MB (max 20 MB)",
                f64::from(doc.file.size) / 1024.0 / 1024.0
            );
        }

        let buffer = download_telegram_file(bot, doc.file.id.clone()).await?;

        info!(
            file_name = ?doc.file_name,
            mime_type = ?doc.mime_type,
            size = buffer.len(),
            "Downloaded document from Telegram"
        );

        return Ok(build_uploaded_file_input(
            buffer,
            doc.file_name.clone().unwrap_or_else(|| "file".to_string()),
            doc.mime_type.as_ref().map(ToString::to_string),
            caption,
        ));
    }

    // Text fallback
    let text = msg
        .text()
        .or_else(|| msg.caption())
        .unwrap_or("")
        .to_string();

    Ok(AgentInput::Text(text))
}

/// Extract agent input from a Telegram message
///
/// Handles:
/// - Voice messages → `AgentInput::Voice`
/// - Photos → `AgentInput::Image`
/// - Videos → `AgentInput::Document` (sandbox-preserved file)
/// - Documents → `AgentInput::Document`
/// - Text/Caption → `AgentInput::Text`
///
/// # Errors
///
/// Returns an error if file download fails or file is too large.
pub async fn extract_agent_input(bot: &Bot, msg: &Message) -> Result<AgentInput> {
    extract_agent_input_inner(bot, msg, false).await
}

/// Extract agent input while forcing binary attachments to be preserved as sandbox files.
pub async fn extract_agent_file_input(bot: &Bot, msg: &Message) -> Result<AgentInput> {
    extract_agent_input_inner(bot, msg, true).await
}

#[cfg(test)]
mod tests {
    use super::mime_extension;

    #[test]
    fn resolves_known_media_extensions() {
        assert_eq!(mime_extension("audio/ogg", "bin"), "ogg");
        assert_eq!(mime_extension("image/jpeg", "bin"), "jpg");
        assert_eq!(mime_extension("video/mp4", "bin"), "mp4");
    }

    #[test]
    fn falls_back_for_unknown_media_extensions() {
        assert_eq!(mime_extension("application/octet-stream", "bin"), "bin");
    }
}
