//! Image analysis for Mistral provider
//! 
//! NOTE: Mistral's vision capabilities are currently limited and produce
//! low-quality results compared to other providers (Gemini, OpenRouter).
//! Implementation is not recommended at this time.
//! 
//! When Mistral improves their vision models, this can be implemented
//! using the Vision API endpoint similar to OpenRouter's approach.

use crate::llm::LlmError;

/// Analyze image using Mistral Vision API
/// 
/// Currently not implemented due to poor quality of Mistral vision models.
/// Use Gemini or OpenRouter for image analysis instead.
pub async fn analyze_image(
    _image_bytes: Vec<u8>,
    _text_prompt: &str,
    _system_prompt: &str,
    _model_id: &str,
) -> Result<String, LlmError> {
    Err(LlmError::Unknown("Not implemented for Mistral".to_string()))
}
