/// Agent-specific bot logic (media extraction)
pub mod agent;
/// Handler for agent-related Telegram events
pub mod agent_handlers;
/// Telegram transport adapter for the agent runtime
pub mod agent_transport;
/// General command and message handlers
pub mod handlers;
/// Common messaging utilities (split long messages, formatting)
pub mod messaging;
/// Progress rendering for UI outputs
pub mod progress_render;
/// Resilient messaging with automatic retry for Telegram API operations
pub mod resilient;
/// User state and dialogue management
pub mod state;
/// Telegram thread/topic helper layer.
pub mod thread;
/// Unauthorized access flood protection
pub mod unauthorized_cache;
/// View layer for UI components (keyboards, messages)
pub mod views;

pub use thread::{
    build_outbound_thread_params, general_forum_topic_id, resolve_thread_spec,
    resolve_thread_spec_from_context, thread_peer_key, thread_peer_key_from_spec,
    OutboundThreadParams, TelegramThreadKind, TelegramThreadSpec,
};
pub use unauthorized_cache::UnauthorizedCache;
