use crate::agent::loop_detection::LoopType;
use crate::agent::progress::ProgressState;
use crate::agent::runtime::{AgentTransport, DeliveryMode};
use crate::bot::progress_render::render_progress_html;
use crate::bot::views::{loop_action_keyboard, loop_type_label};
use anyhow::Result;
use async_trait::async_trait;
use teloxide::prelude::*;
use teloxide::types::{ChatId, InputFile, MessageId, ParseMode};
use tracing::warn;

/// Telegram-specific progress runtime transport.
pub struct TelegramAgentTransport {
    bot: Bot,
    chat_id: ChatId,
    progress_msg_id: MessageId,
}

impl TelegramAgentTransport {
    /// Create a Telegram transport bound to a progress message.
    pub const fn new(bot: Bot, chat_id: ChatId, progress_msg_id: MessageId) -> Self {
        Self {
            bot,
            chat_id,
            progress_msg_id,
        }
    }
}

#[async_trait]
impl AgentTransport for TelegramAgentTransport {
    async fn update_progress(&self, state: &ProgressState) -> Result<()> {
        let text = render_progress_html(state);
        // Preserve existing behavior: resilient helper handles retries and logging internally.
        let _ = crate::bot::resilient::edit_message_safe_resilient(
            &self.bot,
            self.chat_id,
            self.progress_msg_id,
            &text,
        )
        .await;
        Ok(())
    }

    async fn deliver_file(
        &self,
        mode: DeliveryMode,
        file_name: &str,
        content: &[u8],
    ) -> Result<()> {
        match mode {
            DeliveryMode::BestEffort => {
                if let Err(e) = send_file_smart(&self.bot, self.chat_id, file_name, content).await {
                    warn!(file_name = %file_name, error = %e, "Failed to send file");
                    return Err(e);
                }
                Ok(())
            }
            DeliveryMode::Confirmed => {
                crate::utils::retry_telegram_operation(|| async {
                    send_file_smart(&self.bot, self.chat_id, file_name, content)
                        .await
                        .map(|_| ())
                        .map_err(|e| anyhow::anyhow!("Telegram error: {e}"))
                })
                .await
            }
        }
    }

    async fn notify_loop_detected(&self, loop_type: LoopType, iteration: usize) -> Result<()> {
        let text = format!(
            "🔁 <b>Loop Detected in Task Execution</b>\nType: {}\nIteration: {}\n\nSelect an action:",
            loop_type_label(loop_type),
            iteration
        );

        self.bot
            .send_message(self.chat_id, text)
            .parse_mode(ParseMode::Html)
            .reply_markup(loop_action_keyboard())
            .await?;

        Ok(())
    }
}

static VIDEO_EXTENSIONS: &[&str] = &["mp4", "mov", "avi", "mkv", "webm"];
static AUDIO_EXTENSIONS: &[&str] = &["mp3", "wav", "ogg", "m4a", "flac"];

/// Smart file sending that chooses send_video/send_audio/send_document based on extension.
///
/// Implements fallback logic: if native media sending fails, retries as a document.
async fn send_file_smart(
    bot: &Bot,
    chat_id: ChatId,
    file_name: &str,
    content: &[u8],
) -> Result<teloxide::types::Message> {
    let extension = std::path::Path::new(file_name)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|s| s.to_lowercase());

    let file_name_owned = file_name.to_string();
    let make_file = || InputFile::memory(content.to_vec()).file_name(file_name_owned.clone());

    if let Some(ext) = extension.as_deref() {
        if VIDEO_EXTENSIONS.contains(&ext) {
            return match bot.send_video(chat_id, make_file()).await {
                Ok(msg) => Ok(msg),
                Err(e) => {
                    warn!(
                        file_name = %file_name,
                        error = %e,
                        "Failed to send video as native media; falling back to document"
                    );
                    bot.send_document(chat_id, make_file())
                        .await
                        .map_err(Into::into)
                }
            };
        }

        if AUDIO_EXTENSIONS.contains(&ext) {
            return match bot.send_audio(chat_id, make_file()).await {
                Ok(msg) => Ok(msg),
                Err(e) => {
                    warn!(
                        file_name = %file_name,
                        error = %e,
                        "Failed to send audio as native media; falling back to document"
                    );
                    bot.send_document(chat_id, make_file())
                        .await
                        .map_err(Into::into)
                }
            };
        }
    }

    bot.send_document(chat_id, make_file()).await.map_err(Into::into)
}
