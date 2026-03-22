# 10. AgentControl: Control Plane

This is the main orchestration component. It creates agents, registers them, sends input, waits for completion, and stops them.

```rust
#[derive(Clone)]
pub struct AgentControl {
    manager: Weak<ThreadManager>,
    model_factory: Arc<dyn ModelFactory>,
    tools: ToolCallRuntime,
    max_depth: u32,
    max_agents: usize,
    live_agents: Arc<Semaphore>,
}

#[async_trait]
pub trait ModelFactory: Send + Sync {
    async fn build_model(&self, role: Option<&str>) -> Arc<dyn Model>;
}

impl AgentControl {
    pub fn new(
        manager: Weak<ThreadManager>,
        model_factory: Arc<dyn ModelFactory>,
        tools: ToolCallRuntime,
        max_depth: u32,
        max_agents: usize,
    ) -> Self {
        Self {
            manager,
            model_factory,
            tools,
            max_depth,
            max_agents,
            live_agents: Arc::new(Semaphore::new(max_agents)),
        }
    }

    fn upgrade(&self) -> anyhow::Result<Arc<ThreadManager>> {
        self.manager
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("thread manager dropped"))
    }

    pub async fn spawn_agent(&self, req: SpawnRequest) -> anyhow::Result<AgentId> {
        let depth = if let Some(parent_id) = req.parent_id {
            let manager = self.upgrade()?;
            let parent = manager
                .get(parent_id)
                .await
                .ok_or_else(|| anyhow::anyhow!("parent agent not found"))?;
            parent.metadata.depth + 1
        } else {
            0
        };

        if depth > self.max_depth {
            anyhow::bail!("agent depth limit reached");
        }

        let _slot = self
            .live_agents
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| anyhow::anyhow!("agent semaphore closed"))?;

        let id = AgentId::new_v4();
        let (op_tx, op_rx) = mpsc::channel(64);
        let (status_tx, status_rx) = watch::channel(AgentStatus::Starting);

        let metadata = AgentMetadata {
            agent_id: id,
            parent_id: req.parent_id,
            depth,
            task_name: req.task_name.clone(),
            nickname: req.nickname.clone(),
            role: req.role.clone(),
        };

        let handle = AgentHandle {
            id,
            metadata: metadata.clone(),
            op_tx: op_tx.clone(),
            status_rx: status_rx.clone(),
        };

        let manager = self.upgrade()?;
        manager.register(handle).await;

        let model = self.model_factory.build_model(req.role.as_deref()).await;
        let session = Arc::new(Session::default());

        let runtime = AgentRuntime {
            id,
            metadata: metadata.clone(),
            session: session.clone(),
            model,
            tools: self.tools.clone(),
            control: self.clone(),
            op_rx,
            status_tx,
            cancel: CancellationToken::new(),
        };

        tokio::spawn(async move {
            let _permit = _slot;
            runtime.run().await;
        });

        self.start_completion_watcher(id, metadata.parent_id);

        if !req.initial_input.is_empty() {
            self.send_input(id, req.initial_input).await?;
        }

        Ok(id)
    }

    pub async fn send_input(
        &self,
        agent_id: AgentId,
        items: Vec<UserInput>,
    ) -> anyhow::Result<()> {
        let manager = self.upgrade()?;
        let agent = manager
            .get(agent_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("agent not found"))?;

        agent.op_tx
            .send(Op::UserInput { items })
            .await
            .map_err(|_| anyhow::anyhow!("agent mailbox closed"))
    }

    pub async fn interrupt_agent(&self, agent_id: AgentId) -> anyhow::Result<()> {
        let manager = self.upgrade()?;
        let agent = manager
            .get(agent_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("agent not found"))?;

        agent.op_tx
            .send(Op::Interrupt)
            .await
            .map_err(|_| anyhow::anyhow!("agent mailbox closed"))
    }

    pub async fn close_agent(&self, agent_id: AgentId) -> anyhow::Result<()> {
        let manager = self.upgrade()?;
        let agent = manager
            .get(agent_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("agent not found"))?;

        agent.op_tx
            .send(Op::Shutdown)
            .await
            .map_err(|_| anyhow::anyhow!("agent mailbox closed"))
    }

    pub async fn wait_agent(&self, agent_id: AgentId) -> anyhow::Result<AgentStatus> {
        let manager = self.upgrade()?;
        let agent = manager
            .get(agent_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("agent not found"))?;

        let mut rx = agent.status_rx.clone();
        let mut status = rx.borrow().clone();

        if status.is_final() {
            return Ok(status);
        }

        loop {
            if rx.changed().await.is_err() {
                return Ok(AgentStatus::NotFound);
            }

            status = rx.borrow().clone();
            if status.is_final() {
                return Ok(status);
            }
        }
    }

    pub async fn wait_agents(
        &self,
        ids: &[AgentId],
    ) -> anyhow::Result<HashMap<AgentId, AgentStatus>> {
        let mut futures = FuturesUnordered::new();

        for id in ids.iter().copied() {
            let control = self.clone();
            futures.push(async move { (id, control.wait_agent(id).await) });
        }

        let mut result = HashMap::new();
        while let Some((id, status)) = futures.next().await {
            result.insert(id, status.unwrap_or(AgentStatus::NotFound));
        }

        Ok(result)
    }
}
```
