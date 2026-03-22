# 13. Framework Construction

Putting all components together to build the framework.

```rust
pub async fn build_framework() -> AgentControl {
    let manager = Arc::new(ThreadManager::default());

    let mut registry = ToolRegistry::default();
    registry.register(Arc::new(SleepTool));
    registry.register(Arc::new(WriteDbTool));

    let tool_runtime = ToolCallRuntime::new(Arc::new(registry), 16);
    let model_factory = Arc::new(DemoModelFactory);

    AgentControl::new(
        Arc::downgrade(&manager),
        model_factory,
        tool_runtime,
        3,   // max_depth
        64,  // max_agents
    )
}
```

The numbers `3` and `64` are example defaults:

- `max_depth = 3` limits nesting to 3 levels;
- `max_agents = 64` limits concurrent live agents.
