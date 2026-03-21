# 6. Tool Abstraction

Each tool declares whether it supports parallel execution.

```rust
use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("tool failed: {0}")]
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub supports_parallel: bool,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;

    async fn call(&self, args: Value) -> Result<Value, ToolError>;
}

#[derive(Default)]
pub struct ToolRegistry {
    inner: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.inner.insert(tool.spec().name.clone(), tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.inner.get(name).cloned()
    }
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub call_id: String,
    pub tool_name: String,
    pub args: Value,
}
```
