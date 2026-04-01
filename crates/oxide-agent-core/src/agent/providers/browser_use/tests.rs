use super::response::format_http_error;
use super::*;
use reqwest::StatusCode;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[test]
fn test_args_deserialize() {
    let run: Result<RunTaskArgs, _> = serde_json::from_str(
        r#"{"task":"Open docs","start_url":"https://example.com","session_id":"s1","timeout_secs":120}"#,
    );
    assert!(run.is_ok());

    let session: Result<SessionArgs, _> = serde_json::from_str(r#"{"session_id":"s1"}"#);
    assert!(session.is_ok());
}

#[test]
fn test_url_building() {
    let provider = BrowserUseProvider::with_config(
        "http://localhost:8002/",
        Duration::from_secs(1),
        0,
        Duration::from_secs(1),
        Duration::from_secs(10),
    );
    let url = provider.endpoint_url("/sessions/run");
    assert_eq!(url, "http://localhost:8002/sessions/run");
}

#[test]
fn test_http_error_formatting() {
    let msg = format_http_error(StatusCode::SERVICE_UNAVAILABLE, "bridge unavailable");
    assert!(msg.contains("503"));
    assert!(msg.contains("bridge unavailable"));
}

#[tokio::test]
async fn run_task_posts_to_bridge() {
    let state = Arc::new(TestServerState::default());
    let server = TestServer::spawn(
        Arc::clone(&state),
        json_response(
            r#"{"session_id":"browser-use-123","status":"completed","final_url":"https://example.com","summary":"Done"}"#,
        ),
    )
    .await;
    let provider = BrowserUseProvider::with_config(
        &server.base_url,
        Duration::from_secs(3),
        0,
        Duration::from_secs(1),
        Duration::from_secs(2),
    );

    let result = provider
        .execute(TOOL_RUN_TASK, r#"{"task":"Open example"}"#, None, None)
        .await;

    assert!(result.is_ok());
    let output = result.unwrap_or_default();
    assert!(output.contains("browser-use-123"));
    assert!(state
        .request_line()
        .await
        .contains("POST /sessions/run HTTP/1.1"));
}

#[tokio::test]
async fn get_session_reads_bridge_json() {
    let state = Arc::new(TestServerState::default());
    let server = TestServer::spawn(
        Arc::clone(&state),
        json_response(
            r#"{"session_id":"browser-use-123","status":"completed","current_url":"https://example.com"}"#,
        ),
    )
    .await;
    let provider = BrowserUseProvider::with_config(
        &server.base_url,
        Duration::from_secs(3),
        0,
        Duration::from_secs(1),
        Duration::from_secs(2),
    );

    let result = provider
        .execute(
            TOOL_GET_SESSION,
            r#"{"session_id":"browser-use-123"}"#,
            None,
            None,
        )
        .await;

    assert!(result.is_ok());
    let output = result.unwrap_or_default();
    assert!(output.contains("current_url"));
    assert!(state
        .request_line()
        .await
        .contains("GET /sessions/browser-use-123 HTTP/1.1"));
}

#[derive(Default)]
struct TestServerState {
    request: tokio::sync::Mutex<String>,
}

impl TestServerState {
    async fn record(&self, request: String) {
        *self.request.lock().await = request;
    }

    async fn request_line(&self) -> String {
        self.request
            .lock()
            .await
            .lines()
            .next()
            .unwrap_or_default()
            .to_string()
    }
}

struct TestServer {
    base_url: String,
}

impl TestServer {
    async fn spawn(state: Arc<TestServerState>, response: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("read local addr");
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buffer = vec![0_u8; 4096];
                if let Ok(read) = socket.read(&mut buffer).await {
                    let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                    state.record(request).await;
                }
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });
        Self {
            base_url: format!("http://{addr}"),
        }
    }
}

fn json_response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body,
    )
}
