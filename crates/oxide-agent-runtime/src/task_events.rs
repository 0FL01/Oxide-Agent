//! Transport-agnostic runtime task event publishing.

use async_trait::async_trait;
use oxide_agent_core::agent::{TaskEvent, TaskId, TaskSnapshot};
use oxide_agent_core::storage::{StorageError, StorageProvider};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc::UnboundedSender, RwLock};

/// Sink for runtime-published task lifecycle events.
#[async_trait]
pub trait TaskEventPublisher: Send + Sync + 'static {
    /// Publish a task event to the next runtime consumer.
    async fn publish(&self, event: TaskEvent);
}

/// No-op task event publisher used when no sink is configured.
#[derive(Debug, Default)]
pub struct NoopTaskEventPublisher;

#[async_trait]
impl TaskEventPublisher for NoopTaskEventPublisher {
    async fn publish(&self, _event: TaskEvent) {}
}

/// Channel-backed task event publisher for tests and adapter integration.
#[derive(Debug, Clone)]
pub struct ChannelTaskEventPublisher {
    sender: UnboundedSender<TaskEvent>,
}

impl ChannelTaskEventPublisher {
    /// Create a new publisher that forwards task events into a channel.
    #[must_use]
    pub fn new(sender: UnboundedSender<TaskEvent>) -> Self {
        Self { sender }
    }
}

#[async_trait]
impl TaskEventPublisher for ChannelTaskEventPublisher {
    async fn publish(&self, event: TaskEvent) {
        let _ = self.sender.send(event);
    }
}

/// Backpressure policy used by the task event broadcaster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskEventBackpressurePolicy {
    /// Keep the latest bounded window and drop older events for lagging subscribers.
    ///
    /// Subscribers observe drops as `RecvError::Lagged` from `tokio::sync::broadcast`.
    DropOldest,
}

/// Construction options for [`TaskEventBroadcaster`].
pub struct TaskEventBroadcasterOptions {
    /// Storage used to recover persisted snapshots and events for late subscribers.
    pub storage: Arc<dyn StorageProvider>,
    /// Per-task in-memory ring size for live event fan-out.
    pub channel_capacity: usize,
    /// Backpressure behavior for slow subscribers.
    pub backpressure_policy: TaskEventBackpressurePolicy,
}

impl TaskEventBroadcasterOptions {
    /// Build options with sane defaults for runtime fan-out.
    #[must_use]
    pub fn new(storage: Arc<dyn StorageProvider>) -> Self {
        Self {
            storage,
            channel_capacity: 64,
            backpressure_policy: TaskEventBackpressurePolicy::DropOldest,
        }
    }
}

/// Result of task-scoped event subscription.
pub struct TaskEventSubscription {
    /// Current persisted snapshot for the task, if present.
    pub snapshot: Option<TaskSnapshot>,
    /// Persisted events newer than the requested sequence checkpoint.
    pub replay_events: Vec<TaskEvent>,
    /// Live fan-out receiver for future task events.
    ///
    /// This is `None` when the snapshot already reports a terminal state.
    pub live_receiver: Option<broadcast::Receiver<TaskEvent>>,
}

/// Task-scoped multi-subscriber event relay with persistence-backed recovery.
pub struct TaskEventBroadcaster {
    storage: Arc<dyn StorageProvider>,
    channel_capacity: usize,
    backpressure_policy: TaskEventBackpressurePolicy,
    state: RwLock<TaskEventBroadcasterState>,
}

#[derive(Default)]
struct TaskEventBroadcasterState {
    streams: HashMap<TaskId, broadcast::Sender<TaskEvent>>,
    terminal_tasks: HashSet<TaskId>,
    terminal_events: HashMap<TaskId, TaskEvent>,
}

impl TaskEventBroadcaster {
    /// Create a broadcaster that supports fan-out and late-subscriber catch-up.
    #[must_use]
    pub fn new(options: TaskEventBroadcasterOptions) -> Self {
        Self {
            storage: options.storage,
            channel_capacity: options.channel_capacity.max(1),
            backpressure_policy: options.backpressure_policy,
            state: RwLock::new(TaskEventBroadcasterState::default()),
        }
    }

    /// Subscribe to a task event stream with persisted replay recovery.
    ///
    /// `last_seen_sequence` defines the replay cursor: only events with higher
    /// sequence numbers are returned as `replay_events`.
    pub async fn subscribe(
        &self,
        task_id: TaskId,
        last_seen_sequence: Option<u64>,
    ) -> Result<TaskEventSubscription, StorageError> {
        let mut live_receiver = {
            let mut state = self.state.write().await;
            if state.terminal_tasks.contains(&task_id) {
                None
            } else {
                let sender = state.streams.entry(task_id).or_insert_with(|| {
                    let (sender, _) = broadcast::channel(self.channel_capacity);
                    sender
                });
                Some(sender.subscribe())
            }
        };

        let snapshot = self.storage.load_task_snapshot(task_id).await?;
        let mut replay_events = self
            .storage
            .load_task_events(task_id)
            .await?
            .into_iter()
            .filter(|event| {
                last_seen_sequence
                    .map(|sequence| event.sequence > sequence)
                    .unwrap_or(true)
            })
            .collect::<Vec<_>>();

        let terminal_event = self
            .state
            .read()
            .await
            .terminal_events
            .get(&task_id)
            .cloned();

        if let Some(event) = terminal_event {
            if last_seen_sequence
                .map(|sequence| event.sequence > sequence)
                .unwrap_or(true)
                && replay_events
                    .iter()
                    .all(|candidate| candidate.sequence != event.sequence)
            {
                replay_events.push(event);
            }
        }

        if let Some(receiver) = live_receiver.as_mut() {
            while let Ok(event) = receiver.try_recv() {
                if last_seen_sequence
                    .map(|sequence| event.sequence > sequence)
                    .unwrap_or(true)
                    && replay_events
                        .iter()
                        .all(|candidate| candidate.sequence != event.sequence)
                {
                    replay_events.push(event);
                }
            }
        }

        replay_events.sort_by_key(|event| event.sequence);

        let is_terminal = snapshot
            .as_ref()
            .is_some_and(|entry| entry.metadata.state.is_terminal())
            || replay_events.iter().any(|entry| entry.state.is_terminal());

        if is_terminal {
            let mut state = self.state.write().await;
            state.streams.remove(&task_id);
            state.terminal_tasks.insert(task_id);
            live_receiver = None;
        }

        Ok(TaskEventSubscription {
            snapshot,
            replay_events,
            live_receiver,
        })
    }

    /// Return the number of active in-memory task streams.
    pub async fn active_task_streams(&self) -> usize {
        self.state.read().await.streams.len()
    }

    /// Return the current number of live subscribers for a task.
    pub async fn subscriber_count(&self, task_id: &TaskId) -> usize {
        self.state
            .read()
            .await
            .streams
            .get(task_id)
            .map_or(0, broadcast::Sender::receiver_count)
    }
}

#[async_trait]
impl TaskEventPublisher for TaskEventBroadcaster {
    async fn publish(&self, event: TaskEvent) {
        let sender = {
            let mut state = self.state.write().await;
            if event.state.is_terminal() {
                state.terminal_tasks.insert(event.task_id);
                state.terminal_events.insert(event.task_id, event.clone());
                state.streams.remove(&event.task_id)
            } else {
                state.terminal_tasks.remove(&event.task_id);
                state.terminal_events.remove(&event.task_id);
                Some(
                    state
                        .streams
                        .entry(event.task_id)
                        .or_insert_with(|| {
                            let (sender, _) = broadcast::channel(self.channel_capacity);
                            sender
                        })
                        .clone(),
                )
            }
        };

        match self.backpressure_policy {
            TaskEventBackpressurePolicy::DropOldest => {
                if let Some(sender) = sender {
                    let _ = sender.send(event);
                }
            }
        }
    }
}

/// Shared task event publisher trait object.
pub type SharedTaskEventPublisher = Arc<dyn TaskEventPublisher>;

#[cfg(test)]
mod tests {
    use super::{
        TaskEventBackpressurePolicy, TaskEventBroadcaster, TaskEventBroadcasterOptions,
        TaskEventPublisher,
    };
    use async_trait::async_trait;
    use oxide_agent_core::agent::{
        AgentMemory, SessionId, TaskCheckpoint, TaskEvent, TaskEventKind, TaskMetadata,
        TaskSnapshot, TaskState,
    };
    use oxide_agent_core::storage::{
        Message, PendingInputPoll, StorageError, StorageProvider, UserConfig,
    };
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tokio::sync::broadcast::error::RecvError;
    use tokio::sync::{Mutex, Notify};
    use tokio::time::{timeout, Duration};

    #[derive(Default)]
    struct TaskEventStorage {
        snapshots: Mutex<HashMap<oxide_agent_core::agent::TaskId, TaskSnapshot>>,
        events: Mutex<HashMap<oxide_agent_core::agent::TaskId, Vec<TaskEvent>>>,
        block_event_load: AtomicBool,
        event_load_started: Notify,
        release_event_load: Notify,
    }

    impl TaskEventStorage {
        async fn save_snapshot_for_test(&self, snapshot: TaskSnapshot) {
            self.snapshots
                .lock()
                .await
                .insert(snapshot.metadata.id, snapshot);
        }

        async fn save_events_for_test(
            &self,
            task_id: oxide_agent_core::agent::TaskId,
            events: Vec<TaskEvent>,
        ) {
            self.events.lock().await.insert(task_id, events);
        }

        fn block_event_loads(&self) {
            self.block_event_load.store(true, Ordering::SeqCst);
        }

        async fn wait_for_event_load_start(&self) {
            self.event_load_started.notified().await;
        }

        fn release_event_loads(&self) {
            self.block_event_load.store(false, Ordering::SeqCst);
            self.release_event_load.notify_waiters();
        }
    }

    #[async_trait]
    impl StorageProvider for TaskEventStorage {
        async fn get_user_config(&self, _user_id: i64) -> Result<UserConfig, StorageError> {
            Ok(UserConfig::default())
        }

        async fn update_user_config(
            &self,
            _user_id: i64,
            _config: UserConfig,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn update_user_prompt(
            &self,
            _user_id: i64,
            _system_prompt: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_user_prompt(&self, _user_id: i64) -> Result<Option<String>, StorageError> {
            Ok(None)
        }

        async fn update_user_model(
            &self,
            _user_id: i64,
            _model_name: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_user_model(&self, _user_id: i64) -> Result<Option<String>, StorageError> {
            Ok(None)
        }

        async fn update_user_state(
            &self,
            _user_id: i64,
            _state: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_user_state(&self, _user_id: i64) -> Result<Option<String>, StorageError> {
            Ok(None)
        }

        async fn save_message(
            &self,
            _user_id: i64,
            _role: String,
            _content: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_chat_history(
            &self,
            _user_id: i64,
            _limit: usize,
        ) -> Result<Vec<Message>, StorageError> {
            Ok(Vec::new())
        }

        async fn clear_chat_history(&self, _user_id: i64) -> Result<(), StorageError> {
            Ok(())
        }

        async fn save_message_for_chat(
            &self,
            _user_id: i64,
            _chat_uuid: String,
            _role: String,
            _content: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_chat_history_for_chat(
            &self,
            _user_id: i64,
            _chat_uuid: String,
            _limit: usize,
        ) -> Result<Vec<Message>, StorageError> {
            Ok(Vec::new())
        }

        async fn clear_chat_history_for_chat(
            &self,
            _user_id: i64,
            _chat_uuid: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn save_agent_memory(
            &self,
            _user_id: i64,
            _memory: &AgentMemory,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn load_agent_memory(
            &self,
            _user_id: i64,
        ) -> Result<Option<AgentMemory>, StorageError> {
            Ok(None)
        }

        async fn clear_agent_memory(&self, _user_id: i64) -> Result<(), StorageError> {
            Ok(())
        }

        async fn clear_all_context(&self, _user_id: i64) -> Result<(), StorageError> {
            Ok(())
        }

        async fn save_task_snapshot(&self, snapshot: &TaskSnapshot) -> Result<(), StorageError> {
            self.snapshots
                .lock()
                .await
                .insert(snapshot.metadata.id, snapshot.clone());
            Ok(())
        }

        async fn load_task_snapshot(
            &self,
            task_id: oxide_agent_core::agent::TaskId,
        ) -> Result<Option<TaskSnapshot>, StorageError> {
            Ok(self.snapshots.lock().await.get(&task_id).cloned())
        }

        async fn list_task_snapshots(&self) -> Result<Vec<TaskSnapshot>, StorageError> {
            let snapshots = self.snapshots.lock().await;
            Ok(snapshots.values().cloned().collect())
        }

        async fn append_task_event(
            &self,
            task_id: oxide_agent_core::agent::TaskId,
            event: TaskEvent,
        ) -> Result<(), StorageError> {
            self.events
                .lock()
                .await
                .entry(task_id)
                .or_default()
                .push(event);
            Ok(())
        }

        async fn load_task_events(
            &self,
            task_id: oxide_agent_core::agent::TaskId,
        ) -> Result<Vec<TaskEvent>, StorageError> {
            self.event_load_started.notify_waiters();
            if self.block_event_load.load(Ordering::SeqCst) {
                self.release_event_load.notified().await;
            }
            Ok(self
                .events
                .lock()
                .await
                .get(&task_id)
                .cloned()
                .unwrap_or_default())
        }

        async fn save_pending_input_poll(
            &self,
            _poll: &PendingInputPoll,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn save_pending_input_poll_by_id(
            &self,
            _poll: &PendingInputPoll,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn load_pending_input_poll_by_task(
            &self,
            _task_id: oxide_agent_core::agent::TaskId,
        ) -> Result<Option<PendingInputPoll>, StorageError> {
            Ok(None)
        }

        async fn load_pending_input_poll_by_id(
            &self,
            _poll_id: &str,
        ) -> Result<Option<PendingInputPoll>, StorageError> {
            Ok(None)
        }

        async fn delete_pending_input_poll(
            &self,
            _task_id: oxide_agent_core::agent::TaskId,
            _poll_id: &str,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn check_connection(&self) -> Result<(), String> {
            Ok(())
        }
    }

    fn event(
        task_id: oxide_agent_core::agent::TaskId,
        sequence: u64,
        state: TaskState,
    ) -> TaskEvent {
        let kind = if sequence == 1 {
            TaskEventKind::Created
        } else {
            TaskEventKind::StateChanged
        };
        TaskEvent::new(task_id, sequence, kind, state, None)
    }

    fn snapshot(
        task_id: oxide_agent_core::agent::TaskId,
        state: TaskState,
        sequence: u64,
    ) -> TaskSnapshot {
        let mut metadata = TaskMetadata::new();
        metadata.id = task_id;
        metadata.state = state;
        let checkpoint = TaskCheckpoint::new(state, sequence);
        metadata.updated_at = checkpoint.persisted_at;

        TaskSnapshot {
            schema_version: oxide_agent_core::agent::task::TASK_SNAPSHOT_SCHEMA_VERSION,
            metadata,
            session_id: Some(SessionId::from(1)),
            task: "fan-out test".to_string(),
            checkpoint,
            recovery_note: None,
            pending_input: None,
            agent_memory: None,
            stop_report: None,
        }
    }

    #[tokio::test]
    async fn event_broadcaster_fans_out_to_multiple_subscribers() {
        let storage = Arc::new(TaskEventStorage::default());
        let task_id = TaskMetadata::new().id;
        storage
            .save_snapshot_for_test(snapshot(task_id, TaskState::Running, 1))
            .await;

        let broadcaster = TaskEventBroadcaster::new(TaskEventBroadcasterOptions::new(storage));
        let first = broadcaster.subscribe(task_id, Some(1)).await;
        let second = broadcaster.subscribe(task_id, Some(1)).await;

        assert!(matches!(first, Ok(ref subscription) if subscription.live_receiver.is_some()));
        assert!(matches!(second, Ok(ref subscription) if subscription.live_receiver.is_some()));
        assert_eq!(broadcaster.subscriber_count(&task_id).await, 2);

        let mut first_receiver = match first {
            Ok(subscription) => match subscription.live_receiver {
                Some(receiver) => receiver,
                None => panic!("live receiver missing for first subscriber"),
            },
            Err(error) => panic!("first subscription failed: {error}"),
        };
        let mut second_receiver = match second {
            Ok(subscription) => match subscription.live_receiver {
                Some(receiver) => receiver,
                None => panic!("live receiver missing for second subscriber"),
            },
            Err(error) => panic!("second subscription failed: {error}"),
        };

        broadcaster
            .publish(event(task_id, 2, TaskState::Running))
            .await;

        let first_result = timeout(Duration::from_millis(200), first_receiver.recv()).await;
        let second_result = timeout(Duration::from_millis(200), second_receiver.recv()).await;

        assert!(matches!(first_result, Ok(Ok(received)) if received.sequence == 2));
        assert!(matches!(second_result, Ok(Ok(received)) if received.sequence == 2));
    }

    #[tokio::test]
    async fn event_broadcaster_replays_persisted_events_for_late_subscribers() {
        let storage = Arc::new(TaskEventStorage::default());
        let task_id = TaskMetadata::new().id;
        storage
            .save_snapshot_for_test(snapshot(task_id, TaskState::Running, 3))
            .await;
        storage
            .save_events_for_test(
                task_id,
                vec![
                    event(task_id, 1, TaskState::Pending),
                    event(task_id, 2, TaskState::Running),
                    event(task_id, 3, TaskState::WaitingInput),
                ],
            )
            .await;

        let broadcaster = TaskEventBroadcaster::new(TaskEventBroadcasterOptions::new(storage));
        let subscription = broadcaster.subscribe(task_id, Some(1)).await;

        assert!(matches!(
            subscription,
            Ok(ref entry)
                if entry.snapshot.as_ref().map(|snapshot| snapshot.metadata.state) == Some(TaskState::Running)
                    && entry.replay_events.iter().map(|event| event.sequence).collect::<Vec<_>>()
                        == vec![2, 3]
                    && entry.live_receiver.is_some()
        ));
    }

    #[tokio::test]
    async fn event_broadcaster_applies_drop_oldest_backpressure_policy() {
        let storage = Arc::new(TaskEventStorage::default());
        let task_id = TaskMetadata::new().id;
        storage
            .save_snapshot_for_test(snapshot(task_id, TaskState::Running, 1))
            .await;

        let mut options = TaskEventBroadcasterOptions::new(storage);
        options.channel_capacity = 2;
        options.backpressure_policy = TaskEventBackpressurePolicy::DropOldest;
        let broadcaster = TaskEventBroadcaster::new(options);

        let subscription = broadcaster.subscribe(task_id, Some(1)).await;
        assert!(matches!(subscription, Ok(ref entry) if entry.live_receiver.is_some()));

        let mut receiver = match subscription {
            Ok(entry) => match entry.live_receiver {
                Some(receiver) => receiver,
                None => panic!("live receiver missing for backpressure subscription"),
            },
            Err(error) => panic!("backpressure subscription failed: {error}"),
        };

        for sequence in 2..=5 {
            broadcaster
                .publish(event(task_id, sequence, TaskState::Running))
                .await;
        }

        let lagged = timeout(Duration::from_millis(200), receiver.recv()).await;
        assert!(matches!(lagged, Ok(Err(RecvError::Lagged(skipped))) if skipped > 0));

        let newest = timeout(Duration::from_millis(200), receiver.recv()).await;
        assert!(matches!(newest, Ok(Ok(entry)) if entry.sequence >= 4));
    }

    #[tokio::test]
    async fn event_broadcaster_cleans_terminal_streams_and_disables_live_terminal_subscriptions() {
        let storage = Arc::new(TaskEventStorage::default());
        let task_id = TaskMetadata::new().id;
        storage
            .save_snapshot_for_test(snapshot(task_id, TaskState::Running, 1))
            .await;
        storage
            .save_events_for_test(
                task_id,
                vec![
                    event(task_id, 1, TaskState::Pending),
                    event(task_id, 2, TaskState::Running),
                ],
            )
            .await;

        let broadcaster = TaskEventBroadcaster::new(TaskEventBroadcasterOptions::new(Arc::clone(
            &storage,
        )
            as Arc<dyn StorageProvider>));
        let active = broadcaster.subscribe(task_id, Some(1)).await;
        assert!(matches!(active, Ok(ref entry) if entry.live_receiver.is_some()));

        broadcaster
            .publish(event(task_id, 2, TaskState::Running))
            .await;
        broadcaster
            .publish(event(task_id, 3, TaskState::Stopped))
            .await;

        assert_eq!(broadcaster.active_task_streams().await, 0);
        assert_eq!(broadcaster.subscriber_count(&task_id).await, 0);

        storage
            .save_snapshot_for_test(snapshot(task_id, TaskState::Stopped, 3))
            .await;
        storage
            .save_events_for_test(
                task_id,
                vec![
                    event(task_id, 1, TaskState::Pending),
                    event(task_id, 2, TaskState::Running),
                    event(task_id, 3, TaskState::Stopped),
                ],
            )
            .await;
        let late = broadcaster.subscribe(task_id, Some(2)).await;
        assert!(matches!(
            late,
            Ok(ref entry)
                if entry.replay_events.iter().map(|event| event.sequence).collect::<Vec<_>>()
                    == vec![3]
                    && entry.live_receiver.is_none()
        ));
    }

    #[tokio::test]
    async fn event_broadcaster_subscribe_replays_live_events_during_persistence_gap() {
        let storage = Arc::new(TaskEventStorage::default());
        let task_id = TaskMetadata::new().id;
        storage
            .save_snapshot_for_test(snapshot(task_id, TaskState::Running, 1))
            .await;
        storage
            .save_events_for_test(task_id, vec![event(task_id, 1, TaskState::Pending)])
            .await;
        storage.block_event_loads();

        let broadcaster = Arc::new(TaskEventBroadcaster::new(TaskEventBroadcasterOptions::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
        )));

        let subscribe_broadcaster = Arc::clone(&broadcaster);
        let subscribe_handle =
            tokio::spawn(async move { subscribe_broadcaster.subscribe(task_id, Some(1)).await });

        storage.wait_for_event_load_start().await;
        broadcaster
            .publish(event(task_id, 2, TaskState::Running))
            .await;
        storage.release_event_loads();

        let subscription = match subscribe_handle.await {
            Ok(result) => match result {
                Ok(subscription) => subscription,
                Err(error) => panic!("subscription failed: {error}"),
            },
            Err(error) => panic!("join failed: {error}"),
        };

        assert_eq!(
            subscription
                .replay_events
                .iter()
                .map(|entry| entry.sequence)
                .collect::<Vec<_>>(),
            vec![2]
        );
        assert!(subscription.live_receiver.is_some());
    }

    #[tokio::test]
    async fn event_broadcaster_subscribe_never_reopens_live_after_terminal_publish_before_persist()
    {
        let storage = Arc::new(TaskEventStorage::default());
        let task_id = TaskMetadata::new().id;
        storage
            .save_snapshot_for_test(snapshot(task_id, TaskState::Running, 2))
            .await;
        storage
            .save_events_for_test(
                task_id,
                vec![
                    event(task_id, 1, TaskState::Pending),
                    event(task_id, 2, TaskState::Running),
                ],
            )
            .await;
        storage.block_event_loads();

        let broadcaster = Arc::new(TaskEventBroadcaster::new(TaskEventBroadcasterOptions::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
        )));
        let subscribe_broadcaster = Arc::clone(&broadcaster);
        let subscribe_handle =
            tokio::spawn(async move { subscribe_broadcaster.subscribe(task_id, Some(2)).await });

        storage.wait_for_event_load_start().await;
        broadcaster
            .publish(event(task_id, 3, TaskState::Stopped))
            .await;
        storage.release_event_loads();

        let first = match subscribe_handle.await {
            Ok(result) => match result {
                Ok(subscription) => subscription,
                Err(error) => panic!("first subscription failed: {error}"),
            },
            Err(error) => panic!("join failed: {error}"),
        };

        assert_eq!(
            first
                .replay_events
                .iter()
                .map(|entry| entry.sequence)
                .collect::<Vec<_>>(),
            vec![3]
        );
        assert!(first.live_receiver.is_none());

        let second = broadcaster.subscribe(task_id, Some(2)).await;
        assert!(matches!(second, Ok(ref entry) if entry.live_receiver.is_none()));
        assert!(matches!(
            second,
            Ok(ref entry)
                if entry.replay_events.iter().map(|event| event.sequence).collect::<Vec<_>>()
                    == vec![3]
        ));
    }
}
