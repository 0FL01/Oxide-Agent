//! Low-level Server-Sent Events helpers.
//!
//! These helpers intentionally know nothing about provider event schemas.

use crate::llm::LlmError;

pub(crate) fn decode_utf8_prefix(
    pending_bytes: &mut Vec<u8>,
    error_context: &str,
) -> Result<Option<String>, LlmError> {
    match std::str::from_utf8(pending_bytes) {
        Ok(valid) => {
            let decoded = valid.to_string();
            pending_bytes.clear();
            Ok((!decoded.is_empty()).then_some(decoded))
        }
        Err(error) => {
            let valid_up_to = error.valid_up_to();
            if let Some(error_len) = error.error_len() {
                return Err(LlmError::JsonError(format!(
                    "invalid utf-8 in {error_context} at {valid_up_to} (len {error_len})"
                )));
            }

            if valid_up_to == 0 {
                return Ok(None);
            }

            let decoded = String::from_utf8(pending_bytes[..valid_up_to].to_vec())
                .map_err(|error| LlmError::JsonError(error.to_string()))?;
            pending_bytes.drain(..valid_up_to);
            Ok(Some(decoded))
        }
    }
}

pub(crate) fn normalize_newlines_in_place(buffer: &mut String) {
    if buffer.contains('\r') {
        *buffer = buffer.replace("\r\n", "\n");
    }
}

#[must_use]
pub(crate) fn data_payload(raw_event: &str) -> String {
    raw_event
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_sse_decodes_utf8_prefix_without_losing_tail() {
        let mut bytes = vec![b'o', b'k', 0xE2, 0x82];
        assert_eq!(
            decode_utf8_prefix(&mut bytes, "test stream").expect("prefix decodes"),
            Some("ok".to_string())
        );
        assert_eq!(bytes, vec![0xE2, 0x82]);

        bytes.push(0xAC);
        assert_eq!(
            decode_utf8_prefix(&mut bytes, "test stream").expect("tail decodes"),
            Some("€".to_string())
        );
        assert!(bytes.is_empty());
    }

    #[test]
    fn support_sse_normalizes_crlf_boundaries() {
        let mut buffer = "data: one\r\n\r\ndata: two\n\n".to_string();
        normalize_newlines_in_place(&mut buffer);
        assert_eq!(buffer, "data: one\n\ndata: two\n\n");
    }

    #[test]
    fn support_sse_extracts_data_lines_without_schema_assumptions() {
        let raw = "event: message\ndata: {\"a\":1}\ndata: {\"b\":2}\nid: 1";
        assert_eq!(data_payload(raw), "{\"a\":1}\n{\"b\":2}");
    }
}
