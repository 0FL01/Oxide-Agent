#[cfg(oxide_module_llm_provider_anthropic)]
pub mod anthropic;
#[cfg(any(
    oxide_module_llm_provider_openai_chatgpt,
    oxide_module_llm_provider_mistral,
    oxide_module_llm_provider_anthropic,
    oxide_module_llm_provider_openai_base,
    oxide_module_llm_provider_opencode_go,
    oxide_module_llm_provider_openrouter
))]
#[allow(unused_imports, dead_code)]
pub(crate) mod anthropic_messages;
#[cfg(any(
    oxide_module_llm_provider_mistral,
    oxide_module_llm_provider_openai_base,
    oxide_module_llm_provider_opencode_go,
    oxide_module_llm_provider_openrouter
))]
pub(crate) mod chat_completions;
#[allow(missing_docs)]
#[cfg(oxide_module_llm_provider_openai_chatgpt)]
pub mod chatgpt;
#[cfg(any(
    oxide_module_llm_provider_openai_chatgpt,
    oxide_module_llm_provider_mistral,
    oxide_module_llm_provider_anthropic,
    oxide_module_llm_provider_openai_base,
    oxide_module_llm_provider_opencode_go,
    oxide_module_llm_provider_openrouter
))]
pub(crate) mod messages;
pub(crate) mod modules;
#[allow(missing_docs)]
#[cfg(oxide_module_llm_provider_openai_base)]
pub mod openai_base;
#[allow(missing_docs)]
#[cfg(oxide_module_llm_provider_opencode_go)]
pub mod opencode_go;
#[allow(missing_docs)]
#[cfg(oxide_module_llm_provider_openrouter)]
pub mod openrouter;
#[cfg(any(
    oxide_module_llm_provider_openai_chatgpt,
    oxide_module_llm_provider_mistral,
    oxide_module_llm_provider_anthropic,
    oxide_module_llm_provider_openai_base,
    oxide_module_llm_provider_opencode_go,
    oxide_module_llm_provider_openrouter
))]
#[allow(dead_code)]
mod protocol_profiles;
#[cfg(any(
    oxide_module_llm_provider_openai_chatgpt,
    oxide_module_llm_provider_mistral,
    oxide_module_llm_provider_anthropic,
    oxide_module_llm_provider_openai_base,
    oxide_module_llm_provider_opencode_go,
    oxide_module_llm_provider_openrouter
))]
#[allow(dead_code)]
mod tool_call_adapter;
#[cfg(any(
    oxide_module_llm_provider_openai_chatgpt,
    oxide_module_llm_provider_mistral,
    oxide_module_llm_provider_anthropic,
    oxide_module_llm_provider_openai_base,
    oxide_module_llm_provider_opencode_go,
    oxide_module_llm_provider_openrouter
))]
#[allow(dead_code)]
mod tool_call_encoder;
#[cfg(any(
    oxide_module_llm_provider_openai_chatgpt,
    oxide_module_llm_provider_mistral,
    oxide_module_llm_provider_anthropic,
    oxide_module_llm_provider_openai_base,
    oxide_module_llm_provider_opencode_go,
    oxide_module_llm_provider_openrouter
))]
#[allow(dead_code)]
mod tool_correlation;
#[cfg(any(
    oxide_module_llm_provider_openai_chatgpt,
    oxide_module_llm_provider_mistral,
    oxide_module_llm_provider_anthropic,
    oxide_module_llm_provider_openai_base,
    oxide_module_llm_provider_opencode_go,
    oxide_module_llm_provider_openrouter
))]
#[allow(dead_code)]
mod tool_result_encoder;
#[cfg(oxide_module_llm_provider_anthropic)]
pub use anthropic::AnthropicProvider;
#[cfg(oxide_module_llm_provider_openai_chatgpt)]
pub use chatgpt::ChatGptProvider;
#[cfg(oxide_module_llm_provider_openai_base)]
pub use openai_base::OpenAIBaseProvider;
#[cfg(oxide_module_llm_provider_opencode_go)]
pub use opencode_go::OpenCodeGoProvider;
#[cfg(oxide_module_llm_provider_openrouter)]
pub use openrouter::OpenRouterProvider;

pub(crate) use modules::{
    build_configured_providers, canonical_route_provider, provider_capabilities,
    provider_capabilities_for_model, provider_key, provider_media_capabilities,
    provider_media_capabilities_for_model, provider_missing_route_config_message,
    provider_module_id,
};
