//! Boot-time task reconciliation for persisted snapshots.

use crate::task_registry::{TaskRegistry, TaskRegistryError};
use oxide_agent_core::agent::task::TASK_SNAPSHOT_SCHEMA_VERSION;
use oxide_agent_core::agent::{SessionId, TaskCheckpoint, TaskSnapshot, TaskState};
use oxide_agent_core::storage::{StorageError, StorageProvider};
use std::fmt;
use std::sync::Arc;

const RUNNING_TASK_RECOVERY_NOTE: &str =
    "task was marked failed during restart recovery because the previous runtime crashed while it was running";
const MISSING_SESSION_RECOVERY_NOTE: &str =
    "task was marked failed during restart recovery because no persisted session ownership was available";

/// Options required to construct task recovery.
pub struct TaskRecoveryOptions {
    /// Runtime task registry that regains ownership during reconciliation.
    pub task_registry: Arc<TaskRegistry>,
    /// Persistent storage used to enumerate and rewrite task snapshots.
    pub storage: Arc<dyn StorageProvider>,
}

/// Boot-time task recovery service.
pub struct TaskRecovery {
    task_registry: Arc<TaskRegistry>,
    storage: Arc<dyn StorageProvider>,
}

/// Outcome counters for a reconciliation run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskRecoveryReport {
    /// Number of snapshots enumerated from storage.
    pub total_snapshots: usize,
    /// Number of task records restored into runtime ownership.
    pub restored_records: usize,
    /// Number of snapshots rewritten into an explicit failed state.
    pub failed_recoveries: usize,
}

/// Errors returned by boot-time task reconciliation.
#[derive(Debug)]
pub enum TaskRecoveryError {
    /// Storage enumeration or snapshot rewrite failed.
    Storage(StorageError),
    /// Runtime registry restoration failed.
    TaskRegistry(TaskRegistryError),
}

impl fmt::Display for TaskRecoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(error) => write!(f, "{error}"),
            Self::TaskRegistry(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for TaskRecoveryError {}

impl From<StorageError> for TaskRecoveryError {
    fn from(value: StorageError) -> Self {
        Self::Storage(value)
    }
}

impl From<TaskRegistryError> for TaskRecoveryError {
    fn from(value: TaskRegistryError) -> Self {
        Self::TaskRegistry(value)
    }
}

impl TaskRecovery {
    /// Create a new task recovery service.
    #[must_use]
    pub fn new(options: TaskRecoveryOptions) -> Self {
        Self {
            task_registry: options.task_registry,
            storage: options.storage,
        }
    }

    /// Reconcile all persisted task snapshots into runtime ownership.
    pub async fn reconcile(&self) -> Result<TaskRecoveryReport, TaskRecoveryError> {
        let mut snapshots = self.storage.list_task_snapshots().await?;
        snapshots.sort_by(|left, right| {
            left.metadata
                .created_at
                .cmp(&right.metadata.created_at)
                .then_with(|| left.metadata.id.as_uuid().cmp(&right.metadata.id.as_uuid()))
        });

        let mut report = TaskRecoveryReport {
            total_snapshots: snapshots.len(),
            ..TaskRecoveryReport::default()
        };

        for snapshot in snapshots {
            match snapshot.session_id {
                Some(session_id) => {
                    let snapshot =
                        reconcile_owned_snapshot(snapshot, session_id, &self.storage, &mut report)
                            .await?;
                    self.task_registry
                        .restore(
                            snapshot.metadata,
                            session_id,
                            snapshot.checkpoint.last_event_sequence,
                        )
                        .await;
                    report.restored_records += 1;
                }
                None if snapshot.metadata.state.is_non_terminal() => {
                    let failed_snapshot = fail_snapshot(snapshot, MISSING_SESSION_RECOVERY_NOTE);
                    self.storage.save_task_snapshot(&failed_snapshot).await?;
                    report.failed_recoveries += 1;
                }
                None => {}
            }
        }

        Ok(report)
    }
}

async fn reconcile_owned_snapshot(
    snapshot: TaskSnapshot,
    session_id: SessionId,
    storage: &Arc<dyn StorageProvider>,
    report: &mut TaskRecoveryReport,
) -> Result<TaskSnapshot, StorageError> {
    if snapshot.metadata.state == TaskState::Running {
        let failed_snapshot = fail_snapshot(snapshot, RUNNING_TASK_RECOVERY_NOTE);
        storage.save_task_snapshot(&failed_snapshot).await?;
        report.failed_recoveries += 1;
        return Ok(failed_snapshot);
    }

    let mut restored_snapshot = snapshot;
    restored_snapshot.session_id = Some(session_id);
    Ok(restored_snapshot)
}

fn fail_snapshot(mut snapshot: TaskSnapshot, note: &str) -> TaskSnapshot {
    let checkpoint = TaskCheckpoint::new(
        TaskState::Failed,
        snapshot.checkpoint.last_event_sequence.saturating_add(1),
    );
    snapshot.schema_version = TASK_SNAPSHOT_SCHEMA_VERSION;
    snapshot.metadata.state = TaskState::Failed;
    snapshot.metadata.updated_at = checkpoint.persisted_at;
    snapshot.checkpoint = checkpoint;
    snapshot.recovery_note = Some(note.to_string());
    snapshot
}

#[cfg(test)]
mod tests {
    use super::{
        TaskRecovery, TaskRecoveryOptions, MISSING_SESSION_RECOVERY_NOTE,
        RUNNING_TASK_RECOVERY_NOTE,
    };
    use crate::TaskRegistry;
    use async_trait::async_trait;
    use oxide_agent_core::agent::task::TASK_SNAPSHOT_SCHEMA_VERSION;
    use oxide_agent_core::agent::{AgentMemory, SessionId, TaskMetadata, TaskSnapshot, TaskState};
    use oxide_agent_core::storage::{Message, StorageError, StorageProvider, UserConfig};
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[derive(Default)]
    struct RecoveryStorage {
        snapshots: Mutex<HashMap<oxide_agent_core::agent::TaskId, TaskSnapshot>>,
    }

    #[async_trait]
    impl StorageProvider for RecoveryStorage {
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
            _prompt: String,
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
            let mut values = snapshots.values().cloned().collect::<Vec<_>>();
            values.sort_by(|left, right| {
                left.metadata
                    .created_at
                    .cmp(&right.metadata.created_at)
                    .then_with(|| left.metadata.id.as_uuid().cmp(&right.metadata.id.as_uuid()))
            });
            Ok(values)
        }

        async fn check_connection(&self) -> Result<(), String> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn task_recovery_restores_pending_and_terminal_records() {
        let storage = Arc::new(RecoveryStorage::default());
        let registry = Arc::new(TaskRegistry::new());

        let pending = TaskSnapshot::new(
            TaskMetadata::new(),
            SessionId::from(10),
            "pending".to_string(),
            1,
        );
        let mut completed_metadata = TaskMetadata::new();
        completed_metadata.state = TaskState::Completed;
        let completed = TaskSnapshot::new(
            completed_metadata,
            SessionId::from(11),
            "completed".to_string(),
            3,
        );
        assert!(storage.save_task_snapshot(&pending).await.is_ok());
        assert!(storage.save_task_snapshot(&completed).await.is_ok());

        let recovery = TaskRecovery::new(TaskRecoveryOptions {
            task_registry: Arc::clone(&registry),
            storage: Arc::clone(&storage) as Arc<dyn StorageProvider>,
        });

        let report = recovery.reconcile().await;
        assert!(report.is_ok());
        let report = report.unwrap_or_default();
        assert_eq!(report.total_snapshots, 2);
        assert_eq!(report.restored_records, 2);
        assert_eq!(report.failed_recoveries, 0);

        let records = registry.list().await;
        assert_eq!(records.len(), 2);
        assert!(records
            .iter()
            .any(|record| record.metadata.state == TaskState::Pending));
        assert!(records
            .iter()
            .any(|record| record.metadata.state == TaskState::Completed));
    }

    #[tokio::test]
    async fn task_recovery_marks_running_snapshot_failed_before_restore() {
        let storage = Arc::new(RecoveryStorage::default());
        let registry = Arc::new(TaskRegistry::new());

        let mut running_metadata = TaskMetadata::new();
        running_metadata.state = TaskState::Running;
        let running = TaskSnapshot::new(
            running_metadata,
            SessionId::from(21),
            "running".to_string(),
            2,
        );
        let task_id = running.metadata.id;
        assert!(storage.save_task_snapshot(&running).await.is_ok());

        let recovery = TaskRecovery::new(TaskRecoveryOptions {
            task_registry: Arc::clone(&registry),
            storage: Arc::clone(&storage) as Arc<dyn StorageProvider>,
        });

        let report = recovery.reconcile().await;
        assert!(report.is_ok());
        let report = report.unwrap_or_default();
        assert_eq!(report.restored_records, 1);
        assert_eq!(report.failed_recoveries, 1);

        let record = registry.get(&task_id).await;
        assert!(matches!(record, Some(record) if record.metadata.state == TaskState::Failed));

        let snapshot = storage.load_task_snapshot(task_id).await;
        assert!(
            matches!(snapshot, Ok(Some(snapshot)) if snapshot.recovery_note.as_deref() == Some(RUNNING_TASK_RECOVERY_NOTE))
        );
    }

    #[tokio::test]
    async fn task_recovery_fails_non_terminal_snapshot_without_session_ownership() {
        let storage = Arc::new(RecoveryStorage::default());
        let registry = Arc::new(TaskRegistry::new());

        let mut pending = TaskSnapshot::new(
            TaskMetadata::new(),
            SessionId::from(30),
            "pending".to_string(),
            1,
        );
        pending.session_id = None;
        let task_id = pending.metadata.id;
        assert!(storage.save_task_snapshot(&pending).await.is_ok());

        let recovery = TaskRecovery::new(TaskRecoveryOptions {
            task_registry: Arc::clone(&registry),
            storage: Arc::clone(&storage) as Arc<dyn StorageProvider>,
        });

        let report = recovery.reconcile().await;
        assert!(report.is_ok());
        let report = report.unwrap_or_default();
        assert_eq!(report.restored_records, 0);
        assert_eq!(report.failed_recoveries, 1);
        assert!(registry.list().await.is_empty());

        let snapshot = storage.load_task_snapshot(task_id).await;
        assert!(snapshot.is_ok());
        let snapshot = snapshot.ok().flatten();
        assert!(
            matches!(snapshot, Some(ref snapshot) if snapshot.metadata.state == TaskState::Failed)
        );
        assert!(
            matches!(snapshot, Some(ref snapshot) if snapshot.recovery_note.as_deref() == Some(MISSING_SESSION_RECOVERY_NOTE))
        );
    }

    #[tokio::test]
    async fn task_recovery_rewrites_legacy_failed_snapshot_with_current_schema_version() {
        let storage = Arc::new(RecoveryStorage::default());
        let registry = Arc::new(TaskRegistry::new());

        let mut pending = TaskSnapshot::new(
            TaskMetadata::new(),
            SessionId::from(31),
            "legacy".to_string(),
            4,
        );
        pending.schema_version = 1;
        pending.checkpoint.schema_version = 1;
        pending.session_id = None;
        let task_id = pending.metadata.id;
        assert!(storage.save_task_snapshot(&pending).await.is_ok());

        let recovery = TaskRecovery::new(TaskRecoveryOptions {
            task_registry: Arc::clone(&registry),
            storage: Arc::clone(&storage) as Arc<dyn StorageProvider>,
        });

        let report = recovery.reconcile().await;
        assert!(report.is_ok());
        let report = report.unwrap_or_default();
        assert_eq!(report.failed_recoveries, 1);

        let snapshot = storage.load_task_snapshot(task_id).await;
        assert!(snapshot.is_ok());
        let snapshot = snapshot.ok().flatten();
        assert!(matches!(
            snapshot,
            Some(ref snapshot)
                if snapshot.schema_version == TASK_SNAPSHOT_SCHEMA_VERSION
                    && snapshot.checkpoint.schema_version == TASK_SNAPSHOT_SCHEMA_VERSION
                    && snapshot.metadata.state == TaskState::Failed
        ));
    }
}
