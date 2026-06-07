//! Background task execution engine for the web transport.
//!
//! Handles spawning agent tasks, collecting events, persisting results,
//! and tracking timeline milestones.

use super::{
    markdown_preview, pending_user_input_view, progress_snapshot_from_serializable, AppState,
    Milestones, SerializableProgress, TaskTimelineRecord, EVENT_LOGS, YOLO_APPROVAL_DIAGNOSTIC,
};
use crate::persistence::WebUiStore;
use crate::session::{RunningTask, ToolCallTiming, WebSessionManager};
use crate::web_transport::{collect_events, BrowserEventScope, TaskEventLog};
use oxide_agent_core::agent::{
    AgentExecutionEffort, AgentExecutionOptions, AgentExecutionOutcome, AgentUserInput,
    PendingUserInput,
};
use oxide_agent_web_contracts::{
    AgentEffort as WebAgentEffort, PersistedTaskEvent, ProgressSnapshot,
    TaskStatus as ApiTaskStatus, WebTaskRecord,
};
use std::collections::HashMap as StdHashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info, warn};

const WEB_LATENCY_TARGET: &str = "oxide_agent_transport_web::web_latency";

/// Shared state needed by the task executor.
struct TaskExecutorCtx {
    task_progress: Arc<RwLock<StdHashMap<String, SerializableProgress>>>,
    task_timeline: Arc<RwLock<StdHashMap<String, TaskTimelineRecord>>>,
    web_task: Option<WebTaskPersistence>,
    queued_at: Instant,
}

#[derive(Clone)]
pub(crate) struct WebTaskPersistence {
    pub(crate) web_store: Arc<dyn WebUiStore>,
    pub(crate) user_id: i64,
    pub(crate) session_id: String,
    pub(crate) task_id: String,
    /// In-process event log for live SSE subscribers. `None` for tasks that
    /// run without a browser-visible session (tests, internal jobs).
    pub(crate) event_log: Option<TaskEventLog>,
}

struct ExecutorTaskCtx {
    session_manager: Arc<WebSessionManager>,
    session_id: String,
    task_id: String,
    run_request: TaskRunRequest,
    executor_arc: Arc<tokio::sync::RwLock<oxide_agent_core::agent::AgentExecutor>>,
    tx: mpsc::Sender<oxide_agent_core::agent::AgentEvent>,
    timeline_map: Arc<RwLock<StdHashMap<String, TaskTimelineRecord>>>,
    agent_started_at: Instant,
    queued_at: Instant,
    web_task: Option<WebTaskPersistence>,
    event_collector_handle: tokio::task::JoinHandle<()>,
}

pub(crate) enum TaskRunRequest {
    Execute {
        input: AgentUserInput,
        effort: Option<WebAgentEffort>,
    },
    ResumeUserInput {
        input: AgentUserInput,
        effort: Option<WebAgentEffort>,
    },
}

fn execution_options_from_effort(effort: Option<WebAgentEffort>) -> AgentExecutionOptions {
    let Some(effort) = effort else {
        return AgentExecutionOptions::default();
    };
    let effort = match effort {
        WebAgentEffort::Standard => AgentExecutionEffort::Standard,
        WebAgentEffort::Extended => AgentExecutionEffort::Extended,
        WebAgentEffort::Heavy => AgentExecutionEffort::Heavy,
    };
    AgentExecutionOptions::with_effort(effort)
}

pub(crate) async fn spawn_registered_task(
    state: AppState,
    session_id: String,
    running_task: RunningTask,
    run_request: TaskRunRequest,
    web_task: Option<WebTaskPersistence>,
) {
    let queued_at = Instant::now();
    let task_id = running_task.task_id.clone();
    let task_progress = state.task_progress.clone();
    let task_timeline = state.task_timeline.clone();
    let session_manager = state.session_manager.clone();

    {
        let mut tl = task_timeline.write().await;
        tl.insert(
            task_id.clone(),
            TaskTimelineRecord {
                milestones: Milestones {
                    session_ready_ms: None,
                    executor_lock_acquired_ms: None,
                    prepare_execution_done_ms: None,
                    pre_run_compaction_done_ms: None,
                    thinking_sent_ms: None,
                    llm_call_started_ms: None,
                    first_tool_call_ms: None,
                    last_tool_call_ms: None,
                    first_tool_result_ms: None,
                    last_tool_result_ms: None,
                    first_thinking_ms: None,
                    final_response_ms: None,
                },
                tool_calls: Vec::new(),
            },
        );
    }

    {
        let mut logs = EVENT_LOGS.lock().await;
        logs.insert(task_id.clone(), running_task.event_log.clone());
    }

    let ctx = TaskExecutorCtx {
        task_progress,
        task_timeline,
        web_task,
        queued_at,
    };
    let task_handles = state.task_handles.clone();
    let tid_for_cleanup = task_id.clone();
    let session_id_for_task = session_id.clone();
    let task_id_for_log = tid_for_cleanup.clone();
    let session_id_for_log = session_id_for_task.clone();

    let handle = tokio::spawn(async move {
        execute_agent_task(
            session_manager,
            &session_id_for_task,
            &tid_for_cleanup,
            run_request,
            ctx,
        )
        .await;

        let mut handles = task_handles.write().await;
        handles.remove(&tid_for_cleanup);
    });

    {
        let mut handles = state.task_handles.write().await;
        handles.insert(task_id, Arc::new(handle));
    }

    debug!(
        target: WEB_LATENCY_TARGET,
        session_id = %session_id_for_log,
        task_id = %task_id_for_log,
        phase = "task_tokio_spawned",
        elapsed_ms = queued_at.elapsed().as_millis(),
        "web task executor latency"
    );

    tokio::task::yield_now().await;
}

async fn execute_agent_task(
    session_manager: Arc<WebSessionManager>,
    session_id: &str,
    task_id: &str,
    run_request: TaskRunRequest,
    ctx: TaskExecutorCtx,
) {
    let executor_started_at = Instant::now();
    debug!(
        target: WEB_LATENCY_TARGET,
        session_id = %session_id,
        task_id = %task_id,
        phase = "execute_agent_task_started",
        queue_wait_ms = ctx.queued_at.elapsed().as_millis(),
        "web task executor latency"
    );

    let registry = session_manager.session_registry();
    let sid = derive_session_id(&session_manager, session_id).await;
    let Some(sid) = sid else {
        if let Some(web_task) = &ctx.web_task {
            persist_task_failed(web_task, "Runtime session not found.").await;
        }
        session_manager.fail_task(task_id, session_id).await;
        return;
    };

    // Record instant when agent execution starts - used as reference
    // for all latency milestones (NOT HTTP request time).
    let agent_started_at = Instant::now();

    let executor_arc = match registry.get(&sid).await {
        Some(e) => e,
        None => {
            if let Some(web_task) = &ctx.web_task {
                persist_task_failed(web_task, "Runtime executor not found.").await;
            }
            session_manager.fail_task(task_id, session_id).await;
            return;
        }
    };
    debug!(
        target: WEB_LATENCY_TARGET,
        session_id = %session_id,
        task_id = %task_id,
        phase = "runtime_executor_resolved",
        queue_elapsed_ms = ctx.queued_at.elapsed().as_millis(),
        executor_elapsed_ms = executor_started_at.elapsed().as_millis(),
        "web task executor latency"
    );
    let cancellation_token = match registry.get_cancellation_token(&sid).await {
        Some(token) => token,
        None => {
            if let Some(web_task) = &ctx.web_task {
                persist_task_failed(web_task, "Runtime cancellation token not found.").await;
            }
            session_manager.fail_task(task_id, session_id).await;
            return;
        }
    };
    debug!(
        target: WEB_LATENCY_TARGET,
        session_id = %session_id,
        task_id = %task_id,
        phase = "runtime_cancellation_token_resolved",
        queue_elapsed_ms = ctx.queued_at.elapsed().as_millis(),
        executor_elapsed_ms = executor_started_at.elapsed().as_millis(),
        "web task executor latency"
    );

    {
        let lock_started_at = Instant::now();
        let mut executor = executor_arc.write().await;
        executor.session_mut().cancellation_token = (*cancellation_token).clone();
        debug!(
            target: WEB_LATENCY_TARGET,
            session_id = %session_id,
            task_id = %task_id,
            phase = "cancellation_token_installed",
            executor_lock_wait_ms = lock_started_at.elapsed().as_millis(),
            queue_elapsed_ms = ctx.queued_at.elapsed().as_millis(),
            executor_elapsed_ms = executor_started_at.elapsed().as_millis(),
            "web task executor latency"
        );
    }

    // Capture chrono timestamp for calculating offsets from named milestones.
    let agent_started_at_chrono = chrono::Utc::now().timestamp_millis();

    // Get event log from global registry.
    let event_log = {
        let logs = EVENT_LOGS.lock().await;
        logs.get(task_id)
            .cloned()
            .unwrap_or_else(crate::web_transport::TaskEventLog::new)
    };

    let (tx, rx) = mpsc::channel::<oxide_agent_core::agent::AgentEvent>(100);

    let tid = task_id.to_string();
    let event_collector_handle = spawn_event_collector(
        event_log,
        rx,
        ctx.task_progress.clone(),
        ctx.task_timeline.clone(),
        tid.clone(),
        agent_started_at_chrono,
        ctx.web_task.clone(),
    );
    debug!(
        target: WEB_LATENCY_TARGET,
        session_id = %session_id,
        task_id = %task_id,
        phase = "event_collector_spawned",
        queue_elapsed_ms = ctx.queued_at.elapsed().as_millis(),
        executor_elapsed_ms = executor_started_at.elapsed().as_millis(),
        "web task executor latency"
    );
    spawn_executor_task(ExecutorTaskCtx {
        session_manager,
        session_id: session_id.to_string(),
        task_id: tid,
        run_request,
        executor_arc,
        tx,
        timeline_map: ctx.task_timeline.clone(),
        agent_started_at,
        queued_at: ctx.queued_at,
        web_task: ctx.web_task,
        event_collector_handle,
    });
}

fn spawn_event_collector(
    event_log: crate::web_transport::TaskEventLog,
    rx: mpsc::Receiver<oxide_agent_core::agent::AgentEvent>,
    progress_map: Arc<RwLock<StdHashMap<String, SerializableProgress>>>,
    timeline_map: Arc<RwLock<StdHashMap<String, TaskTimelineRecord>>>,
    task_id: String,
    agent_started_at_ms: i64,
    web_task: Option<WebTaskPersistence>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let browser_event_scope = web_task.as_ref().map(|web_task| {
            BrowserEventScope::new(
                web_task.user_id,
                web_task.session_id.clone(),
                web_task.task_id.clone(),
            )
        });
        let (live_event_tx, live_persister_handle) =
            web_task.clone().map_or((None, None), |web_task| {
                let (tx, rx) = mpsc::unbounded_channel();
                (Some(tx), Some(spawn_live_event_persister(web_task, rx)))
            });
        let (live_progress_tx, live_progress_persister_handle) =
            web_task.clone().map_or((None, None), |web_task| {
                let (tx, rx) = mpsc::unbounded_channel();
                (Some(tx), Some(spawn_live_progress_persister(web_task, rx)))
            });
        let collected = collect_events(
            event_log,
            rx,
            browser_event_scope,
            web_task.as_ref().map(|web_task| web_task.web_store.clone()),
            live_event_tx,
            live_progress_tx,
        )
        .await;
        let progress = SerializableProgress::from_state(&collected.state);

        {
            let mut pm = progress_map.write().await;
            pm.insert(task_id.clone(), progress.clone());
        }

        let mut tl = timeline_map.write().await;
        if let Some(record) = tl.get_mut(&task_id) {
            apply_event_collection(record, &collected, agent_started_at_ms);
        }

        if let Some(web_task) = web_task {
            if let Some(handle) = live_persister_handle {
                if let Err(error) = handle.await {
                    warn!(
                        task_id = %web_task.task_id,
                        error = %error,
                        "Live web event persistence task failed"
                    );
                }
            } else {
                persist_task_events(&web_task, collected.persisted_events).await;
            }
            if let Some(handle) = live_progress_persister_handle
                && let Err(error) = handle.await {
                    warn!(
                        task_id = %web_task.task_id,
                        error = %error,
                        "Live web progress persistence task failed"
                    );
                }
            persist_task_progress(&web_task, progress).await;
        }
    })
}

fn spawn_live_event_persister(
    web_task: WebTaskPersistence,
    mut rx: mpsc::UnboundedReceiver<PersistedTaskEvent>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            persist_task_events(&web_task, vec![event]).await;
        }
    })
}

pub(crate) fn spawn_live_progress_persister(
    web_task: WebTaskPersistence,
    mut rx: mpsc::UnboundedReceiver<oxide_agent_core::agent::progress::ProgressState>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(state) = rx.recv().await {
            persist_task_progress(&web_task, SerializableProgress::from_state(&state)).await;
        }
    })
}

fn spawn_executor_task(ctx: ExecutorTaskCtx) {
    tokio::spawn(async move {
        let ExecutorTaskCtx {
            session_manager,
            session_id,
            task_id,
            run_request,
            executor_arc,
            tx,
            timeline_map,
            agent_started_at,
            queued_at,
            web_task,
            event_collector_handle,
        } = ctx;

        let result = {
            let executor_lock_started_at = Instant::now();
            debug!(
                target: WEB_LATENCY_TARGET,
                session_id = %session_id,
                task_id = %task_id,
                phase = "executor_lock_wait_started",
                queue_elapsed_ms = queued_at.elapsed().as_millis(),
                agent_elapsed_ms = agent_started_at.elapsed().as_millis(),
                "web task executor latency"
            );
            let mut executor = executor_arc.write().await;
            let executor_lock_acquired_ms = Some(agent_started_at.elapsed().as_millis() as i64);
            record_executor_lock_acquired(&timeline_map, &task_id, executor_lock_acquired_ms).await;
            debug!(
                target: WEB_LATENCY_TARGET,
                session_id = %session_id,
                task_id = %task_id,
                phase = "executor_lock_acquired",
                executor_lock_wait_ms = executor_lock_started_at.elapsed().as_millis(),
                queue_elapsed_ms = queued_at.elapsed().as_millis(),
                agent_elapsed_ms = agent_started_at.elapsed().as_millis(),
                "web task executor latency"
            );
            match run_request {
                TaskRunRequest::Execute {
                    input: task_text,
                    effort,
                } => {
                    let options = execution_options_from_effort(effort);
                    debug!(
                        target: WEB_LATENCY_TARGET,
                        session_id = %session_id,
                        task_id = %task_id,
                        run_kind = "execute",
                        phase = "core_executor_call_started",
                        queue_elapsed_ms = queued_at.elapsed().as_millis(),
                        agent_elapsed_ms = agent_started_at.elapsed().as_millis(),
                        "web task executor latency"
                    );
                    executor
                        .execute_user_input_with_options(task_text, Some(tx), options)
                        .await
                }
                TaskRunRequest::ResumeUserInput { input, effort } => {
                    let options = execution_options_from_effort(effort);
                    debug!(
                        target: WEB_LATENCY_TARGET,
                        session_id = %session_id,
                        task_id = %task_id,
                        run_kind = "resume_user_input",
                        phase = "core_executor_call_started",
                        queue_elapsed_ms = queued_at.elapsed().as_millis(),
                        agent_elapsed_ms = agent_started_at.elapsed().as_millis(),
                        "web task executor latency"
                    );
                    executor
                        .resume_user_input_with_options(input, Some(tx), options)
                        .await
                }
            }
        };

        if let Err(error) = event_collector_handle.await {
            warn!(
                task_id = %task_id,
                error = %error,
                "Task event collector failed before outcome persistence"
            );
        }

        match result {
            Ok(AgentExecutionOutcome::Completed(final_response)) => {
                if let Some(web_task) = &web_task {
                    persist_task_completed(web_task, final_response).await;
                }
                session_manager.complete_task(&task_id, &session_id).await;
                info!(task_id = %task_id, "Task completed");
            }
            Ok(AgentExecutionOutcome::WaitingForUserInput(pending)) => {
                if let Some(web_task) = &web_task {
                    persist_task_waiting_for_user_input(web_task, pending).await;
                }
                session_manager.complete_task(&task_id, &session_id).await;
                info!(task_id = %task_id, "Task paused waiting for user input");
            }
            Ok(AgentExecutionOutcome::WaitingForApproval) => {
                if let Some(web_task) = &web_task {
                    persist_task_failed(web_task, YOLO_APPROVAL_DIAGNOSTIC).await;
                }
                session_manager.fail_task(&task_id, &session_id).await;
                info!(task_id = %task_id, "Task failed after unexpected approval wait");
            }
            Err(e) => {
                if let Some(web_task) = &web_task {
                    persist_task_failed(web_task, e.to_string()).await;
                }
                session_manager.fail_task(&task_id, &session_id).await;
                info!(task_id = %task_id, error = %e, "Task failed");
            }
        }
    });
}

async fn persist_task_completed(web_task: &WebTaskPersistence, final_response: String) {
    let now = chrono::Utc::now();
    let preview = markdown_preview(&final_response);
    let updated = update_web_task_unless_cancelled(web_task, |task| {
        task.status = ApiTaskStatus::Completed;
        task.final_response_markdown = Some(final_response);
        task.error_message = None;
        task.pending_user_input = None;
        task.updated_at = now;
        task.finished_at = Some(now);
    })
    .await;
    if updated {
        update_web_session_for_task(web_task, ApiTaskStatus::Completed, None, Some(preview), now)
            .await;
    }
    broadcast_status_if_present(web_task, ApiTaskStatus::Completed, true).await;
    close_event_log_if_present(web_task).await;
}

async fn persist_task_waiting_for_user_input(
    web_task: &WebTaskPersistence,
    pending: PendingUserInput,
) {
    let now = chrono::Utc::now();
    let updated = update_web_task_unless_cancelled(web_task, |task| {
        task.status = ApiTaskStatus::WaitingForUserInput;
        task.pending_user_input = Some(pending_user_input_view(pending));
        task.error_message = None;
        task.updated_at = now;
        task.finished_at = None;
    })
    .await;
    if updated {
        update_web_session_for_task(
            web_task,
            ApiTaskStatus::WaitingForUserInput,
            Some(web_task.task_id.clone()),
            None,
            now,
        )
        .await;
    }
    broadcast_status_if_present(web_task, ApiTaskStatus::WaitingForUserInput, false).await;
    close_event_log_if_present(web_task).await;
}

async fn persist_task_failed(web_task: &WebTaskPersistence, message: impl Into<String>) {
    let now = chrono::Utc::now();
    let message = message.into();
    let updated = update_web_task_unless_cancelled(web_task, |task| {
        task.status = ApiTaskStatus::Failed;
        task.error_message = Some(message);
        task.pending_user_input = None;
        task.updated_at = now;
        task.finished_at = Some(now);
    })
    .await;
    if updated {
        update_web_session_for_task(web_task, ApiTaskStatus::Failed, None, None, now).await;
    }
    broadcast_status_if_present(web_task, ApiTaskStatus::Failed, false).await;
    close_event_log_if_present(web_task).await;
}

async fn close_event_log_if_present(web_task: &WebTaskPersistence) {
    if let Some(event_log) = web_task.event_log.as_ref() {
        event_log.close().await;
        if let Some(closed_at) = event_log.closed_at().await {
            spawn_event_log_cleanup(web_task.task_id.clone(), closed_at);
        }
    }
}

/// Spawn a background task that removes the `TaskEventLog` entry for
/// `task_id` from the global `EVENT_LOGS` registry after
/// `EVENT_LOG_RETENTION_AFTER_CLOSE`. Late subscribers that connect
/// during the retention window can still query the in-memory snapshot
/// for replay; after the window expires the entry is evicted and
/// subscribers fall back to the durable DB.
fn spawn_event_log_cleanup(task_id: String, closed_at: std::time::Instant) {
    tokio::spawn(async move {
        tokio::time::sleep(super::EVENT_LOG_RETENTION_AFTER_CLOSE).await;
        let mut logs = EVENT_LOGS.lock().await;
        // Only evict if the entry in the map is still the same closed
        // log. A fresh task that re-used the same id would have a
        // different `closed_at` (or `None`), and we must not touch it.
        if let Some(current) = logs.get(&task_id)
            && current.closed_at().await == Some(closed_at) {
                logs.remove(&task_id);
            }
    });
}

async fn broadcast_status_if_present(
    web_task: &WebTaskPersistence,
    status: ApiTaskStatus,
    final_response_available: bool,
) {
    let Some(event_log) = web_task.event_log.as_ref() else {
        return;
    };
    let last_seq = web_task
        .web_store
        .load_task_event_state(web_task.user_id, &web_task.session_id, &web_task.task_id)
        .await
        .ok()
        .flatten()
        .map(|state| state.last_event_seq)
        .unwrap_or_else(|| 0);
    event_log
        .notify_status(status, final_response_available, last_seq)
        .await;
}

async fn broadcast_progress_if_present(web_task: &WebTaskPersistence, snapshot: ProgressSnapshot) {
    let Some(event_log) = web_task.event_log.as_ref() else {
        return;
    };
    let last_seq = web_task
        .web_store
        .load_task_event_state(web_task.user_id, &web_task.session_id, &web_task.task_id)
        .await
        .ok()
        .flatten()
        .map(|state| state.last_event_seq)
        .unwrap_or_else(|| 0);
    event_log.notify_progress(snapshot, last_seq).await;
}

async fn persist_task_progress(web_task: &WebTaskPersistence, progress: SerializableProgress) {
    let now = chrono::Utc::now();
    let snapshot = progress_snapshot_from_serializable(progress);
    update_web_task_unless_cancelled(web_task, |task| {
        task.last_progress = Some(snapshot.clone());
        task.updated_at = now;
    })
    .await;
    broadcast_progress_if_present(web_task, snapshot).await;
}

async fn persist_task_events(
    web_task: &WebTaskPersistence,
    events: Vec<oxide_agent_web_contracts::PersistedTaskEvent>,
) {
    let Some(last_seq) = events.last().map(|event| event.seq) else {
        return;
    };
    if web_task_is_cancelled(web_task).await {
        return;
    }

    if let Err(error) = web_task
        .web_store
        .append_task_events(
            web_task.user_id,
            &web_task.session_id,
            &web_task.task_id,
            events.clone(),
        )
        .await
    {
        warn!(
            task_id = %web_task.task_id,
            error = %error,
            "Failed to persist web task events"
        );
        return;
    }

    if let Some(event_log) = web_task.event_log.as_ref() {
        for event in &events {
            event_log.push_persisted(event.clone()).await;
        }
    }

    let now = chrono::Utc::now();
    update_web_task_unless_cancelled(web_task, |task| {
        task.last_event_seq = task.last_event_seq.max(last_seq);
        task.updated_at = now;
    })
    .await;
}

async fn web_task_is_cancelled(web_task: &WebTaskPersistence) -> bool {
    let task = web_task
        .web_store
        .load_task(web_task.user_id, &web_task.session_id, &web_task.task_id)
        .await;
    matches!(task, Ok(Some(task)) if task.status == ApiTaskStatus::Cancelled)
}

async fn update_web_task_unless_cancelled(
    web_task: &WebTaskPersistence,
    update: impl FnOnce(&mut WebTaskRecord),
) -> bool {
    let task = web_task
        .web_store
        .load_task(web_task.user_id, &web_task.session_id, &web_task.task_id)
        .await;
    let Ok(Some(mut task)) = task else {
        warn!(
            task_id = %web_task.task_id,
            "Failed to load web task for terminal persistence update"
        );
        return false;
    };
    if task.status == ApiTaskStatus::Cancelled {
        return false;
    }

    update(&mut task);
    if let Err(error) = web_task.web_store.save_task(task).await {
        warn!(
            task_id = %web_task.task_id,
            error = %error,
            "Failed to persist terminal web task update"
        );
        return false;
    }
    true
}

async fn update_web_session_for_task(
    web_task: &WebTaskPersistence,
    status: ApiTaskStatus,
    active_task_id: Option<String>,
    last_preview: Option<String>,
    updated_at: chrono::DateTime<chrono::Utc>,
) {
    let session = web_task
        .web_store
        .load_session(web_task.user_id, &web_task.session_id)
        .await;
    let Ok(Some(mut session)) = session else {
        warn!(
            session_id = %web_task.session_id,
            "Failed to load web session for task status update"
        );
        return;
    };

    session.active_task_id = active_task_id;
    session.last_task_status = Some(status);
    if let Some(last_preview) = last_preview {
        session.last_preview = Some(last_preview);
    }
    session.updated_at = updated_at;

    if let Err(error) = web_task.web_store.save_session(session).await {
        warn!(
            session_id = %web_task.session_id,
            error = %error,
            "Failed to persist web session task status update"
        );
    }
}

async fn record_executor_lock_acquired(
    timeline_map: &Arc<RwLock<StdHashMap<String, TaskTimelineRecord>>>,
    task_id: &str,
    executor_lock_acquired_ms: Option<i64>,
) {
    let mut tl = timeline_map.write().await;
    if let Some(record) = tl.get_mut(task_id) {
        record.milestones.executor_lock_acquired_ms = executor_lock_acquired_ms;
        record.milestones.session_ready_ms = executor_lock_acquired_ms;
    }
}

fn apply_event_collection(
    record: &mut TaskTimelineRecord,
    collected: &crate::web_transport::EventCollectionResult,
    agent_started_at_ms: i64,
) {
    let tool_calls = collected
        .tool_calls
        .iter()
        .map(|timing| ToolCallTiming {
            name: timing.name.clone(),
            started_at_ms: relative_timestamp_ms(agent_started_at_ms, timing.started_at),
            finished_at_ms: timing
                .finished_at
                .map(|finished_at| relative_timestamp_ms(agent_started_at_ms, finished_at)),
        })
        .collect::<Vec<_>>();

    record.tool_calls = tool_calls;

    // Derive llm_call_started_ms from the collector-side clock (first Thinking or
    // Reasoning event). This keeps it in the same time domain as first_tool_call_ms
    // and makes monotonicity assertions meaningful.
    let llm_started_at = earliest_of(
        collected.timestamps.first_thinking_at,
        collected.timestamps.first_reasoning_at,
    );
    record.milestones.llm_call_started_ms =
        llm_started_at.map(|ts| relative_timestamp_ms(agent_started_at_ms, ts));

    record.milestones.first_thinking_ms = collected
        .timestamps
        .first_thinking_at
        .map(|ts| relative_timestamp_ms(agent_started_at_ms, ts));
    record.milestones.final_response_ms = collected
        .timestamps
        .finished_at
        .map(|ts| relative_timestamp_ms(agent_started_at_ms, ts));
    record.milestones.first_tool_call_ms = record
        .tool_calls
        .iter()
        .map(|timing| timing.started_at_ms)
        .min();
    record.milestones.last_tool_call_ms = record
        .tool_calls
        .iter()
        .map(|timing| timing.started_at_ms)
        .max();
    record.milestones.first_tool_result_ms = record
        .tool_calls
        .iter()
        .filter_map(|timing| timing.finished_at_ms)
        .min();
    record.milestones.last_tool_result_ms = record
        .tool_calls
        .iter()
        .filter_map(|timing| timing.finished_at_ms)
        .max();

    // Named milestones that use the agent's own Unix timestamps.
    // Note: "llm_call_started" is intentionally NOT applied here — it is
    // already derived from the collector-side clock above.
    for (name, ts) in &collected.timestamps.named_milestones {
        let ms = Some(ts.timestamp_millis() - agent_started_at_ms);
        match name.as_str() {
            "prepare_execution_done" => record.milestones.prepare_execution_done_ms = ms,
            "pre_run_compaction_done" => record.milestones.pre_run_compaction_done_ms = ms,
            "thinking_sent" => record.milestones.thinking_sent_ms = ms,
            _ => {}
        }
    }
}

fn earliest_of(
    a: Option<chrono::DateTime<chrono::Utc>>,
    b: Option<chrono::DateTime<chrono::Utc>>,
) -> Option<chrono::DateTime<chrono::Utc>> {
    match (a, b) {
        (Some(ts), None) => Some(ts),
        (None, Some(ts)) => Some(ts),
        (Some(a), Some(b)) => Some(a.min(b)),
        (None, None) => None,
    }
}

fn relative_timestamp_ms(
    agent_started_at_ms: i64,
    timestamp: chrono::DateTime<chrono::Utc>,
) -> i64 {
    timestamp.timestamp_millis() - agent_started_at_ms
}

async fn derive_session_id(
    session_manager: &WebSessionManager,
    session_id: &str,
) -> Option<oxide_agent_core::agent::SessionId> {
    let meta = session_manager.get_session(session_id).await?;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    session_id.hash(&mut h);
    meta.user_id.hash(&mut h);
    Some(oxide_agent_core::agent::SessionId::from(h.finish() as i64))
}
