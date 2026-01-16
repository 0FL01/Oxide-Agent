use super::response::format_http_error;
use super::*;
use reqwest::StatusCode;
use std::time::Duration;

#[test]
fn test_args_deserialize() {
    let crawl: Result<DeepCrawlArgs, _> =
        serde_json::from_str(r#"{"urls":["https://example.com"],"max_depth":2}"#);
    assert!(crawl.is_ok());

    if let Ok(args) = crawl {
        assert_eq!(args.urls.len(), 1);
        assert_eq!(args.max_depth, Some(2));
    }

    let md: Result<WebMarkdownArgs, _> =
        serde_json::from_str(r#"{"url":"https://example.com"}"#);
    assert!(md.is_ok());

    let pdf: Result<WebPdfArgs, _> =
        serde_json::from_str(r#"{"url":"https://example.com"}"#);
    assert!(pdf.is_ok());
}

#[test]
fn test_url_building() {
    let provider = Crawl4aiProvider::with_timeout("http://localhost:11235/", Duration::from_secs(1));
    let url = provider.endpoint_url("/crawl");
    assert_eq!(url, "http://localhost:11235/crawl");
}

#[test]
fn test_http_error_formatting() {
    let msg = format_http_error(StatusCode::BAD_REQUEST, "bad request");
    assert!(msg.contains("400"));
    assert!(msg.contains("bad request"));
}
