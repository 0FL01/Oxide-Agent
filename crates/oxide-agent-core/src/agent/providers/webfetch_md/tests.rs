use super::convert::{html_to_markdown, truncate_chars};
use super::error::reject_anti_bot_challenge;
use super::known_sources::{KnownMarkdownSource, classify as classify_known_source};
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

async fn serve_http_sequence(responses: Vec<(&'static str, &'static str)>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local test server");
    let addr = listener.local_addr().expect("local address");
    tokio::spawn(async move {
        for (body, content_type) in responses {
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
        }
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
    assert!(stdout.starts_with("## Web Markdown"));
    assert!(stdout.contains("URL: http://example.test/article"));
    assert!(stdout.contains("Fetched-Bytes:"));
    assert!(stdout.contains("### Content"));
    assert!(stdout.contains("# Hello"));
    assert!(stdout.contains("Readable page."));
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

// -- Known Markdown source tests --

#[test]
fn maps_github_repo_root_to_raw_readme() {
    let url = Url::parse("https://github.com/owner/repo").expect("url");
    let source = classify_known_source(&url).expect("known markdown source");

    assert_eq!(source.source_url(), &url);
    assert_eq!(
        source.fetch_url().as_str(),
        "https://raw.githubusercontent.com/owner/repo/HEAD/README.md"
    );
    assert!(matches!(source, KnownMarkdownSource::DirectReadme { .. }));
    assert_eq!(source.mode(), "github_readme_fast_path");
}

#[test]
fn maps_github_readme_blob_to_raw_url() {
    let url = Url::parse("https://github.com/owner/repo/blob/main/docs/README.md").expect("url");
    let source = classify_known_source(&url).expect("known markdown source");

    assert_eq!(
        source.fetch_url().as_str(),
        "https://raw.githubusercontent.com/owner/repo/main/docs/README.md"
    );
    assert_eq!(source.mode(), "github_blob_fast_path");
}

#[test]
fn maps_github_gist_to_api_plan() {
    let url = Url::parse("https://gist.github.com/DocShotgun/a02a4c0c0a57e43ff4f038b46ca66ae0")
        .expect("url");
    let source = classify_known_source(&url).expect("known markdown source");

    assert_eq!(source.source_url(), &url);
    assert_eq!(
        source.fetch_url().as_str(),
        "https://api.github.com/gists/a02a4c0c0a57e43ff4f038b46ca66ae0"
    );
    assert!(matches!(source, KnownMarkdownSource::GitHubGist { .. }));
    assert_eq!(source.mode(), "github_gist_fast_path");
}

#[test]
fn maps_github_gist_permalink_comment_to_api_plan() {
    let url = Url::parse(
        "https://gist.github.com/DocShotgun/a02a4c0c0a57e43ff4f038b46ca66ae0?permalink_comment_id=5946304",
    )
    .expect("url");
    let source = classify_known_source(&url).expect("known markdown source");

    match source {
        KnownMarkdownSource::GitHubGist { comment_id, .. } => {
            assert_eq!(comment_id.as_deref(), Some("5946304"));
        }
        _ => panic!("expected GitHub Gist source"),
    }
}

#[test]
fn maps_huggingface_model_root_to_resolve_readme() {
    let url = Url::parse("https://huggingface.co/owner/model").expect("url");
    let source = classify_known_source(&url).expect("known markdown source");

    assert_eq!(
        source.fetch_url().as_str(),
        "https://huggingface.co/owner/model/resolve/main/README.md"
    );
    assert_eq!(source.mode(), "huggingface_readme_fast_path");
}

#[test]
fn maps_huggingface_dataset_blob_to_resolve_readme() {
    let url =
        Url::parse("https://huggingface.co/datasets/owner/data/blob/dev/README.md").expect("url");
    let source = classify_known_source(&url).expect("known markdown source");

    assert_eq!(
        source.fetch_url().as_str(),
        "https://huggingface.co/datasets/owner/data/resolve/dev/README.md"
    );
    assert_eq!(source.mode(), "huggingface_blob_fast_path");
}

#[test]
fn maps_huggingface_blog_to_html_fast_path() {
    for raw in [
        "https://huggingface.co/blog/slug-only",
        "https://huggingface.co/blog/junafinity/flash-load-step-37-flash-q8-mlx-100gb-ram#section",
    ] {
        let url = Url::parse(raw).expect("url");
        let source = classify_known_source(&url).expect("known markdown source");

        assert!(matches!(
            source,
            KnownMarkdownSource::HuggingFaceBlog { .. }
        ));
        assert_eq!(source.mode(), "huggingface_blog_fast_path");
        assert_eq!(source.fetch_url().fragment(), None);
    }
}

#[test]
fn maps_gitlab_repo_root_to_raw_readme() {
    let url = Url::parse("https://gitlab.com/gitlab-org/gitlab").expect("url");
    let source = classify_known_source(&url).expect("known markdown source");

    assert_eq!(
        source.fetch_url().as_str(),
        "https://gitlab.com/gitlab-org/gitlab/-/raw/HEAD/README.md"
    );
    assert_eq!(source.mode(), "gitlab_readme_fast_path");
}

#[test]
fn maps_gitlab_nested_group_root_to_raw_readme() {
    let url = Url::parse("https://gitlab.com/group/subgroup/project").expect("url");
    let source = classify_known_source(&url).expect("known markdown source");

    assert_eq!(
        source.fetch_url().as_str(),
        "https://gitlab.com/group/subgroup/project/-/raw/HEAD/README.md"
    );
    assert_eq!(source.mode(), "gitlab_readme_fast_path");
}

#[test]
fn maps_gitlab_readme_blob_to_raw_url() {
    let url = Url::parse("https://gitlab.com/group/subgroup/project/-/blob/main/docs/README.md")
        .expect("url");
    let source = classify_known_source(&url).expect("known markdown source");

    assert_eq!(
        source.fetch_url().as_str(),
        "https://gitlab.com/group/subgroup/project/-/raw/main/docs/README.md"
    );
    assert_eq!(source.mode(), "gitlab_blob_fast_path");
}

#[test]
fn maps_codeberg_repo_root_to_raw_readme() {
    let url = Url::parse("https://codeberg.org/forgejo/forgejo").expect("url");
    let source = classify_known_source(&url).expect("known markdown source");

    assert_eq!(
        source.fetch_url().as_str(),
        "https://codeberg.org/forgejo/forgejo/raw/branch/HEAD/README.md"
    );
    assert_eq!(source.mode(), "gitea_readme_fast_path");
}

#[test]
fn maps_gitea_src_branch_readme_to_raw_url() {
    let url =
        Url::parse("https://gitea.com/owner/repo/src/branch/main/docs/README.md").expect("url");
    let source = classify_known_source(&url).expect("known markdown source");

    assert_eq!(
        source.fetch_url().as_str(),
        "https://gitea.com/owner/repo/raw/branch/main/docs/README.md"
    );
    assert_eq!(source.mode(), "gitea_src_fast_path");
}

#[test]
fn maps_crates_io_package_to_readme_api_plan() {
    let url = Url::parse("https://crates.io/crates/tokio").expect("url");
    let source = classify_known_source(&url).expect("known markdown source");

    assert_eq!(source.source_url(), &url);
    assert_eq!(
        source.fetch_url().as_str(),
        "https://crates.io/api/v1/crates/tokio"
    );
    assert!(matches!(source, KnownMarkdownSource::CrateReadme { .. }));
    assert_eq!(source.mode(), "crates_io_readme_fast_path");
}

#[test]
fn maps_docs_rs_urls_to_crates_io_readme_api_plan() {
    for (raw, expected_version) in [
        ("https://docs.rs/tokio", None),
        ("https://docs.rs/tokio/latest/tokio/", None),
        ("https://docs.rs/tokio/1.48.0/tokio/", Some("1.48.0")),
        ("https://docs.rs/crate/tokio/1.48.0", Some("1.48.0")),
    ] {
        let url = Url::parse(raw).expect("url");
        let source = classify_known_source(&url).expect("known markdown source");

        assert_eq!(
            source.fetch_url().as_str(),
            "https://crates.io/api/v1/crates/tokio"
        );
        assert_eq!(source.mode(), "docs_rs_readme_fast_path");
        match source {
            KnownMarkdownSource::CrateReadme { version, .. } => {
                assert_eq!(version.as_deref(), expected_version);
            }
            _ => panic!("expected crate README source"),
        }
    }
}

#[test]
fn maps_pypi_project_to_json_api_plan() {
    let url = Url::parse("https://pypi.org/project/requests/").expect("url");
    let source = classify_known_source(&url).expect("known markdown source");

    assert_eq!(source.source_url(), &url);
    assert_eq!(
        source.fetch_url().as_str(),
        "https://pypi.org/pypi/requests/json"
    );
    assert!(matches!(source, KnownMarkdownSource::PypiProject { .. }));
    assert_eq!(source.mode(), "pypi_project_fast_path");
}

#[tokio::test]
async fn fetches_crates_io_readme_via_metadata_api() {
    let addr = serve_http_sequence(vec![
        (
            r#"{"crate":{"newest_version":"1.2.3"}}"#,
            "application/json",
        ),
        (
            "# Demo crate\n\nREADME from API.",
            "text/markdown; charset=utf-8",
        ),
    ])
    .await;
    let client = reqwest::Client::builder()
        .resolve("crates.io", addr)
        .build()
        .expect("test client");
    let provider = WebFetchMdProvider::with_client(client);

    let output = provider
        .fetch_markdown(
            WebMarkdownArgs {
                url: "http://crates.io/crates/demo".to_string(),
                timeout_secs: Some(5),
            },
            None,
        )
        .await
        .expect("crates.io README fast path succeeds");

    assert!(output.contains("URL: http://crates.io/api/v1/crates/demo/1.2.3/readme"));
    assert!(output.contains("Source-URL: http://crates.io/crates/demo"));
    assert!(output.contains("Mode: crates_io_readme_fast_path"));
    assert!(output.contains("Crate: demo"));
    assert!(output.contains("Version: 1.2.3"));
    assert!(output.contains("# Demo crate"));
    assert!(output.contains("README from API."));
}

#[tokio::test]
async fn fetches_github_gist_files_and_permalink_comment_via_api() {
    let metadata_json = r##"{
        "files": {
            "README.md": {
                "filename": "README.md",
                "type": "text/markdown",
                "language": "Markdown",
                "raw_url": "http://gist.githubusercontent.com/DocShotgun/raw/readme",
                "size": 28,
                "truncated": false,
                "content": "# Demo gist\n\nGist file body."
            },
            "image.png": {
                "filename": "image.png",
                "type": "image/png",
                "raw_url": "http://gist.githubusercontent.com/DocShotgun/raw/image",
                "truncated": false
            }
        }
    }"##;
    let comment_json = r##"{"body":"Comment **markdown** body."}"##;
    let addr = serve_http_sequence(vec![
        (metadata_json, "application/json"),
        (comment_json, "application/json"),
    ])
    .await;
    let client = reqwest::Client::builder()
        .resolve("api.github.com", addr)
        .build()
        .expect("test client");
    let provider = WebFetchMdProvider::with_client(client);

    let output = provider
        .fetch_markdown(
            WebMarkdownArgs {
                url: "http://gist.github.com/DocShotgun/a02a4c0c0a57e43ff4f038b46ca66ae0?permalink_comment_id=5946304".to_string(),
                timeout_secs: Some(5),
            },
            None,
        )
        .await
        .expect("GitHub Gist fast path succeeds");

    assert!(output.contains("URL: http://api.github.com/gists/a02a4c0c0a57e43ff4f038b46ca66ae0"));
    assert!(output.contains("Source-URL: http://gist.github.com/DocShotgun/a02a4c0c0a57e43ff4f038b46ca66ae0?permalink_comment_id=5946304"));
    assert!(output.contains("Mode: github_gist_fast_path"));
    assert!(output.contains("Owner: DocShotgun"));
    assert!(output.contains("Gist-ID: a02a4c0c0a57e43ff4f038b46ca66ae0"));
    assert!(output.contains("Comment-ID: 5946304"));
    assert!(output.contains("Files: README.md"));
    assert!(output.contains("### File: README.md"));
    assert!(output.contains("# Demo gist"));
    assert!(output.contains("### Permalink Comment"));
    assert!(output.contains("Comment **markdown** body."));
    assert!(!output.contains("image.png"));
}

#[tokio::test]
async fn fetches_github_gist_truncated_file_from_raw_url() {
    let metadata_json = r##"{
        "files": {
            "long.md": {
                "filename": "long.md",
                "type": "text/markdown",
                "language": "Markdown",
                "raw_url": "http://gist.githubusercontent.com/DocShotgun/raw/long",
                "size": 2000000,
                "truncated": true,
                "content": "partial"
            }
        }
    }"##;
    let addr = serve_http_sequence(vec![
        (metadata_json, "application/json"),
        ("# Full raw gist\n\nFetched from raw_url.", "text/markdown"),
    ])
    .await;
    let client = reqwest::Client::builder()
        .resolve("api.github.com", addr)
        .resolve("gist.githubusercontent.com", addr)
        .build()
        .expect("test client");
    let provider = WebFetchMdProvider::with_client(client);

    let output = provider
        .fetch_markdown(
            WebMarkdownArgs {
                url: "http://gist.github.com/DocShotgun/a02a4c0c0a57e43ff4f038b46ca66ae0"
                    .to_string(),
                timeout_secs: Some(5),
            },
            None,
        )
        .await
        .expect("GitHub Gist raw_url fallback succeeds");

    assert!(output.contains("Files: long.md"));
    assert!(output.contains("# Full raw gist"));
    assert!(output.contains("Fetched from raw_url."));
    assert!(!output.contains("partial"));
}

#[tokio::test]
async fn fetches_huggingface_blog_content_despite_waf_markers() {
    let html = r#"
        <html>
          <head><script>window.hubConfig = {"captchaApiKey":"test"};</script></head>
          <body>
            <main>
              <div class="blog-content prose">
                <h1>Flash Load</h1>
                <p>The runtime has three practical pieces.</p>
                <script src="https://example.awswaf.com/challenge.js"></script>
              </div>
            </main>
          </body>
        </html>
    "#;
    let addr = serve_http_once(html, "text/html; charset=utf-8").await;
    let client = reqwest::Client::builder()
        .resolve("huggingface.co", addr)
        .build()
        .expect("test client");
    let provider = WebFetchMdProvider::with_client(client);

    let output = provider
        .fetch_markdown(
            WebMarkdownArgs {
                url: "http://huggingface.co/blog/junafinity/flash-load-step-37-flash-q8-mlx-100gb-ram"
                    .to_string(),
                timeout_secs: Some(5),
            },
            None,
        )
        .await
        .expect("HuggingFace Blog fast path succeeds");

    assert!(output.contains("Mode: huggingface_blog_fast_path"));
    assert!(output.contains("# Flash Load"));
    assert!(output.contains("The runtime has three practical pieces."));
    assert!(!output.contains("captchaApiKey"));
    assert!(!output.contains("challenge.js"));
}

#[tokio::test]
async fn huggingface_blog_without_blog_content_falls_back_to_antibot_failure() {
    let waf_html = r#"
        <html>
          <head><script src="https://example.awswaf.com/challenge.js"></script></head>
          <body>captcha challenge</body>
        </html>
    "#;
    let addr = serve_http_sequence(vec![
        (waf_html, "text/html; charset=utf-8"),
        (waf_html, "text/html; charset=utf-8"),
    ])
    .await;
    let client = reqwest::Client::builder()
        .resolve("huggingface.co", addr)
        .build()
        .expect("test client");
    let provider = WebFetchMdProvider::with_client(client);

    let error = provider
        .fetch_markdown(
            WebMarkdownArgs {
                url: "http://huggingface.co/blog/junafinity/flash-load-step-37-flash-q8-mlx-100gb-ram"
                    .to_string(),
                timeout_secs: Some(5),
            },
            None,
        )
        .await
        .expect_err("WAF-only HuggingFace Blog fails as anti-bot");

    assert!(format!("{error:#}").contains("anti-bot protection"));
}

#[tokio::test]
async fn fetches_pypi_project_description_via_json_api() {
    let metadata_json = r##"{
        "info": {
            "name": "demo-pkg",
            "version": "2.3.4",
            "summary": "Demo package summary",
            "description": "# Demo package\n\nLong description from PyPI.",
            "description_content_type": "text/markdown",
            "project_urls": {
                "Source": "https://example.test/demo-pkg"
            }
        }
    }"##;
    let addr = serve_http_once(metadata_json, "application/json").await;
    let client = reqwest::Client::builder()
        .resolve("pypi.org", addr)
        .build()
        .expect("test client");
    let provider = WebFetchMdProvider::with_client(client);

    let output = provider
        .fetch_markdown(
            WebMarkdownArgs {
                url: "http://pypi.org/project/demo-pkg/".to_string(),
                timeout_secs: Some(5),
            },
            None,
        )
        .await
        .expect("PyPI project fast path succeeds");

    assert!(output.contains("URL: http://pypi.org/pypi/demo-pkg/json"));
    assert!(output.contains("Source-URL: http://pypi.org/project/demo-pkg/"));
    assert!(output.contains("Mode: pypi_project_fast_path"));
    assert!(output.contains("Package: demo-pkg"));
    assert!(output.contains("Version: 2.3.4"));
    assert!(output.contains("Summary: Demo package summary"));
    assert!(output.contains("Project-URL: https://example.test/demo-pkg"));
    assert!(output.contains("# Demo package"));
    assert!(output.contains("Long description from PyPI."));
}

#[test]
fn maps_generic_gitea_src_branch_readme_to_raw_url() {
    let url =
        Url::parse("https://git.example.test/owner/repo/src/branch/dev/README.md").expect("url");
    let source = classify_known_source(&url).expect("known markdown source");

    assert_eq!(
        source.fetch_url().as_str(),
        "https://git.example.test/owner/repo/raw/branch/dev/README.md"
    );
    assert_eq!(source.mode(), "gitea_src_fast_path");
}

#[test]
fn ignores_non_readme_known_source_pages() {
    for raw in [
        "https://github.com/owner/repo/issues/1",
        "https://github.com/owner/repo/blob/main/src/lib.rs",
        "https://gitlab.com/group/project/-/issues/1",
        "https://gitlab.com/group/project/-/blob/main/src/lib.rs",
        "https://codeberg.org/owner/repo/issues/1",
        "https://codeberg.org/owner/repo/src/branch/main/src/lib.rs",
        "https://git.example.test/owner/repo",
        "https://git.example.test/owner/repo/src/branch/main/src/lib.rs",
        "https://huggingface.co/owner/model/discussions/1",
        "https://huggingface.co/owner/model/blob/main/config.json",
        "https://pypi.org/project/requests/docs/",
        "https://pypi.org/simple/requests/",
    ] {
        let url = Url::parse(raw).expect("url");
        assert!(
            classify_known_source(&url).is_none(),
            "expected no known markdown source for {raw}"
        );
    }
}

#[test]
fn xml_tag_returns_none_for_missing_tag() {
    assert!(xml_tag_text("no tags here", "title").is_none());
    assert!(xml_tag_block("no tags here", "title").is_none());
}
