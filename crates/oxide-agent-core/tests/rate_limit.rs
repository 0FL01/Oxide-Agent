//! Integration tests for rate limit handling in LLM providers
//!
//! Tests verify that:
//! 1. 429 errors are properly detected and converted to RateLimit
//! 2. Retry-After headers are parsed correctly
//! 3. Provider-specific rate limit info is extracted (OpenRouter X-RateLimit-Reset, ZAI flush time)

use oxide_agent_core::llm::http::parse_retry_after;
use oxide_agent_core::llm::providers::openrouter::parse_openrouter_rate_limit;
use oxide_agent_core::llm::providers::parse_zai_flush_time;
use reqwest::header::HeaderMap;
use reqwest::header::RETRY_AFTER;

#[test]
fn parse_retry_after_seconds() {
    let mut headers = HeaderMap::new();
    headers.insert(
        RETRY_AFTER,
        "120".parse().expect("valid retry-after header"),
    );

    let wait_secs = parse_retry_after(&headers);
    assert_eq!(wait_secs, Some(120));
}

#[test]
fn parse_retry_after_http_date() {
    let mut headers = HeaderMap::new();
    // Future date: 1 hour from now
    let future_dt = chrono::Utc::now() + chrono::Duration::hours(1);
    let future_date = future_dt.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
    headers.insert(
        RETRY_AFTER,
        future_date.parse().expect("valid retry-after header"),
    );

    let wait_secs = parse_retry_after(&headers).expect("wait_secs should be Some for valid date");
    assert!(wait_secs >= 3500, "~1 hour"); // ~1 hour
}

#[test]
fn parse_retry_after_missing() {
    let headers = HeaderMap::new();
    let wait_secs = parse_retry_after(&headers);
    assert_eq!(wait_secs, None);
}

#[test]
fn parse_retry_after_invalid() {
    let mut headers = HeaderMap::new();
    headers.insert(
        RETRY_AFTER,
        "invalid".parse().expect("header parse should not panic"),
    );

    let wait_secs = parse_retry_after(&headers);
    assert_eq!(wait_secs, None);
}

// OpenRouter rate limit parsing tests
mod openrouter_rate_limit {
    use super::*;

    #[test]
    fn parse_openrouter_reset_from_error_body() {
        // Future timestamp (1 hour from now in milliseconds)
        let future_ms = (chrono::Utc::now().timestamp_millis() + 3_600_000).to_string();
        let body = format!(
            r#"{{
            "error": {{
                "message": "Rate limit exceeded",
                "code": 429,
                "metadata": {{
                    "headers": {{
                        "X-RateLimit-Reset": "{}"
                    }}
                }}
            }}
        }}"#,
            future_ms
        );

        let wait_secs = parse_openrouter_rate_limit(&body);
        // Should be positive (~3600 seconds)
        let wait_secs = wait_secs.expect("wait_secs should be Some for valid body");
        assert!(wait_secs >= 3500, "~1 hour"); // ~1 hour
    }

    #[test]
    fn parse_openrouter_rate_limit_invalid_body() {
        let body = "not valid json";
        let wait_secs = parse_openrouter_rate_limit(body);
        assert_eq!(wait_secs, None);
    }

    #[test]
    fn parse_openrouter_rate_limit_missing_reset() {
        let body = r#"{"error": {"message": "Rate limit exceeded", "code": 429}}"#;
        let wait_secs = parse_openrouter_rate_limit(body);
        assert_eq!(wait_secs, None);
    }
}

// ZAI rate limit parsing tests
mod zai_rate_limit {
    use super::*;

    #[test]
    fn parse_zai_flush_time_unix_timestamp() {
        // Future timestamp (5 minutes from now)
        let future_ts = (chrono::Utc::now().timestamp() + 300).to_string();
        let message = format!(
            "Usage limit reached. Your limit will reset at {}",
            future_ts
        );

        let wait_secs =
            parse_zai_flush_time(&message).expect("wait_secs should be Some for unix timestamp");
        assert!((wait_secs as i64 - 300).abs() < 5, "~300 seconds"); // ~300 seconds
    }

    #[test]
    fn parse_zai_flush_time_milliseconds() {
        // Future timestamp in milliseconds (5 minutes from now)
        let future_ms = (chrono::Utc::now().timestamp_millis() + 300_000).to_string();
        let message = format!(
            "Usage limit reached. Your limit will reset at {}",
            future_ms
        );

        let wait_secs = parse_zai_flush_time(&message);
        assert!(wait_secs.is_some());
    }

    #[test]
    fn parse_zai_flush_time_no_timestamp() {
        let message = "Rate limit exceeded. Please try again later.";
        let wait_secs = parse_zai_flush_time(message);
        assert_eq!(wait_secs, None);
    }

    #[test]
    fn parse_zai_flush_time_iso_datetime() {
        // Future ISO datetime (5 minutes from now)
        let future_dt = chrono::Utc::now() + chrono::Duration::minutes(5);
        let future_str = future_dt.format("%Y-%m-%dT%H:%M:%SZ").to_string();

        let message = format!(
            "Usage limit reached. Your limit will reset at {}",
            future_str
        );
        let wait_secs = parse_zai_flush_time(&message)
            .expect("wait_secs should be Some for valid ISO datetime");
        assert!(wait_secs >= 200, "~5 minutes"); // ~5 minutes
    }
}
