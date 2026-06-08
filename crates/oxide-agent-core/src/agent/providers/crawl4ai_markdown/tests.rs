use super::reddit_rss::{reddit_atom_to_crawl_result, reddit_thread_rss_url};
use super::*;

use crate::agent::identity::SessionId;
use crate::agent::tool_runtime::{
    ModelMetadata, ProviderMetadata, ToolBatchId, ToolCallId, ToolExecutionContext, ToolInvocation,
    ToolName, ToolOutputStatus, ToolTimeoutConfig, TurnId,
};
use crate::llm::InvocationId;
use anyhow::anyhow;
use chrono::Utc;
use reqwest::Url;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

const PUBLIC_TEST_URL: &str = "http://93.184.216.34/article";

#[derive(Clone)]
struct MockResponse {
    status: u16,
    body: &'static str,
}

#[derive(Debug)]
struct ObservedRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: String,
}

fn runtime_invocation(raw_arguments: &str) -> ToolInvocation {
    let now = Utc::now();
    ToolInvocation {
        session_id: SessionId::from(77),
        turn_id: TurnId::from("turn-crawl4ai-markdown"),
        batch_id: ToolBatchId::from("batch-crawl4ai-markdown"),
        batch_index: 0,
        invocation_id: InvocationId::from("invoke-crawl4ai-markdown"),
        tool_call_id: ToolCallId::from("call-crawl4ai-markdown"),
        provider_tool_call_id: None,
        tool_name: ToolName::from(TOOL_CRAWL4AI_MARKDOWN),
        raw_provider_payload: json!({}),
        raw_arguments: raw_arguments.to_string(),
        normalized_arguments: serde_json::Value::Null,
        cancellation_token: CancellationToken::new(),
        timeout: ToolTimeoutConfig::default(),
        execution_context: ToolExecutionContext::new(std::env::temp_dir()),
        provider_metadata: ProviderMetadata {
            provider: "test".to_string(),
            protocol: "chat_like".to_string(),
        },
        model_metadata: ModelMetadata {
            model: "test-model".to_string(),
        },
        working_directory: None,
        environment_metadata: None,
        created_at: now,
        started_at: Some(now),
    }
}

fn test_config(base_url: Url) -> Crawl4AiMarkdownConfig {
    Crawl4AiMarkdownConfig {
        base_url,
        api_token: Some("test-token".to_string()),
        default_timeout_secs: 5,
        max_timeout_secs: 10,
        max_output_chars: DEFAULT_MAX_OUTPUT_CHARS,
        health_timeout_ms: 1_000,
        jitter_min_ms: 0,
        jitter_max_ms: 0,
        max_retries: 0,
        text_mode: true,
        light_mode: true,
        avoid_ads: true,
    }
}

async fn serve_crawl4ai_sequence(
    responses: Vec<MockResponse>,
) -> (SocketAddr, Arc<Mutex<Vec<ObservedRequest>>>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local test server");
    let addr = listener.local_addr().expect("local address");
    let observed = Arc::new(Mutex::new(Vec::new()));
    let observed_for_task = Arc::clone(&observed);

    tokio::spawn(async move {
        for response in responses {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            let request = read_http_request(&mut stream).await;
            observed_for_task
                .lock()
                .expect("observed request lock")
                .push(request);
            let status_text = match response.status {
                200 => "OK",
                429 => "Too Many Requests",
                500 => "Internal Server Error",
                503 => "Service Unavailable",
                _ => "Error",
            };
            let raw_response = format!(
                "HTTP/1.1 {} {status_text}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.status,
                response.body.len(),
                response.body
            );
            stream
                .write_all(raw_response.as_bytes())
                .await
                .expect("write response");
        }
    });

    (addr, observed)
}

async fn read_http_request(stream: &mut tokio::net::TcpStream) -> ObservedRequest {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    let header_len = loop {
        let read = stream.read(&mut buffer).await.expect("read request");
        if read == 0 {
            break request.len();
        }
        request.extend_from_slice(&buffer[..read]);
        if let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break header_end + 4;
        }
    };

    let headers_raw = String::from_utf8_lossy(&request[..header_len]);
    let mut lines = headers_raw.lines();
    let request_line = lines.next().expect("request line");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let headers: HashMap<String, String> = lines
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.to_ascii_lowercase(), value.trim().to_string()))
        })
        .collect();
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    while request.len().saturating_sub(header_len) < content_length {
        let read = stream.read(&mut buffer).await.expect("read request body");
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
    }

    let body = String::from_utf8_lossy(&request[header_len..]).to_string();
    ObservedRequest {
        method,
        path,
        headers,
        body,
    }
}

#[test]
fn tool_definition_is_static_and_bounded() {
    let spec = Crawl4AiMarkdownProvider::tool_definition();

    assert_eq!(spec.name, TOOL_CRAWL4AI_MARKDOWN);
    assert!(
        spec.description
            .contains("configured Crawl4AI REST service")
    );
    assert!(
        spec.description
            .contains("Use after selecting specific URLs from brave_search or searxng_search")
    );
    assert!(
        spec.description
            .contains("Do not crawl every search result")
    );
    assert!(
        spec.description
            .contains("For Reddit thread URLs, omit max_chars or use 15000-30000")
    );
    assert_eq!(spec.parameters["required"], json!(["url"]));
    assert_eq!(spec.parameters["additionalProperties"], json!(false));
    assert!(spec.parameters["properties"].get("headers").is_none());
    assert!(spec.parameters["properties"].get("js").is_none());
    assert!(spec.parameters["properties"].get("base_url").is_none());
}

#[test]
fn typed_runtime_lists_only_crawl4ai_markdown_tool() {
    let provider = Arc::new(Crawl4AiMarkdownProvider::new());
    let tools = provider.tool_runtime_executors();

    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name().as_str(), TOOL_CRAWL4AI_MARKDOWN);
}

#[tokio::test]
async fn typed_runtime_executor_posts_expected_crawl_contract() {
    let (addr, observed) = serve_crawl4ai_sequence(vec![
        MockResponse {
            status: 200,
            body: r#"{"status":"ok"}"#,
        },
        MockResponse {
            status: 200,
            body: r##"{"results":[{"success":true,"url":"http://93.184.216.34/final","status_code":200,"elapsed_ms":42,"markdown":{"raw_markdown":"# Rendered\n\nArticle body"}}]}"##,
        },
    ])
    .await;
    let config = test_config(Url::parse(&format!("http://{addr}")).expect("base url"));
    let provider = Arc::new(Crawl4AiMarkdownProvider::with_config(config));
    let executor = provider
        .tool_runtime_executors()
        .into_iter()
        .find(|executor| executor.name().as_str() == TOOL_CRAWL4AI_MARKDOWN)
        .expect("typed crawl4ai_markdown executor registered");

    let output = executor
        .execute(runtime_invocation(&format!(
            r#"{{"url":"{PUBLIC_TEST_URL}","timeout_secs":3,"wait_for":"main article","fresh":true,"max_chars":1000}}"#
        )))
        .await
        .expect("crawl4ai_markdown runtime output");

    assert_eq!(output.status, ToolOutputStatus::Success);
    let stdout = output.stdout.text.as_deref().expect("stdout text");
    let payload: Value = serde_json::from_str(stdout).expect("success payload json");
    assert_eq!(payload["provider"], json!(TOOL_CRAWL4AI_MARKDOWN));
    assert_eq!(payload["url"], json!(PUBLIC_TEST_URL));
    assert_eq!(payload["final_url"], json!("http://93.184.216.34/final"));
    assert_eq!(payload["status_code"], json!(200));
    assert_eq!(payload["markdown_kind"], json!("raw_markdown"));
    assert_eq!(payload["content_mode"], json!("crawl4ai_raw_markdown"));
    assert_eq!(payload["source_kind"], json!("web_page"));
    assert_eq!(payload["markdown"], json!("# Rendered\n\nArticle body"));
    assert_eq!(payload["selected_chars"], json!(24));
    assert_eq!(payload["entries_count"], Value::Null);
    assert_eq!(payload["noise_filtered"], json!(false));
    assert_eq!(payload["fresh"], json!(true));

    let observed = observed.lock().expect("observed request lock");
    assert_eq!(observed.len(), 2);
    assert_eq!(observed[0].method, "GET");
    assert_eq!(observed[0].path, "/health");
    assert_eq!(
        observed[0].headers.get("authorization"),
        Some(&"Bearer test-token".to_string())
    );
    assert_eq!(observed[1].method, "POST");
    assert_eq!(observed[1].path, "/crawl");
    assert_eq!(
        observed[1].headers.get("authorization"),
        Some(&"Bearer test-token".to_string())
    );
    let crawl_request: Value = serde_json::from_str(&observed[1].body).expect("crawl request json");
    assert_eq!(crawl_request["urls"], json!([PUBLIC_TEST_URL]));
    assert_eq!(
        crawl_request["browser_config"]["params"]["browser_type"],
        json!("chromium")
    );
    assert_eq!(
        crawl_request["browser_config"]["params"]["headless"],
        json!(true)
    );
    assert_eq!(
        crawl_request["crawler_config"]["params"]["cache_mode"],
        json!("bypass")
    );
    assert_eq!(
        crawl_request["crawler_config"]["params"]["wait_for"],
        json!("css:main article")
    );
    assert_eq!(
        crawl_request["crawler_config"]["params"]["page_timeout"],
        json!(3000)
    );
    assert_eq!(
        crawl_request["crawler_config"]["params"]["exclude_external_links"],
        json!(true)
    );
    assert_eq!(
        crawl_request["crawler_config"]["params"]["exclude_social_media_links"],
        json!(true)
    );
    assert_eq!(
        crawl_request["crawler_config"]["params"]["word_count_threshold"],
        json!(3)
    );
    assert_eq!(
        crawl_request["crawler_config"]["params"]["markdown_generator"]["type"],
        json!("DefaultMarkdownGenerator")
    );
    assert!(
        crawl_request["crawler_config"]["params"]
            .get("js_code")
            .is_none()
    );
}

#[tokio::test]
async fn health_unavailable_returns_structured_failure() {
    let (addr, observed) = serve_crawl4ai_sequence(vec![MockResponse {
        status: 503,
        body: r#"{"status":"down"}"#,
    }])
    .await;
    let config = test_config(Url::parse(&format!("http://{addr}")).expect("base url"));
    let provider = Arc::new(Crawl4AiMarkdownProvider::with_config(config));
    let executor = provider
        .tool_runtime_executors()
        .into_iter()
        .next()
        .expect("executor");

    let output = executor
        .execute(runtime_invocation(&format!(
            r#"{{"url":"{PUBLIC_TEST_URL}"}}"#
        )))
        .await
        .expect("structured failure output");

    assert_eq!(output.status, ToolOutputStatus::Failure);
    let payload = output
        .structured_payload
        .expect("structured crawl4ai failure payload");
    assert_eq!(payload["error_kind"], json!("crawl4ai_unavailable"));
    assert_eq!(payload["provider_unavailable"], json!(true));
    assert_eq!(payload["retryable"], json!(true));
    assert_eq!(payload["status_code"], json!(503));
    assert!(!payload.to_string().contains("test-token"));
    let observed = observed.lock().expect("observed request lock");
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].path, "/health");
}

#[tokio::test]
async fn retries_retryable_crawl_status_once_when_configured() {
    let (addr, observed) = serve_crawl4ai_sequence(vec![
        MockResponse {
            status: 200,
            body: r#"{"status":"ok"}"#,
        },
        MockResponse {
            status: 500,
            body: r#"{"error":"temporary"}"#,
        },
        MockResponse {
            status: 200,
            body: r#"{"status":"ok"}"#,
        },
        MockResponse {
            status: 200,
            body: r##"{"results":[{"success":true,"url":"http://93.184.216.34/article","status_code":200,"markdown":"# Retry succeeded"}]}"##,
        },
    ])
    .await;
    let mut config = test_config(Url::parse(&format!("http://{addr}")).expect("base url"));
    config.max_retries = 1;
    let provider = Arc::new(Crawl4AiMarkdownProvider::with_config(config));
    let executor = provider
        .tool_runtime_executors()
        .into_iter()
        .next()
        .expect("executor");

    let output = executor
        .execute(runtime_invocation(&format!(
            r#"{{"url":"{PUBLIC_TEST_URL}"}}"#
        )))
        .await
        .expect("retry success output");

    assert_eq!(output.status, ToolOutputStatus::Success);
    let stdout = output.stdout.text.as_deref().expect("stdout text");
    assert!(stdout.contains("# Retry succeeded"));
    let observed = observed.lock().expect("observed request lock");
    let paths = observed
        .iter()
        .map(|request| request.path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(paths, vec!["/health", "/crawl", "/health", "/crawl"]);
}

#[test]
fn rejects_non_http_urls_and_no_host() {
    assert!(parse_public_http_url("file:///etc/passwd").is_err());
    assert!(parse_public_http_url("data:text/plain,hello").is_err());
    assert!(parse_public_http_url("https://").is_err());
}

#[test]
fn rejects_localhost_and_private_ips() {
    for raw_url in [
        "http://localhost/page",
        "http://app.localhost/page",
        "http://127.0.0.1/page",
        "http://10.0.0.1/page",
        "http://172.16.0.1/page",
        "http://192.168.0.1/page",
        "http://169.254.169.254/latest/meta-data",
        "http://0.0.0.0/page",
        "http://255.255.255.255/page",
        "http://[::1]/page",
        "http://[::]/page",
        "http://[fd00::1]/page",
        "http://[fe80::1]/page",
        "http://[::ffff:192.168.0.1]/page",
    ] {
        let error = parse_public_http_url(raw_url).err();
        assert!(error.is_some(), "expected {raw_url} to be rejected");
    }
}

#[test]
fn allows_public_url_hosts_before_dns_preflight() {
    let url = parse_public_http_url("https://example.com/page");
    assert!(url.is_ok());
}

#[test]
fn rejects_direct_media_urls() {
    let url = Url::parse("https://example.com/photo.jpg").expect("url");
    assert!(reject_media_url(&url).is_err());
}

#[test]
fn wait_for_accepts_only_css_selectors() {
    assert_eq!(
        normalize_wait_for(Some(".main")).expect("css selector accepted"),
        Some("css:.main".to_string())
    );
    assert_eq!(
        normalize_wait_for(Some("css:#article")).expect("prefixed css selector accepted"),
        Some("css:#article".to_string())
    );
    assert!(normalize_wait_for(Some("js:document.readyState === 'complete'")).is_err());
    assert!(normalize_wait_for(Some("() => true")).is_err());
    assert!(normalize_wait_for(Some("main; body")).is_err());
}

#[test]
fn parses_markdown_string_and_object_shapes() {
    let string_result = json!({"markdown":"# Title"});
    let selected = select_markdown(&string_result).expect("string markdown");
    assert_eq!(selected.kind, "raw_markdown");
    assert_eq!(selected.text, "# Title");

    let object_result = json!({"markdown":{"raw_markdown":"# Raw navigation and article body", "markdown_with_citations":"# Cited", "fit_markdown":"# Clean article body"}});
    let selected = select_markdown(&object_result).expect("object markdown");
    assert_eq!(selected.kind, "fit_markdown");
    assert_eq!(selected.content_mode, "crawl4ai_fit_markdown");
    assert_eq!(selected.text, "# Clean article body");
    assert_eq!(selected.raw_chars, 33);
    assert!(selected.noise_filtered);
}

#[tokio::test]
async fn blocked_markdown_returns_structured_failure() {
    let (addr, _observed) = serve_crawl4ai_sequence(vec![
        MockResponse {
            status: 200,
            body: r#"{"status":"ok"}"#,
        },
        MockResponse {
            status: 200,
            body: r##"{"results":[{"success":true,"url":"http://93.184.216.34/article","status_code":200,"markdown":{"fit_markdown":"You've been blocked by network security. To continue, log in to your Reddit account and use your developer token."}}]}"##,
        },
    ])
    .await;
    let config = test_config(Url::parse(&format!("http://{addr}")).expect("base url"));
    let provider = Arc::new(Crawl4AiMarkdownProvider::with_config(config));
    let executor = provider
        .tool_runtime_executors()
        .into_iter()
        .next()
        .expect("executor");

    let output = executor
        .execute(runtime_invocation(&format!(
            r#"{{"url":"{PUBLIC_TEST_URL}"}}"#
        )))
        .await
        .expect("structured blocked output");

    assert_eq!(output.status, ToolOutputStatus::Failure);
    let payload = output.structured_payload.expect("failure payload");
    assert_eq!(payload["error_kind"], json!("blocked_or_noise"));
    assert!(
        payload["message"]
            .as_str()
            .unwrap_or_default()
            .contains("blocked/noise")
    );
}

#[test]
fn reddit_thread_url_normalizes_to_rss() {
    let url = Url::parse(
        "https://sh.reddit.com/r/LocalLLaMA/comments/1tes1wx/mtp_support_merged_into_llamacpp/?utm_source=x#fragment",
    )
    .expect("reddit url");

    let rss_url = reddit_thread_rss_url(&url).expect("reddit rss url");

    assert_eq!(
        rss_url.as_str(),
        "https://www.reddit.com/r/LocalLLaMA/comments/1tes1wx/mtp_support_merged_into_llamacpp/.rss"
    );
}

#[test]
fn reddit_atom_feed_converts_to_compact_markdown() {
    let target_url = Url::parse("https://www.reddit.com/r/LocalLLaMA/comments/1tes1wx/thread/")
        .expect("target url");
    let rss_url = reddit_thread_rss_url(&target_url).expect("rss url");
    let atom = r#"
        <feed><title>Reddit title</title>
          <entry><title>Original title</title><author><name>op_user</name></author><content type="html">&lt;p&gt;Post body &lt;strong&gt;important&lt;/strong&gt;.&lt;/p&gt;</content></entry>
          <entry><title>Comment title</title><author><name>commenter</name></author><content type="html">&lt;p&gt;Useful comment.&lt;/p&gt;</content></entry>
        </feed>
    "#;

    let result =
        reddit_atom_to_crawl_result(&target_url, &rss_url, 200, atom).expect("reddit atom parsed");

    assert_eq!(result.content_mode, "reddit_rss_fallback");
    assert_eq!(result.source_kind, "reddit_thread");
    assert_eq!(result.entries_count, Some(2));
    assert!(result.noise_filtered);
    assert!(result.markdown.contains("Mode: reddit_rss_fallback"));
    assert!(result.markdown.contains("## Original post"));
    assert!(result.markdown.contains("Author: op_user"));
    assert!(result.markdown.contains("Useful comment"));
}

#[test]
fn reddit_rss_fallback_output_respects_max_chars() {
    let target_url = Url::parse("https://www.reddit.com/r/LocalLLaMA/comments/1tes1wx/thread/")
        .expect("target url");
    let rss_url = reddit_thread_rss_url(&target_url).expect("rss url");
    let atom = r#"
        <feed><title>Reddit title</title>
          <entry><title>Original title</title><author><name>op_user</name></author><content type="html">&lt;p&gt;This is a deliberately long Reddit post body with enough text to exceed the tiny test cap.&lt;/p&gt;</content></entry>
        </feed>
    "#;
    let result =
        reddit_atom_to_crawl_result(&target_url, &rss_url, 200, atom).expect("reddit atom parsed");
    let provider = Crawl4AiMarkdownProvider::with_config(test_config(
        Url::parse(DEFAULT_BASE_URL).expect("base url"),
    ));
    let args = Crawl4AiMarkdownArgs {
        url: target_url.to_string(),
        timeout_secs: None,
        wait_for: None,
        fresh: false,
        max_chars: Some(60),
    };

    let output = provider
        .success_payload(&args, &target_url, result, 60, Instant::now())
        .expect("success payload");
    let payload: Value = serde_json::from_str(&output).expect("payload json");

    assert_eq!(payload["truncated"], json!(true));
    assert_eq!(payload["content_mode"], json!("reddit_rss_fallback"));
    assert!(
        payload["markdown"]
            .as_str()
            .unwrap_or_default()
            .contains("... (truncated)")
    );
}

#[test]
fn falls_back_to_html_when_markdown_is_empty() {
    let result = json!({
        "markdown": {"raw_markdown": "", "markdown_with_citations": "", "fit_markdown": ""},
        "html": "<html><head><title>Ignored</title></head><body><main><h1>Gemma Guide</h1><p>Article body.</p><script>ignore()</script></main></body></html>"
    });

    let selected = select_markdown(&result).expect("html fallback markdown");

    assert_eq!(selected.kind, "html_fallback");
    assert!(selected.text.contains("Gemma Guide"));
    assert!(selected.text.contains("Article body"));
    assert!(!selected.text.contains("ignore()"));
}

#[test]
fn classifies_provider_unavailable_without_leaking_token() {
    let config = Crawl4AiMarkdownConfig {
        base_url: Url::parse(DEFAULT_BASE_URL).expect("url"),
        api_token: Some("secret-token".to_string()),
        default_timeout_secs: DEFAULT_TIMEOUT_SECS,
        max_timeout_secs: DEFAULT_MAX_TIMEOUT_SECS,
        max_output_chars: DEFAULT_MAX_OUTPUT_CHARS,
        health_timeout_ms: DEFAULT_HEALTH_TIMEOUT_MS,
        jitter_min_ms: DEFAULT_JITTER_MIN_MS,
        jitter_max_ms: DEFAULT_JITTER_MAX_MS,
        max_retries: DEFAULT_MAX_RETRIES,
        text_mode: true,
        light_mode: true,
        avoid_ads: true,
    };
    let args = Crawl4AiMarkdownArgs {
        url: "https://example.com".to_string(),
        timeout_secs: None,
        wait_for: None,
        fresh: false,
        max_chars: None,
    };
    let error = anyhow!("crawl4ai health request failed: connection refused");

    let payload = crawl4ai_failure_payload(Some(&args), &config, &error);

    assert_eq!(payload["error_kind"], json!("crawl4ai_unavailable"));
    assert_eq!(payload["provider_unavailable"], json!(true));
    assert_eq!(payload["retryable"], json!(true));
    assert!(!payload.to_string().contains("secret-token"));
}

#[tokio::test]
async fn anti_bot_500_returns_structured_failure() {
    let (addr, observed) = serve_crawl4ai_sequence(vec![
        MockResponse {
            status: 200,
            body: r#"{"status":"ok"}"#,
        },
        MockResponse {
            status: 500,
            body: r#"{"detail":"Blocked by anti-bot protection: Cloudflare JS challenge"}"#,
        },
    ])
    .await;
    let config = test_config(Url::parse(&format!("http://{addr}")).expect("base url"));
    let provider = Arc::new(Crawl4AiMarkdownProvider::with_config(config));
    let executor = provider
        .tool_runtime_executors()
        .into_iter()
        .next()
        .expect("executor");

    let output = executor
        .execute(runtime_invocation(&format!(
            r#"{{"url":"{PUBLIC_TEST_URL}"}}"#
        )))
        .await
        .expect("structured failure output");

    assert_eq!(output.status, ToolOutputStatus::Failure);
    let payload = output.structured_payload.expect("failure payload");
    assert_eq!(payload["error_kind"], json!("anti_bot"));
    assert_eq!(payload["provider_unavailable"], json!(true));
    assert_eq!(payload["retryable"], json!(false));
    let message = payload["message"].as_str().expect("message");
    assert!(
        message.contains("anti-bot protection"),
        "message should mention anti-bot: {message}"
    );
    assert!(
        message.contains("Cloudflare JS challenge"),
        "message should contain detail: {message}"
    );
    assert!(
        message.contains("Do not retry"),
        "message should instruct not to retry: {message}"
    );
    let observed = observed.lock().expect("observed request lock");
    assert_eq!(observed.len(), 2); // health + crawl
    assert_eq!(observed[0].path, "/health");
    assert_eq!(observed[1].path, "/crawl");
}

#[test]
fn anti_bot_error_classification_unit() {
    // Anti-bot in response tail is classified as anti_bot, not crawl4ai_http_status.
    let error = anyhow::anyhow!(
        "crawl4ai returned non-success status: 500; response_tail: {{\"detail\":\"Blocked by anti-bot protection: Cloudflare JS challenge\"}}"
    );
    let kind = crawl4ai_error_kind(&error);
    assert_eq!(kind, "anti_bot");
    assert!(!crawl4ai_error_retryable(kind, &error));

    // Non-anti-bot 500 stays as crawl4ai_http_status.
    let generic_error = anyhow::anyhow!(
        "crawl4ai returned non-success status: 500; response_tail: {{\"error\":\"internal\"}}"
    );
    let generic_kind = crawl4ai_error_kind(&generic_error);
    assert_eq!(generic_kind, "crawl4ai_http_status");
    assert!(crawl4ai_error_retryable(generic_kind, &generic_error));
}
