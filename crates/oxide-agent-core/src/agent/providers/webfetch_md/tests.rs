use super::convert::{html_to_markdown, truncate_chars};
use super::error::reject_anti_bot_challenge;
use super::reddit::{
    RedditAtomEntry, parse_reddit_atom_entries, reddit_thread_rss_url, render_reddit_atom_markdown,
    xml_tag_block, xml_tag_text,
};
use super::url::{parse_web_url, reject_media_url, reject_unsafe_url};
use super::*;
use crate::agent::identity::SessionId;
use crate::agent::tool_runtime::{
    ModelMetadata, ProviderMetadata, ToolBatchId, ToolCallId, ToolExecutionContext,
    ToolOutputStatus, ToolTimeoutConfig, TurnId,
};
use crate::llm::InvocationId;
use chrono::Utc;
use reqwest::Url;
use reqwest::header::HeaderValue;
use reqwest::header::{HeaderMap, SERVER};
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

fn runtime_invocation(raw_arguments: &str) -> ToolInvocation {
    let now = Utc::now();
    ToolInvocation {
        session_id: SessionId::from(77),
        turn_id: TurnId::from("turn-webfetch-md"),
        batch_id: ToolBatchId::from("batch-webfetch-md"),
        batch_index: 0,
        invocation_id: InvocationId::from("invoke-web-markdown"),
        tool_call_id: ToolCallId::from("call-web-markdown"),
        provider_tool_call_id: None,
        tool_name: ToolName::from(TOOL_WEB_MARKDOWN),
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

async fn serve_http_once(body: &'static str, content_type: &'static str) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local test server");
    let addr = listener.local_addr().expect("local address");
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept request");
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let read = stream.read(&mut buffer).await.expect("read request");
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }

        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream
            .write_all(response.as_bytes())
            .await
            .expect("write response");
    });
    addr
}

#[test]
fn typed_runtime_lists_only_web_markdown_tool() {
    let provider = Arc::new(WebFetchMdProvider::new());
    let tools = provider.tool_runtime_executors();

    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name().as_str(), TOOL_WEB_MARKDOWN);
}

#[tokio::test]
async fn typed_runtime_executor_fetches_web_markdown() {
    let addr = serve_http_once(
        "<html><body><main><h1>Hello</h1><p>Readable page.</p></main></body></html>",
        "text/html; charset=utf-8",
    )
    .await;
    let client = reqwest::Client::builder()
        .resolve("example.test", addr)
        .build()
        .expect("test client");
    let provider = Arc::new(WebFetchMdProvider::with_client(client));
    let executor = provider
        .tool_runtime_executors()
        .into_iter()
        .find(|executor| executor.name().as_str() == TOOL_WEB_MARKDOWN)
        .expect("typed web_markdown executor registered");

    let output = executor
        .execute(runtime_invocation(
            r#"{"url":"http://example.test/article","timeout_secs":5}"#,
        ))
        .await
        .expect("typed web_markdown succeeds");

    assert_eq!(output.status, ToolOutputStatus::Success);
    let stdout = output.stdout.text.as_deref().expect("stdout text");
    assert!(stdout.contains("URL: http://example.test/article"));
    assert!(stdout.contains("# Hello"));
    assert!(stdout.contains("Readable page."));
    let payload = output.structured_payload.expect("success payload");
    assert_eq!(payload["provider"], "web_markdown");
    assert_eq!(payload["kind"], "fetch");
    assert_eq!(payload["url"], "http://example.test/article");
    assert_eq!(payload["final_url"], "http://example.test/article");
    assert_eq!(payload["status_code"], 200);
    assert_eq!(payload["snippet_only"], false);
}

#[test]
fn converts_html_to_markdown_and_skips_chrome_tags() {
    let markdown = html_to_markdown(
        r#"
            <html>
                <body>
                    <nav>skip navigation</nav>
                    <main><h1>Hello</h1><p>Readable page.</p></main>
                    <script>alert(1)</script>
                </body>
            </html>
            "#,
    );

    assert!(markdown.is_ok());
    let markdown = markdown.unwrap_or_default();
    assert!(markdown.contains("# Hello"));
    assert!(markdown.contains("Readable page."));
    assert!(!markdown.contains("skip navigation"));
    assert!(!markdown.contains("alert"));
}

#[test]
fn rejects_non_http_urls() {
    let error = parse_web_url("file:///etc/passwd").err();
    assert!(error.is_some());
    assert!(
        error
            .map(|error| error.to_string().contains("unsupported URL scheme"))
            .unwrap_or(false)
    );
}

#[test]
fn rejects_localhost_and_private_ips() {
    let localhost = Url::parse("http://localhost/page");
    assert!(localhost.is_ok());
    assert!(
        localhost
            .ok()
            .and_then(|url| reject_unsafe_url(&url).err())
            .is_some()
    );

    let private_ip = Url::parse("http://192.168.1.1/page");
    assert!(private_ip.is_ok());
    assert!(
        private_ip
            .ok()
            .and_then(|url| reject_unsafe_url(&url).err())
            .is_some()
    );

    let metadata_ip = Url::parse("http://169.254.169.254/latest/meta-data");
    assert!(metadata_ip.is_ok());
    assert!(
        metadata_ip
            .ok()
            .and_then(|url| reject_unsafe_url(&url).err())
            .is_some()
    );

    let unique_local_ipv6 = Url::parse("http://[fd00::1]/page");
    assert!(unique_local_ipv6.is_ok());
    assert!(
        unique_local_ipv6
            .ok()
            .and_then(|url| reject_unsafe_url(&url).err())
            .is_some()
    );
}

#[test]
fn allows_public_urls() {
    let public_url = Url::parse("https://example.com/page");
    assert!(public_url.is_ok());
    assert!(
        public_url
            .ok()
            .map(|url| reject_unsafe_url(&url).is_ok())
            .unwrap_or(false)
    );
}

#[test]
fn rejects_direct_media_urls() {
    let url = Url::parse("https://example.com/photo.jpg");
    assert!(url.is_ok());
    assert!(
        url.ok()
            .and_then(|url| reject_media_url(&url).err())
            .is_some()
    );
}

#[test]
fn detects_cf_mitigated_challenge_header() {
    let mut headers = HeaderMap::new();
    headers.insert("cf-mitigated", HeaderValue::from_static("challenge"));

    let error = reject_anti_bot_challenge(&headers, "").expect_err("challenge must fail");

    assert_eq!(error.to_string(), ANTI_BOT_ERROR);
}

#[test]
fn detects_cloudflare_server_with_challenge_marker() {
    let mut headers = HeaderMap::new();
    headers.insert(SERVER, HeaderValue::from_static("cloudflare"));

    let error = reject_anti_bot_challenge(&headers, "<html>challenge platform</html>")
        .expect_err("cloudflare challenge must fail");

    assert_eq!(error.to_string(), ANTI_BOT_ERROR);
}

#[test]
fn detects_common_antibot_body_markers() {
    let headers = HeaderMap::new();

    for body in [
        "Just a moment...",
        "Making sure you're not a bot!",
        "Checking your browser before accessing the site",
        "Please enable JavaScript and cookies to continue",
        "Anubis uses a Proof-of-Work scheme to protect the server",
        "This page requires the use of modern JavaScript features",
        "<script src=\"/cdn-cgi/challenge-platform/h/b/cf-chl-jschl\"></script>",
        "captcha verification required",
    ] {
        let error = reject_anti_bot_challenge(&headers, body).expect_err("marker must fail");
        assert_eq!(error.to_string(), ANTI_BOT_ERROR);
    }
}

#[test]
fn allows_regular_html_without_antibot_markers() {
    let headers = HeaderMap::new();

    assert!(
        reject_anti_bot_challenge(
            &headers,
            "<html><body><h1>Regular article</h1></body></html>",
        )
        .is_ok()
    );
}

#[test]
fn truncates_long_output() {
    let output = truncate_chars("abcdef".to_string(), 3);

    assert!(output.was_truncated);
    assert_eq!(output.text, "abc\n\n... (truncated)");
}

#[tokio::test]
async fn typed_runtime_executor_returns_structured_antibot_failure() {
    let addr = serve_http_once(
        r#"<html><body><h1>Making sure you're not a bot!</h1><p>Anubis uses a Proof-of-Work scheme to protect the server.</p></body></html>"#,
        "text/html; charset=utf-8",
    )
    .await;
    let client = reqwest::Client::builder()
        .resolve("example.test", addr)
        .build()
        .expect("test client");
    let provider = Arc::new(WebFetchMdProvider::with_client(client));
    let executor = provider
        .tool_runtime_executors()
        .into_iter()
        .find(|executor| executor.name().as_str() == TOOL_WEB_MARKDOWN)
        .expect("typed web_markdown executor registered");

    let output = executor
        .execute(runtime_invocation(
            r#"{"url":"http://example.test/protected","timeout_secs":5}"#,
        ))
        .await
        .expect("typed web_markdown returns failure output");

    assert_eq!(output.status, ToolOutputStatus::Failure);
    assert!(
        output
            .error_message
            .as_deref()
            .expect("error message")
            .contains("anti-bot protection at example.test")
    );

    let payload = output.structured_payload.expect("structured payload");
    assert_eq!(
        payload.get("provider").and_then(|value| value.as_str()),
        Some("web_markdown")
    );
    assert_eq!(
        payload.get("error_kind").and_then(|value| value.as_str()),
        Some("anti_bot")
    );
    assert_eq!(
        payload.get("host").and_then(|value| value.as_str()),
        Some("example.test")
    );
    assert_eq!(
        payload
            .get("provider_unavailable")
            .and_then(|value| value.as_bool()),
        Some(true)
    );
    assert_eq!(
        payload.get("retryable").and_then(|value| value.as_bool()),
        Some(false)
    );
}

// -- Reddit RSS tests --

#[test]
fn detects_reddit_thread_urls() {
    let cases = [
        ("https://www.reddit.com/r/rust/comments/abc123/title/", true),
        ("https://old.reddit.com/r/rust/comments/abc123/title/", true),
        ("https://new.reddit.com/r/rust/comments/abc123/title/", true),
        ("https://sh.reddit.com/r/rust/comments/abc123/title/", true),
        ("https://reddit.com/r/rust/comments/abc123/title/", true),
    ];
    for (raw, expected) in cases {
        let url = Url::parse(raw).expect("url");
        assert_eq!(
            reddit_thread_rss_url(&url).is_some(),
            expected,
            "expected is_some={expected} for {raw}"
        );
    }
}

#[test]
fn rejects_non_thread_reddit_urls() {
    let cases = [
        "https://www.reddit.com/r/rust/",
        "https://www.reddit.com/r/rust/hot/",
        "https://www.reddit.com/user/example/",
        "https://www.reddit.com/",
    ];
    for raw in cases {
        let url = Url::parse(raw).expect("url");
        assert!(
            reddit_thread_rss_url(&url).is_none(),
            "expected None for non-thread URL: {raw}"
        );
    }
}

#[test]
fn rejects_non_reddit_hosts() {
    let url = Url::parse("https://example.com/r/rust/comments/abc123/title/").expect("url");
    assert!(reddit_thread_rss_url(&url).is_none());
}

#[test]
fn builds_rss_url_from_reddit_thread() {
    let url = Url::parse("https://old.reddit.com/r/rust/comments/abc123/some_title/?sort=top")
        .expect("url");
    let rss = reddit_thread_rss_url(&url).expect("rss url");
    assert_eq!(
        rss.as_str(),
        "https://www.reddit.com/r/rust/comments/abc123/some_title/.rss"
    );
}

#[test]
fn strips_query_and_fragment_from_rss_url() {
    let url = Url::parse("https://www.reddit.com/r/rust/comments/abc123/t/#comment1").expect("url");
    let rss = reddit_thread_rss_url(&url).expect("rss url");
    assert_eq!(rss.query(), None);
    assert_eq!(rss.fragment(), None);
}

#[test]
fn parses_reddit_atom_feed() {
    let atom = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Test Thread : r/rust</title>
  <entry>
    <title>Test post title</title>
    <author><name>test_user</name></author>
    <content type="html">&lt;p&gt;This is the post body.&lt;/p&gt;</content>
  </entry>
  <entry>
    <title>First comment</title>
    <author><name>commenter</name></author>
    <content type="html">&lt;p&gt;Comment text here.&lt;/p&gt;</content>
  </entry>
</feed>"#;

    let entries = parse_reddit_atom_entries(atom).expect("entries");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].title, "Test post title");
    assert_eq!(entries[0].author.as_deref(), Some("test_user"));
    assert!(entries[0].markdown.contains("This is the post body."));
    assert_eq!(entries[1].title, "First comment");
    assert!(entries[1].markdown.contains("Comment text here."));
}

#[test]
fn renders_reddit_atom_markdown() {
    let target = Url::parse("https://www.reddit.com/r/rust/comments/abc/test/").expect("url");
    let entries = vec![
        RedditAtomEntry {
            title: "Post title".to_string(),
            author: Some("op_user".to_string()),
            markdown: "Post body text".to_string(),
        },
        RedditAtomEntry {
            title: "Comment 1".to_string(),
            author: Some("commenter".to_string()),
            markdown: "Comment text".to_string(),
        },
        RedditAtomEntry {
            title: "Comment 2".to_string(),
            author: None,
            markdown: "Another comment".to_string(),
        },
    ];
    let md = render_reddit_atom_markdown(&target, "Thread Title", &entries);

    assert!(md.starts_with("# Thread Title"));
    assert!(md.contains("## Original post"));
    assert!(md.contains("**Post title**"));
    assert!(md.contains("Author: op_user"));
    assert!(md.contains("## Comments"));
    assert!(md.contains("### 1. Comment 1"));
    assert!(md.contains("Author: commenter"));
    assert!(md.contains("### 2. Comment 2"));
    assert!(!md.contains("Author: \n"));
    assert!(md.contains(&format!("Source: {target}")));
    assert!(md.contains("Mode: reddit_rss_fallback"));
    assert!(md.contains("Entries: 3"));
}

#[test]
fn xml_tag_text_decodes_html_entities() {
    let input = "<title>&amp; &lt;hello&gt;</title>";
    assert_eq!(xml_tag_text(input, "title").as_deref(), Some("& <hello>"));
}

#[test]
fn xml_tag_block_extracts_inner_content() {
    let input = "<content type=\"html\">hello world</content>";
    assert_eq!(xml_tag_block(input, "content"), Some("hello world"));
}

#[test]
fn xml_tag_returns_none_for_missing_tag() {
    assert!(xml_tag_text("no tags here", "title").is_none());
    assert!(xml_tag_block("no tags here", "title").is_none());
}
