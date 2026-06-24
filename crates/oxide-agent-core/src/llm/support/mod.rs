pub(crate) mod backoff;
pub(crate) mod history;
#[cfg(any(
    oxide_module_llm_provider_openai_chatgpt,
    oxide_module_llm_provider_anthropic,
    oxide_module_llm_provider_openai_base,
    oxide_module_llm_provider_opencode_go,
    oxide_module_llm_provider_openrouter
))]
pub mod http;
#[cfg(any(
    oxide_module_llm_provider_openai_base,
    oxide_module_llm_provider_opencode_go,
    oxide_module_llm_provider_openrouter
))]
pub(crate) mod media;
#[cfg(any(
    oxide_module_llm_provider_openai_chatgpt,
    oxide_module_llm_provider_openai_base,
    oxide_module_llm_provider_opencode_go,
    oxide_module_llm_provider_openrouter
))]
pub(crate) mod sse;
