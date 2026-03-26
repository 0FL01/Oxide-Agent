use super::{
    builders::build_agent_flow_record,
    keys::{
        user_agent_memory_key, user_context_agent_flow_key, user_context_agent_flow_memory_key,
        user_context_agent_flow_prefix, user_context_agent_flows_prefix,
        user_context_agent_memory_key,
    },
    r2::{PersistedAgentMemoryRef, R2Storage},
    telemetry::with_storage_reason,
    utils::current_timestamp_unix_secs,
    AgentFlowRecord, StorageError,
};
use crate::agent::memory::AgentMemory;
use std::collections::BTreeSet;

impl R2Storage {
    /// List all persisted topic-scoped agent memory records present in R2.
    ///
    /// This scans topic-level memory snapshots and detached flow memories for all users.
    pub async fn list_persisted_agent_memories(
        &self,
    ) -> Result<Vec<PersistedAgentMemoryRef>, StorageError> {
        with_storage_reason("list_persisted_agent_memories", async {
            let keys = self.list_keys_under_prefix("users/").await?;
            let mut memories = BTreeSet::new();

            for key in keys {
                if let Some(reference) = parse_persisted_agent_memory_key(&key) {
                    memories.insert(reference);
                }
            }

            Ok(memories.into_iter().collect())
        })
        .await
    }

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

fn parse_persisted_agent_memory_key(key: &str) -> Option<PersistedAgentMemoryRef> {
    let parts = key.split('/').collect::<Vec<_>>();

    match parts.as_slice() {
        ["users", user_id, "topics", context_key, "agent_memory.json"] => {
            Some(PersistedAgentMemoryRef {
                user_id: user_id.parse().ok()?,
                context_key: (*context_key).to_string(),
                flow_id: None,
            })
        }
        ["users", user_id, "topics", context_key, "flows", flow_id, "memory.json"] => {
            Some(PersistedAgentMemoryRef {
                user_id: user_id.parse().ok()?,
                context_key: (*context_key).to_string(),
                flow_id: Some((*flow_id).to_string()),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::parse_persisted_agent_memory_key;
    use crate::storage::PersistedAgentMemoryRef;

    #[test]
    fn parses_context_scoped_agent_memory_key() {
        let parsed = parse_persisted_agent_memory_key("users/42/topics/-1001:77/agent_memory.json");

        assert_eq!(
            parsed,
            Some(PersistedAgentMemoryRef {
                user_id: 42,
                context_key: "-1001:77".to_string(),
                flow_id: None,
            })
        );
    }

    #[test]
    fn parses_flow_scoped_agent_memory_key() {
        let parsed =
            parse_persisted_agent_memory_key("users/42/topics/-1001:77/flows/flow-123/memory.json");

        assert_eq!(
            parsed,
            Some(PersistedAgentMemoryRef {
                user_id: 42,
                context_key: "-1001:77".to_string(),
                flow_id: Some("flow-123".to_string()),
            })
        );
    }

    #[test]
    fn ignores_non_memory_keys() {
        assert_eq!(
            parse_persisted_agent_memory_key("users/42/topics/-1001:77/flows/flow-123/meta.json"),
            None
        );
        assert_eq!(
            parse_persisted_agent_memory_key("users/42/config.json"),
            None
        );
    }
}
