# 9. Agent Runtime

Each agent is an actor with an inbox. It receives commands, runs the turn loop, and updates its status.

```rust
pub struct AgentRuntime {
    id: AgentId,
    metadata: AgentMetadata,
    session: Arc<Session>,
    model: Arc<dyn Model>,
    tools: ToolCallRuntime,
    control: AgentControl,
    op_rx: mpsc::Receiver<Op>,
    status_tx: watch::Sender<AgentStatus>,
    cancel: CancellationToken,
}

impl AgentRuntime {
    pub async fn run(mut self) {
        let _ = self.status_tx.send(AgentStatus::Idle);

        while let Some(op) = self.op_rx.recv().await {
            match op {
                Op::UserInput { items } => {
                    let _ = self.status_tx.send(AgentStatus::Running);
                    self.session.push_user_items(&items).await;

                    let result = self.run_turn().await;
                    match result {
                        Ok(Some(final_text)) => {
                            let _ = self
                                .status_tx
                                .send(AgentStatus::Completed { summary: final_text });
                            break;
                        }
                        Ok(None) => {
                            let _ = self.status_tx.send(AgentStatus::WaitingForInput);
                        }
                        Err(err) => {
                            let _ = self.status_tx.send(AgentStatus::Failed {
                                error: err.to_string(),
                            });
                            break;
                        }
                    }
                }
                Op::Interrupt => {
                    self.cancel.cancel();
                    let _ = self.status_tx.send(AgentStatus::Cancelled);
                    break;
                }
                Op::Shutdown => {
                    self.cancel.cancel();
                    let _ = self.status_tx.send(AgentStatus::Shutdown);
                    break;
                }
            }
        }
    }

    async fn run_turn(&self) -> anyhow::Result<Option<String>> {
        loop {
            if self.cancel.is_cancelled() {
                anyhow::bail!("agent cancelled");
            }

            match self.model.next_action(self.session.clone()).await? {
                ModelAction::Final(text) => {
                    self.session.push_assistant(text.clone()).await;
                    return Ok(Some(text));
                }
                ModelAction::ToolCalls(calls) => {
                    let results = self.tools.execute_batch(calls).await;
                    for (call_id, result) in results {
                        match result {
                            Ok(value) => {
                                self.session
                                    .push_assistant(format!("tool_result[{call_id}]: {value}"))
                                    .await;
                            }
                            Err(err) => {
                                self.session
                                    .push_assistant(format!("tool_error[{call_id}]: {err}"))
                                    .await;
                            }
                        }
                    }
                }
                ModelAction::SpawnSubAgent {
                    task_name,
                    role,
                    prompt,
                } => {
                    let child_id = self
                        .control
                        .spawn_agent(SpawnRequest {
                            parent_id: Some(self.id),
                            task_name: Some(task_name),
                            nickname: None,
                            role,
                            initial_input: vec![UserInput::Text(prompt)],
                            inherit_history: false,
                        })
                        .await?;

                    self.session
                        .push_assistant(format!("spawned sub-agent: {child_id}"))
                        .await;
                }
                ModelAction::WaitOnAgents(agent_ids) => {
                    let statuses = self.control.wait_agents(&agent_ids).await?;
                    self.session
                        .push_assistant(format!("wait result: {:?}", statuses))
                        .await;
                    return Ok(None);
                }
            }
        }
    }
}
```
