use super::response::format_http_error;
use super::response::is_retryable_error;
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

    let md: Result<WebMarkdownArgs, _> = serde_json::from_str(r#"{"url":"https://example.com"}"#);
    assert!(md.is_ok());

    let pdf: Result<WebPdfArgs, _> = serde_json::from_str(r#"{"url":"https://example.com"}"#);
    assert!(pdf.is_ok());
}

#[test]
fn test_url_building() {
    let provider = Crawl4aiProvider::with_config(
        "http://localhost:11235/",
        Duration::from_secs(1),
        0,
        Duration::from_secs(1),
        Duration::from_secs(10),
    );
    let url = provider.endpoint_url("/crawl");
    assert_eq!(url, "http://localhost:11235/crawl");
}

#[test]
fn test_http_error_formatting() {
    let msg = format_http_error(StatusCode::BAD_REQUEST, "bad request");
    assert!(msg.contains("400"));
    assert!(msg.contains("bad request"));
}

#[test]
fn test_media_url_detection() {
    assert!(Crawl4aiProvider::is_media_url(
        "https://github.com/Tarquinen/oc-tps/blob/main/assets/demo.gif"
    ));
    assert!(Crawl4aiProvider::is_media_url(
        "https://example.com/image.png"
    ));
    assert!(!Crawl4aiProvider::is_media_url("https://example.com/page"));
}

#[test]
fn test_media_url_rejection_message() {
    let error = Crawl4aiProvider::reject_media_url(
        "https://github.com/Tarquinen/oc-tps/blob/main/assets/demo.gif",
        "web_markdown",
    )
    .expect_err("media url must be rejected")
    .to_string();

    assert!(error.contains("describe_image_file"));
    assert!(error.contains("web_markdown"));
}

#[test]
fn test_err_failed_is_not_retryable() {
    let error = "500: Unexpected error in _crawl_web at line 778 in _crawl_web: Error: Failed on navigating ACS-GOTO:\nPage.goto: net::ERR_FAILED at https://github.com/Tarquinen/oc-tps/blob/main/assets/demo.gif";
    assert!(!is_retryable_error(error));
}
