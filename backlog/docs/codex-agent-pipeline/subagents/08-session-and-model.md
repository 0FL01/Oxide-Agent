# 8. Session and Model Layer

This is a simplified abstraction. In a real framework, this layer owns history, prompt building, LLM invocation, and parsing model output.

```rust
#[derive(Debug, Clone)]
pub enum ModelAction {
    Final(String),
    ToolCalls(Vec<ToolCall>),
    SpawnSubAgent {
        task_name: String,
        role: Option<String>,
        prompt: String,
    },
    WaitOnAgents(Vec<AgentId>),
}

#[derive(Default)]
pub struct Session {
    pub history: Mutex<Vec<String>>,
}

impl Session {
    pub async fn push_user_items(&self, items: &[UserInput]) {
        let mut h = self.history.lock().await;
        for item in items {
            match item {
                UserInput::Text(text) => h.push(format!("user: {}", text)),
            }
        }
    }

    pub async fn push_assistant(&self, text: impl Into<String>) {
        self.history.lock().await.push(format!("assistant: {}", text.into()));
    }
}

#[async_trait]
pub trait Model: Send + Sync {
    async fn next_action(&self, session: Arc<Session>) -> anyhow::Result<ModelAction>;
}
```
