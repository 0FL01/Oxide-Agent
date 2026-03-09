use crate::config::{BotSettings, TelegramTopicSettings};
use teloxide::types::Message;

/// Inbound message context required for topic route checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicRouteContext<'a> {
    /// Incoming text payload (if present).
    pub text: Option<&'a str>,
    /// Incoming media caption payload (if present).
    pub caption: Option<&'a str>,
    /// Whether the message is a reply to a bot-authored message.
    pub reply_to_bot: bool,
}

/// Effective topic route decision for a single inbound message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicRouteDecision {
    /// Topic-level processing switch.
    pub enabled: bool,
    /// Whether mention/reply signal is required in this topic.
    pub require_mention: bool,
    /// Whether mention/reply requirement is satisfied by current message.
    pub mention_satisfied: bool,
    /// Optional topic-level system prompt override.
    pub system_prompt_override: Option<String>,
    /// Selected topic agent identifier (informational in current stage).
    pub agent_id: Option<String>,
}

impl TopicRouteDecision {
    /// Returns true when inbound message should be processed.
    #[must_use]
    pub const fn allows_processing(&self) -> bool {
        self.enabled && (!self.require_mention || self.mention_satisfied)
    }
}

/// Resolve topic decision from Telegram settings and inbound message.
#[must_use]
pub fn resolve_topic_route(settings: &BotSettings, message: &Message) -> TopicRouteDecision {
    let thread_id = message.thread_id.map(|thread| thread.0 .0);
    let topic = settings
        .telegram
        .resolve_topic_config(message.chat.id.0, thread_id);
    let context = TopicRouteContext {
        text: message.text(),
        caption: message.caption(),
        reply_to_bot: is_reply_to_bot(message),
    };

    resolve_topic_route_decision(topic, &context)
}

/// Resolve topic decision using pre-extracted context and optional topic config.
#[must_use]
pub fn resolve_topic_route_decision(
    topic: Option<&TelegramTopicSettings>,
    context: &TopicRouteContext<'_>,
) -> TopicRouteDecision {
    if let Some(topic_config) = topic {
        let mention_satisfied = if topic_config.require_mention {
            context.reply_to_bot
                || contains_mention(context.text)
                || contains_mention(context.caption)
        } else {
            true
        };

        return TopicRouteDecision {
            enabled: topic_config.enabled,
            require_mention: topic_config.require_mention,
            mention_satisfied,
            system_prompt_override: topic_config.system_prompt.clone(),
            agent_id: topic_config.agent_id.clone(),
        };
    }

    TopicRouteDecision {
        enabled: true,
        require_mention: false,
        mention_satisfied: true,
        system_prompt_override: None,
        agent_id: None,
    }
}

fn is_reply_to_bot(message: &Message) -> bool {
    message
        .reply_to_message()
        .and_then(|reply| reply.from.as_ref())
        .is_some_and(|user| user.is_bot)
}

fn contains_mention(value: Option<&str>) -> bool {
    value.is_some_and(|text| text.contains('@'))
}

#[cfg(test)]
mod tests {
    use super::{resolve_topic_route_decision, TopicRouteContext};
    use crate::config::TelegramTopicSettings;

    fn topic(
        enabled: bool,
        require_mention: bool,
        system_prompt: Option<&str>,
        agent_id: Option<&str>,
    ) -> TelegramTopicSettings {
        TelegramTopicSettings {
            chat_id: -10001,
            thread_id: Some(42),
            agent_id: agent_id.map(str::to_string),
            enabled,
            require_mention,
            skills: Vec::new(),
            system_prompt: system_prompt.map(str::to_string),
        }
    }

    #[test]
    fn route_defaults_to_enabled_without_topic() {
        let context = TopicRouteContext {
            text: None,
            caption: None,
            reply_to_bot: false,
        };
        let decision = resolve_topic_route_decision(None, &context);

        assert!(decision.allows_processing());
        assert_eq!(decision.system_prompt_override, None);
        assert_eq!(decision.agent_id, None);
    }

    #[test]
    fn route_blocks_when_topic_disabled() {
        let context = TopicRouteContext {
            text: Some("hello"),
            caption: None,
            reply_to_bot: false,
        };
        let cfg = topic(false, false, None, Some("support-agent"));
        let decision = resolve_topic_route_decision(Some(&cfg), &context);

        assert!(!decision.allows_processing());
        assert_eq!(decision.agent_id.as_deref(), Some("support-agent"));
    }

    #[test]
    fn route_requires_mention_and_accepts_text_mention() {
        let context = TopicRouteContext {
            text: Some("ping @oxide"),
            caption: None,
            reply_to_bot: false,
        };
        let cfg = topic(true, true, Some("topic prompt"), None);
        let decision = resolve_topic_route_decision(Some(&cfg), &context);

        assert!(decision.allows_processing());
        assert_eq!(
            decision.system_prompt_override.as_deref(),
            Some("topic prompt")
        );
    }

    #[test]
    fn route_requires_mention_and_accepts_reply_to_bot() {
        let context = TopicRouteContext {
            text: Some("no mention here"),
            caption: None,
            reply_to_bot: true,
        };
        let cfg = topic(true, true, None, None);
        let decision = resolve_topic_route_decision(Some(&cfg), &context);

        assert!(decision.allows_processing());
    }

    #[test]
    fn route_requires_mention_and_blocks_without_signal() {
        let context = TopicRouteContext {
            text: Some("no mention"),
            caption: Some("still no mention"),
            reply_to_bot: false,
        };
        let cfg = topic(true, true, None, None);
        let decision = resolve_topic_route_decision(Some(&cfg), &context);

        assert!(!decision.allows_processing());
    }
}
