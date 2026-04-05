use super::{
    keys::{
        persistent_memory_episode_key, persistent_memory_session_state_key,
        persistent_memory_thread_key,
    },
    r2::R2Storage,
    StorageError,
};
use oxide_agent_memory::{EpisodeRecord, SessionStateRecord, ThreadRecord};

impl R2Storage {
    pub(super) async fn upsert_memory_thread_inner(
        &self,
        record: ThreadRecord,
    ) -> Result<ThreadRecord, StorageError> {
        let key = persistent_memory_thread_key(&record.thread_id);
        let stored = if let Some(existing) = self.load_json::<ThreadRecord>(&key).await? {
            ThreadRecord {
                created_at: existing.created_at,
                ..record
            }
        } else {
            record
        };
        self.save_json(&key, &stored).await?;
        Ok(stored)
    }

    pub(super) async fn create_memory_episode_inner(
        &self,
        record: EpisodeRecord,
    ) -> Result<EpisodeRecord, StorageError> {
        let key = persistent_memory_episode_key(&record.thread_id, &record.episode_id);
        if self.load_json::<EpisodeRecord>(&key).await?.is_some() {
            return Err(StorageError::InvalidInput(format!(
                "persistent episode {} already exists",
                record.episode_id
            )));
        }
        self.save_json(&key, &record).await?;
        Ok(record)
    }

    pub(super) async fn upsert_memory_session_state_inner(
        &self,
        record: SessionStateRecord,
    ) -> Result<SessionStateRecord, StorageError> {
        let key = persistent_memory_session_state_key(&record.session_id);
        self.save_json(&key, &record).await?;
        Ok(record)
    }
}
