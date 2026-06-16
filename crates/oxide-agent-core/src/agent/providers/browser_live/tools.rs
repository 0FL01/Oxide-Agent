use super::artifacts::{BrowserArtifactPurpose, BrowserArtifactSettings};
use super::client::{BrowserSidecar, BrowserSidecarClient, IdempotencyKey};
use super::error::BrowserSidecarError;
use super::mimo::{BrowserMimoDecider, BrowserMimoError};
use super::prompt::{BrowserDecisionPromptContext, viewport_from_observation};
use super::session::BrowserSessionState;
use super::types::{
    BrowserProfile, CloseReason, CloseSessionRequest, ConsoleDebugQuery, ConsoleLevel,
    CreateSessionRequest, DebugLevel, NetworkDebugQuery, NetworkFilter, ObserveQuery,
    ScreenshotArtifact, ScreenshotFormat, ScreenshotQuery, Viewport,
};
use crate::agent::progress::{AgentEvent, AgentEventSource};
use crate::agent::tool_runtime::{
    OutputNormalizer, ToolExecutor, ToolInvocation, ToolName, ToolOutput, ToolRuntimeConfig,
    ToolRuntimeError,
};
use crate::llm::LlmClient;
use crate::llm::ToolDefinition;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc::Sender};

/// `browser_start` tool name.
pub const TOOL_BROWSER_START: &str = "browser_start";
/// `browser_observe` tool name.
pub const TOOL_BROWSER_OBSERVE: &str = "browser_observe";
/// `browser_step` tool name.
pub const TOOL_BROWSER_STEP: &str = "browser_step";
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
    decision_engine: Option<BrowserMimoDecider>,
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
        llm_client: Arc<LlmClient>,
    ) -> Result<Self, BrowserSidecarError> {
        Ok(Self::new(
            Arc::new(BrowserSidecarClient::new(base_url, token)?),
            artifact_settings,
            progress_tx,
        ))
        .map(|provider| provider.with_decision_engine(BrowserMimoDecider::new(llm_client)))
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
            decision_engine: None,
        }
    }

    /// Attach the Browser MiMo decision engine used by `browser_step`.
    #[must_use]
    pub fn with_decision_engine(mut self, decision_engine: BrowserMimoDecider) -> Self {
        self.decision_engine = Some(decision_engine);
        self
    }

    /// Build typed runtime executors for Browser Live tools.
    #[must_use]
    pub fn tool_runtime_executors(self: &Arc<Self>) -> Vec<Arc<dyn ToolExecutor>> {
        [
            TOOL_BROWSER_START,
            TOOL_BROWSER_OBSERVE,
            TOOL_BROWSER_STEP,
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

    async fn start(
        &self,
        invocation: &ToolInvocation,
        args: StartArgs,
    ) -> Result<Value, ToolRuntimeError> {
        ensure_not_cancelled(invocation)?;
        let viewport = args.viewport.unwrap_or_default();
        let request = CreateSessionRequest {
            task_id: args
                .task_id
                .unwrap_or_else(|| invocation.session_id.to_string()),
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
            .sidecar
            .create_session(&request, &key)
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

    async fn observe(
        &self,
        invocation: &ToolInvocation,
        args: ObserveArgs,
    ) -> Result<Value, ToolRuntimeError> {
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
            .sidecar
            .observe(&args.session_id, &query)
            .await
            .map_err(sidecar_runtime_error)?;
        let frame = {
            let mut states = self.states.lock().await;
            let state = states.get_mut(&args.session_id).ok_or_else(|| {
                ToolRuntimeError::Failure("browser session is not started".to_string())
            })?;
            state
                .record_observation(&response.observation, BrowserArtifactPurpose::LiveFrame, 0)
                .map_err(|error| ToolRuntimeError::Failure(error.to_string()))?
                .clone()
        };
        self.emit_progress(format!(
            "Browser session {} observed at action_seq {}",
            args.session_id, frame.action_seq
        ))
        .await;

        Ok(observation_payload(
            &args.session_id,
            &response.observation.screenshot,
            &frame,
        ))
    }

    async fn step(
        &self,
        invocation: &ToolInvocation,
        args: StepArgs,
    ) -> Result<Value, ToolRuntimeError> {
        ensure_not_cancelled(invocation)?;
        let query = ObserveQuery {
            fresh: true,
            include_dom: false,
            include_a11y: false,
            include_network_summary: true,
            include_console_summary: true,
            max_debug_items: 20,
        };
        let response = self
            .sidecar
            .observe(&args.session_id, &query)
            .await
            .map_err(sidecar_runtime_error)?;
        let (frame, history_summary) = {
            let mut states = self.states.lock().await;
            let state = states.get_mut(&args.session_id).ok_or_else(|| {
                ToolRuntimeError::Failure("browser session is not started".to_string())
            })?;
            let frame = state
                .record_observation(&response.observation, BrowserArtifactPurpose::LiveFrame, 0)
                .map_err(|error| ToolRuntimeError::Failure(error.to_string()))?
                .clone();
            (frame, state.compact_history_summary())
        };
        let observe =
            observation_payload(&args.session_id, &response.observation.screenshot, &frame);

        let Some(decision_engine) = &self.decision_engine else {
            return Ok(json!({
                "status": "decision_pending",
                "message": "browser_step MiMo decision engine is not configured; current checkpoint returns a fresh observation shell",
                "observation": observe,
            }));
        };

        let screenshot_bytes = self
            .sidecar
            .latest_screenshot_bytes(
                &args.session_id,
                &ScreenshotQuery {
                    format: ScreenshotFormat::Binary,
                    max_width: Some(response.observation.viewport.width),
                    redacted: true,
                },
            )
            .await
            .map_err(sidecar_runtime_error)?;
        let task = args
            .task
            .as_deref()
            .unwrap_or("Continue the user's browser task safely.");
        let prompt_context = BrowserDecisionPromptContext {
            task,
            session_id: &args.session_id,
            observation: &response.observation,
            history_summary: Some(&history_summary),
        };
        let decision = decision_engine
            .decide(
                screenshot_bytes,
                &prompt_context,
                viewport_from_observation(&response.observation),
            )
            .await
            .map_err(mimo_runtime_error)?;
        self.emit_progress(format!(
            "Browser session {} produced validated MiMo decision",
            args.session_id
        ))
        .await;

        Ok(json!({
            "status": "decision_ready",
            "message": "validated MiMo decision returned; action execution is added by CP-9",
            "decision": decision,
            "observation": observe,
        }))
    }

    async fn debug(
        &self,
        invocation: &ToolInvocation,
        args: DebugArgs,
    ) -> Result<Value, ToolRuntimeError> {
        ensure_not_cancelled(invocation)?;
        let network = if args.include_network {
            Some(
                self.sidecar
                    .debug_network(
                        &args.session_id,
                        &NetworkDebugQuery {
                            since_action_seq: args.since_action_seq.unwrap_or_default(),
                            level: DebugLevel::Summary,
                            include_bodies: false,
                            filter: NetworkFilter::Failed,
                            limit: args.limit.unwrap_or(20),
                        },
                    )
                    .await
                    .map_err(sidecar_runtime_error)?
                    .network,
            )
        } else {
            None
        };
        let console = if args.include_console {
            Some(
                self.sidecar
                    .debug_console(
                        &args.session_id,
                        &ConsoleDebugQuery {
                            since_action_seq: args.since_action_seq.unwrap_or_default(),
                            level: DebugLevel::Summary,
                            min_level: ConsoleLevel::Error,
                            limit: args.limit.unwrap_or(20),
                        },
                    )
                    .await
                    .map_err(sidecar_runtime_error)?
                    .console,
            )
        } else {
            None
        };
        self.emit_progress(format!("Browser debug fetched for {}", args.session_id))
            .await;
        Ok(json!({
            "status": "debug",
            "session_id": args.session_id,
            "network": network,
            "console": console,
        }))
    }

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
            .sidecar
            .close_session(
                &args.session_id,
                &CloseSessionRequest {
                    purge_profile: args.purge_profile.unwrap_or(true),
                    keep_artifacts: args.keep_artifacts.unwrap_or(true),
                    reason: args.reason.unwrap_or(CloseReason::Done),
                },
                &key,
            )
            .await
            .map_err(sidecar_runtime_error)?;
        self.states.lock().await.remove(&args.session_id);
        self.emit_progress(format!("Browser session {} closed", args.session_id))
            .await;
        Ok(json!({
            "status": "closed",
            "session_id": response.session_id,
            "closed": response.closed,
            "profile_purged": response.profile_purged,
            "artifacts_kept": response.artifacts_kept,
            "retained_artifact_refs": retained_artifacts,
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

    fn spec(&self) -> ToolDefinition {
        browser_tool_definition(self.tool_name)
    }

    async fn execute(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolRuntimeError> {
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig {
            timeout: invocation.timeout.clone(),
            ..ToolRuntimeConfig::default()
        });
        let payload = match self.tool_name {
            TOOL_BROWSER_START => {
                let args = parse_args::<StartArgs>(&invocation)?;
                self.provider.start(&invocation, args).await?
            }
            TOOL_BROWSER_OBSERVE => {
                let args = parse_args::<ObserveArgs>(&invocation)?;
                self.provider.observe(&invocation, args).await?
            }
            TOOL_BROWSER_STEP => {
                let args = parse_args::<StepArgs>(&invocation)?;
                self.provider.step(&invocation, args).await?
            }
            TOOL_BROWSER_DEBUG => {
                let args = parse_args::<DebugArgs>(&invocation)?;
                self.provider.debug(&invocation, args).await?
            }
            TOOL_BROWSER_CLOSE => {
                let args = parse_args::<CloseArgs>(&invocation)?;
                self.provider.close(&invocation, args).await?
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
struct StepArgs {
    session_id: String,
    #[serde(default)]
    task: Option<String>,
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
    purge_profile: Option<bool>,
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

fn mimo_runtime_error(error: BrowserMimoError) -> ToolRuntimeError {
    ToolRuntimeError::Failure(error.to_string())
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

fn browser_tool_definition(name: &str) -> ToolDefinition {
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
            "Return compact browser state and latest screenshot artifact reference.",
            json!({
                "type": "object",
                "required": ["session_id"],
                "properties": {
                    "session_id": {"type": "string"},
                    "fresh": {"type": "boolean", "default": false},
                    "include_a11y": {"type": "boolean", "default": false},
                    "max_debug_items": {"type": "integer", "minimum": 0, "maximum": 100}
                },
                "additionalProperties": false
            }),
        ),
        TOOL_BROWSER_STEP => (
            "Run one Browser Live MiMo decision step and return a validated decision without executing it.",
            json!({
                "type": "object",
                "required": ["session_id"],
                "properties": {
                    "session_id": {"type": "string"},
                    "task": {"type": "string"}
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
    ToolDefinition {
        name: name.to_string(),
        description: description.to_string(),
        parameters,
    }
}

const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::identity::SessionId;
    use crate::agent::providers::browser_live::test_support::FakeBrowserSidecar;
    use crate::agent::tool_runtime::{
        ModelMetadata, ProviderMetadata, ToolBatchId, ToolCallId, ToolExecutionContext,
        ToolTimeoutConfig, TurnId,
    };
    use crate::llm::InvocationId;
    use chrono::Utc;
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
                TOOL_BROWSER_STEP,
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
        assert_eq!(
            close.structured_payload.as_ref().expect("payload")["status"],
            "closed"
        );
    }

    #[tokio::test]
    async fn browser_step_is_placeholder_observation_shell() {
        let provider = test_provider();
        let executors = provider.tool_runtime_executors();
        let start = execute(&executors, TOOL_BROWSER_START, r#"{"task_id":"task-1"}"#).await;
        let session_id = start.structured_payload.as_ref().expect("payload")["session_id"]
            .as_str()
            .expect("session id");
        let step_args = format!(r#"{{"session_id":"{session_id}"}}"#);

        let step = execute(&executors, TOOL_BROWSER_STEP, &step_args).await;

        assert!(step.success);
        assert_eq!(
            step.structured_payload.as_ref().expect("payload")["status"],
            "decision_pending"
        );
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
