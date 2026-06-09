#[allow(missing_docs)]
#[cfg(feature = "llm-chatgpt")]
pub mod chatgpt;
#[cfg(feature = "llm-minimax")]
pub mod minimax;
#[allow(missing_docs)]
#[cfg(feature = "llm-mistral")]
pub mod mistral;
pub(crate) mod modules;
#[allow(missing_docs)]
#[cfg(feature = "llm-nvidia")]
pub mod nvidia;
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
    feature = "llm-zai",
    feature = "llm-nvidia",
    feature = "llm-opencode-go",
    feature = "llm-openrouter"
))]
#[allow(dead_code)]
mod protocol_profiles;
#[cfg(any(
    feature = "llm-chatgpt",
    feature = "llm-mistral",
    feature = "llm-minimax",
    feature = "llm-zai",
    feature = "llm-nvidia",
    feature = "llm-opencode-go",
    feature = "llm-openrouter"
))]
#[allow(dead_code)]
mod tool_call_adapter;
#[cfg(any(
    feature = "llm-chatgpt",
    feature = "llm-mistral",
    feature = "llm-minimax",
    feature = "llm-zai",
    feature = "llm-nvidia",
    feature = "llm-opencode-go",
    feature = "llm-openrouter"
))]
#[allow(dead_code)]
mod tool_call_encoder;
#[cfg(any(
    feature = "llm-chatgpt",
    feature = "llm-mistral",
    feature = "llm-minimax",
    feature = "llm-zai",
    feature = "llm-nvidia",
    feature = "llm-opencode-go",
    feature = "llm-openrouter"
))]
#[allow(dead_code)]
mod tool_correlation;
#[cfg(any(
    feature = "llm-chatgpt",
    feature = "llm-mistral",
    feature = "llm-minimax",
    feature = "llm-zai",
    feature = "llm-nvidia",
    feature = "llm-opencode-go",
    feature = "llm-openrouter"
))]
#[allow(dead_code)]
mod tool_result_encoder;
#[allow(missing_docs)]
#[cfg(feature = "llm-zai")]
pub mod zai;

#[cfg(feature = "llm-chatgpt")]
pub use chatgpt::ChatGptProvider;
#[cfg(feature = "llm-minimax")]
pub use minimax::MiniMaxProvider;
#[cfg(feature = "llm-mistral")]
pub use mistral::MistralProvider;
#[cfg(feature = "llm-nvidia")]
pub use nvidia::NvidiaProvider;
#[cfg(feature = "llm-opencode-go")]
pub use opencode_go::OpenCodeGoProvider;
#[cfg(feature = "llm-openrouter")]
pub use openrouter::OpenRouterProvider;
#[cfg(feature = "llm-zai")]
pub use zai::{ZaiProvider, parse_zai_flush_time};

pub(crate) use modules::{
    build_configured_providers, provider_capabilities, provider_capabilities_for_model,
    provider_key, provider_media_capabilities, provider_media_capabilities_for_model,
    provider_missing_route_config_message, provider_module_id,
};
