# 4. Base Types

These are the core shared types used across all modules.

```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Weak};
use tokio::sync::{mpsc, watch, Mutex, RwLock, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub type AgentId = Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentStatus {
    Starting,
    Idle,
    Running,
    WaitingForInput,
    Completed { summary: String },
    Failed { error: String },
    Cancelled,
    Shutdown,
    NotFound,
}

impl AgentStatus {
    pub fn is_final(&self) -> bool {
        matches!(
            self,
            AgentStatus::Completed { .. }
                | AgentStatus::Failed { .. }
                | AgentStatus::Cancelled
                | AgentStatus::Shutdown
                | AgentStatus::NotFound
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UserInput {
    Text(String),
}

#[derive(Debug, Clone)]
pub struct AgentMetadata {
    pub agent_id: AgentId,
    pub parent_id: Option<AgentId>,
    pub depth: u32,
    pub task_name: Option<String>,
    pub nickname: Option<String>,
    pub role: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Op {
    UserInput { items: Vec<UserInput> },
    Interrupt,
    Shutdown,
}

#[derive(Debug, Clone)]
pub struct SpawnRequest {
    pub parent_id: Option<AgentId>,
    pub task_name: Option<String>,
    pub nickname: Option<String>,
    pub role: Option<String>,
    pub initial_input: Vec<UserInput>,
    pub inherit_history: bool,
}

#[derive(Debug, Clone)]
pub struct AgentHandle {
    pub id: AgentId,
    pub metadata: AgentMetadata,
    pub op_tx: mpsc::Sender<Op>,
    pub status_rx: watch::Receiver<AgentStatus>,
}
```
