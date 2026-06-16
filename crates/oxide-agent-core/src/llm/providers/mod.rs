#[cfg(feature = "llm-minimax")]
pub mod anthropic;
#[cfg(any(
    feature = "llm-chatgpt",
    feature = "llm-mistral",
    feature = "llm-minimax",
    feature = "llm-openai-base",
    feature = "llm-opencode-go",
    feature = "llm-openrouter"
))]
#[allow(unused_imports, dead_code)]
pub(crate) mod anthropic_messages;
#[cfg(any(
    feature = "llm-mistral",
    feature = "llm-openai-base",
    feature = "llm-opencode-go",
    feature = "llm-openrouter"
))]
pub(crate) mod chat_completions;
#[allow(missing_docs)]
#[cfg(feature = "llm-chatgpt")]
pub mod chatgpt;
#[cfg(any(
    feature = "llm-chatgpt",
    feature = "llm-mistral",
    feature = "llm-minimax",
    feature = "llm-openai-base",
    feature = "llm-opencode-go",
    feature = "llm-openrouter"
))]
pub(crate) mod messages;
pub(crate) mod modules;
#[allow(missing_docs)]
#[cfg(feature = "llm-openai-base")]
pub mod openai_base;
#[allow(missing_docs)]
#[cfg(feature = "llm-opencode-go")]
pub mod opencode_go;
#[allow(missing_docs)]
#[cfg(feature = "llm-openrouter")]
pub mod openrouter;
#[cfg(any(
    feature = "llm-chatgpt",
    feature = "llm-mistral",
    feature = "llm-minimax",
    feature = "llm-openai-base",
    feature = "llm-opencode-go",
    feature = "llm-openrouter"
))]
#[allow(dead_code)]
mod protocol_profiles;
#[cfg(any(
    feature = "llm-chatgpt",
    feature = "llm-mistral",
    feature = "llm-minimax",
    feature = "llm-openai-base",
    feature = "llm-opencode-go",
    feature = "llm-openrouter"
))]
#[allow(dead_code)]
mod tool_call_adapter;
#[cfg(any(
    feature = "llm-chatgpt",
    feature = "llm-mistral",
    feature = "llm-minimax",
    feature = "llm-openai-base",
    feature = "llm-opencode-go",
    feature = "llm-openrouter"
))]
#[allow(dead_code)]
mod tool_call_encoder;
#[cfg(any(
    feature = "llm-chatgpt",
    feature = "llm-mistral",
    feature = "llm-minimax",
    feature = "llm-openai-base",
    feature = "llm-opencode-go",
    feature = "llm-openrouter"
))]
#[allow(dead_code)]
mod tool_correlation;
#[cfg(any(
    feature = "llm-chatgpt",
    feature = "llm-mistral",
    feature = "llm-minimax",
    feature = "llm-openai-base",
    feature = "llm-opencode-go",
    feature = "llm-openrouter"
))]
#[allow(dead_code)]
mod tool_result_encoder;
#[cfg(feature = "llm-minimax")]
pub use anthropic::AnthropicProvider;
#[cfg(feature = "llm-chatgpt")]
pub use chatgpt::ChatGptProvider;
#[cfg(feature = "llm-openai-base")]
pub use openai_base::OpenAIBaseProvider;
#[cfg(feature = "llm-opencode-go")]
pub use opencode_go::OpenCodeGoProvider;
#[cfg(feature = "llm-openrouter")]
pub use openrouter::OpenRouterProvider;

pub(crate) use modules::{
    build_configured_providers, canonical_route_provider, provider_capabilities,
    provider_capabilities_for_model, provider_key, provider_media_capabilities,
    provider_media_capabilities_for_model, provider_missing_route_config_message,
    provider_module_id,
};
