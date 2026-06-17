use super::actions::{BrowserExecutePlan, plan_browser_action};
use super::artifacts::{BrowserArtifactPurpose, BrowserArtifactSettings};
use super::client::{BrowserSidecar, BrowserSidecarClient, IdempotencyKey};
use super::error::BrowserSidecarError;
use super::metrics::BrowserMetricsCollector;
use super::session::{BrowserFrame, BrowserSessionState};
use super::types::{
    ActionRequest, BrowserAction, BrowserObservation, BrowserProfile, CloseReason,
    CloseSessionRequest, ConsoleDebugQuery, ConsoleLevel, CreateSessionRequest, DebugLevel,
    NetworkDebugQuery, NetworkFilter, ObserveQuery, ScreenshotArtifact, ScreenshotFormat,
    ScreenshotQuery, Viewport,
};
use super::verification::{
    BrowserActionVerification, BrowserVerificationStatus, timeout_report, verify_by_result,
    verify_navigation, verify_sidecar_action,
};
use crate::agent::progress::{AgentEvent, AgentEventSource};
use crate::agent::tool_runtime::{
    OutputNormalizer, ToolExecutor, ToolInvocation, ToolName, ToolOutput,
    ToolOutputImageAttachment, ToolRuntimeConfig, ToolRuntimeError,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc::Sender};
use tokio::time::{Duration, timeout};
use tracing::instrument;

/// `browser_start` tool name.
pub const TOOL_BROWSER_START: &str = "browser_start";
/// `browser_observe` tool name.
pub const TOOL_BROWSER_OBSERVE: &str = "browser_observe";
/// `browser_execute` tool name.
pub const TOOL_BROWSER_EXECUTE: &str = "browser_execute";
/// `browser_extract` tool name.
pub const TOOL_BROWSER_EXTRACT: &str = "browser_extract";
/// `browser_debug` tool name.
pub const TOOL_BROWSER_DEBUG: &str = "browser_debug";
/// `browser_close` tool name.
pub const TOOL_BROWSER_CLOSE: &str = "browser_close";

/// Browser Live provider backing native tool executors.
#[derive(Clone)]
pub struct BrowserLiveProvider {
    sidecar: Arc<dyn BrowserSidecar>,
    states: Arc<Mutex<BTreeMap<String, BrowserSessionState>>>,
    artifact_settings: BrowserArtifactSettings,
    progress_tx: Option<Sender<AgentEvent>>,
    metrics: Arc<BrowserMetricsCollector>,
}

/// Result returned by the `browser_observe` tool executor.
///
/// Contains the compact model-facing JSON payload plus an optional image
/// attachment for vision-capable main-agent routes.
#[derive(Debug, Clone)]
struct ObserveToolResult {
    payload: Value,
    image_attachment: Option<ToolOutputImageAttachment>,
}

/// Result returned by the `browser_execute` tool executor.
///
/// Contains the compact model-facing JSON payload plus an optional post-action
/// screenshot image attachment.
#[derive(Debug, Clone)]
struct ExecuteToolResult {
    payload: Value,
    image_attachment: Option<ToolOutputImageAttachment>,
}

fn screenshot_image_attachment(frame: &BrowserFrame) -> Option<ToolOutputImageAttachment> {
    if frame.screenshot.redacted || frame.screenshot.byte_size == 0 {
        return None;
    }
    let file_name = frame
        .artifact
        .local_path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| format!("screenshot-{}.png", frame.screenshot.screenshot_id));
    Some(ToolOutputImageAttachment::image(
        file_name,
        Some(frame.screenshot.mime_type.clone()),
        frame.screenshot.byte_size,
        frame.artifact.local_path.to_string_lossy().to_string(),
    ))
}

impl BrowserLiveProvider {
    /// Create a provider backed by the real typed sidecar client.
    ///
    /// # Errors
    /// Returns a sidecar client configuration error when URL/token are invalid.
    pub fn from_sidecar_config(
        base_url: &str,
        token: &str,
        artifact_settings: BrowserArtifactSettings,
        progress_tx: Option<Sender<AgentEvent>>,
    ) -> Result<Self, BrowserSidecarError> {
        Ok(Self::new(
            Arc::new(BrowserSidecarClient::new(base_url, token)?),
            artifact_settings,
            progress_tx,
        ))
    }

    /// Create a provider with an injected sidecar implementation.
    #[must_use]
    pub fn new(
        sidecar: Arc<dyn BrowserSidecar>,
        artifact_settings: BrowserArtifactSettings,
        progress_tx: Option<Sender<AgentEvent>>,
    ) -> Self {
        Self {
            sidecar,
            states: Arc::new(Mutex::new(BTreeMap::new())),
            artifact_settings,
            progress_tx,
            metrics: Arc::new(BrowserMetricsCollector::new()),
        }
    }

    /// Return a snapshot of current browser metrics.
    #[must_use]
    pub fn metrics_snapshot(&self) -> super::metrics::BrowserMetricsSnapshot {
        self.metrics.snapshot()
    }

    /// Build typed runtime executors for Browser Live tools.
    #[must_use]
    pub fn tool_runtime_executors(self: &Arc<Self>) -> Vec<Arc<dyn ToolExecutor>> {
        [
            TOOL_BROWSER_START,
            TOOL_BROWSER_OBSERVE,
            TOOL_BROWSER_EXECUTE,
            TOOL_BROWSER_EXTRACT,
            TOOL_BROWSER_DEBUG,
            TOOL_BROWSER_CLOSE,
        ]
        .into_iter()
        .map(|tool| {
            Arc::new(BrowserLiveToolExecutor {
                provider: Arc::clone(self),
                tool_name: tool,
            }) as Arc<dyn ToolExecutor>
        })
        .collect()
    }

    async fn emit_progress(&self, summary: impl Into<String>) {
        if let Some(tx) = &self.progress_tx {
            let _ = tx
                .send(AgentEvent::Reasoning {
                    source: AgentEventSource::Root,
                    summary: summary.into(),
                })
                .await;
        }
    }

    async fn measure_sidecar<T>(
        &self,
        f: impl std::future::Future<Output = Result<T, BrowserSidecarError>>,
    ) -> Result<T, BrowserSidecarError> {
        let start = tokio::time::Instant::now();
        let result = f.await;
        let latency = start.elapsed();
        self.metrics.record_sidecar_request(latency);
        if result.is_err() {
            self.metrics.record_sidecar_error();
        }
        result
    }

    fn record_observation_metrics(&self, frame: &super::session::BrowserFrame) {
        self.metrics.record_observation_fetched();
        self.metrics
            .record_screenshot_captured(frame.screenshot.byte_size);
    }

    #[instrument(
        name = "browser_start",
        skip(self, invocation, args),
        fields(task_id, start_url)
    )]
    async fn start(
        &self,
        invocation: &ToolInvocation,
        args: StartArgs,
    ) -> Result<Value, ToolRuntimeError> {
        ensure_not_cancelled(invocation)?;
        let viewport = args.viewport.unwrap_or_default();
        let task_id = args
            .task_id
            .clone()
            .unwrap_or_else(|| invocation.session_id.to_string());
        tracing::Span::current().record("task_id", tracing::field::display(&task_id));
        if let Some(start_url) = &args.start_url {
            tracing::Span::current().record("start_url", tracing::field::display(start_url));
        }
        let request = CreateSessionRequest {
            task_id: task_id.clone(),
            profile: BrowserProfile::Ephemeral,
            viewport,
            timezone: args.timezone,
            locale: args.locale,
            record_console: true,
            record_network: true,
            allow_downloads: false,
            allow_uploads: false,
            start_url: args.start_url,
        };
        let key = idempotency_key(invocation, "start", &request.task_id)?;
        let response = self
            .measure_sidecar(self.sidecar.create_session(&request, &key))
            .await
            .map_err(sidecar_runtime_error)?;
        let state = BrowserSessionState::new(
            request.task_id,
            response.session_id.clone(),
            response.viewport,
            self.artifact_settings.clone(),
        );
        self.states
            .lock()
            .await
            .insert(response.session_id.clone(), state);
        self.metrics.record_session_start();
        tracing::info!(
            session_id = %response.session_id,
            task_id = %task_id,
            "browser session started"
        );
        self.emit_progress(format!("Browser session {} started", response.session_id))
            .await;

        Ok(json!({
            "status": "started",
            "session_id": response.session_id,
            "artifact_root": response.artifact_root,
            "viewport": response.viewport,
            "browser": response.browser,
        }))
    }

    #[instrument(
        name = "browser_observe",
        skip(self, invocation, args),
        fields(session_id = %args.session_id),
    )]
    async fn observe(
        &self,
        invocation: &ToolInvocation,
        args: ObserveArgs,
    ) -> Result<ObserveToolResult, ToolRuntimeError> {
        ensure_not_cancelled(invocation)?;
        let query = ObserveQuery {
            fresh: args.fresh,
            include_dom: false,
            include_a11y: args.include_a11y,
            include_network_summary: true,
            include_console_summary: true,
            max_debug_items: args.max_debug_items.unwrap_or(20),
        };
        let response = self
            .measure_sidecar(self.sidecar.observe(&args.session_id, &query))
            .await
            .map_err(sidecar_runtime_error)?;
        let mut frame = {
            let mut states = self.states.lock().await;
            let state = states.get_mut(&args.session_id).ok_or_else(|| {
                ToolRuntimeError::Failure("browser session is not started".to_string())
            })?;
            state
                .record_observation(&response.observation, BrowserArtifactPurpose::LiveFrame, 0)
                .map_err(|error| ToolRuntimeError::Failure(error.to_string()))?
                .clone()
        };
        self.record_observation_metrics(&frame);
        tracing::info!(
            session_id = %args.session_id,
            action_seq = frame.action_seq,
            "browser observation recorded"
        );
        self.emit_progress(format!(
            "Browser session {} observed at action_seq {}",
            args.session_id, frame.action_seq
        ))
        .await;
        self.persist_latest_screenshot(&args.session_id, &mut frame)
            .await?;

        let image_attachment = screenshot_image_attachment(&frame);
        Ok(ObserveToolResult {
            payload: observation_payload(&args.session_id, &frame.screenshot, &frame),
            image_attachment,
        })
    }

    #[instrument(
        name = "browser_execute",
        skip(self, invocation, args),
        fields(session_id = %args.session_id, action_seq),
    )]
    async fn execute(
        &self,
        invocation: &ToolInvocation,
        args: ExecuteArgs,
    ) -> Result<ExecuteToolResult, ToolRuntimeError> {
        ensure_not_cancelled(invocation)?;
        let action_seq = {
            let states = self.states.lock().await;
            let state = states.get(&args.session_id).ok_or_else(|| {
                ToolRuntimeError::Failure("browser session is not started".to_string())
            })?;
            state.action_seq().saturating_add(1)
        };
        tracing::Span::current().record("action_seq", action_seq);

        let expected_result = args
            .expected_result
            .as_deref()
            .unwrap_or("browser action executed");
        let timeout_ms = args.timeout_ms.unwrap_or(30_000).clamp(1, 60_000);
        let plan = plan_browser_action(
            args.action,
            action_seq,
            timeout_ms,
            expected_result.to_string(),
        );

        let action_kind = match &plan {
            BrowserExecutePlan::Navigate(_) => "navigate".to_string(),
            BrowserExecutePlan::SidecarAction(request) => serde_json::to_value(&request.action)
                .ok()
                .and_then(|value| {
                    value
                        .get("kind")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "action".to_string()),
        };
        self.emit_progress(format!(
            "BrowserAction session_id={} action_seq={} kind={}",
            args.session_id, action_seq, action_kind
        ))
        .await;

        let before = self.observe_for_timeout(&args.session_id).await?;
        let _before_frame = self
            .record_after_observation(&args.session_id, &before)
            .await?;

        match plan {
            BrowserExecutePlan::Navigate(request) => {
                let key = idempotency_key(
                    invocation,
                    "goto",
                    &format!("{}:{}", args.session_id, action_seq),
                )?;
                let result = timeout(
                    Duration::from_millis(timeout_ms),
                    self.measure_sidecar(self.sidecar.goto(&args.session_id, &request, &key)),
                )
                .await;
                let response = match result {
                    Ok(response) => response.map_err(sidecar_runtime_error)?,
                    Err(_) => {
                        let verification = timeout_report(
                            expected_result,
                            &before,
                            format!(
                                "browser_execute navigation exceeded timeout of {}ms",
                                timeout_ms
                            ),
                        );
                        self.emit_verification(&args.session_id, action_seq, &verification)
                            .await;
                        return Ok(execute_payload(
                            &args.session_id,
                            action_seq,
                            None,
                            None,
                            verification,
                            None,
                        ));
                    }
                };
                ensure_not_cancelled(invocation)?;
                let after = response
                    .observation
                    .as_ref()
                    .cloned()
                    .unwrap_or(self.observe_for_timeout(&args.session_id).await?);
                let (after_frame, after_payload) = self
                    .record_after_observation(&args.session_id, &after)
                    .await?;
                let image_attachment = screenshot_image_attachment(&after_frame);
                let verification =
                    verify_navigation(expected_result, &before, &response.navigation, &after);
                self.emit_verification(&args.session_id, action_seq, &verification)
                    .await;
                let action_result = serde_json::to_value(response.navigation).ok();
                Ok(execute_payload(
                    &args.session_id,
                    action_seq,
                    action_result,
                    Some(after_payload),
                    verification,
                    image_attachment,
                ))
            }
            BrowserExecutePlan::SidecarAction(request) => {
                let key = idempotency_key(
                    invocation,
                    "action",
                    &format!("{}:{}", args.session_id, action_seq),
                )?;
                let result = timeout(
                    Duration::from_millis(timeout_ms),
                    self.measure_sidecar(self.sidecar.execute_action(
                        &args.session_id,
                        &request,
                        &key,
                    )),
                )
                .await;
                let response = match result {
                    Ok(response) => response.map_err(sidecar_runtime_error)?,
                    Err(_) => {
                        let verification = timeout_report(
                            expected_result,
                            &before,
                            format!(
                                "browser_execute action exceeded timeout of {}ms",
                                timeout_ms
                            ),
                        );
                        self.emit_verification(&args.session_id, action_seq, &verification)
                            .await;
                        return Ok(execute_payload(
                            &args.session_id,
                            action_seq,
                            None,
                            None,
                            verification,
                            None,
                        ));
                    }
                };
                ensure_not_cancelled(invocation)?;
                if request.capture_after {
                    let after = response
                        .post_observation
                        .as_ref()
                        .cloned()
                        .unwrap_or(self.observe_for_timeout(&args.session_id).await?);
                    let (after_frame, after_payload) = self
                        .record_after_observation(&args.session_id, &after)
                        .await?;
                    let image_attachment = screenshot_image_attachment(&after_frame);
                    let verification = verify_sidecar_action(
                        expected_result,
                        &before,
                        &response.action_result,
                        &after,
                    );
                    self.emit_verification(&args.session_id, action_seq, &verification)
                        .await;
                    Ok(execute_payload(
                        &args.session_id,
                        action_seq,
                        serde_json::to_value(response.action_result).ok(),
                        Some(after_payload),
                        verification,
                        image_attachment,
                    ))
                } else {
                    let verification =
                        verify_by_result(expected_result, &before, &response.action_result);
                    self.emit_verification(&args.session_id, action_seq, &verification)
                        .await;
                    Ok(execute_payload(
                        &args.session_id,
                        action_seq,
                        serde_json::to_value(response.action_result).ok(),
                        None,
                        verification,
                        None,
                    ))
                }
            }
        }
    }

    #[instrument(
        name = "browser_extract",
        skip(self, invocation, args),
        fields(session_id = %args.session_id, source = ?args.source),
    )]
    async fn extract(
        &self,
        invocation: &ToolInvocation,
        args: ExtractArgs,
    ) -> Result<Value, ToolRuntimeError> {
        ensure_not_cancelled(invocation)?;
        let _ = self
            .states
            .lock()
            .await
            .get(&args.session_id)
            .ok_or_else(|| {
                ToolRuntimeError::Failure("browser session is not started".to_string())
            })?;
        self.emit_progress(format!(
            "BrowserExtract session_id={} source={:?}",
            args.session_id, args.source
        ))
        .await;

        let max_results = args.max_results.unwrap_or(10).clamp(1, 100);
        let matches = match args.source {
            ExtractSource::Dom => extract_from_dom(&args, self, invocation).await?,
            ExtractSource::Network => extract_from_network(&args, self, max_results).await?,
        };

        tracing::info!(
            session_id = %args.session_id,
            source = ?args.source,
            matches = matches.len(),
            "browser extract completed"
        );
        Ok(json!({
            "status": "extracted",
            "source": match args.source {
                ExtractSource::Dom => "dom",
                ExtractSource::Network => "network",
            },
            "session_id": args.session_id,
            "matches": matches,
        }))
    }

    async fn observe_for_timeout(
        &self,
        session_id: &str,
    ) -> Result<BrowserObservation, ToolRuntimeError> {
        Ok(self
            .measure_sidecar(self.sidecar.observe(
                session_id,
                &ObserveQuery {
                    fresh: true,
                    include_dom: false,
                    include_a11y: false,
                    include_network_summary: true,
                    include_console_summary: true,
                    max_debug_items: 20,
                },
            ))
            .await
            .map_err(sidecar_runtime_error)?
            .observation)
    }

    async fn record_after_observation(
        &self,
        session_id: &str,
        observation: &BrowserObservation,
    ) -> Result<(BrowserFrame, Value), ToolRuntimeError> {
        let mut frame = {
            let mut states = self.states.lock().await;
            let state = states.get_mut(session_id).ok_or_else(|| {
                ToolRuntimeError::Failure("browser session is not started".to_string())
            })?;
            state
                .record_observation(observation, BrowserArtifactPurpose::Milestone, 0)
                .map_err(|error| ToolRuntimeError::Failure(error.to_string()))?
                .clone()
        };
        self.record_observation_metrics(&frame);
        self.persist_latest_screenshot(session_id, &mut frame)
            .await?;
        let payload = observation_payload(session_id, &frame.screenshot, &frame);
        Ok((frame, payload))
    }

    async fn persist_latest_screenshot(
        &self,
        session_id: &str,
        frame: &mut BrowserFrame,
    ) -> Result<Vec<u8>, ToolRuntimeError> {
        let bytes = self
            .measure_sidecar(self.sidecar.latest_screenshot_bytes(
                session_id,
                &ScreenshotQuery {
                    format: ScreenshotFormat::Binary,
                    max_width: None,
                    redacted: false,
                },
            ))
            .await
            .map_err(sidecar_runtime_error)?;
        if let Some(parent) = frame.artifact.local_path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|error| {
                ToolRuntimeError::Failure(format!(
                    "failed to create screenshot artifact directory: {error}"
                ))
            })?;
        }
        tokio::fs::write(&frame.artifact.local_path, &bytes)
            .await
            .map_err(|error| {
                ToolRuntimeError::Failure(format!(
                    "failed to write screenshot artifact {}: {error}",
                    frame.artifact.local_path.display()
                ))
            })?;
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        let byte_size = bytes.len() as u64;
        {
            let mut states = self.states.lock().await;
            let state = states.get_mut(session_id).ok_or_else(|| {
                ToolRuntimeError::Failure("browser session is not started".to_string())
            })?;
            state
                .update_latest_artifact_bytes(&bytes, sha256.clone())
                .map_err(|error| ToolRuntimeError::Failure(error.to_string()))?;
        }
        frame.screenshot.byte_size = byte_size;
        frame.screenshot.sha256 = sha256.clone();
        frame.artifact.bytes = byte_size;
        frame.artifact.sha256 = Some(sha256);
        Ok(bytes)
    }

    async fn emit_verification(
        &self,
        session_id: &str,
        action_seq: u64,
        verification: &BrowserActionVerification,
    ) {
        self.emit_progress(format!(
            "BrowserVerification session_id={} action_seq={} status={:?}",
            session_id, action_seq, verification.status
        ))
        .await;
    }

    #[instrument(
        name = "browser_debug",
        skip(self, invocation, args),
        fields(session_id = %args.session_id),
    )]
    async fn debug(
        &self,
        invocation: &ToolInvocation,
        args: DebugArgs,
    ) -> Result<Value, ToolRuntimeError> {
        ensure_not_cancelled(invocation)?;
        let network = if args.include_network {
            Some(
                self.measure_sidecar(self.sidecar.debug_network(
                    &args.session_id,
                    &NetworkDebugQuery {
                        since_action_seq: args.since_action_seq.unwrap_or_default(),
                        level: DebugLevel::Summary,
                        include_bodies: false,
                        filter: NetworkFilter::Failed,
                        limit: args.limit.unwrap_or(20),
                    },
                ))
                .await
                .map_err(sidecar_runtime_error)?
                .network,
            )
        } else {
            None
        };
        let console = if args.include_console {
            Some(
                self.measure_sidecar(self.sidecar.debug_console(
                    &args.session_id,
                    &ConsoleDebugQuery {
                        since_action_seq: args.since_action_seq.unwrap_or_default(),
                        level: DebugLevel::Summary,
                        min_level: ConsoleLevel::Error,
                        limit: args.limit.unwrap_or(20),
                    },
                ))
                .await
                .map_err(sidecar_runtime_error)?
                .console,
            )
        } else {
            None
        };
        tracing::info!(session_id = %args.session_id, "browser debug fetched");
        self.emit_progress(format!("Browser debug fetched for {}", args.session_id))
            .await;
        Ok(json!({
            "status": "debug",
            "session_id": args.session_id,
            "network": network,
            "console": console,
        }))
    }

    #[instrument(
        name = "browser_close",
        skip(self, invocation, args),
        fields(session_id = %args.session_id),
    )]
    async fn close(
        &self,
        invocation: &ToolInvocation,
        args: CloseArgs,
    ) -> Result<Value, ToolRuntimeError> {
        ensure_not_cancelled(invocation)?;
        let retained_artifacts = self
            .states
            .lock()
            .await
            .get(&args.session_id)
            .map(|state| {
                state
                    .retained_artifacts()
                    .iter()
                    .map(|artifact| artifact.uri.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let key = idempotency_key(invocation, "close", &args.session_id)?;
        let response = self
            .measure_sidecar(self.sidecar.close_session(
                &args.session_id,
                &CloseSessionRequest {
                    purge_profile: true,
                    keep_artifacts: args.keep_artifacts.unwrap_or(true),
                    reason: args.reason.unwrap_or(CloseReason::Done),
                },
                &key,
            ))
            .await
            .map_err(sidecar_runtime_error)?;
        self.states.lock().await.remove(&args.session_id);
        self.metrics.record_session_close();
        tracing::info!(
            session_id = %args.session_id,
            closed = response.closed,
            profile_purged = response.profile_purged,
            "browser session closed"
        );
        self.emit_progress(format!("Browser session {} closed", args.session_id))
            .await;
        let metrics = self.metrics.snapshot();
        Ok(json!({
            "status": "closed",
            "session_id": response.session_id,
            "closed": response.closed,
            "profile_purged": response.profile_purged,
            "artifacts_kept": response.artifacts_kept,
            "retained_artifact_refs": retained_artifacts,
            "metrics": metrics,
        }))
    }
}

struct BrowserLiveToolExecutor {
    provider: Arc<BrowserLiveProvider>,
    tool_name: &'static str,
}

#[async_trait]
impl ToolExecutor for BrowserLiveToolExecutor {
    fn name(&self) -> ToolName {
        ToolName::new(self.tool_name)
    }

    fn spec(&self) -> crate::llm::ToolDefinition {
        browser_tool_definition(self.tool_name)
    }

    async fn execute(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolRuntimeError> {
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig {
            timeout: invocation.timeout.clone(),
            ..ToolRuntimeConfig::default()
        });
        let (payload, image_attachment) = match self.tool_name {
            TOOL_BROWSER_START => {
                let args = parse_args::<StartArgs>(&invocation)?;
                (self.provider.start(&invocation, args).await?, None)
            }
            TOOL_BROWSER_OBSERVE => {
                let args = parse_args::<ObserveArgs>(&invocation)?;
                let result = self.provider.observe(&invocation, args).await?;
                (result.payload, result.image_attachment)
            }
            TOOL_BROWSER_EXECUTE => {
                let args = parse_args::<ExecuteArgs>(&invocation)?;
                let result = self.provider.execute(&invocation, args).await?;
                (result.payload, result.image_attachment)
            }
            TOOL_BROWSER_EXTRACT => {
                let args = parse_args::<ExtractArgs>(&invocation)?;
                (self.provider.extract(&invocation, args).await?, None)
            }
            TOOL_BROWSER_DEBUG => {
                let args = parse_args::<DebugArgs>(&invocation)?;
                (self.provider.debug(&invocation, args).await?, None)
            }
            TOOL_BROWSER_CLOSE => {
                let args = parse_args::<CloseArgs>(&invocation)?;
                (self.provider.close(&invocation, args).await?, None)
            }
            other => {
                return Err(ToolRuntimeError::Internal(format!(
                    "unknown browser tool {other}"
                )));
            }
        };
        let text = serde_json::to_string_pretty(&payload)
            .map_err(|error| ToolRuntimeError::Internal(error.to_string()))?;
        let mut output = normalizer.success(&invocation, &text, "");
        output.structured_payload = Some(payload);
        if let Some(image) = image_attachment {
            output = output.with_image_attachment(image);
        }
        Ok(output)
    }
}

#[derive(Debug, Deserialize)]
struct StartArgs {
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default)]
    start_url: Option<String>,
    #[serde(default)]
    viewport: Option<Viewport>,
    #[serde(default)]
    timezone: Option<String>,
    #[serde(default)]
    locale: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ObserveArgs {
    session_id: String,
    #[serde(default)]
    fresh: bool,
    #[serde(default)]
    include_a11y: bool,
    #[serde(default)]
    max_debug_items: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ExecuteArgs {
    session_id: String,
    action: BrowserAction,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    expected_result: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExtractSource {
    Dom,
    Network,
}

#[derive(Debug, Deserialize)]
struct ExtractArgs {
    session_id: String,
    source: ExtractSource,
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    attribute: Option<String>,
    #[serde(default)]
    url_pattern: Option<String>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    status_code: Option<u16>,
    #[serde(default)]
    max_results: Option<u32>,
    #[serde(default)]
    include_bodies: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct DebugArgs {
    session_id: String,
    #[serde(default = "default_true")]
    include_network: bool,
    #[serde(default = "default_true")]
    include_console: bool,
    #[serde(default)]
    since_action_seq: Option<u64>,
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct CloseArgs {
    session_id: String,
    #[serde(default)]
    #[serde(rename = "purge_profile")]
    _purge_profile: Option<bool>,
    #[serde(default)]
    keep_artifacts: Option<bool>,
    #[serde(default)]
    reason: Option<CloseReason>,
}

fn parse_args<T>(invocation: &ToolInvocation) -> Result<T, ToolRuntimeError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_str(&invocation.raw_arguments)
        .map_err(|error| ToolRuntimeError::InvalidArguments(error.to_string()))
}

fn ensure_not_cancelled(invocation: &ToolInvocation) -> Result<(), ToolRuntimeError> {
    if invocation.cancellation_token.is_cancelled() {
        return Err(ToolRuntimeError::Failure(
            "browser tool invocation was cancelled".to_string(),
        ));
    }
    Ok(())
}

fn idempotency_key(
    invocation: &ToolInvocation,
    operation: &str,
    suffix: &str,
) -> Result<IdempotencyKey, ToolRuntimeError> {
    IdempotencyKey::new(format!(
        "{}:{}:{}:{}",
        invocation.turn_id, invocation.tool_call_id, operation, suffix
    ))
    .map_err(|error| ToolRuntimeError::InvalidArguments(error.to_string()))
}

fn sidecar_runtime_error(error: BrowserSidecarError) -> ToolRuntimeError {
    match error {
        BrowserSidecarError::MissingToken
        | BrowserSidecarError::MissingIdempotencyKey
        | BrowserSidecarError::InvalidBaseUrl(_)
        | BrowserSidecarError::InvalidSessionId => {
            ToolRuntimeError::InvalidArguments(error.to_string())
        }
        _ => ToolRuntimeError::Failure(error.agent_message()),
    }
}

fn observation_payload(
    session_id: &str,
    screenshot: &ScreenshotArtifact,
    frame: &super::session::BrowserFrame,
) -> Value {
    json!({
        "status": "observed",
        "session_id": session_id,
        "observation_id": frame.observation_id,
        "action_seq": frame.action_seq,
        "url": frame.url,
        "title": frame.title,
        "loading_state": frame.loading_state,
        "network_summary": frame.network_summary,
        "console_summary": frame.console_summary,
        "screenshot": {
            "screenshot_id": screenshot.screenshot_id,
            "artifact_uri": frame.artifact.uri,
            "mime_type": screenshot.mime_type,
            "width": screenshot.width,
            "height": screenshot.height,
            "sha256": screenshot.sha256,
            "redacted": screenshot.redacted,
        }
    })
}

fn execute_payload(
    session_id: &str,
    action_seq: u64,
    action_result: Option<Value>,
    post_observation: Option<Value>,
    verification: BrowserActionVerification,
    image_attachment: Option<ToolOutputImageAttachment>,
) -> ExecuteToolResult {
    let status = match verification.status {
        BrowserVerificationStatus::ActionVerified => "executed",
        BrowserVerificationStatus::VerificationFailed => "verification_failed",
        BrowserVerificationStatus::Done => "done",
        BrowserVerificationStatus::Timeout => "timeout",
    };
    let payload = json!({
        "status": status,
        "session_id": session_id,
        "action_seq": action_seq,
        "action_result": action_result,
        "post_observation": post_observation,
        "verification": verification,
    });
    ExecuteToolResult {
        payload,
        image_attachment,
    }
}

fn browser_tool_definition(name: &str) -> crate::llm::ToolDefinition {
    let (description, parameters) = match name {
        TOOL_BROWSER_START => (
            "Start a task-local autonomous headless browser session.",
            json!({
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "start_url": {"type": "string"},
                    "timezone": {"type": "string"},
                    "locale": {"type": "string"}
                },
                "additionalProperties": false
            }),
        ),
        TOOL_BROWSER_OBSERVE => (
            "Return compact browser state (url, title, loading state, network/console summaries) and attach the latest screenshot as a native image for vision models.",
            json!({
                "type": "object",
                "required": ["session_id"],
                "properties": {
                    "session_id": {"type": "string"},
                    "fresh": {"type": "boolean", "default": false, "description": "capture a fresh screenshot instead of reusing the last cached one"},
                    "include_a11y": {"type": "boolean", "default": false},
                    "max_debug_items": {"type": "integer", "minimum": 0, "maximum": 100}
                },
                "additionalProperties": false
            }),
        ),
        TOOL_BROWSER_EXECUTE => (
            "Execute a single concrete BrowserAction in the session and return the action result plus a post-action screenshot when available.",
            json!({
                "type": "object",
                "required": ["session_id", "action"],
                "properties": {
                    "session_id": {"type": "string"},
                    "action": {
                        "type": "object",
                        "description": "BrowserAction schema: one of click_xy, click_selector, click_target_id, fill, type_text, press, scroll, get_element_value, execute_javascript, wait, wait_for_selector, wait_for_text, script, navigate"
                    },
                    "timeout_ms": {"type": "integer", "minimum": 1, "maximum": 60000},
                    "expected_result": {"type": "string"}
                },
                "additionalProperties": false
            }),
        ),
        TOOL_BROWSER_EXTRACT => (
            "Extract structured data from the current page: network response bodies or DOM element properties.",
            json!({
                "type": "object",
                "required": ["session_id", "source"],
                "properties": {
                    "session_id": {"type": "string"},
                    "source": {"type": "string", "enum": ["dom", "network"]},
                    "selector": {"type": "string", "description": "CSS selector for dom source"},
                    "attribute": {"type": "string", "description": "preferred dom attribute: value, innerText, innerHTML, textContent (all are returned if omitted)"},
                    "url_pattern": {"type": "string", "description": "glob/ substring pattern for network source, e.g. */api/create"},
                    "method": {"type": "string", "description": "HTTP method filter for network source"},
                    "status_code": {"type": "integer", "description": "HTTP status filter for network source"},
                    "max_results": {"type": "integer", "minimum": 1, "maximum": 100},
                    "include_bodies": {"type": "boolean", "default": true, "description": "include response bodies for network source"}
                },
                "additionalProperties": false
            }),
        ),
        TOOL_BROWSER_DEBUG => (
            "Fetch browser console/network debug summaries as compact artifact-backed diagnostics.",
            json!({
                "type": "object",
                "required": ["session_id"],
                "properties": {
                    "session_id": {"type": "string"},
                    "include_network": {"type": "boolean", "default": true},
                    "include_console": {"type": "boolean", "default": true},
                    "since_action_seq": {"type": "integer", "minimum": 0},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 100}
                },
                "additionalProperties": false
            }),
        ),
        TOOL_BROWSER_CLOSE => (
            "Close a browser session and finalize retained browser artifacts.",
            json!({
                "type": "object",
                "required": ["session_id"],
                "properties": {
                    "session_id": {"type": "string"},
                    "purge_profile": {"type": "boolean", "default": true},
                    "keep_artifacts": {"type": "boolean", "default": true},
                    "reason": {"type": "string", "enum": ["done", "cancelled", "error", "timeout", "user_requested"]}
                },
                "additionalProperties": false
            }),
        ),
        _ => ("Unknown browser tool.", json!({"type": "object"})),
    };
    crate::llm::ToolDefinition {
        name: name.to_string(),
        description: description.to_string(),
        parameters,
    }
}

const fn default_true() -> bool {
    true
}

async fn extract_from_network(
    args: &ExtractArgs,
    provider: &BrowserLiveProvider,
    max_results: u32,
) -> Result<Vec<Value>, ToolRuntimeError> {
    let query = NetworkDebugQuery {
        since_action_seq: 0,
        level: DebugLevel::Summary,
        include_bodies: args.include_bodies.unwrap_or(true),
        filter: NetworkFilter::All,
        limit: max_results,
    };
    let response = provider
        .measure_sidecar(provider.sidecar.debug_network(&args.session_id, &query))
        .await
        .map_err(sidecar_runtime_error)?;
    let mut matches = Vec::new();
    for item in response.network.items {
        let method = item.method.to_ascii_uppercase();
        if let Some(ref expected) = args.method
            && method != expected.to_ascii_uppercase()
        {
            continue;
        }
        if let Some(expected_status) = args.status_code
            && item.status != Some(expected_status)
        {
            continue;
        }
        if let Some(ref pattern) = args.url_pattern
            && !url_matches_pattern(&item.url_redacted, pattern)
        {
            continue;
        }
        matches.push(json!({
            "timestamp": item.timestamp,
            "method": method,
            "url": item.url_redacted,
            "status": item.status,
            "resource_type": item.resource_type,
            "error_text": item.error_text,
            "body": item.body,
        }));
        if matches.len() >= max_results as usize {
            break;
        }
    }
    Ok(matches)
}

async fn extract_from_dom(
    args: &ExtractArgs,
    provider: &BrowserLiveProvider,
    invocation: &ToolInvocation,
) -> Result<Vec<Value>, ToolRuntimeError> {
    let selector = args.selector.as_deref().ok_or_else(|| {
        ToolRuntimeError::InvalidArguments("dom source requires selector".to_string())
    })?;
    let expression = dom_extract_expression(selector);
    let action_seq = {
        let states = provider.states.lock().await;
        let state = states.get(&args.session_id).ok_or_else(|| {
            ToolRuntimeError::Failure("browser session is not started".to_string())
        })?;
        state.action_seq().saturating_add(1)
    };
    let key = idempotency_key(
        invocation,
        "extract",
        &format!("{}:{}", args.session_id, action_seq),
    )?;
    let request = ActionRequest {
        action_seq,
        action: BrowserAction::ExecuteJavaScript { expression },
        expected_result: "extract DOM data".to_string(),
        timeout_ms: 10_000,
        capture_after: false,
        wait_for_stability: false,
    };
    let response = provider
        .measure_sidecar(
            provider
                .sidecar
                .execute_action(&args.session_id, &request, &key),
        )
        .await
        .map_err(sidecar_runtime_error)?;
    let raw = response.action_result.result.unwrap_or_default();
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let parsed: Value = serde_json::from_str(&raw).map_err(|error| {
        ToolRuntimeError::Failure(format!("failed to parse DOM extraction result: {error}"))
    })?;
    let array = parsed.as_array().ok_or_else(|| {
        ToolRuntimeError::Failure("DOM extraction result is not a JSON array".to_string())
    })?;
    let attribute = args.attribute.as_deref().unwrap_or("innerText");
    Ok(array
        .iter()
        .map(|element| {
            json!({
                "selector": selector,
                "attribute": attribute,
                "value": element.get("value"),
                "innerText": element.get("innerText"),
                "innerHTML": element.get("innerHTML"),
                "textContent": element.get("textContent"),
            })
        })
        .collect())
}

fn url_matches_pattern(url: &str, pattern: &str) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() || pattern == "*" {
        return true;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return url.contains(parts[0]);
    }
    let mut position = 0_usize;
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        let found = url[position..].find(part);
        match found {
            Some(offset) => {
                if index == 0 && offset != 0 {
                    return false;
                }
                position += offset + part.len();
            }
            None => return false,
        }
    }
    if let Some(last) = parts.last()
        && !last.is_empty()
        && !url.ends_with(last)
    {
        return false;
    }
    true
}

fn dom_extract_expression(selector: &str) -> String {
    let selector_json = serde_json::to_string(selector).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        "JSON.stringify(Array.from(document.querySelectorAll(JSON.parse({selector_json}))).map(el => ({{ value: el.value !== undefined ? el.value : null, innerText: el.innerText, innerHTML: el.innerHTML, textContent: el.textContent }})))"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::identity::SessionId;
    use crate::agent::providers::browser_live::test_support::{
        FakeActionOutcome, FakeBrowserSidecar,
    };
    use crate::agent::tool_runtime::{
        ModelMetadata, ProviderMetadata, ToolBatchId, ToolCallId, ToolExecutionContext,
        ToolTimeoutConfig, TurnId,
    };
    use crate::llm::InvocationId;
    use chrono::Utc;
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn browser_tools_register_all_expected_specs() {
        let provider = test_provider();
        let executors = provider.tool_runtime_executors();
        let names = executors
            .iter()
            .map(|executor| executor.name().into_inner())
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            [
                TOOL_BROWSER_START,
                TOOL_BROWSER_OBSERVE,
                TOOL_BROWSER_EXECUTE,
                TOOL_BROWSER_EXTRACT,
                TOOL_BROWSER_DEBUG,
                TOOL_BROWSER_CLOSE
            ]
        );
        assert!(
            executors
                .iter()
                .all(|executor| executor.spec().parameters["type"] == "object")
        );
    }

    #[tokio::test]
    async fn browser_execute_spec_has_action_parameter() {
        let spec = browser_tool_definition(TOOL_BROWSER_EXECUTE);
        let params = spec.parameters;
        assert_eq!(params["required"], json![["session_id", "action"]]);
        assert!(params["properties"].get("action").is_some());
        assert!(params["properties"].get("timeout_ms").is_some());
        assert!(params["properties"].get("expected_result").is_some());
    }

    #[tokio::test]
    async fn browser_extract_spec_has_source_and_filters() {
        let spec = browser_tool_definition(TOOL_BROWSER_EXTRACT);
        let params = spec.parameters;
        assert_eq!(params["required"], json![["session_id", "source"]]);
        assert!(params["properties"].get("source").is_some());
        assert!(params["properties"].get("selector").is_some());
        assert!(params["properties"].get("url_pattern").is_some());
        assert!(params["properties"].get("method").is_some());
        assert!(params["properties"].get("status_code").is_some());
    }

    #[tokio::test]
    async fn browser_extract_network_returns_matching_request_bodies() {
        let fake = FakeBrowserSidecar::new();
        fake.add_network_request(
            "https://example.test/api/create",
            "POST",
            201,
            Some(r#"{"secret_id":"abc"}"#),
        );
        fake.add_network_request("https://example.test/api/other", "GET", 200, None);
        let provider = Arc::new(BrowserLiveProvider::new(
            Arc::new(fake),
            BrowserArtifactSettings::default(),
            None,
        ));
        let executors = provider.tool_runtime_executors();
        let start = execute(&executors, TOOL_BROWSER_START, r#"{"task_id":"task-1"}"#).await;
        let session_id = start.structured_payload.as_ref().expect("payload")["session_id"]
            .as_str()
            .expect("session id")
            .to_string();
        let extract_args = format!(
            r#"{{"session_id":"{session_id}","source":"network","url_pattern":"*/api/create","method":"POST","status_code":201}}"#
        );
        let result = execute(&executors, TOOL_BROWSER_EXTRACT, &extract_args).await;
        let payload = result.structured_payload.as_ref().expect("payload");
        assert_eq!(payload["status"], "extracted");
        assert_eq!(payload["source"], "network");
        let matches = payload["matches"].as_array().expect("matches array");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0]["method"], "POST");
        assert_eq!(matches[0]["status"], 201);
        assert_eq!(matches[0]["body"], r#"{"secret_id":"abc"}"#);
    }

    #[tokio::test]
    async fn browser_extract_dom_returns_js_result_elements() {
        let js_result = serde_json::json!([
            {"value": "hello", "innerText": "hello", "innerHTML": "<b>hello</b>", "textContent": "hello"}
        ]);
        let fake = FakeBrowserSidecar::new()
            .with_action_script(vec![FakeActionOutcome::JsResult(js_result.to_string())]);
        let provider = Arc::new(BrowserLiveProvider::new(
            Arc::new(fake),
            BrowserArtifactSettings::default(),
            None,
        ));
        let executors = provider.tool_runtime_executors();
        let start = execute(&executors, TOOL_BROWSER_START, r#"{"task_id":"task-1"}"#).await;
        let session_id = start.structured_payload.as_ref().expect("payload")["session_id"]
            .as_str()
            .expect("session id")
            .to_string();
        let extract_args = format!(
            r#"{{"session_id":"{session_id}","source":"dom","selector":"input[readonly]","attribute":"value"}}"#
        );
        let result = execute(&executors, TOOL_BROWSER_EXTRACT, &extract_args).await;
        let payload = result.structured_payload.as_ref().expect("payload");
        assert_eq!(payload["status"], "extracted");
        assert_eq!(payload["source"], "dom");
        let matches = payload["matches"].as_array().expect("matches array");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0]["value"], "hello");
        assert_eq!(matches[0]["innerText"], "hello");
    }

    #[test]
    fn url_matches_pattern_substring_and_wildcards() {
        assert!(url_matches_pattern(
            "https://example.test/api/create",
            "*/api/create"
        ));
        assert!(url_matches_pattern(
            "https://example.test/api/create",
            "*api*"
        ));
        assert!(url_matches_pattern(
            "https://example.test/api/create",
            "https://example.test/*"
        ));
        assert!(!url_matches_pattern(
            "https://example.test/api/list",
            "*/api/create"
        ));
        assert!(url_matches_pattern("https://example.test/api/create", ""));
        assert!(url_matches_pattern("https://example.test/api/create", "*"));
    }

    #[tokio::test]
    async fn start_observe_close_with_fake_sidecar_returns_compact_outputs() {
        let provider = test_provider();
        let executors = provider.tool_runtime_executors();
        let start = execute(
            &executors,
            TOOL_BROWSER_START,
            r#"{"task_id":"task-1","start_url":"https://example.test"}"#,
        )
        .await;
        let session_id = start.structured_payload.as_ref().expect("payload")["session_id"]
            .as_str()
            .expect("session id")
            .to_string();

        let observe_args = format!(r#"{{"session_id":"{session_id}","fresh":true}}"#);
        let observe = execute(&executors, TOOL_BROWSER_OBSERVE, &observe_args).await;
        let close_args = format!(r#"{{"session_id":"{session_id}"}}"#);
        let close = execute(&executors, TOOL_BROWSER_CLOSE, &close_args).await;

        assert!(start.success);
        assert!(observe.success);
        assert!(close.success);
        let observe_text = observe.stdout.text.as_deref().expect("observe stdout");
        assert!(observe_text.contains("artifact://browser/task-1/"));
        assert!(!observe_text.contains("base64"));
        assert!(!observe_text.contains("data:image"));
        let image = observe
            .image_attachment
            .as_ref()
            .expect("observe screenshot image attachment");
        assert!(
            image
                .mime_type
                .as_deref()
                .expect("mime")
                .starts_with("image/")
        );
        assert!(image.size_bytes > 0);
        assert!(std::path::Path::new(&image.sandbox_path).exists());
        assert!(image.sandbox_path.contains("step-"));
        assert_eq!(
            close.structured_payload.as_ref().expect("payload")["status"],
            "closed"
        );
    }

    #[test]
    fn screenshot_image_attachment_skips_redacted_or_empty_screenshots() {
        use crate::agent::providers::browser_live::types::LoadingState;
        use crate::agent::tool_runtime::artifacts::{ArtifactKind, ArtifactRef};
        let base = BrowserFrame {
            observation_id: "obs-1".to_string(),
            action_seq: 1,
            screenshot: ScreenshotArtifact {
                screenshot_id: "shot-1".to_string(),
                artifact_uri: "browser/task/session/shot-1.jpg".to_string(),
                mime_type: "image/jpeg".to_string(),
                width: 1365,
                height: 768,
                sha256: "sha-1".to_string(),
                captured_at: None,
                redacted: false,
                byte_size: 42,
            },
            url: "https://example.test".to_string(),
            title: "Example".to_string(),
            loading_state: LoadingState::Idle,
            network_summary: None,
            console_summary: None,
            artifact: ArtifactRef::internal(
                "artifact://browser/task/session/step-0001-live.png",
                std::path::PathBuf::from("/tmp/step-0001-live.png"),
                ArtifactKind::File,
                42,
            ),
            retained: false,
        };
        assert!(screenshot_image_attachment(&base).is_some());

        let mut redacted = base.clone();
        redacted.screenshot.redacted = true;
        assert!(screenshot_image_attachment(&redacted).is_none());

        let mut empty = base;
        empty.screenshot.byte_size = 0;
        assert!(screenshot_image_attachment(&empty).is_none());
    }

    #[tokio::test]
    async fn browser_start_yolo_allows_any_start_url() {
        let provider = test_provider();
        let executors = provider.tool_runtime_executors();

        let start = execute(
            &executors,
            TOOL_BROWSER_START,
            r#"{"task_id":"task-1","start_url":"file:///etc/passwd"}"#,
        )
        .await;
        assert!(start.success);

        let start = execute(
            &executors,
            TOOL_BROWSER_START,
            r#"{"task_id":"task-2","start_url":"https://example.test"}"#,
        )
        .await;
        assert!(start.success);
    }

    #[tokio::test]
    async fn browser_close_always_purges_ephemeral_profile() {
        let provider = test_provider();
        let executors = provider.tool_runtime_executors();
        let start = execute(&executors, TOOL_BROWSER_START, r#"{"task_id":"task-1"}"#).await;
        let session_id = start.structured_payload.as_ref().expect("payload")["session_id"]
            .as_str()
            .expect("session id");
        let close_args = format!(r#"{{"session_id":"{session_id}","purge_profile":false}}"#);

        let close = execute(&executors, TOOL_BROWSER_CLOSE, &close_args).await;

        assert_eq!(
            close.structured_payload.as_ref().expect("payload")["profile_purged"],
            true
        );
    }

    #[tokio::test]
    async fn browser_execute_direct_click_returns_executed_and_screenshot_attachment() {
        let provider = test_provider();
        let executors = provider.tool_runtime_executors();
        let start = execute(&executors, TOOL_BROWSER_START, r#"{"task_id":"task-1"}"#).await;
        let session_id = start.structured_payload.as_ref().expect("payload")["session_id"]
            .as_str()
            .expect("session id");
        let execute_args = format!(
            r#"{{"session_id":"{session_id}","action":{{"kind":"click_xy","x":10,"y":20,"target_description":"login"}},"expected_result":"button clicked"}}"#
        );

        let result = execute(&executors, TOOL_BROWSER_EXECUTE, &execute_args).await;
        let payload = result.structured_payload.as_ref().expect("payload");

        assert!(result.success);
        assert_eq!(payload["status"], "executed");
        assert_eq!(payload["action_result"]["status"], "executed");
        assert_eq!(payload["action_result"]["kind"], "click_xy");
        assert!(payload["post_observation"].is_object());
        let image = result
            .image_attachment
            .as_ref()
            .expect("post-action screenshot image attachment");
        assert!(
            image
                .mime_type
                .as_deref()
                .expect("mime")
                .starts_with("image/")
        );
        assert!(image.size_bytes > 0);
    }

    #[tokio::test]
    async fn browser_execute_direct_navigate_returns_executed_and_final_url() {
        let provider = test_provider();
        let executors = provider.tool_runtime_executors();
        let start = execute(&executors, TOOL_BROWSER_START, r#"{"task_id":"task-1"}"#).await;
        let session_id = start.structured_payload.as_ref().expect("payload")["session_id"]
            .as_str()
            .expect("session id");
        let execute_args = format!(
            r#"{{"session_id":"{session_id}","action":{{"kind":"navigate","url":"https://example.test/dashboard"}},"expected_result":"navigated"}}"#
        );

        let result = execute(&executors, TOOL_BROWSER_EXECUTE, &execute_args).await;
        let payload = result.structured_payload.as_ref().expect("payload");

        assert!(result.success);
        assert_eq!(payload["status"], "executed");
        let action_result = payload["action_result"].as_object().expect("action_result");
        assert_eq!(
            action_result.get("final_url").and_then(|v| v.as_str()),
            Some("https://example.test/dashboard")
        );
        assert!(payload["post_observation"].is_object());
    }

    #[tokio::test]
    async fn browser_execute_direct_script_returns_executed() {
        let provider = test_provider();
        let executors = provider.tool_runtime_executors();
        let start = execute(&executors, TOOL_BROWSER_START, r#"{"task_id":"task-1"}"#).await;
        let session_id = start.structured_payload.as_ref().expect("payload")["session_id"]
            .as_str()
            .expect("session id");
        let execute_args = format!(
            r##"{{"session_id":"{session_id}","action":{{"kind":"script","steps":[{{"kind":"fill","selector":"#secret","value":"hello"}},{{"kind":"click_selector","selector":"button[type=submit]"}}]}},"expected_result":"form submitted"}}"##
        );

        let result = execute(&executors, TOOL_BROWSER_EXECUTE, &execute_args).await;
        let payload = result.structured_payload.as_ref().expect("payload");

        assert!(result.success);
        assert_eq!(payload["status"], "executed");
        assert_eq!(payload["action_result"]["kind"], "script");
        assert_eq!(payload["action_result"]["status"], "executed");
    }

    #[tokio::test]
    async fn browser_execute_short_timeout_returns_timeout() {
        let fake =
            FakeBrowserSidecar::new().with_action_script(vec![FakeActionOutcome::DelaySuccess(
                Duration::from_millis(50),
            )]);
        let provider = Arc::new(BrowserLiveProvider::new(
            Arc::new(fake),
            BrowserArtifactSettings::default(),
            None,
        ));
        let executors = provider.tool_runtime_executors();
        let start = execute(&executors, TOOL_BROWSER_START, r#"{"task_id":"task-1"}"#).await;
        let session_id = start.structured_payload.as_ref().expect("payload")["session_id"]
            .as_str()
            .expect("session id");
        let execute_args = format!(
            r#"{{"session_id":"{session_id}","action":{{"kind":"click_xy","x":10,"y":20}},"timeout_ms":5,"expected_result":"click"}}"#
        );

        let result = execute(&executors, TOOL_BROWSER_EXECUTE, &execute_args).await;
        let payload = result.structured_payload.as_ref().expect("payload");

        assert!(result.success);
        assert_eq!(payload["status"], "timeout");
        assert!(
            payload["verification"]["reason"]
                .as_str()
                .expect("reason")
                .contains("exceeded timeout")
        );
    }

    #[tokio::test]
    async fn browser_close_returns_metrics_snapshot() {
        let provider = test_provider();
        let executors = provider.tool_runtime_executors();
        let start = execute(&executors, TOOL_BROWSER_START, r#"{"task_id":"task-1"}"#).await;
        let session_id = start.structured_payload.as_ref().expect("payload")["session_id"]
            .as_str()
            .expect("session id");
        let close_args = format!(r#"{{"session_id":"{session_id}"}}"#);

        let close = execute(&executors, TOOL_BROWSER_CLOSE, &close_args).await;
        let payload = close.structured_payload.as_ref().expect("payload");

        assert_eq!(payload["status"], "closed");
        assert_eq!(payload["metrics"]["sessions_started"], 1);
        assert_eq!(payload["metrics"]["sessions_closed"], 1);
        assert!(payload["metrics"]["sidecar_requests"].as_u64().unwrap_or(0) >= 2);
    }

    #[tokio::test]
    async fn browser_execute_populates_provider_metrics() {
        let provider = test_provider();
        let executors = provider.tool_runtime_executors();
        let start = execute(&executors, TOOL_BROWSER_START, r#"{"task_id":"task-1"}"#).await;
        let session_id = start.structured_payload.as_ref().expect("payload")["session_id"]
            .as_str()
            .expect("session id");
        let execute_args = format!(
            r#"{{"session_id":"{session_id}","action":{{"kind":"click_xy","x":10,"y":20}},"expected_result":"click"}}"#
        );

        let result = execute(&executors, TOOL_BROWSER_EXECUTE, &execute_args).await;
        let payload = result.structured_payload.as_ref().expect("payload");

        assert_eq!(payload["status"], "executed");
        let metrics = provider.metrics_snapshot();
        assert_eq!(metrics.sessions_started, 1);
        assert!(metrics.observations_fetched >= 1);
        assert!(metrics.screenshots_captured >= 2);
        assert!(metrics.sidecar_requests >= 2);
    }

    #[tokio::test]
    async fn browser_progress_and_outputs_exclude_screenshot_bytes() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(32);
        let executors = {
            let provider = Arc::new(BrowserLiveProvider::new(
                Arc::new(FakeBrowserSidecar::new()),
                BrowserArtifactSettings::default(),
                Some(tx),
            ));
            provider.tool_runtime_executors()
        };
        let start = execute(&executors, TOOL_BROWSER_START, r#"{"task_id":"task-1"}"#).await;
        let session_id = start.structured_payload.as_ref().expect("payload")["session_id"]
            .as_str()
            .expect("session id");
        let observe_args = format!(r#"{{"session_id":"{session_id}"}}"#);
        let observe = execute(&executors, TOOL_BROWSER_OBSERVE, &observe_args).await;
        let execute_args = format!(
            r#"{{"session_id":"{session_id}","action":{{"kind":"click_xy","x":10,"y":20}},"expected_result":"click"}}"#
        );
        let execute_result = execute(&executors, TOOL_BROWSER_EXECUTE, &execute_args).await;
        let close_args = format!(r#"{{"session_id":"{session_id}"}}"#);
        let close = execute(&executors, TOOL_BROWSER_CLOSE, &close_args).await;

        let texts = [
            observe.stdout.text.as_deref().expect("observe text"),
            execute_result.stdout.text.as_deref().expect("execute text"),
            close.stdout.text.as_deref().expect("close text"),
        ];
        for text in &texts {
            assert!(!text.contains("base64"));
            assert!(!text.contains("data:image"));
        }

        drop(executors);
        let mut progress_texts = Vec::new();
        while let Some(event) = rx.recv().await {
            if let AgentEvent::Reasoning { summary, .. } = event {
                progress_texts.push(summary);
            }
        }
        let combined = progress_texts.join("\n");
        assert!(!combined.contains("base64"));
        assert!(!combined.contains("data:image"));
        assert!(!combined.is_empty());
    }

    #[tokio::test]
    async fn browser_tool_observes_cancelled_invocation_before_sidecar_call() {
        let provider = test_provider();
        let executors = provider.tool_runtime_executors();
        let token = CancellationToken::new();
        token.cancel();
        let mut invocation = invocation(TOOL_BROWSER_START, r#"{"task_id":"task-1"}"#);
        invocation.cancellation_token = token;

        let error = executors
            .iter()
            .find(|executor| executor.name().as_str() == TOOL_BROWSER_START)
            .expect("start executor")
            .execute(invocation)
            .await
            .expect_err("cancelled invocation should fail before sidecar call");

        assert!(error.to_string().contains("cancelled"));
    }

    fn test_provider() -> Arc<BrowserLiveProvider> {
        Arc::new(BrowserLiveProvider::new(
            Arc::new(FakeBrowserSidecar::new()),
            BrowserArtifactSettings::default(),
            None,
        ))
    }

    async fn execute(executors: &[Arc<dyn ToolExecutor>], name: &str, args: &str) -> ToolOutput {
        let executor = executors
            .iter()
            .find(|executor| executor.name().as_str() == name)
            .expect("executor");
        executor
            .execute(invocation(name, args))
            .await
            .expect("tool execution")
    }

    fn invocation(name: &str, args: &str) -> ToolInvocation {
        ToolInvocation {
            session_id: SessionId::from(1),
            turn_id: TurnId::new("turn"),
            batch_id: ToolBatchId::new("batch"),
            batch_index: 0,
            invocation_id: InvocationId::new("invocation"),
            tool_call_id: ToolCallId::new(format!("call-{name}")),
            provider_tool_call_id: None,
            tool_name: ToolName::new(name),
            raw_provider_payload: json!({}),
            raw_arguments: args.to_string(),
            normalized_arguments: serde_json::from_str(args).expect("json args"),
            cancellation_token: CancellationToken::new(),
            timeout: ToolTimeoutConfig::default(),
            execution_context: ToolExecutionContext::new(".oxide/tool-artifacts"),
            provider_metadata: ProviderMetadata {
                provider: "test".to_string(),
                protocol: "test".to_string(),
            },
            model_metadata: ModelMetadata {
                model: "test".to_string(),
            },
            working_directory: None,
            environment_metadata: None,
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
        }
    }
}
