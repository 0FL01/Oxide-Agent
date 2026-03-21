# 7. Parallel Tool Runtime

Parallel-safe tools acquire a shared read lock. Exclusive tools acquire a write lock. A semaphore limits total concurrent tool jobs.

```rust
use futures::{stream::FuturesUnordered, StreamExt};

#[derive(Clone)]
pub struct ToolCallRuntime {
    registry: Arc<ToolRegistry>,
    parallel_gate: Arc<RwLock<()>>,
    global_parallel_limit: Arc<Semaphore>,
}

impl ToolCallRuntime {
    pub fn new(registry: Arc<ToolRegistry>, max_parallel_tools: usize) -> Self {
        Self {
            registry,
            parallel_gate: Arc::new(RwLock::new(())),
            global_parallel_limit: Arc::new(Semaphore::new(max_parallel_tools)),
        }
    }

    pub async fn execute_one(&self, call: ToolCall) -> Result<Value, ToolError> {
        let tool = self
            .registry
            .get(&call.tool_name)
            .ok_or_else(|| ToolError::Failed(format!("unknown tool: {}", call.tool_name)))?;

        let spec = tool.spec();
        let _permit = self
            .global_parallel_limit
            .acquire()
            .await
            .map_err(|_| ToolError::Failed("parallel semaphore closed".into()))?;

        if spec.supports_parallel {
            let _guard = self.parallel_gate.read().await;
            tool.call(call.args).await
        } else {
            let _guard = self.parallel_gate.write().await;
            tool.call(call.args).await
        }
    }

    pub async fn execute_batch(
        &self,
        calls: Vec<ToolCall>,
    ) -> Vec<(String, Result<Value, ToolError>)> {
        let mut futures = FuturesUnordered::new();

        for call in calls {
            let runtime = self.clone();
            futures.push(async move {
                let id = call.call_id.clone();
                let result = runtime.execute_one(call).await;
                (id, result)
            });
        }

        let mut results = Vec::new();
        while let Some(result) = futures.next().await {
            results.push(result);
        }
        results
    }
}
```
