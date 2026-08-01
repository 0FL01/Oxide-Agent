//! Common messaging utilities for Telegram bot.
//!
//! Contains reusable functions for sending formatted messages,
//! handling long message splitting, and other Telegram-specific transformations.

use anyhow::{Result, ensure};
use oxide_agent_core::utils;
use teloxide::prelude::*;
use teloxide::types::{ChatId, InlineKeyboardMarkup, MessageId, ParseMode, ThreadId};

use super::resilient::EditMessageOutcome;

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
    let parts = formatted_message_parts(text);
    send_formatted_parts(bot, chat_id, parts, message_thread_id, final_reply_markup).await
}

/// Replace a progress anchor with the first final chunk and send only overflow
/// chunks. If the anchor was deleted, send the complete canonical final
/// sequence instead; any other permanent edit failure is returned.
pub async fn replace_message_with_long_text(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    text: &str,
    message_thread_id: Option<ThreadId>,
    final_reply_markup: Option<InlineKeyboardMarkup>,
) -> Result<()> {
    let parts = formatted_message_parts(text);
    ensure!(!parts.is_empty(), "Telegram terminal message is empty");
    let last_index = parts.len().saturating_sub(1);
    let first_markup = if last_index == 0 {
        Some(
            final_reply_markup
                .clone()
                .unwrap_or_else(crate::bot::views::empty_inline_keyboard),
        )
    } else {
        Some(crate::bot::views::empty_inline_keyboard())
    };
    let outcome = super::resilient::edit_message_resilient_with_outcome(
        bot,
        chat_id,
        message_id,
        parts[0].clone(),
        Some(ParseMode::Html),
        first_markup,
    )
    .await?;

    let first_to_send = match outcome {
        EditMessageOutcome::Edited | EditMessageOutcome::NotModified => 1,
        EditMessageOutcome::AnchorMissing => 0,
    };
    send_formatted_parts(
        bot,
        chat_id,
        parts.into_iter().skip(first_to_send).collect(),
        message_thread_id,
        final_reply_markup,
    )
    .await
}

fn formatted_message_parts(text: &str) -> Vec<String> {
    utils::split_long_message(text, TELEGRAM_MESSAGE_LIMIT)
        .into_iter()
        .map(|part| utils::format_text(&part))
        .collect()
}

async fn send_formatted_parts(
    bot: &Bot,
    chat_id: ChatId,
    parts: Vec<String>,
    message_thread_id: Option<ThreadId>,
    final_reply_markup: Option<InlineKeyboardMarkup>,
) -> Result<()> {
    let last_index = parts.len().saturating_sub(1);

    for (index, part) in parts.into_iter().enumerate() {
        let reply_markup = (index == last_index)
            .then(|| final_reply_markup.clone().map(Into::into))
            .flatten();
        super::resilient::send_message_resilient_with_thread_and_markup(
            bot,
            chat_id,
            part,
            Some(ParseMode::Html),
            message_thread_id,
            reply_markup,
        )
        .await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{TELEGRAM_MESSAGE_LIMIT, formatted_message_parts};

    #[test]
    fn terminal_parts_preserve_limit_and_independent_formatting() {
        let text = format!(
            "**start**\n{}\n**end**",
            "a".repeat(TELEGRAM_MESSAGE_LIMIT * 2)
        );

        let parts = formatted_message_parts(&text);

        assert!(parts.len() >= 3);
        assert!(parts.iter().all(|part| !part.is_empty()));
        assert!(
            parts
                .iter()
                .all(|part| part.chars().count() <= TELEGRAM_MESSAGE_LIMIT + 128)
        );
    }

    #[test]
    fn terminal_parts_escape_hostile_text() {
        let parts = formatted_message_parts("<script>alert('&')</script>");

        assert_eq!(parts.len(), 1);
        assert!(!parts[0].contains("<script>"));
        assert!(parts[0].contains("&lt;script&gt;"));
    }
}
