//! Chrome DevTools Protocol (CDP) client over a single WebSocket.
//!
//! One `CdpClient` owns exactly one WebSocket connection to a CDP page target.
//! Commands are correlated by incrementing `id`; events (messages without an
//! `id` field) are broadcast to subscribers.  Background reader and writer
//! tasks multiplex concurrent commands on the single connection (G3).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use thiserror::Error;
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// Errors from the CDP client.
#[derive(Debug, Error)]
pub enum CdpError {
    /// CDP returned an error response for a command.
    #[error("CDP error: {message}")]
    Cdp {
        code: i64,
        message: String,
        data: Value,
    },
    /// WebSocket transport error.
    #[error("WebSocket error: {0}")]
    WebSocket(String),
    /// JSON serialization/deserialization error.
    #[error("JSON error: {0}")]
    Json(String),
    /// The WebSocket connection was closed before a response arrived.
    #[error("CDP connection closed")]
    ConnectionClosed,
    /// The command did not complete within the timeout.
    #[error("CDP command timed out after {0:?}")]
    Timeout(Duration),
}

/// A CDP event (unsolicited message without an `id`).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CdpEvent {
    /// CDP method name, e.g. `Page.loadEventFired`.
    pub method: String,
    /// Event parameters.
    pub params: Value,
}

/// Type alias for the pending-request map (avoids clippy type_complexity).
type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, CdpError>>>>>;

/// Client for a single CDP WebSocket connection.
///
/// Cloning is cheap — all state is behind `Arc` / channel senders.
/// Multiple clones can issue concurrent `send_command` calls; the background
/// writer serializes writes and the reader routes responses by `id`.
#[derive(Clone)]
pub struct CdpClient {
    next_id: Arc<AtomicU64>,
    pending: PendingMap,
    write_tx: mpsc::Sender<String>,
    event_tx: broadcast::Sender<CdpEvent>,
}

impl CdpClient {
    /// Connect to a CDP WebSocket endpoint and spawn reader/writer tasks.
    ///
    /// Returns the client handle and an initial event subscriber.
    pub async fn connect(ws_url: &str) -> Result<(Self, broadcast::Receiver<CdpEvent>), CdpError> {
        tracing::debug!(url = ws_url, "connecting CDP WebSocket");

        let (ws_stream, _response) = tokio_tungstenite::connect_async(ws_url)
            .await
            .map_err(|e| CdpError::WebSocket(e.to_string()))?;

        let (mut ws_sink, ws_stream) = ws_stream.split();

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let (write_tx, mut write_rx) = mpsc::channel::<String>(64);
        let (event_tx, event_rx) = broadcast::channel::<CdpEvent>(256);

        // --- Reader task ---
        let pending_r = pending.clone();
        let event_tx_r = event_tx.clone();
        tokio::spawn(async move {
            let mut ws_stream = ws_stream;
            while let Some(msg) = ws_stream.next().await {
                match msg {
                    Ok(WsMessage::Text(text)) => {
                        Self::dispatch_text(&text, &pending_r, &event_tx_r).await;
                    }
                    Ok(WsMessage::Binary(data)) => {
                        if let Ok(text) = std::str::from_utf8(&data) {
                            Self::dispatch_text(text, &pending_r, &event_tx_r).await;
                        }
                    }
                    Ok(WsMessage::Close(_)) => {
                        tracing::debug!("CDP WebSocket closed by remote");
                        break;
                    }
                    Ok(_) => {} // Ping / Pong handled by tungstenite automatically
                    Err(e) => {
                        tracing::warn!("CDP WebSocket read error: {e}");
                        break;
                    }
                }
            }
            // Notify all pending requests that the connection is gone.
            let mut map = pending_r.lock().await;
            for (_, sender) in map.drain() {
                let _ = sender.send(Err(CdpError::ConnectionClosed));
            }
        });

        // --- Writer task ---
        tokio::spawn(async move {
            while let Some(json_str) = write_rx.recv().await {
                let msg = WsMessage::Text(json_str.into());
                if ws_sink.send(msg).await.is_err() {
                    tracing::warn!("CDP WebSocket write failed");
                    break;
                }
            }
            // Close the sink when the channel is dropped.
            let _ = ws_sink.close().await;
        });

        let client = Self {
            next_id: Arc::new(AtomicU64::new(1)),
            pending,
            write_tx,
            event_tx,
        };

        Ok((client, event_rx))
    }

    /// Send a CDP command and await its response.
    ///
    /// `params` is serialized as the `params` field; pass `Value::Null` to
    /// omit it.
    pub async fn send_command(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, CdpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let msg = if params.is_null() {
            json!({"id": id, "method": method})
        } else {
            json!({"id": id, "method": method, "params": params})
        };

        let json_str = serde_json::to_string(&msg).map_err(|e| CdpError::Json(e.to_string()))?;

        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.lock().await;
            map.insert(id, tx);
        }

        self.write_tx
            .send(json_str)
            .await
            .map_err(|_| CdpError::ConnectionClosed)?;

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(CdpError::ConnectionClosed),
            Err(_) => {
                // Remove the pending entry on timeout.
                let mut map = self.pending.lock().await;
                map.remove(&id);
                Err(CdpError::Timeout(timeout))
            }
        }
    }

    /// Subscribe to CDP events.
    pub fn subscribe(&self) -> broadcast::Receiver<CdpEvent> {
        self.event_tx.subscribe()
    }

    /// Create an isolated execution context (world) in the given frame.
    ///
    /// Isolated worlds have their own JavaScript heap (global object,
    /// prototypes) but share the DOM.  Page JavaScript that monkey-patches
    /// `document.querySelectorAll` or other DOM APIs does NOT affect code
    /// running in the isolated world — each world gets fresh native wrappers.
    ///
    /// Returns the `executionContextId` to pass to `eval_in_context`.
    ///
    /// Does NOT require `Runtime.enable`.
    pub async fn create_isolated_world(
        &self,
        frame_id: &str,
        world_name: &str,
        timeout: Duration,
    ) -> Result<u64, CdpError> {
        let params = json!({
            "frameId": frame_id,
            "worldName": world_name,
        });
        let result = self
            .send_command("Page.createIsolatedWorld", params, timeout)
            .await?;
        parse_execution_context_id(&result)
    }

    /// Evaluate a JavaScript expression in a specific execution context
    /// (isolated world) and return the value.
    ///
    /// `returnByValue` is true and `awaitPromise` is true, so the expression
    /// may be async and the result is deserialized as JSON.
    ///
    /// Does NOT require `Runtime.enable` — `Runtime.evaluate` is a command,
    /// not an event subscription.
    pub async fn eval_in_context(
        &self,
        context_id: u64,
        expression: &str,
        timeout: Duration,
    ) -> Result<Value, CdpError> {
        let params = json!({
            "expression": expression,
            "contextId": context_id,
            "returnByValue": true,
            "awaitPromise": true,
        });
        let result = self
            .send_command("Runtime.evaluate", params, timeout)
            .await?;
        parse_eval_result(&result)
    }

    /// Evaluate a read-only expression, preferring an isolated world when
    /// available. Falls back to main-world eval if the context is `None` or
    /// stale (e.g., after a client-side navigation destroyed the frame
    /// without going through our `navigate()` method).
    ///
    /// Both paths return the extracted result value (via `parse_eval_result`).
    /// Does NOT require `Runtime.enable` — `Runtime.evaluate` is a command,
    /// not an event subscription.
    pub async fn eval_readonly(
        &self,
        context_id: Option<u64>,
        expression: &str,
        timeout: Duration,
    ) -> Result<Value, CdpError> {
        if let Some(ctx) = context_id {
            match self.eval_in_context(ctx, expression, timeout).await {
                Ok(value) => return Ok(value),
                Err(e) => {
                    tracing::debug!(
                        error = %e,
                        "isolated world eval failed, falling back to main world"
                    );
                }
            }
        }
        let raw = self
            .send_command(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "returnByValue": true,
                    "awaitPromise": true,
                }),
                timeout,
            )
            .await?;
        parse_eval_result(&raw)
    }

    /// Whether the underlying connection is still alive (writer channel open).
    #[allow(dead_code)]
    pub fn is_alive(&self) -> bool {
        !self.write_tx.is_closed()
    }

    // ── Internal ──────────────────────────────────────────────────────

    /// Parse a text WebSocket frame and route it as a response or event.
    async fn dispatch_text(
        text: &str,
        pending: &PendingMap,
        event_tx: &broadcast::Sender<CdpEvent>,
    ) {
        let parsed: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(_) => return,
        };

        if let Some(id) = parsed.get("id").and_then(|v| v.as_u64()) {
            // Response to a command.
            let result = if let Some(err) = parsed.get("error") {
                Err(CdpError::Cdp {
                    code: err.get("code").and_then(|v| v.as_i64()).unwrap_or(0),
                    message: err
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string(),
                    data: err.get("data").cloned().unwrap_or(Value::Null),
                })
            } else {
                Ok(parsed.get("result").cloned().unwrap_or(Value::Null))
            };

            let mut map = pending.lock().await;
            if let Some(sender) = map.remove(&id) {
                let _ = sender.send(result);
            }
        } else if let Some(method) = parsed.get("method").and_then(|v| v.as_str()) {
            // Event.
            let event = CdpEvent {
                method: method.to_string(),
                params: parsed.get("params").cloned().unwrap_or(Value::Null),
            };
            let _ = event_tx.send(event);
        }
    }
}

/// Extract `executionContextId` from a `Page.createIsolatedWorld` response.
fn parse_execution_context_id(result: &Value) -> Result<u64, CdpError> {
    result
        .get("executionContextId")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| CdpError::Json("missing executionContextId in response".into()))
}

/// Extract the value from a `Runtime.evaluate` response, or return an error
/// if the evaluation threw an exception.
fn parse_eval_result(result: &Value) -> Result<Value, CdpError> {
    if let Some(exception) = result.get("exceptionDetails")
        && !exception.is_null()
    {
        let text = exception
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(CdpError::Cdp {
            code: 0,
            message: format!("JS exception: {text}"),
            data: exception.clone(),
        });
    }
    Ok(result
        .get("result")
        .and_then(|r| r.get("value"))
        .cloned()
        .unwrap_or(Value::Null))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cdp_error_display() {
        let err = CdpError::Timeout(Duration::from_secs(5));
        assert!(err.to_string().contains("5s"));
    }

    #[test]
    fn cdp_event_clone() {
        let event = CdpEvent {
            method: "Page.loadEventFired".to_string(),
            params: json!({"timestamp": 12345.0}),
        };
        let cloned = event.clone();
        assert_eq!(event.method, cloned.method);
    }

    #[test]
    fn parse_execution_context_id_extracts_id() {
        let resp = json!({"executionContextId": 42});
        assert_eq!(
            parse_execution_context_id(&resp).expect("parse context id"),
            42
        );
    }

    #[test]
    fn parse_execution_context_id_errors_when_missing() {
        let resp = json!({"someOtherField": true});
        assert!(parse_execution_context_id(&resp).is_err());
    }

    #[test]
    fn parse_eval_result_extracts_value() {
        let resp = json!({
            "result": {"type": "string", "value": "hello"},
            "exceptionDetails": null
        });
        assert_eq!(
            parse_eval_result(&resp).expect("parse eval"),
            json!("hello")
        );
    }

    #[test]
    fn parse_eval_result_returns_null_when_no_value() {
        let resp = json!({
            "result": {"type": "undefined"},
        });
        assert_eq!(parse_eval_result(&resp).expect("parse eval"), Value::Null);
    }

    #[test]
    fn parse_eval_result_errors_on_exception() {
        let resp = json!({
            "result": {"type": "object"},
            "exceptionDetails": {
                "text": "SyntaxError: unexpected token",
                "exceptionId": 1,
            }
        });
        let err = parse_eval_result(&resp).expect_err("expected JS exception");
        assert!(err.to_string().contains("SyntaxError"));
    }
}
