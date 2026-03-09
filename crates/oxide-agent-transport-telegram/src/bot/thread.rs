//! Telegram thread/topic helpers for inbound and outbound flows.

use teloxide::types::{Chat, ChatId, ChatKind, Message, MessageId, PublicChatKind, ThreadId};

const GENERAL_FORUM_TOPIC_ID: i32 = 1;

/// Logical thread context for Telegram message handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelegramThreadKind {
    /// Forum/supergroup topic context.
    Forum,
    /// Direct/private chat context.
    Dm,
    /// No thread context should be applied.
    None,
}

/// Resolved Telegram thread specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TelegramThreadSpec {
    /// Logical thread context kind.
    pub kind: TelegramThreadKind,
    /// Telegram message thread identifier when available.
    pub thread_id: Option<ThreadId>,
}

impl TelegramThreadSpec {
    /// Constructs a new thread specification.
    #[must_use]
    pub const fn new(kind: TelegramThreadKind, thread_id: Option<ThreadId>) -> Self {
        Self { kind, thread_id }
    }
}

/// Outbound thread params for Telegram send operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutboundThreadParams {
    /// Optional thread id that can be applied to send requests.
    pub message_thread_id: Option<ThreadId>,
}

/// Resolves thread spec from inbound Telegram message context.
#[must_use]
pub fn resolve_thread_spec(message: &Message) -> TelegramThreadSpec {
    let is_group = message.chat.is_group() || message.chat.is_supergroup();
    let is_forum = is_forum_chat(&message.chat) || message.is_topic_message;
    resolve_thread_spec_from_context(is_group, is_forum, message.thread_id)
}

/// Resolves thread spec from normalized message context parts.
#[must_use]
pub fn resolve_thread_spec_from_context(
    is_group: bool,
    is_forum: bool,
    thread_id: Option<ThreadId>,
) -> TelegramThreadSpec {
    if is_group && is_forum {
        return TelegramThreadSpec::new(
            TelegramThreadKind::Forum,
            Some(thread_id.unwrap_or(general_forum_topic_id())),
        );
    }

    if !is_group {
        return TelegramThreadSpec::new(TelegramThreadKind::Dm, None);
    }

    TelegramThreadSpec::new(TelegramThreadKind::None, None)
}

/// Builds outbound thread params for send operations.
///
/// Telegram forum edge case: topic `1` (General) must omit `message_thread_id`.
#[must_use]
pub fn build_outbound_thread_params(spec: TelegramThreadSpec) -> OutboundThreadParams {
    let message_thread_id = match spec.kind {
        TelegramThreadKind::Forum => spec.thread_id.and_then(|thread_id| {
            if thread_id == general_forum_topic_id() {
                None
            } else {
                Some(thread_id)
            }
        }),
        TelegramThreadKind::Dm | TelegramThreadKind::None => None,
    };

    OutboundThreadParams { message_thread_id }
}

/// Returns a reusable chat+thread peer key for routing.
#[must_use]
pub fn thread_peer_key(chat_id: ChatId, thread_id: Option<ThreadId>) -> String {
    let thread_part = thread_id.map_or(0, |id| id.0 .0);
    format!("{}:{thread_part}", chat_id.0)
}

/// Returns a reusable chat+thread peer key from resolved spec.
#[must_use]
pub fn thread_peer_key_from_spec(chat_id: ChatId, spec: TelegramThreadSpec) -> String {
    thread_peer_key(chat_id, spec.thread_id)
}

/// Returns `true` when chat is a forum-enabled supergroup.
#[must_use]
pub fn is_forum_chat(chat: &Chat) -> bool {
    let ChatKind::Public(chat_public) = &chat.kind else {
        return false;
    };

    let PublicChatKind::Supergroup(supergroup) = &chat_public.kind else {
        return false;
    };

    supergroup.is_forum
}

/// Returns Telegram general forum topic id.
#[must_use]
pub const fn general_forum_topic_id() -> ThreadId {
    ThreadId(MessageId(GENERAL_FORUM_TOPIC_ID))
}

#[cfg(test)]
mod tests {
    use super::{
        build_outbound_thread_params, general_forum_topic_id, resolve_thread_spec_from_context,
        TelegramThreadKind,
    };
    use teloxide::types::{ChatId, MessageId, ThreadId};

    #[test]
    fn resolves_forum_with_default_general_topic() {
        let spec = resolve_thread_spec_from_context(true, true, None);

        assert_eq!(spec.kind, TelegramThreadKind::Forum);
        assert_eq!(spec.thread_id, Some(general_forum_topic_id()));
    }

    #[test]
    fn resolves_dm_without_thread() {
        let spec = resolve_thread_spec_from_context(false, false, Some(ThreadId(MessageId(7))));

        assert_eq!(spec.kind, TelegramThreadKind::Dm);
        assert_eq!(spec.thread_id, None);
    }

    #[test]
    fn omits_general_topic_from_outbound_params() {
        let spec = resolve_thread_spec_from_context(true, true, Some(general_forum_topic_id()));
        let params = build_outbound_thread_params(spec);

        assert_eq!(params.message_thread_id, None);
    }

    #[test]
    fn keeps_non_general_topic_for_outbound_params() {
        let spec = resolve_thread_spec_from_context(true, true, Some(ThreadId(MessageId(42))));
        let params = build_outbound_thread_params(spec);

        assert_eq!(params.message_thread_id, Some(ThreadId(MessageId(42))));
    }

    #[test]
    fn builds_peer_key() {
        let key = super::thread_peer_key(ChatId(-1001), Some(ThreadId(MessageId(11))));
        assert_eq!(key, "-1001:11");
    }
}
