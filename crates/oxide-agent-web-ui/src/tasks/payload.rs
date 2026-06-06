use oxide_agent_web_contracts::PersistedTaskEvent;
use serde_json::Value;

/// Extract a string field from an event's payload.
pub(super) fn payload_str_event(event: &PersistedTaskEvent, key: &str) -> Option<String> {
    event
        .payload
        .get(key)
        .and_then(|v| v.as_str())
        .map(ToString::to_string)
}

pub(super) fn is_sub_agent_event(event: &PersistedTaskEvent) -> bool {
    event.payload.get("source").and_then(|value| value.as_str()) == Some("sub_agent")
}

/// Parse the nested JSON inside `output_preview` for ToolResult events.
/// The output_preview field contains a JSON string (the ToolOutput encode_model_content).
pub(super) fn parse_output_json(event: &PersistedTaskEvent) -> Option<Value> {
    let raw = event
        .payload
        .get("output_preview")
        .and_then(|v| v.as_str())?;
    serde_json::from_str::<Value>(raw).ok()
}

/// Extract a string field from a JSON value.
pub(super) fn field_str(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .map(ToString::to_string)
}

/// Extract an i64 field from a JSON value.
pub(super) fn field_i64(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(|v| v.as_i64())
}

pub(super) fn raw_output_preview(result: Option<&PersistedTaskEvent>) -> Option<Value> {
    result.and_then(|e| e.payload.get("output_preview").cloned())
}

pub(super) fn input_preview_json(call: Option<&PersistedTaskEvent>) -> Option<Value> {
    call.and_then(|event| payload_str_event(event, "input_preview"))
        .and_then(|input| serde_json::from_str::<Value>(&input).ok())
}

pub(super) fn input_preview_field_str(
    call: Option<&PersistedTaskEvent>,
    key: &str,
) -> Option<String> {
    input_preview_json(call)
        .and_then(|payload| payload.get(key).and_then(Value::as_str).map(String::from))
}

/// Extract text from a stream object (stdout/stderr) in the ToolOutput JSON.
/// Handles both `text` field and `head`/`tail` for truncated output.
pub(super) fn stream_text(output: &Value, stream_name: &str) -> Option<String> {
    let stream = output.get(stream_name)?;

    if stream
        .get("binary")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Some("[binary output hidden]".to_string());
    }

    if let Some(text) = stream.get("text").and_then(Value::as_str) {
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }

    let head = stream.get("head").and_then(Value::as_str);
    let tail = stream.get("tail").and_then(Value::as_str);

    match (head, tail) {
        (Some(h), Some(t)) => Some(format!("{h}\n...\n{t}")),
        (Some(h), None) => Some(h.to_string()),
        (None, Some(t)) => Some(t.to_string()),
        _ => None,
    }
}
