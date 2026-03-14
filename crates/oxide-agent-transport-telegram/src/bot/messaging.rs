//! Common messaging utilities for Telegram bot.
//!
//! Contains reusable functions for sending formatted messages,
//! handling long message splitting, and other Telegram-specific transformations.

use anyhow::Result;
use oxide_agent_core::utils;
use teloxide::prelude::*;
use teloxide::types::{ChatId, InlineKeyboardMarkup, ParseMode, ThreadId};

/// Maximum message length for Telegram with safety margin.
/// Telegram's official limit is 4096, but we use 4000 to account for
/// HTML tags and other formatting that may be added.
pub const TELEGRAM_MESSAGE_LIMIT: usize = 4000;

/// Sends a long message by splitting it into multiple parts.
///
/// This function:
/// 1. Formats the text using markdown-to-HTML conversion
/// 2. Splits long messages respecting code blocks and Telegram limits
/// 3. Sends each part as a separate message with HTML parsing
///
/// # Arguments
///
/// * `bot` - The Telegram bot instance
/// * `chat_id` - The chat to send messages to
/// * `text` - The raw text to format and send
///
/// # Errors
///
/// Returns an error if any message fails to send.
///
/// # Examples
///
/// ```ignore
/// use oxide_agent_transport_telegram::bot::messaging::send_long_message;
///
/// // Will automatically split if text exceeds 4000 characters
/// send_long_message(&bot, chat_id, &very_long_response).await?;
/// ```
pub async fn send_long_message(bot: &Bot, chat_id: ChatId, text: &str) -> Result<()> {
    send_long_message_in_thread(bot, chat_id, text, None).await
}

/// Sends a long message by splitting it into multiple parts in specific thread.
pub async fn send_long_message_in_thread(
    bot: &Bot,
    chat_id: ChatId,
    text: &str,
    message_thread_id: Option<ThreadId>,
) -> Result<()> {
    send_long_message_in_thread_with_final_markup(bot, chat_id, text, message_thread_id, None).await
}

/// Sends a long message by splitting it into multiple parts in specific thread.
/// Optional inline markup is attached only to the final part.
pub async fn send_long_message_in_thread_with_final_markup(
    bot: &Bot,
    chat_id: ChatId,
    text: &str,
    message_thread_id: Option<ThreadId>,
    final_reply_markup: Option<InlineKeyboardMarkup>,
) -> Result<()> {
    // Split raw Markdown first - split_long_message correctly handles ``` fences
    let parts = utils::split_long_message(text, TELEGRAM_MESSAGE_LIMIT);
    let last_index = parts.len().saturating_sub(1);

    for (index, part) in parts.into_iter().enumerate() {
        // Format each part to HTML after splitting to ensure proper tag closure
        let formatted = utils::format_text(&part);
        let reply_markup = (index == last_index)
            .then(|| final_reply_markup.clone().map(Into::into))
            .flatten();
        super::resilient::send_message_resilient_with_thread_and_markup(
            bot,
            chat_id,
            formatted,
            Some(ParseMode::Html),
            message_thread_id,
            reply_markup,
        )
        .await?;
    }

    Ok(())
}
