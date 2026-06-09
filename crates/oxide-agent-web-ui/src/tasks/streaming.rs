use crate::sse::{TaskStreamConfig, spawn_task_stream};
use leptos::prelude::{ReadSignal, Set, WriteSignal};
use oxide_agent_web_contracts::{
    PersistedTaskEvent, ProgressSnapshot, SessionSummary, TaskDetail, TaskSummary,
};

#[derive(Clone, Copy)]
pub(super) struct StreamUiSignals {
    pub(super) set_events: WriteSignal<Vec<PersistedTaskEvent>>,
    pub(super) set_progress: WriteSignal<Option<ProgressSnapshot>>,
    pub(super) set_active_task: WriteSignal<Option<TaskDetail>>,
    pub(super) set_tasks: WriteSignal<Vec<TaskSummary>>,
    pub(super) set_error: WriteSignal<Option<String>>,
    pub(super) streaming_task_id: ReadSignal<Option<String>>,
    pub(super) set_streaming_task_id: WriteSignal<Option<String>>,
    pub(super) set_sessions: WriteSignal<Vec<SessionSummary>>,
}

pub(super) fn start_task_stream(
    client: crate::api::ApiClient,
    session_id: String,
    task_id: String,
    initial_last_seq: u64,
    signals: StreamUiSignals,
) {
    signals.set_streaming_task_id.set(Some(task_id.clone()));
    spawn_task_stream(TaskStreamConfig {
        client,
        session_id,
        task_id,
        initial_last_seq,
        set_sessions: signals.set_sessions,
        set_events: signals.set_events,
        set_progress: signals.set_progress,
        set_active_task: signals.set_active_task,
        set_tasks: signals.set_tasks,
        set_error: signals.set_error,
        streaming_task_id: signals.streaming_task_id,
        set_streaming_task_id: signals.set_streaming_task_id,
    });
}
