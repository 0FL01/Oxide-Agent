mod chat;
mod client;
mod history;
mod media;
mod response;
mod tools;

#[cfg(test)]
mod tests;

/// LLM provider implementation for Google Gemini.
pub struct GeminiProvider {
    pub(super) api_key: String,
}

impl GeminiProvider {
    /// Create a new Gemini provider instance.
    #[must_use]
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }
}
