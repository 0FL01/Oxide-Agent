pub(crate) mod backoff;
#[cfg(any(feature = "llm-groq", feature = "llm-mistral"))]
pub(crate) mod common;
pub(crate) mod history;
#[cfg(any(
    feature = "llm-chatgpt",
    feature = "llm-mistral",
    feature = "llm-zai",
    feature = "llm-nvidia",
    feature = "llm-opencode-go",
    feature = "llm-openrouter"
))]
pub mod http;
#[cfg(any(feature = "llm-groq", feature = "llm-mistral"))]
pub(crate) mod openai_compat;
