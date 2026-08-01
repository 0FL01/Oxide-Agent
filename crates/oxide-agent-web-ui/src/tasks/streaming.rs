use crate::sse::{TaskStreamConfig, spawn_task_stream};
use leptos::prelude::{Callback, GetUntracked, ReadSignal, Set, WriteSignal};
use oxide_agent_web_contracts::{
    PersistedTaskEvent, ProgressSnapshot, SessionSummary, TaskDetail, TaskSummary,
};

#[derive(Clone, Copy)]
pub(super) struct StreamUiSignals {
    pub(super) set_events: WriteSignal<Vec<PersistedTaskEvent>>,
    pub(super) update_progress: Callback<(String, Option<ProgressSnapshot>)>,
    pub(super) begin_activity_run: Callback<(String, u64)>,
    pub(super) set_active_task: WriteSignal<Option<TaskDetail>>,
    pub(super) set_tasks: WriteSignal<Vec<TaskSummary>>,
    pub(super) set_error: WriteSignal<Option<String>>,
    pub(super) stream_owner: ReadSignal<Option<(String, u64)>>,
    pub(super) set_stream_owner: WriteSignal<Option<(String, u64)>>,
    pub(super) stream_generation: ReadSignal<u64>,
    pub(super) set_stream_generation: WriteSignal<u64>,
    pub(super) set_sessions: WriteSignal<Vec<SessionSummary>>,
}

pub(super) fn start_task_stream(
    client: crate::api::ApiClient,
    session_id: String,
    task_id: String,
    initial_last_seq: u64,
    signals: StreamUiSignals,
) {
    let generation = signals.stream_generation.get_untracked().wrapping_add(1);
    signals.set_stream_generation.set(generation);
    signals
        .set_stream_owner
        .set(Some((task_id.clone(), generation)));
    spawn_task_stream(TaskStreamConfig {
        client,
        session_id,
        task_id,
        initial_last_seq,
        set_sessions: signals.set_sessions,
        set_events: signals.set_events,
        update_progress: signals.update_progress,
        set_active_task: signals.set_active_task,
        set_tasks: signals.set_tasks,
        set_error: signals.set_error,
        stream_owner: signals.stream_owner,
        set_stream_owner: signals.set_stream_owner,
        stream_generation: generation,
    });
}
