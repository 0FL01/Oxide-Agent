use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// User-specific configuration persisted in storage.
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct UserConfig {
    /// Custom system prompt.
    pub system_prompt: Option<String>,
    /// Selected LLM model name.
    pub model_name: Option<String>,
    /// Current dialogue state.
    pub state: Option<String>,
    /// Active chat UUID for chat mode context isolation.
    pub current_chat_uuid: Option<String>,
    /// Context-scoped runtime metadata keyed by transport peer/thread identifier.
    #[serde(default)]
    pub contexts: HashMap<String, UserContextConfig>,
}

/// Context-scoped user configuration persisted alongside the global config.
#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq, Eq)]
pub struct UserContextConfig {
    /// Persisted dialogue state for this specific transport context.
    pub state: Option<String>,
    /// Active chat UUID for this specific transport context.
    pub current_chat_uuid: Option<String>,
    /// Active agent flow UUID for this specific transport context.
    #[serde(default)]
    pub current_agent_flow_id: Option<String>,
    /// Optional transport chat identifier associated with the context.
    #[serde(default)]
    pub chat_id: Option<i64>,
    /// Optional transport thread/topic identifier associated with the context.
    #[serde(default)]
    pub thread_id: Option<i64>,
    /// Persisted Telegram forum topic title for manager topic discovery.
    #[serde(default)]
    pub forum_topic_name: Option<String>,
    /// Persisted Telegram forum topic icon color.
    #[serde(default)]
    pub forum_topic_icon_color: Option<u32>,
    /// Persisted Telegram forum topic custom emoji identifier.
    #[serde(default)]
    pub forum_topic_icon_custom_emoji_id: Option<String>,
    /// Whether the persisted Telegram forum topic is currently closed.
    #[serde(default)]
    pub forum_topic_closed: bool,
}

/// A message in the chat history.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Message {
    /// Role of the message sender (user or assistant).
    pub role: String,
    /// Text content of the message.
    pub content: String,
}
