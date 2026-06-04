#[cfg(feature = "tool-searxng")]
mod searxng_tests {
    use oxide_agent_core::agent::identity::SessionId;
    use oxide_agent_core::agent::providers::SearxngProvider;
    use oxide_agent_core::agent::tool_runtime::{
        ModelMetadata, ProviderMetadata, ToolBatchId, ToolCallId, ToolExecutionContext,
        ToolInvocation, ToolName, ToolOutputStatus, ToolTimeoutConfig, TurnId,
    };
    use oxide_agent_core::llm::InvocationId;
    use serde_json::json;
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio_util::sync::CancellationToken;

    const SEARCH_BODY: &str = r#"{
        "results": [
            {
                "title": "Rust documentation",
                "url": "https://doc.rust-lang.org/",
                "content": "Official Rust documentation.",
                "engine": "test"
            }
        ],
        "answers": [],
        "suggestions": [],
        "corrections": [],
        "number_of_results": 1,
        "unresponsive_engines": []
    }"#;

    fn runtime_invocation(raw_arguments: &str) -> ToolInvocation {
        let now = chrono::Utc::now();
        ToolInvocation {
            session_id: SessionId::from(77),
            turn_id: TurnId::from("turn-searxng"),
            batch_id: ToolBatchId::from("batch-searxng"),
            batch_index: 0,
            invocation_id: InvocationId::from("invoke-searxng"),
            tool_call_id: ToolCallId::from("call-searxng"),
            provider_tool_call_id: None,
            tool_name: ToolName::from("searxng_search"),
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

    async fn serve_search_once(
        body: &'static str,
        expected_authorization: Option<&'static str>,
    ) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local SearXNG test server");
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
            let request_text = String::from_utf8_lossy(&request);
            assert!(request_text.contains("GET /search?"));
            assert!(request_text.contains("format=json"));
            match expected_authorization {
                Some(expected) => assert!(
                    request_text.contains(&format!("Authorization: {expected}"))
                        || request_text.contains(&format!("authorization: {expected}"))
                ),
                None => assert!(!request_text
                    .to_ascii_lowercase()
                    .contains("\r\nauthorization:")),
            }

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
    fn searxng_typed_runtime_registers_only_search_tool() {
        let provider = std::sync::Arc::new(
            SearxngProvider::new("http://localhost:8080")
                .expect("provider should construct with valid base URL"),
        );
        let tools = provider.tool_runtime_executors();

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name().as_str(), "searxng_search");
    }

    #[test]
    fn searxng_typed_runtime_spec_uses_search_tool_name() {
        let provider = std::sync::Arc::new(
            SearxngProvider::new("http://localhost:8080")
                .expect("provider should construct with valid base URL"),
        );
        let spec = provider
            .tool_runtime_executors()
            .into_iter()
            .next()
            .expect("typed SearXNG executor registered")
            .spec();

        assert_eq!(spec.name, "searxng_search");
        assert!(spec.description.contains("SearXNG"));
    }

    #[test]
    fn searxng_typed_runtime_spec_requires_query() {
        let provider = std::sync::Arc::new(
            SearxngProvider::new("http://localhost:8080")
                .expect("provider should construct with valid base URL"),
        );
        let spec = provider
            .tool_runtime_executors()
            .into_iter()
            .next()
            .expect("typed SearXNG executor registered")
            .spec();

        assert_eq!(spec.parameters["required"], serde_json::json!(["query"]));
    }

    #[tokio::test]
    async fn typed_runtime_executor_searches_local_searxng() {
        let addr = serve_search_once(SEARCH_BODY, None).await;
        let provider = std::sync::Arc::new(
            SearxngProvider::new_with_timeout_and_bearer_token(
                &format!("http://{addr}"),
                Duration::from_secs(5),
                None,
            )
            .expect("provider should construct"),
        );
        let executor = provider
            .tool_runtime_executors()
            .into_iter()
            .find(|executor| executor.name().as_str() == "searxng_search")
            .expect("typed SearXNG executor registered");

        let output = executor
            .execute(runtime_invocation(
                r#"{"query":"rust docs","max_results":3}"#,
            ))
            .await
            .expect("typed search succeeds");

        assert_eq!(output.status, ToolOutputStatus::Success);
        let stdout = output.stdout.text.as_deref().expect("stdout text");
        assert!(stdout.contains("## SearXNG results for: rust docs"));
        assert!(stdout.contains("Rust documentation"));
        assert!(stdout.contains("https://doc.rust-lang.org/"));
    }

    #[tokio::test]
    async fn typed_runtime_executor_sends_optional_bearer_token() {
        let addr = serve_search_once(SEARCH_BODY, Some("Bearer test-token")).await;
        let provider = std::sync::Arc::new(
            SearxngProvider::new_with_timeout_and_bearer_token(
                &format!("http://{addr}"),
                Duration::from_secs(5),
                Some("test-token".to_string()),
            )
            .expect("provider should construct"),
        );
        let executor = provider
            .tool_runtime_executors()
            .into_iter()
            .find(|executor| executor.name().as_str() == "searxng_search")
            .expect("typed SearXNG executor registered");

        let output = executor
            .execute(runtime_invocation(
                r#"{"query":"rust docs","max_results":3}"#,
            ))
            .await
            .expect("typed search succeeds");

        assert_eq!(output.status, ToolOutputStatus::Success);
    }
}
