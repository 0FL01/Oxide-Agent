use axum::{
    Json,
    extract::{Multipart, State},
    http::{HeaderMap, StatusCode},
};
use oxide_agent_core::audio_stt::{AudioTranscriptionInput, build_audio_transcriber};
use oxide_agent_web_contracts::{ApiVoiceTranscriptionResponse, ErrorCode, ErrorEnvelope};

use super::{
    AppState, api_error, authenticated_user_with_csrf, backend_unavailable_response,
    web_voice_transcription_limit_mb,
};

pub(crate) async fn api_transcribe_voice(
    State(state): State<AppState>,
    headers: HeaderMap,
    multipart: Multipart,
) -> Result<Json<ApiVoiceTranscriptionResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    authenticated_user_with_csrf(&state, &headers).await?;
    transcribe_voice_upload(&state, multipart).await
}

async fn transcribe_voice_upload(
    state: &AppState,
    mut multipart: Multipart,
) -> Result<Json<ApiVoiceTranscriptionResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let upload = read_single_voice_upload(&mut multipart).await?;
    let audio_config = state.session_manager.agent_settings().audio_stt_config();
    let transcriber = build_audio_transcriber(&audio_config);
    let transcript = transcriber
        .transcribe(
            AudioTranscriptionInput::new(upload.bytes, upload.mime_type)
                .with_file_name(upload.file_name),
        )
        .await
        .map_err(|error| {
            backend_unavailable_response(format!("Voice transcription failed: {error}"))
        })?;

    Ok(Json(ApiVoiceTranscriptionResponse {
        text: transcript.text,
    }))
}

struct VoiceUpload {
    bytes: Vec<u8>,
    mime_type: String,
    file_name: String,
}

async fn read_single_voice_upload(
    multipart: &mut Multipart,
) -> Result<VoiceUpload, (StatusCode, Json<ErrorEnvelope>)> {
    let limit_mb = web_voice_transcription_limit_mb();
    let max_bytes = limit_mb.saturating_mul(1024 * 1024);
    let mut upload = None;

    while let Some(field) = multipart.next_field().await.map_err(|error| {
        api_error(
            StatusCode::BAD_REQUEST,
            ErrorCode::ValidationError,
            format!("Invalid multipart voice payload: {error}"),
            false,
        )
    })? {
        if field.name() != Some("audio") {
            continue;
        }
        if upload.is_some() {
            return Err(validation_error(
                "Exactly one audio field must be provided for voice transcription.",
            ));
        }

        let file_name = field
            .file_name()
            .filter(|value| !value.trim().is_empty())
            .map(ToString::to_string);
        let mime_type = normalized_voice_mime_type(field.content_type())?;
        let bytes = field.bytes().await.map_err(|error| {
            api_error(
                StatusCode::BAD_REQUEST,
                ErrorCode::ValidationError,
                format!("Failed to read uploaded voice bytes: {error}"),
                false,
            )
        })?;
        if bytes.is_empty() {
            return Err(validation_error("Voice recording must not be empty."));
        }
        if bytes.len() as u64 > max_bytes {
            return Err(validation_error(format!(
                "Voice recording size must be at most {limit_mb} MB."
            )));
        }

        upload = Some(VoiceUpload {
            file_name: file_name.unwrap_or_else(|| default_voice_upload_file_name(&mime_type)),
            mime_type,
            bytes: bytes.to_vec(),
        });
    }

    upload.ok_or_else(|| validation_error("A multipart audio field is required."))
}

fn normalized_voice_mime_type(
    raw: Option<&str>,
) -> Result<String, (StatusCode, Json<ErrorEnvelope>)> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Err(validation_error("Voice recording MIME type is required."));
    };
    let base = raw
        .split(';')
        .next()
        .unwrap_or(raw)
        .trim()
        .to_ascii_lowercase();
    if supported_voice_mime_type(&base) {
        Ok(base)
    } else {
        Err(validation_error(format!(
            "Unsupported voice recording MIME type: {raw}."
        )))
    }
}

fn supported_voice_mime_type(mime_type: &str) -> bool {
    matches!(
        mime_type,
        "audio/webm"
            | "audio/ogg"
            | "audio/opus"
            | "audio/mp4"
            | "audio/x-m4a"
            | "audio/mpeg"
            | "audio/mp3"
            | "audio/wav"
            | "audio/x-wav"
            | "audio/flac"
    )
}

fn default_voice_upload_file_name(mime_type: &str) -> String {
    format!("web-voice.{}", voice_extension_from_mime_type(mime_type))
}

fn voice_extension_from_mime_type(mime_type: &str) -> &'static str {
    match mime_type {
        "audio/webm" => "webm",
        "audio/ogg" | "audio/opus" => "ogg",
        "audio/mp4" | "audio/x-m4a" => "m4a",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/flac" => "flac",
        _ => "bin",
    }
}

fn validation_error(message: impl Into<String>) -> (StatusCode, Json<ErrorEnvelope>) {
    api_error(
        StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::ValidationError,
        message.into(),
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::{default_voice_upload_file_name, normalized_voice_mime_type};

    #[test]
    fn normalizes_browser_codec_mime_to_supported_base_type() {
        assert_eq!(
            normalized_voice_mime_type(Some("audio/webm;codecs=opus"))
                .expect("webm opus should be supported"),
            "audio/webm"
        );
    }

    #[test]
    fn rejects_non_audio_mime_types() {
        assert!(normalized_voice_mime_type(Some("application/octet-stream")).is_err());
    }

    #[test]
    fn derives_stable_upload_file_extension_from_mime_type() {
        assert_eq!(
            default_voice_upload_file_name("audio/webm"),
            "web-voice.webm"
        );
        assert_eq!(default_voice_upload_file_name("audio/mp4"), "web-voice.m4a");
    }
}
