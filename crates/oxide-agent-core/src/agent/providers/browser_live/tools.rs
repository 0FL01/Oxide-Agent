use super::actions::{BrowserActionPlan, plan_browser_action};
use super::artifacts::{BrowserArtifactPurpose, BrowserArtifactSettings};
use super::client::{BrowserSidecar, BrowserSidecarClient, IdempotencyKey};
use super::error::BrowserSidecarError;
use super::mimo::{BrowserDecisionEngine, BrowserMimoDecider, BrowserMimoError};
use super::policy::{
    BrowserPolicyError, policy_audit_event, validate_decision_policy, validate_navigation_url,
    validate_session_policy,
};
use super::prompt::{BrowserDecisionPromptContext, viewport_from_observation};
use super::recovery::{
    BrowserRecoveryPlan, BrowserRecoveryReport, BrowserRecoverySettings, BrowserRecoveryStatus,
    attach_recovery_result, build_recovery_report, recovery_loop_signature,
};
use super::session::{BrowserFrame, BrowserSessionState};
use super::types::{
    ActionRequest, ActionResponse, BrowserDecision, BrowserObservation, BrowserProfile,
    CloseReason, CloseSessionRequest, ConsoleDebugPayload, ConsoleDebugQuery, ConsoleLevel,
    CreateSessionRequest, DebugLevel, GotoResponse, NetworkDebugPayload, NetworkDebugQuery,
    NetworkFilter, ObserveQuery, ScreenshotArtifact, ScreenshotFormat, ScreenshotQuery, Viewport,
};
use super::verification::{
    BrowserActionVerification, BrowserVerificationStatus, terminal_debug, terminal_done,
    terminal_needs_user, timeout_report, verify_navigation, verify_sidecar_action,
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
use tokio::time::{Duration, timeout};

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
    decision_engine: Option<Arc<dyn BrowserDecisionEngine>>,
    recovery_settings: BrowserRecoverySettings,
    recovery_signatures: Arc<Mutex<BTreeMap<String, BTreeMap<String, u32>>>>,
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
            recovery_settings: BrowserRecoverySettings::default(),
            recovery_signatures: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Attach the Browser MiMo decision engine used by `browser_step`.
    #[must_use]
    pub fn with_decision_engine(
        mut self,
        decision_engine: impl BrowserDecisionEngine + 'static,
    ) -> Self {
        self.decision_engine = Some(Arc::new(decision_engine));
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
        if let Some(start_url) = &request.start_url {
            validate_navigation_url(start_url).map_err(policy_runtime_error)?;
        }
        validate_session_policy(
            request.profile,
            request.allow_downloads,
            request.allow_uploads,
        )
        .map_err(policy_runtime_error)?;
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
        let max_actions = args.max_actions();
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
        let (frame, history_summary, action_seq) = {
            let mut states = self.states.lock().await;
            let state = states.get_mut(&args.session_id).ok_or_else(|| {
                ToolRuntimeError::Failure("browser session is not started".to_string())
            })?;
            let frame = state
                .record_observation(&response.observation, BrowserArtifactPurpose::Milestone, 0)
                .map_err(|error| ToolRuntimeError::Failure(error.to_string()))?
                .clone();
            (
                frame,
                state.compact_history_summary(),
                state.action_seq().saturating_add(1),
            )
        };
        let observe =
            observation_payload(&args.session_id, &response.observation.screenshot, &frame);

        let Some(decision_engine) = &self.decision_engine else {
            return Ok(json!({
                "status": "decision_pending",
                "message": "browser_step MiMo decision engine is not configured; current checkpoint returns a fresh observation shell",
                "max_actions": max_actions,
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
        if let Err(error) = validate_decision_policy(&decision) {
            let audit = policy_audit_event(&decision, false, error.to_string());
            self.emit_progress(format!(
                "BrowserPolicy session_id={} action_seq={} decision=block reason={}",
                args.session_id, action_seq, audit.reason
            ))
            .await;
            return Ok(json!({
                "status": "blocked",
                "session_id": args.session_id,
                "action_seq": action_seq,
                "message": error.to_string(),
                "decision": decision,
                "observation": observe,
                "policy_audit": audit,
            }));
        }
        let plan = plan_browser_action(&decision, action_seq, args.action_timeout_ms())
            .map_err(|error| ToolRuntimeError::InvalidArguments(error.to_string()))?;
        self.emit_progress(format!(
            "BrowserAction session_id={} action_seq={} kind={}",
            args.session_id,
            action_seq,
            action_plan_kind(&plan)
        ))
        .await;

        let action_timeout = Duration::from_millis(args.action_timeout_ms());
        let timeout_decision = decision.clone();
        let timeout_observation = response.observation.clone();
        let timeout_before_payload = observe.clone();
        let result = timeout(
            action_timeout,
            self.execute_action_plan(
                invocation,
                &args.session_id,
                action_seq,
                response.observation,
                frame,
                decision,
                observe,
                plan,
            ),
        )
        .await;

        match result {
            Ok(result) => result,
            Err(_) => {
                let verification = timeout_report(
                    &timeout_decision,
                    &timeout_observation,
                    format!(
                        "browser_step action exceeded timeout of {}ms",
                        args.action_timeout_ms()
                    ),
                );
                self.emit_progress(format!(
                    "BrowserVerification session_id={} action_seq={} status=timeout",
                    args.session_id, action_seq
                ))
                .await;
                Ok(json!({
                    "status": "timeout",
                    "session_id": args.session_id,
                    "action_seq": action_seq,
                    "message": verification.reason.clone(),
                    "decision": timeout_decision,
                    "observation": timeout_before_payload,
                    "verification": verification,
                }))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_action_plan(
        &self,
        invocation: &ToolInvocation,
        session_id: &str,
        action_seq: u64,
        before_observation: BrowserObservation,
        before_frame: BrowserFrame,
        decision: BrowserDecision,
        before_payload: Value,
        plan: BrowserActionPlan,
    ) -> Result<Value, ToolRuntimeError> {
        ensure_not_cancelled(invocation)?;
        match plan {
            BrowserActionPlan::SidecarAction(request) => {
                let key =
                    idempotency_key(invocation, "action", &format!("{session_id}:{action_seq}"))?;
                let action = self
                    .sidecar
                    .execute_action(session_id, &request, &key)
                    .await
                    .map_err(sidecar_runtime_error)?;
                ensure_not_cancelled(invocation)?;
                let after = self.observe_after_action(session_id).await?;
                let (after_frame, after_payload) =
                    self.record_after_observation(session_id, &after).await?;
                let verification = verify_sidecar_action(
                    &decision,
                    &before_observation,
                    &action.action_result,
                    &after,
                );
                let recovery = self
                    .recover_after_verification_failure(
                        invocation,
                        session_id,
                        action_seq,
                        &decision,
                        &verification,
                        Some(&action.action_result),
                    )
                    .await?;
                self.emit_verification(session_id, action_seq, &verification)
                    .await;
                Ok(action_step_payload(
                    session_id,
                    action_seq,
                    decision,
                    before_payload,
                    &before_frame,
                    after_payload,
                    &after_frame,
                    verification,
                    Some(action),
                    None,
                    recovery,
                ))
            }
            BrowserActionPlan::Navigate(request) => {
                let key =
                    idempotency_key(invocation, "goto", &format!("{session_id}:{action_seq}"))?;
                let navigation = self
                    .sidecar
                    .goto(session_id, &request, &key)
                    .await
                    .map_err(sidecar_runtime_error)?;
                ensure_not_cancelled(invocation)?;
                let after = self.observe_after_action(session_id).await?;
                let (after_frame, after_payload) =
                    self.record_after_observation(session_id, &after).await?;
                let verification = verify_navigation(
                    &decision,
                    &before_observation,
                    &navigation.navigation,
                    &after,
                );
                let recovery = self
                    .recover_after_verification_failure(
                        invocation,
                        session_id,
                        action_seq,
                        &decision,
                        &verification,
                        None,
                    )
                    .await?;
                self.emit_verification(session_id, action_seq, &verification)
                    .await;
                Ok(action_step_payload(
                    session_id,
                    action_seq,
                    decision,
                    before_payload,
                    &before_frame,
                    after_payload,
                    &after_frame,
                    verification,
                    None,
                    Some(navigation),
                    recovery,
                ))
            }
            BrowserActionPlan::Done {
                final_answer,
                evidence,
            } => {
                let verification = terminal_done(
                    &decision,
                    &before_observation,
                    "done decision includes final visual evidence".to_string(),
                );
                self.record_final_observation(session_id, &before_observation)
                    .await?;
                self.emit_verification(session_id, action_seq, &verification)
                    .await;
                Ok(json!({
                    "status": "done",
                    "session_id": session_id,
                    "action_seq": action_seq,
                    "task_success": true,
                    "final_answer": final_answer,
                    "evidence": evidence,
                    "decision": decision,
                    "observation": before_payload,
                    "verification": verification,
                }))
            }
            BrowserActionPlan::AskUser { question } => {
                let verification = terminal_needs_user(
                    &decision,
                    &before_observation,
                    "browser decision requires user input or approval".to_string(),
                );
                self.emit_verification(session_id, action_seq, &verification)
                    .await;
                Ok(json!({
                    "status": "blocked",
                    "session_id": session_id,
                    "action_seq": action_seq,
                    "question": question,
                    "decision": decision,
                    "observation": before_payload,
                    "verification": verification,
                }))
            }
            BrowserActionPlan::Debug { reason } => {
                let verification = terminal_debug(
                    &decision,
                    &before_observation,
                    "browser decision requested debug diagnostics before action".to_string(),
                );
                self.emit_verification(session_id, action_seq, &verification)
                    .await;
                Ok(json!({
                    "status": "debug_requested",
                    "session_id": session_id,
                    "action_seq": action_seq,
                    "reason": reason,
                    "decision": decision,
                    "observation": before_payload,
                    "verification": verification,
                }))
            }
        }
    }

    async fn observe_after_action(
        &self,
        session_id: &str,
    ) -> Result<BrowserObservation, ToolRuntimeError> {
        Ok(self
            .sidecar
            .observe(
                session_id,
                &ObserveQuery {
                    fresh: true,
                    include_dom: false,
                    include_a11y: false,
                    include_network_summary: true,
                    include_console_summary: true,
                    max_debug_items: 20,
                },
            )
            .await
            .map_err(sidecar_runtime_error)?
            .observation)
    }

    async fn record_after_observation(
        &self,
        session_id: &str,
        observation: &BrowserObservation,
    ) -> Result<(BrowserFrame, Value), ToolRuntimeError> {
        let frame = {
            let mut states = self.states.lock().await;
            let state = states.get_mut(session_id).ok_or_else(|| {
                ToolRuntimeError::Failure("browser session is not started".to_string())
            })?;
            state
                .record_observation(observation, BrowserArtifactPurpose::Milestone, 0)
                .map_err(|error| ToolRuntimeError::Failure(error.to_string()))?
                .clone()
        };
        let payload = observation_payload(session_id, &observation.screenshot, &frame);
        Ok((frame, payload))
    }

    async fn record_final_observation(
        &self,
        session_id: &str,
        observation: &BrowserObservation,
    ) -> Result<(), ToolRuntimeError> {
        let mut states = self.states.lock().await;
        let state = states.get_mut(session_id).ok_or_else(|| {
            ToolRuntimeError::Failure("browser session is not started".to_string())
        })?;
        state
            .record_observation(observation, BrowserArtifactPurpose::Final, 0)
            .map_err(|error| ToolRuntimeError::Failure(error.to_string()))?;
        Ok(())
    }

    async fn recover_after_verification_failure(
        &self,
        invocation: &ToolInvocation,
        session_id: &str,
        action_seq: u64,
        decision: &BrowserDecision,
        verification: &BrowserActionVerification,
        action_result: Option<&super::types::ActionResult>,
    ) -> Result<Option<BrowserRecoveryReport>, ToolRuntimeError> {
        if verification.status != BrowserVerificationStatus::VerificationFailed {
            return Ok(None);
        }
        let (network, console) = self.recovery_debug(session_id, action_seq).await?;
        let signature = recovery_loop_signature(decision, {
            let preliminary = build_recovery_report(
                decision,
                verification,
                action_result,
                &network,
                &console,
                self.recovery_settings,
                false,
            );
            preliminary.kind
        });
        let repeated = self.mark_recovery_signature(session_id, &signature).await;
        let mut report = build_recovery_report(
            decision,
            verification,
            action_result,
            &network,
            &console,
            self.recovery_settings,
            repeated,
        );

        if report.status == BrowserRecoveryStatus::Attempted
            && let BrowserRecoveryPlan::SidecarAction { action } = report.plan.clone()
        {
            let recovery_seq = action_seq.saturating_add(1);
            let request = ActionRequest {
                action_seq: recovery_seq,
                action,
                expected_result: format!("bounded recovery for {:?}", report.kind),
                timeout_ms: 5_000,
                capture_after: true,
                wait_for_stability: true,
            };
            let key = idempotency_key(
                invocation,
                "recovery",
                &format!("{session_id}:{recovery_seq}"),
            )?;
            let recovery_action = self
                .sidecar
                .execute_action(session_id, &request, &key)
                .await
                .map_err(sidecar_runtime_error)?;
            let recovery_observation = self.observe_after_action(session_id).await?;
            let (recovery_frame, _) = self
                .record_after_observation(session_id, &recovery_observation)
                .await?;
            attach_recovery_result(
                &mut report,
                &recovery_action.action_result,
                recovery_frame.observation_id,
                recovery_frame.screenshot.screenshot_id,
            );
        }

        self.emit_progress(format!(
            "BrowserRecovery session_id={} action_seq={} status={:?} kind={:?}",
            session_id, action_seq, report.status, report.kind
        ))
        .await;
        Ok(Some(report))
    }

    async fn recovery_debug(
        &self,
        session_id: &str,
        action_seq: u64,
    ) -> Result<(NetworkDebugPayload, ConsoleDebugPayload), ToolRuntimeError> {
        let network = self
            .sidecar
            .debug_network(
                session_id,
                &NetworkDebugQuery {
                    since_action_seq: action_seq.saturating_sub(1),
                    level: DebugLevel::Summary,
                    include_bodies: false,
                    filter: NetworkFilter::Failed,
                    limit: 20,
                },
            )
            .await
            .map_err(sidecar_runtime_error)?
            .network;
        let console = self
            .sidecar
            .debug_console(
                session_id,
                &ConsoleDebugQuery {
                    since_action_seq: action_seq.saturating_sub(1),
                    level: DebugLevel::Summary,
                    min_level: ConsoleLevel::Error,
                    limit: 20,
                },
            )
            .await
            .map_err(sidecar_runtime_error)?
            .console;
        Ok((network, console))
    }

    async fn mark_recovery_signature(&self, session_id: &str, signature: &str) -> bool {
        let mut signatures = self.recovery_signatures.lock().await;
        let session = signatures.entry(session_id.to_string()).or_default();
        let count = session.entry(signature.to_string()).or_default();
        let repeated = *count > 0;
        *count += 1;
        repeated
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
                    purge_profile: true,
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
    #[serde(default)]
    action_timeout_ms: Option<u64>,
    #[serde(default)]
    max_actions: Option<u32>,
}

impl StepArgs {
    fn action_timeout_ms(&self) -> u64 {
        self.action_timeout_ms.unwrap_or(30_000).clamp(1, 60_000)
    }

    fn max_actions(&self) -> u32 {
        self.max_actions.unwrap_or(1).clamp(1, 1)
    }
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

fn mimo_runtime_error(error: BrowserMimoError) -> ToolRuntimeError {
    ToolRuntimeError::Failure(error.to_string())
}

fn policy_runtime_error(error: BrowserPolicyError) -> ToolRuntimeError {
    ToolRuntimeError::InvalidArguments(error.to_string())
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

#[allow(clippy::too_many_arguments)]
fn action_step_payload(
    session_id: &str,
    action_seq: u64,
    decision: BrowserDecision,
    before_payload: Value,
    before_frame: &BrowserFrame,
    after_payload: Value,
    after_frame: &BrowserFrame,
    verification: BrowserActionVerification,
    action: Option<ActionResponse>,
    navigation: Option<GotoResponse>,
    recovery: Option<BrowserRecoveryReport>,
) -> Value {
    json!({
        "status": step_status(&verification),
        "session_id": session_id,
        "action_seq": action_seq,
        "max_actions": 1,
        "task_success": verification.task_success,
        "decision": decision,
        "before": before_payload,
        "after": after_payload,
        "before_artifact_ref": before_frame.artifact.uri.clone(),
        "after_artifact_ref": after_frame.artifact.uri.clone(),
        "action_result": action.map(|response| response.action_result),
        "navigation": navigation.map(|response| response.navigation),
        "verification": verification,
        "recovery": recovery,
    })
}

fn step_status(verification: &BrowserActionVerification) -> &'static str {
    match verification.status {
        BrowserVerificationStatus::ActionVerified => "action_verified",
        BrowserVerificationStatus::VerificationFailed => "verification_failed",
        BrowserVerificationStatus::Done => "done",
        BrowserVerificationStatus::NeedsUser => "blocked",
        BrowserVerificationStatus::DebugRequested => "debug_requested",
        BrowserVerificationStatus::Timeout => "timeout",
    }
}

fn action_plan_kind(plan: &BrowserActionPlan) -> &'static str {
    match plan {
        BrowserActionPlan::SidecarAction(_) => "action",
        BrowserActionPlan::Navigate(_) => "goto",
        BrowserActionPlan::Debug { .. } => "debug",
        BrowserActionPlan::AskUser { .. } => "ask_user",
        BrowserActionPlan::Done { .. } => "done",
    }
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
            "Run one bounded Browser Live cycle: decide, execute one action, observe, and verify.",
            json!({
                "type": "object",
                "required": ["session_id"],
                "properties": {
                    "session_id": {"type": "string"},
                    "task": {"type": "string"},
                    "action_timeout_ms": {"type": "integer", "minimum": 1, "maximum": 60000},
                    "max_actions": {"type": "integer", "const": 1, "default": 1}
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
    use crate::agent::providers::browser_live::test_support::{
        FakeActionOutcome, FakeBrowserSidecar,
    };
    use crate::agent::providers::browser_live::types::{
        BrowserDecisionAction, BrowserDecisionRisk, BrowserSensitiveAction,
    };
    use crate::agent::tool_runtime::{
        ModelMetadata, ProviderMetadata, ToolBatchId, ToolCallId, ToolExecutionContext,
        ToolTimeoutConfig, TurnId,
    };
    use crate::llm::InvocationId;
    use async_trait::async_trait;
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
    async fn browser_start_rejects_non_web_start_url_and_disables_downloads_uploads() {
        let provider = test_provider();
        let executors = provider.tool_runtime_executors();

        let error = execute_result(
            &executors,
            TOOL_BROWSER_START,
            r#"{"task_id":"task-1","start_url":"file:///etc/passwd"}"#,
        )
        .await
        .expect_err("non-web start_url should fail before sidecar call");

        assert!(error.to_string().contains("http or https"));

        let start = execute(
            &executors,
            TOOL_BROWSER_START,
            r#"{"task_id":"task-1","start_url":"https://example.test"}"#,
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
    async fn browser_step_executes_click_and_verifies_fresh_after_screenshot() {
        let provider = test_provider_with_decision(click_decision(), FakeBrowserSidecar::new());
        let executors = provider.tool_runtime_executors();
        let start = execute(&executors, TOOL_BROWSER_START, r#"{"task_id":"task-1"}"#).await;
        let session_id = start.structured_payload.as_ref().expect("payload")["session_id"]
            .as_str()
            .expect("session id");
        let step_args = format!(r#"{{"session_id":"{session_id}","task":"click login"}}"#);

        let step = execute(&executors, TOOL_BROWSER_STEP, &step_args).await;
        let payload = step.structured_payload.as_ref().expect("payload");

        assert!(step.success);
        assert_eq!(payload["status"], "action_verified");
        assert_eq!(payload["task_success"], false);
        assert_eq!(payload["action_result"]["status"], "executed");
        assert_ne!(
            payload["verification"]["before_screenshot_id"],
            payload["verification"]["after_screenshot_id"]
        );
        assert!(
            payload["after_artifact_ref"]
                .as_str()
                .expect("after artifact")
                .contains("milestone")
        );
    }

    #[tokio::test]
    async fn browser_step_noop_click_returns_verification_failure() {
        let fake = FakeBrowserSidecar::new().with_action_script(vec![FakeActionOutcome::NoOp]);
        let provider = test_provider_with_decision(click_decision(), fake);
        let executors = provider.tool_runtime_executors();
        let start = execute(&executors, TOOL_BROWSER_START, r#"{"task_id":"task-1"}"#).await;
        let session_id = start.structured_payload.as_ref().expect("payload")["session_id"]
            .as_str()
            .expect("session id");
        let step_args = format!(r#"{{"session_id":"{session_id}"}}"#);

        let step = execute(&executors, TOOL_BROWSER_STEP, &step_args).await;
        let payload = step.structured_payload.as_ref().expect("payload");

        assert!(step.success);
        assert_eq!(payload["status"], "verification_failed");
        assert_eq!(payload["verification"]["status"], "verification_failed");
        assert!(
            payload["verification"]["reason"]
                .as_str()
                .expect("reason")
                .contains("not verified visual success")
        );
    }

    #[tokio::test]
    async fn browser_step_navigation_captures_fresh_screenshot() {
        let provider = test_provider_with_decision(navigate_decision(), FakeBrowserSidecar::new());
        let executors = provider.tool_runtime_executors();
        let start = execute(&executors, TOOL_BROWSER_START, r#"{"task_id":"task-1"}"#).await;
        let session_id = start.structured_payload.as_ref().expect("payload")["session_id"]
            .as_str()
            .expect("session id");
        let step_args = format!(r#"{{"session_id":"{session_id}"}}"#);

        let step = execute(&executors, TOOL_BROWSER_STEP, &step_args).await;
        let payload = step.structured_payload.as_ref().expect("payload");

        assert!(step.success);
        assert_eq!(payload["status"], "action_verified");
        assert_eq!(
            payload["navigation"]["final_url"],
            "https://example.test/dashboard"
        );
        assert_ne!(
            payload["verification"]["before_screenshot_id"],
            payload["verification"]["after_screenshot_id"]
        );
    }

    #[tokio::test]
    async fn browser_step_done_requires_final_evidence() {
        let provider = test_provider_with_decision(done_decision(), FakeBrowserSidecar::new());
        let executors = provider.tool_runtime_executors();
        let start = execute(&executors, TOOL_BROWSER_START, r#"{"task_id":"task-1"}"#).await;
        let session_id = start.structured_payload.as_ref().expect("payload")["session_id"]
            .as_str()
            .expect("session id");
        let step_args = format!(r#"{{"session_id":"{session_id}"}}"#);

        let step = execute(&executors, TOOL_BROWSER_STEP, &step_args).await;
        let payload = step.structured_payload.as_ref().expect("payload");

        assert!(step.success);
        assert_eq!(payload["status"], "done");
        assert_eq!(payload["task_success"], true);
        assert_eq!(payload["verification"]["status"], "done");
        assert_eq!(
            payload["evidence"],
            "The dashboard success banner is visible."
        );
    }

    #[tokio::test]
    async fn browser_step_timeout_produces_structured_report() {
        let fake =
            FakeBrowserSidecar::new().with_action_script(vec![FakeActionOutcome::DelaySuccess(
                Duration::from_millis(50),
            )]);
        let provider = test_provider_with_decision(click_decision(), fake);
        let executors = provider.tool_runtime_executors();
        let start = execute(&executors, TOOL_BROWSER_START, r#"{"task_id":"task-1"}"#).await;
        let session_id = start.structured_payload.as_ref().expect("payload")["session_id"]
            .as_str()
            .expect("session id");
        let step_args = format!(r#"{{"session_id":"{session_id}","action_timeout_ms":5}}"#);

        let step = execute(&executors, TOOL_BROWSER_STEP, &step_args).await;
        let payload = step.structured_payload.as_ref().expect("payload");

        assert!(step.success);
        assert_eq!(payload["status"], "timeout");
        assert_eq!(payload["verification"]["status"], "timeout");
        assert!(
            payload["message"]
                .as_str()
                .expect("message")
                .contains("exceeded timeout")
        );
    }

    #[tokio::test]
    async fn browser_step_recovery_classifies_coordinate_drift() {
        let fake = FakeBrowserSidecar::new().with_action_script(vec![
            FakeActionOutcome::CoordinateDrift,
            FakeActionOutcome::Success,
        ]);
        let provider = test_provider_with_decision(click_decision(), fake);
        let executors = provider.tool_runtime_executors();
        let start = execute(&executors, TOOL_BROWSER_START, r#"{"task_id":"task-1"}"#).await;
        let session_id = start.structured_payload.as_ref().expect("payload")["session_id"]
            .as_str()
            .expect("session id");
        let step_args = format!(r#"{{"session_id":"{session_id}"}}"#);

        let step = execute(&executors, TOOL_BROWSER_STEP, &step_args).await;
        let payload = step.structured_payload.as_ref().expect("payload");

        assert_eq!(payload["status"], "verification_failed");
        assert_eq!(payload["recovery"]["kind"], "coordinate_mismatch");
        assert_eq!(payload["recovery"]["status"], "attempted");
        assert_eq!(payload["recovery"]["attempted_steps"], 1);
        assert_eq!(payload["recovery"]["plan"]["action"]["kind"], "scroll");
    }

    #[tokio::test]
    async fn browser_step_recovery_handles_stale_screenshot() {
        let fake = FakeBrowserSidecar::new().with_action_script(vec![
            FakeActionOutcome::StaleFrame,
            FakeActionOutcome::Success,
        ]);
        let provider = test_provider_with_decision(click_decision(), fake);
        let executors = provider.tool_runtime_executors();
        let start = execute(&executors, TOOL_BROWSER_START, r#"{"task_id":"task-1"}"#).await;
        let session_id = start.structured_payload.as_ref().expect("payload")["session_id"]
            .as_str()
            .expect("session id");
        let step_args = format!(r#"{{"session_id":"{session_id}"}}"#);

        let step = execute(&executors, TOOL_BROWSER_STEP, &step_args).await;
        let payload = step.structured_payload.as_ref().expect("payload");

        assert_eq!(payload["recovery"]["kind"], "stale_frame");
        assert_eq!(payload["recovery"]["plan"]["action"]["kind"], "wait");
        assert_eq!(payload["recovery"]["attempted_steps"], 1);
    }

    #[tokio::test]
    async fn browser_step_recovery_handles_modal_overlay() {
        let fake = FakeBrowserSidecar::new()
            .with_action_script(vec![FakeActionOutcome::NoOp, FakeActionOutcome::Success]);
        fake.add_console_error("modal overlay intercepted click");
        let provider = test_provider_with_decision(click_decision(), fake);
        let executors = provider.tool_runtime_executors();
        let start = execute(&executors, TOOL_BROWSER_START, r#"{"task_id":"task-1"}"#).await;
        let session_id = start.structured_payload.as_ref().expect("payload")["session_id"]
            .as_str()
            .expect("session id");
        let step_args = format!(r#"{{"session_id":"{session_id}"}}"#);

        let step = execute(&executors, TOOL_BROWSER_STEP, &step_args).await;
        let payload = step.structured_payload.as_ref().expect("payload");

        assert_eq!(payload["recovery"]["kind"], "modal_overlay");
        assert_eq!(payload["recovery"]["plan"]["action"]["kind"], "press");
        assert_eq!(payload["recovery"]["plan"]["action"]["key"], "Escape");
    }

    #[tokio::test]
    async fn browser_step_recovery_stops_repeated_noop_loop() {
        let fake = FakeBrowserSidecar::new().with_action_script(vec![
            FakeActionOutcome::NoOp,
            FakeActionOutcome::Success,
            FakeActionOutcome::NoOp,
            FakeActionOutcome::Success,
        ]);
        let provider = test_provider_with_decision(click_decision(), fake);
        let executors = provider.tool_runtime_executors();
        let start = execute(&executors, TOOL_BROWSER_START, r#"{"task_id":"task-1"}"#).await;
        let session_id = start.structured_payload.as_ref().expect("payload")["session_id"]
            .as_str()
            .expect("session id");
        let step_args = format!(r#"{{"session_id":"{session_id}"}}"#);

        let first = execute(&executors, TOOL_BROWSER_STEP, &step_args).await;
        let second = execute(&executors, TOOL_BROWSER_STEP, &step_args).await;
        let first_payload = first.structured_payload.as_ref().expect("first payload");
        let second_payload = second.structured_payload.as_ref().expect("second payload");

        assert_eq!(first_payload["recovery"]["status"], "attempted");
        assert_eq!(
            second_payload["recovery"]["status"],
            "repeated_loop_stopped"
        );
        assert_eq!(second_payload["recovery"]["repeated"], true);
        assert_eq!(second_payload["recovery"]["attempted_steps"], 0);
    }

    #[tokio::test]
    async fn browser_step_recovery_attaches_debug_artifacts_and_disables_js_fallback() {
        let fake = FakeBrowserSidecar::new().with_action_script(vec![FakeActionOutcome::NoOp]);
        fake.add_network_failure("https://example.test/api", "connection reset");
        fake.add_console_error("Uncaught fake error");
        let provider = test_provider_with_decision(click_decision(), fake);
        let executors = provider.tool_runtime_executors();
        let start = execute(&executors, TOOL_BROWSER_START, r#"{"task_id":"task-1"}"#).await;
        let session_id = start.structured_payload.as_ref().expect("payload")["session_id"]
            .as_str()
            .expect("session id");
        let step_args = format!(r#"{{"session_id":"{session_id}"}}"#);

        let step = execute(&executors, TOOL_BROWSER_STEP, &step_args).await;
        let payload = step.structured_payload.as_ref().expect("payload");

        assert_eq!(payload["recovery"]["kind"], "network_failure");
        assert_eq!(payload["recovery"]["status"], "safe_stopped");
        assert_eq!(payload["recovery"]["js_click_allowed"], false);
        assert!(
            payload["recovery"]["diagnostics"]["network_artifact_uri"]
                .as_str()
                .expect("network artifact")
                .contains("network.json")
        );
        assert!(
            payload["recovery"]["diagnostics"]["console_artifact_uri"]
                .as_str()
                .expect("console artifact")
                .contains("console.json")
        );
    }

    #[tokio::test]
    async fn browser_step_sensitive_policy_blocks_before_action() {
        let mut decision = click_decision();
        decision.expected_result = "CAPTCHA solved".to_string();
        let provider = test_provider_with_decision(decision, FakeBrowserSidecar::new());
        let executors = provider.tool_runtime_executors();
        let start = execute(&executors, TOOL_BROWSER_START, r#"{"task_id":"task-1"}"#).await;
        let session_id = start.structured_payload.as_ref().expect("payload")["session_id"]
            .as_str()
            .expect("session id");
        let step_args = format!(r#"{{"session_id":"{session_id}"}}"#);

        let step = execute(&executors, TOOL_BROWSER_STEP, &step_args).await;
        let payload = step.structured_payload.as_ref().expect("payload");

        assert!(step.success);
        assert_eq!(payload["status"], "blocked");
        assert_eq!(payload["policy_audit"]["event"], "browser_policy");
        assert_eq!(payload["policy_audit"]["decision"], "block");
        assert!(
            !serde_json::to_string(payload)
                .expect("json")
                .contains("base64")
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

    fn test_provider_with_decision(
        decision: BrowserDecision,
        fake: FakeBrowserSidecar,
    ) -> Arc<BrowserLiveProvider> {
        Arc::new(
            BrowserLiveProvider::new(Arc::new(fake), BrowserArtifactSettings::default(), None)
                .with_decision_engine(StaticDecisionEngine { decision }),
        )
    }

    #[derive(Clone)]
    struct StaticDecisionEngine {
        decision: BrowserDecision,
    }

    #[async_trait]
    impl BrowserDecisionEngine for StaticDecisionEngine {
        async fn decide(
            &self,
            _image_bytes: Vec<u8>,
            _context: &BrowserDecisionPromptContext<'_>,
            _viewport: Viewport,
        ) -> Result<BrowserDecision, BrowserMimoError> {
            Ok(self.decision.clone())
        }
    }

    fn click_decision() -> BrowserDecision {
        decision(BrowserDecisionAction::ClickXy {
            x: 10,
            y: 20,
            target_description: Some("login button".to_string()),
        })
    }

    fn navigate_decision() -> BrowserDecision {
        decision(BrowserDecisionAction::Navigate {
            url: "https://example.test/dashboard".to_string(),
        })
    }

    fn done_decision() -> BrowserDecision {
        decision(BrowserDecisionAction::Done {
            final_answer: "Dashboard opened.".to_string(),
            evidence: "The dashboard success banner is visible.".to_string(),
        })
    }

    fn decision(action: BrowserDecisionAction) -> BrowserDecision {
        BrowserDecision {
            schema_version: 1,
            rationale: "test decision".to_string(),
            action,
            expected_result: "The visible browser state changes as expected.".to_string(),
            confidence: 0.9,
            risk: BrowserDecisionRisk::Low,
            sensitive_action: BrowserSensitiveAction {
                required: false,
                category: None,
                reason: None,
            },
            needs_debug: false,
        }
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

    async fn execute_result(
        executors: &[Arc<dyn ToolExecutor>],
        name: &str,
        args: &str,
    ) -> Result<ToolOutput, ToolRuntimeError> {
        let executor = executors
            .iter()
            .find(|executor| executor.name().as_str() == name)
            .expect("executor");
        executor.execute(invocation(name, args)).await
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
