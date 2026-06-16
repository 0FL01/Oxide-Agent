pub(crate) mod backoff;
pub(crate) mod history;
#[cfg(any(
    feature = "llm-chatgpt",
    feature = "llm-minimax",
    feature = "llm-mistral",
    feature = "llm-openai-base",
    feature = "llm-opencode-go",
    feature = "llm-openrouter"
))]
pub mod http;
#[cfg(any(
    feature = "llm-mistral",
    feature = "llm-openai-base",
    feature = "llm-opencode-go",
    feature = "llm-openrouter"
))]
pub(crate) mod media;
#[cfg(any(
    feature = "llm-chatgpt",
    feature = "llm-mistral",
    feature = "llm-openai-base",
    feature = "llm-opencode-go",
    feature = "llm-openrouter"
))]
pub(crate) mod sse;
