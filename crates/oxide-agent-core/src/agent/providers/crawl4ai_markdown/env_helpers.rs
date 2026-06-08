//! Environment-variable helpers and output truncation utilities.

use std::time::Duration;

use reqwest::Url;

pub(crate) fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn env_url(name: &str, default: &str) -> Url {
    let raw = env_non_empty(name).unwrap_or_else(|| default.to_string());
    Url::parse(&raw)
        .unwrap_or_else(|_| Url::parse(default).expect("valid default Crawl4AI base URL"))
}

pub(crate) fn env_u64(name: &str, default: u64) -> u64 {
    env_non_empty(name)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

pub(crate) fn env_usize(name: &str, default: usize) -> usize {
    env_non_empty(name)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

pub(crate) fn env_bool(name: &str, default: bool) -> bool {
    env_non_empty(name)
        .map(|value| matches!(value.to_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(default)
}

pub(crate) struct TruncatedOutput {
    pub(crate) text: String,
    pub(crate) was_truncated: bool,
}

pub(crate) fn truncate_chars(input: String, max_chars: usize) -> TruncatedOutput {
    if input.chars().count() <= max_chars {
        return TruncatedOutput {
            text: input,
            was_truncated: false,
        };
    }

    let mut text = input.chars().take(max_chars).collect::<String>();
    text.push_str("\n\n... (truncated)");
    TruncatedOutput {
        text,
        was_truncated: true,
    }
}

pub(crate) fn truncate_for_message(input: &str, max_chars: usize) -> String {
    truncate_chars(input.to_string(), max_chars).text
}

pub(crate) fn response_tail(body: &[u8], max_chars: usize) -> String {
    let text = String::from_utf8_lossy(body);
    let total_chars = text.chars().count();
    if total_chars <= max_chars {
        return text.into_owned();
    }
    text.chars()
        .skip(total_chars.saturating_sub(max_chars))
        .collect()
}

pub(crate) fn millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
