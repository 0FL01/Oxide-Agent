//! Resilient messaging utilities with automatic retry for Telegram API operations.
//!
//! This module provides wrappers around Telegram API operations that automatically
//! retry on transient network failures using exponential backoff with jitter.
//!
//! # Usage
//!
//! ```ignore
//! use oxide_agent_transport_telegram::bot::resilient::{send_message_resilient, edit_message_safe_resilient};
//!
//! // Send with automatic retry
//! let msg = send_message_resilient(&bot, chat_id, "Hello!", Some(ParseMode::Html)).await?;
//!
//! // Edit with graceful degradation
//! let success = edit_message_safe_resilient(&bot, chat_id, msg.id, "Updated!").await;
//! ```

use anyhow::Result;
use teloxide::prelude::*;
use teloxide::types::{ChatId, Message, MessageId, ParseMode};
use tracing::{debug, warn};

/// Send a message with automatic retry on network failures.
///
/// Uses [`oxide_agent_core::utils::retry_transport_operation`] with exponential backoff
/// to handle transient network errors.
///
/// # Arguments
///
/// * `bot` - The Telegram bot instance
/// * `chat_id` - Target chat ID
/// * `text` - Message text to send
/// * `parse_mode` - Optional parse mode (HTML, Markdown, etc.)
///
/// # Returns
///
/// The sent [`Message`] on success, or an error after all retries are exhausted.
///
/// # Examples
///
/// ```ignore
/// let msg = send_message_resilient(&bot, chat_id, "⏳ Processing...", Some(ParseMode::Html)).await?;
/// ```
pub async fn send_message_resilient(
    bot: &Bot,
    chat_id: ChatId,
    text: impl Into<String>,
    parse_mode: Option<ParseMode>,
) -> Result<Message> {
    let text = text.into();
    oxide_agent_core::utils::retry_transport_operation(|| async {
        let mut req = bot.send_message(chat_id, text.clone());
        if let Some(pm) = parse_mode {
            req = req.parse_mode(pm);
        }
        req.await
            .map_err(|e| anyhow::anyhow!("Telegram send error: {e}"))
    })
    .await
}

/// Edit a message with automatic retry on network failures.
///
/// Returns `Result` to allow explicit error handling by the caller.
///
/// # Arguments
///
/// * `bot` - The Telegram bot instance
/// * `chat_id` - Chat ID containing the message
/// * `msg_id` - ID of the message to edit
/// * `text` - New message text
/// * `parse_mode` - Optional parse mode
///
/// # Returns
///
/// The edited [`Message`] on success, or an error after all retries are exhausted.
pub async fn edit_message_resilient(
    bot: &Bot,
    chat_id: ChatId,
    msg_id: MessageId,
    text: impl Into<String>,
    parse_mode: Option<ParseMode>,
) -> Result<Option<Message>> {
    let text = text.into();
    oxide_agent_core::utils::retry_transport_operation(|| async {
        let mut req = bot.edit_message_text(chat_id, msg_id, text.clone());
        if let Some(pm) = parse_mode {
            req = req.parse_mode(pm);
        }
        match req.await {
            Ok(msg) => Ok(Some(msg)),
            Err(e) => {
                let err_msg = e.to_string();
                if err_msg.contains("message is not modified") {
                    return Ok(None);
                }
                Err(anyhow::anyhow!("Telegram edit error: {e}"))
            }
        }
    })
    .await
}

/// Edit message with graceful degradation and automatic retry.
///
/// This function:
/// 1. Truncates text to 4000 characters if needed
/// 2. Retries on transient network errors
/// 3. Gracefully handles expected errors ("message not modified", "not found")
///
/// # Returns
///
/// - `true` if message was successfully edited
/// - `false` if edit was skipped (not modified / not found) or failed after retries
///
/// # Examples
///
/// ```ignore
/// let success = edit_message_safe_resilient(&bot, chat_id, msg_id, "Updated progress").await;
/// if !success {
///     // Handle gracefully - maybe send a new message
/// }
/// ```
pub async fn edit_message_safe_resilient(
    bot: &Bot,
    chat_id: ChatId,
    msg_id: MessageId,
    text: &str,
) -> bool {
    const ERROR_NOT_FOUND: &str = "message to edit not found";

    // Truncate if too long (Telegram limit is 4096, we use 4000 for safety)
    let truncated = if text.chars().count() > 4000 {
        let truncated_text = oxide_agent_core::utils::truncate_str(text, 4000);
        format!("{truncated_text}...\n\n<i>(message truncated)</i>")
    } else {
        text.to_string()
    };

    match edit_message_resilient(bot, chat_id, msg_id, truncated, Some(ParseMode::Html)).await {
        Ok(Some(_)) => true,
        Ok(None) => {
            debug!("Message update skipped: message is not modified");
            true
        }
        Err(e) => {
            let err_msg = e.to_string();
            if err_msg.contains(ERROR_NOT_FOUND) {
                debug!("Message update skipped: {err_msg}");
            } else {
                warn!("Failed to edit message after retries: {e}");
            }
            false
        }
    }
}
