pub(crate) mod backoff;
pub(crate) mod history;
#[cfg(any(
    feature = "llm-chatgpt",
    feature = "llm-mistral",
    feature = "llm-zai",
    feature = "llm-openai-base",
    feature = "llm-opencode-go",
    feature = "llm-openrouter"
))]
pub mod http;
