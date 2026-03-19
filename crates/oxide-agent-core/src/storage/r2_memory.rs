use super::{
    build_agent_flow_record, current_timestamp_unix_secs, user_agent_memory_key,
    user_context_agent_flow_key, user_context_agent_flow_memory_key,
    user_context_agent_flow_prefix, user_context_agent_flows_prefix, user_context_agent_memory_key,
    AgentFlowRecord, R2Storage, StorageError,
};
use crate::agent::memory::AgentMemory;

impl R2Storage {
    pub(super) async fn save_agent_memory_inner(
        &self,
        user_id: i64,
        memory: &AgentMemory,
    ) -> Result<(), StorageError> {
        self.save_json(&user_agent_memory_key(user_id), memory)
            .await
    }

    pub(super) async fn save_agent_memory_for_context_inner(
        &self,
        user_id: i64,
        context_key: String,
        memory: &AgentMemory,
    ) -> Result<(), StorageError> {
        self.save_json(
            &user_context_agent_memory_key(user_id, &context_key),
            memory,
        )
        .await
    }

    pub(super) async fn load_agent_memory_inner(
        &self,
        user_id: i64,
    ) -> Result<Option<AgentMemory>, StorageError> {
        self.load_json(&user_agent_memory_key(user_id)).await
    }

    pub(super) async fn load_agent_memory_for_context_inner(
        &self,
        user_id: i64,
        context_key: String,
    ) -> Result<Option<AgentMemory>, StorageError> {
        self.load_json(&user_context_agent_memory_key(user_id, &context_key))
            .await
    }

    pub(super) async fn clear_agent_memory_inner(&self, user_id: i64) -> Result<(), StorageError> {
        self.delete_object(&user_agent_memory_key(user_id)).await
    }

    pub(super) async fn clear_agent_memory_for_context_inner(
        &self,
        user_id: i64,
        context_key: String,
    ) -> Result<(), StorageError> {
        self.delete_prefix(&user_context_agent_flows_prefix(user_id, &context_key))
            .await?;
        self.delete_object(&user_context_agent_memory_key(user_id, &context_key))
            .await
    }

    pub(super) async fn save_agent_memory_for_flow_inner(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
        memory: &AgentMemory,
    ) -> Result<(), StorageError> {
        self.save_json(
            &user_context_agent_flow_memory_key(user_id, &context_key, &flow_id),
            memory,
        )
        .await
    }

    pub(super) async fn load_agent_memory_for_flow_inner(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<Option<AgentMemory>, StorageError> {
        self.load_json(&user_context_agent_flow_memory_key(
            user_id,
            &context_key,
            &flow_id,
        ))
        .await
    }

    pub(super) async fn clear_agent_memory_for_flow_inner(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<(), StorageError> {
        self.delete_prefix(&user_context_agent_flow_prefix(
            user_id,
            &context_key,
            &flow_id,
        ))
        .await
    }

    pub(super) async fn get_agent_flow_record_inner(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<Option<AgentFlowRecord>, StorageError> {
        self.load_json(&user_context_agent_flow_key(
            user_id,
            &context_key,
            &flow_id,
        ))
        .await
    }

    pub(super) async fn upsert_agent_flow_record_inner(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<AgentFlowRecord, StorageError> {
        let key = user_context_agent_flow_key(user_id, &context_key, &flow_id);
        let now = current_timestamp_unix_secs();
        let existing = self.load_json::<AgentFlowRecord>(&key).await?;
        let record = build_agent_flow_record(user_id, context_key, flow_id, existing, now);
        self.save_json(&key, &record).await?;
        Ok(record)
    }
}
