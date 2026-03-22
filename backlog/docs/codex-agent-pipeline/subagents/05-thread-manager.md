# 5. Thread Manager (Global Live-Agent Registry)

This is the equivalent of a thread manager. It stores live agent handles, metadata, and parent-child relationships.

```rust
#[derive(Default)]
pub struct ThreadManager {
    agents: RwLock<HashMap<AgentId, AgentHandle>>,
    children: RwLock<HashMap<AgentId, Vec<AgentId>>>,
}

impl ThreadManager {
    pub async fn register(&self, handle: AgentHandle) {
        let id = handle.id;
        if let Some(parent_id) = handle.metadata.parent_id {
            let mut children = self.children.write().await;
            children.entry(parent_id).or_default().push(id);
        }

        self.agents.write().await.insert(id, handle);
    }

    pub async fn get(&self, id: AgentId) -> Option<AgentHandle> {
        self.agents.read().await.get(&id).cloned()
    }

    pub async fn remove(&self, id: AgentId) {
        self.agents.write().await.remove(&id);
    }

    pub async fn children_of(&self, parent_id: AgentId) -> Vec<AgentId> {
        self.children
            .read()
            .await
            .get(&parent_id)
            .cloned()
            .unwrap_or_default()
    }
}
```
