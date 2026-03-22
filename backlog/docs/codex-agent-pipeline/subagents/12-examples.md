# 12. Examples: ModelFactory, Tools, Usage

## Example ModelFactory and Mock Model

```rust
pub struct DemoModelFactory;

#[async_trait]
impl ModelFactory for DemoModelFactory {
    async fn build_model(&self, role: Option<&str>) -> Arc<dyn Model> {
        Arc::new(DemoModel {
            role: role.unwrap_or("default").to_string(),
        })
    }
}

pub struct DemoModel {
    role: String,
}

#[async_trait]
impl Model for DemoModel {
    async fn next_action(&self, session: Arc<Session>) -> anyhow::Result<ModelAction> {
        let history = session.history.lock().await.clone();
        let last = history.last().cloned().unwrap_or_default();

        if last.contains("sub-agent") {
            return Ok(ModelAction::Final(format!(
                "[{}] integrated child result",
                self.role
            )));
        }

        if self.role == "researcher" {
            return Ok(ModelAction::Final("research finished".into()));
        }

        if last.contains("start") {
            return Ok(ModelAction::SpawnSubAgent {
                task_name: "research-track".into(),
                role: Some("researcher".into()),
                prompt: "collect supporting facts".into(),
            });
        }

        Ok(ModelAction::Final(format!("[{}] done", self.role)))
    }
}
```

---

## Example Tools

```rust
pub struct SleepTool;

#[async_trait]
impl Tool for SleepTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "sleep".into(),
            supports_parallel: true,
        }
    }

    async fn call(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let ms = args["ms"].as_u64().unwrap_or(100);
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
        Ok(serde_json::json!({ "slept_ms": ms }))
    }
}

pub struct WriteDbTool;

#[async_trait]
impl Tool for WriteDbTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write_db".into(),
            supports_parallel: false,
        }
    }

    async fn call(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let key = args["key"].as_str().unwrap_or("unknown");
        Ok(serde_json::json!({ "written": key }))
    }
}
```

---

## Example Usage

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let control = build_framework().await;

    let root_id = control
        .spawn_agent(SpawnRequest {
            parent_id: None,
            task_name: Some("root".into()),
            nickname: Some("main".into()),
            role: Some("orchestrator".into()),
            initial_input: vec![UserInput::Text("start".into())],
            inherit_history: false,
        })
        .await?;

    let status = control.wait_agent(root_id).await?;
    println!("root status: {:?}", status);

    Ok(())
}
```
