//! Media extraction from Telegram messages
//!
//! Converts Telegram message types (voice, photo, document) to `AgentInput`.

use crate::agent::preprocessor::AgentInput;
use anyhow::Result;
use teloxide::net::Download;
use teloxide::prelude::*;
use tracing::info;

/// Maximum file size for document uploads (20 MB)
const MAX_FILE_SIZE: u32 = 20 * 1024 * 1024;

/// Extract agent input from a Telegram message
///
/// Handles:
/// - Voice messages → `AgentInput::Voice`
/// - Photos → `AgentInput::Image`
/// - Documents → `AgentInput::Document`
/// - Text/Caption → `AgentInput::Text`
///
/// # Errors
///
/// Returns an error if file download fails or file is too large.
pub async fn extract_agent_input(bot: &Bot, msg: &Message) -> Result<AgentInput> {
    // Voice message
    if let Some(voice) = msg.voice() {
        let buffer = crate::utils::retry_telegram_operation(|| async {
            let file = bot.get_file(voice.file.id.clone()).await?;
            let mut buf = Vec::new();
            bot.download_file(&file.path, &mut buf).await?;
            Ok(buf)
        })
        .await?;

        let mime_type = voice
            .mime_type
            .as_ref()
            .map_or_else(|| "audio/ogg".to_string(), ToString::to_string);

        return Ok(AgentInput::Voice {
            bytes: buffer,
            mime_type,
        });
    }

    // Photo
    if let Some(photos) = msg.photo() {
        if let Some(photo) = photos.last() {
            let buffer = crate::utils::retry_telegram_operation(|| async {
                let file = bot.get_file(photo.file.id.clone()).await?;
                let mut buf = Vec::new();
                bot.download_file(&file.path, &mut buf).await?;
                Ok(buf)
            })
            .await?;

            let caption = msg.caption().map(ToString::to_string);
            return Ok(AgentInput::Image {
                bytes: buffer,
                context: caption,
            });
        }
    }

    // Document
    if let Some(doc) = msg.document() {
        if doc.file.size > MAX_FILE_SIZE {
            anyhow::bail!(
                "File too large: {:.1} MB (max 20 MB)",
                f64::from(doc.file.size) / 1024.0 / 1024.0
            );
        }

        let buffer = crate::utils::retry_telegram_operation(|| async {
            let file = bot.get_file(doc.file.id.clone()).await?;
            let mut buf = Vec::new();
            bot.download_file(&file.path, &mut buf).await?;
            Ok(buf)
        })
        .await?;

        info!(
            file_name = ?doc.file_name,
            mime_type = ?doc.mime_type,
            size = buffer.len(),
            "Downloaded document from Telegram"
        );

        return Ok(AgentInput::Document {
            bytes: buffer,
            file_name: doc.file_name.clone().unwrap_or_else(|| "file".to_string()),
            mime_type: doc.mime_type.as_ref().map(ToString::to_string),
            caption: msg.caption().map(String::from),
        });
    }

    // Text fallback
    let text = msg
        .text()
        .or_else(|| msg.caption())
        .unwrap_or("")
        .to_string();

    Ok(AgentInput::Text(text))
}
