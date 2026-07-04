use serde::{Deserialize, Serialize};

/// Response returned after server-side voice transcription.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiVoiceTranscriptionResponse {
    /// Plain transcript text produced by the configured speech-to-text backend.
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::ApiVoiceTranscriptionResponse;

    #[test]
    fn voice_transcription_response_serializes_text() {
        let value = serde_json::to_value(ApiVoiceTranscriptionResponse {
            text: "hello from voice".to_string(),
        })
        .expect("voice transcription response should serialize");

        assert_eq!(value["text"], "hello from voice");
    }
}
