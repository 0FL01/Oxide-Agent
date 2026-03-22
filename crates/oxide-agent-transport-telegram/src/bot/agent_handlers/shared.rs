use super::input::PendingTextInputBatch;
use crate::bot::{general_forum_topic_id, TelegramThreadKind, TelegramThreadSpec};
use crate::config::BotSettings;
use oxide_agent_core::agent::SessionId;
use oxide_agent_core::storage::ReminderThreadKind;
use std::collections::HashMap;
use std::sync::LazyLock;
use teloxide::prelude::*;
use teloxide::types::MessageId;
use tokio::sync::{Mutex, RwLock};

fn thread_id_value(thread_spec: TelegramThreadSpec) -> Option<i32> {
    thread_spec.thread_id.map(|thread_id| thread_id.0 .0)
}

pub(crate) fn manager_default_chat_id(
    settings: &BotSettings,
    chat_id: ChatId,
    thread_spec: TelegramThreadSpec,
) -> Option<ChatId> {
    if settings.telegram.manager_home_chat_id.is_some() {
        return settings
            .telegram
            .manager_home_matches(chat_id.0, thread_id_value(thread_spec))
            .then_some(chat_id);
    }

    matches!(thread_spec.kind, TelegramThreadKind::Forum).then_some(chat_id)
}

pub(crate) fn reminder_thread_kind(thread_spec: TelegramThreadSpec) -> ReminderThreadKind {
    match thread_spec.kind {
        TelegramThreadKind::Dm => ReminderThreadKind::Dm,
        TelegramThreadKind::Forum => ReminderThreadKind::Forum,
        TelegramThreadKind::None => ReminderThreadKind::None,
    }
}

pub(crate) fn is_general_forum_topic(thread_spec: TelegramThreadSpec) -> bool {
    matches!(thread_spec.kind, TelegramThreadKind::Forum)
        && thread_spec.thread_id == Some(general_forum_topic_id())
}

pub(crate) fn manager_control_plane_enabled(
    settings: &BotSettings,
    user_id: i64,
    chat_id: ChatId,
    thread_spec: TelegramThreadSpec,
) -> bool {
    if !settings.telegram.manager_allowed_users().contains(&user_id) {
        return false;
    }

    if settings.telegram.manager_home_chat_id.is_some() {
        return settings
            .telegram
            .manager_home_matches(chat_id.0, thread_id_value(thread_spec));
    }

    is_general_forum_topic(thread_spec)
}

pub(crate) static PENDING_CANCEL_MESSAGES: LazyLock<RwLock<HashMap<SessionId, MessageId>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
pub(crate) static PENDING_CANCEL_CONFIRMATIONS: LazyLock<RwLock<HashMap<SessionId, MessageId>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
pub(crate) static PENDING_TEXT_INPUT_BATCHES: LazyLock<
    Mutex<HashMap<SessionId, PendingTextInputBatch>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));
