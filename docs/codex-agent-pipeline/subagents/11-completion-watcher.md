# 11. Completion Watcher

The completion watcher runs as a detached background task. When the child agent reaches a final status, it injects a completion notification into the parent's mailbox.

```rust
impl AgentControl {
    fn start_completion_watcher(&self, child_id: AgentId, parent_id: Option<AgentId>) {
        let Some(parent_id) = parent_id else {
            return;
        };

        let control = self.clone();

        tokio::spawn(async move {
            let final_status = match control.wait_agent(child_id).await {
                Ok(status) => status,
                Err(_) => AgentStatus::NotFound,
            };

            let manager = match control.upgrade() {
                Ok(m) => m,
                Err(_) => return,
            };

            let Some(parent) = manager.get(parent_id).await else {
                return;
            };

            let message = format!(
                "sub-agent {} finished with status: {:?}",
                child_id, final_status
            );

            let _ = parent
                .op_tx
                .send(Op::UserInput {
                    items: vec![UserInput::Text(message)],
                })
                .await;
        });
    }
}
```

## Why the Completion Watcher Matters

Without a completion watcher, the parent must repeatedly call `wait_agent`, which adds unnecessary tool calls, latency, and orchestration friction.

With a watcher:

1. the parent spawns the child;
2. the child runs in the background;
3. the parent continues useful work;
4. when the child finishes, the watcher injects a completion message into the parent;
5. the parent decides when and how to integrate the result.

This is one of the biggest practical UX and performance wins of the Codex-style approach.
