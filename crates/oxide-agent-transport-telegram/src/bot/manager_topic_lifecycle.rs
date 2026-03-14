use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use oxide_agent_core::agent::providers::{
    ForumTopicActionResult, ForumTopicCreateRequest, ForumTopicCreateResult, ForumTopicEditRequest,
    ForumTopicEditResult, ForumTopicThreadRequest, ManagerTopicLifecycle,
};
use teloxide::payloads::{CreateForumTopicSetters, EditForumTopicSetters};
use teloxide::prelude::Requester;
use teloxide::types::{ChatId, CustomEmojiId, MessageId, Rgb, ThreadId};
use teloxide::Bot;

/// Telegram transport implementation of manager forum topic lifecycle.
#[derive(Clone)]
pub(crate) struct TelegramManagerTopicLifecycle {
    bot: Bot,
    default_chat_id: Option<ChatId>,
}

impl TelegramManagerTopicLifecycle {
    /// Creates a lifecycle adapter for a specific bot context.
    pub(crate) const fn new(bot: Bot, default_chat_id: Option<ChatId>) -> Self {
        Self {
            bot,
            default_chat_id,
        }
    }

    fn resolve_chat_id(&self, requested_chat_id: Option<i64>) -> Result<ChatId> {
        if let Some(chat_id) = requested_chat_id {
            return Ok(ChatId(chat_id));
        }

        self.default_chat_id
            .ok_or_else(|| anyhow!("chat_id is required in this context"))
    }

    fn to_thread_id(thread_id: i64) -> Result<ThreadId> {
        if thread_id <= 0 {
            bail!("thread_id must be a positive integer");
        }

        let value =
            i32::try_from(thread_id).map_err(|_| anyhow!("thread_id exceeds Telegram range"))?;
        Ok(ThreadId(MessageId(value)))
    }
}

#[async_trait]
impl ManagerTopicLifecycle for TelegramManagerTopicLifecycle {
    async fn forum_topic_create(
        &self,
        request: ForumTopicCreateRequest,
    ) -> Result<ForumTopicCreateResult> {
        let chat_id = self.resolve_chat_id(request.chat_id)?;
        let mut api = self.bot.create_forum_topic(chat_id, request.name);
        if let Some(icon_color) = request.icon_color {
            api = api.icon_color(Rgb::from_u32(icon_color));
        }
        if let Some(icon_custom_emoji_id) = request.icon_custom_emoji_id {
            api = api.icon_custom_emoji_id(CustomEmojiId(icon_custom_emoji_id));
        }

        let created = api
            .await
            .map_err(|err| anyhow!("telegram forum topic create failed: {err}"))?;

        Ok(ForumTopicCreateResult {
            chat_id: chat_id.0,
            thread_id: i64::from(created.thread_id.0 .0),
            name: created.name,
            icon_color: created.icon_color.to_u32(),
            icon_custom_emoji_id: created.icon_custom_emoji_id.map(|id| id.0),
        })
    }

    async fn forum_topic_edit(
        &self,
        request: ForumTopicEditRequest,
    ) -> Result<ForumTopicEditResult> {
        let chat_id = self.resolve_chat_id(request.chat_id)?;
        let thread_id = Self::to_thread_id(request.thread_id)?;
        let mut api = self.bot.edit_forum_topic(chat_id, thread_id);
        if let Some(name) = request.name.clone() {
            api = api.name(name);
        }
        if let Some(icon_custom_emoji_id) = request.icon_custom_emoji_id.clone() {
            api = api.icon_custom_emoji_id(CustomEmojiId(icon_custom_emoji_id));
        }

        api.await
            .map_err(|err| anyhow!("telegram forum topic edit failed: {err}"))?;

        Ok(ForumTopicEditResult {
            chat_id: chat_id.0,
            thread_id: request.thread_id,
            name: request.name,
            icon_custom_emoji_id: request.icon_custom_emoji_id,
        })
    }

    async fn forum_topic_close(
        &self,
        request: ForumTopicThreadRequest,
    ) -> Result<ForumTopicActionResult> {
        let chat_id = self.resolve_chat_id(request.chat_id)?;
        let thread_id = Self::to_thread_id(request.thread_id)?;
        self.bot
            .close_forum_topic(chat_id, thread_id)
            .await
            .map_err(|err| anyhow!("telegram forum topic close failed: {err}"))?;

        Ok(ForumTopicActionResult {
            chat_id: chat_id.0,
            thread_id: request.thread_id,
        })
    }

    async fn forum_topic_reopen(
        &self,
        request: ForumTopicThreadRequest,
    ) -> Result<ForumTopicActionResult> {
        let chat_id = self.resolve_chat_id(request.chat_id)?;
        let thread_id = Self::to_thread_id(request.thread_id)?;
        self.bot
            .reopen_forum_topic(chat_id, thread_id)
            .await
            .map_err(|err| anyhow!("telegram forum topic reopen failed: {err}"))?;

        Ok(ForumTopicActionResult {
            chat_id: chat_id.0,
            thread_id: request.thread_id,
        })
    }

    async fn forum_topic_delete(
        &self,
        request: ForumTopicThreadRequest,
    ) -> Result<ForumTopicActionResult> {
        let chat_id = self.resolve_chat_id(request.chat_id)?;
        let thread_id = Self::to_thread_id(request.thread_id)?;
        self.bot
            .delete_forum_topic(chat_id, thread_id)
            .await
            .map_err(|err| anyhow!("telegram forum topic delete failed: {err}"))?;

        Ok(ForumTopicActionResult {
            chat_id: chat_id.0,
            thread_id: request.thread_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::TelegramManagerTopicLifecycle;
    use teloxide::types::ChatId;
    use teloxide::Bot;

    #[test]
    fn resolves_chat_id_from_context_when_present() {
        let lifecycle = TelegramManagerTopicLifecycle::new(Bot::new("token"), Some(ChatId(-101)));
        let resolved = lifecycle
            .resolve_chat_id(None)
            .expect("default chat should be resolved");

        assert_eq!(resolved, ChatId(-101));
    }

    #[test]
    fn resolves_explicit_chat_id_over_context() {
        let lifecycle = TelegramManagerTopicLifecycle::new(Bot::new("token"), Some(ChatId(-101)));
        let resolved = lifecycle
            .resolve_chat_id(Some(-202))
            .expect("explicit chat should be resolved");

        assert_eq!(resolved, ChatId(-202));
    }

    #[test]
    fn rejects_missing_chat_id_without_context() {
        let lifecycle = TelegramManagerTopicLifecycle::new(Bot::new("token"), None);
        let err = lifecycle
            .resolve_chat_id(None)
            .expect_err("chat id should be required when context is absent");

        assert!(err.to_string().contains("chat_id is required"));
    }

    #[test]
    fn rejects_invalid_thread_id() {
        let err =
            TelegramManagerTopicLifecycle::to_thread_id(0).expect_err("thread id must be positive");
        assert!(err.to_string().contains("positive integer"));
    }
}
