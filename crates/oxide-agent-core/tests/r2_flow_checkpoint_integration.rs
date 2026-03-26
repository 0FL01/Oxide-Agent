use anyhow::{anyhow, Result};
use async_trait::async_trait;
use dotenvy::dotenv;
use oxide_agent_core::agent::{AgentMemory, AgentMemoryCheckpoint, AgentSession, SessionId};
use oxide_agent_core::config::AgentSettings;
use oxide_agent_core::storage::{
    user_context_agent_flow_key, user_context_agent_flow_memory_key, R2Storage, StorageProvider,
};
use std::env;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::time::{sleep, timeout, Duration};
use uuid::Uuid;

#[derive(Clone)]
struct CountingFlowCheckpoint {
    storage: Arc<dyn StorageProvider>,
    user_id: i64,
    context_key: String,
    flow_id: String,
    persist_count: Arc<AtomicUsize>,
}

#[async_trait]
impl AgentMemoryCheckpoint for CountingFlowCheckpoint {
    async fn persist(&self, memory: &AgentMemory) -> Result<()> {
        self.storage
            .save_agent_memory_for_flow(
                self.user_id,
                self.context_key.clone(),
                self.flow_id.clone(),
                memory,
            )
            .await?;
        self.persist_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct FlowFixture {
    storage: Arc<R2Storage>,
    provider: Arc<dyn StorageProvider>,
    user_id: i64,
    context_key: String,
    flow_id: String,
}

impl FlowFixture {
    async fn new() -> Result<Self> {
        init_env();
        let storage = Arc::new(R2Storage::new(&load_r2_settings()?).await?);
        let provider: Arc<dyn StorageProvider> = storage.clone();
        let suffix = Uuid::new_v4().to_string();

        Ok(Self {
            storage,
            provider,
            user_id: 900_000
                + i64::from(u16::from_be_bytes([
                    suffix.as_bytes()[0],
                    suffix.as_bytes()[1],
                ])),
            context_key: format!("itest-flow-{suffix}"),
            flow_id: format!("flow-{suffix}"),
        })
    }

    fn checkpoint(&self) -> (Arc<AtomicUsize>, Arc<dyn AgentMemoryCheckpoint>) {
        let persist_count = Arc::new(AtomicUsize::new(0));
        let checkpoint: Arc<dyn AgentMemoryCheckpoint> = Arc::new(CountingFlowCheckpoint {
            storage: self.provider.clone(),
            user_id: self.user_id,
            context_key: self.context_key.clone(),
            flow_id: self.flow_id.clone(),
            persist_count: persist_count.clone(),
        });
        (persist_count, checkpoint)
    }

    async fn verify_write_access(&self) -> Result<()> {
        let probe_key = format!(
            "users/{}/integration-probe/{}/{}.txt",
            self.user_id, self.context_key, self.flow_id
        );
        self.storage
            .save_text(&probe_key, "phase-1-checkpoint-probe")
            .await
            .map_err(|error| anyhow!("R2 write probe failed for integration test: {error}"))?;
        self.storage.delete_object(&probe_key).await?;
        Ok(())
    }

    async fn cleanup(&self) {
        let _ = self
            .provider
            .clear_agent_memory_for_flow(
                self.user_id,
                self.context_key.clone(),
                self.flow_id.clone(),
            )
            .await;
        let _ = self
            .storage
            .delete_object(&user_context_agent_flow_key(
                self.user_id,
                &self.context_key,
                &self.flow_id,
            ))
            .await;
        let _ = self
            .storage
            .delete_object(&user_context_agent_flow_memory_key(
                self.user_id,
                &self.context_key,
                &self.flow_id,
            ))
            .await;
    }
}

fn init_env() {
    let _ = dotenv();
}

fn load_r2_settings() -> Result<AgentSettings> {
    Ok(AgentSettings {
        r2_access_key_id: Some(env::var("R2_ACCESS_KEY_ID")?),
        r2_secret_access_key: Some(env::var("R2_SECRET_ACCESS_KEY")?),
        r2_endpoint_url: Some(env::var("R2_ENDPOINT_URL")?),
        r2_bucket_name: Some(env::var("R2_BUCKET_NAME")?),
        r2_region: env::var("R2_REGION").unwrap_or_else(|_| "auto".to_string()),
        ..AgentSettings::default()
    })
}

async fn wait_for_persist_count(counter: &Arc<AtomicUsize>, expected: usize) -> Result<()> {
    timeout(Duration::from_secs(10), async {
        loop {
            if counter.load(Ordering::SeqCst) >= expected {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .map_err(|_| anyhow!("timed out waiting for persist count {expected}"))?;
    Ok(())
}

#[tokio::test]
#[ignore = "Requires real R2 credentials"]
async fn background_checkpoint_coalesces_real_r2_writes() -> Result<()> {
    let fixture = FlowFixture::new().await?;
    fixture.verify_write_access().await?;
    let (persist_count, checkpoint) = fixture.checkpoint();
    let mut session = AgentSession::new(SessionId::from(fixture.user_id));
    session.set_memory_checkpoint(checkpoint);

    session.memory.upsert_topic_agents_md("alpha snapshot");
    session.persist_memory_checkpoint_background();
    session.memory.upsert_topic_agents_md("beta snapshot");
    session.persist_memory_checkpoint_background();

    wait_for_persist_count(&persist_count, 1).await?;
    sleep(Duration::from_millis(200)).await;

    let persisted = fixture
        .provider
        .load_agent_memory_for_flow(
            fixture.user_id,
            fixture.context_key.clone(),
            fixture.flow_id.clone(),
        )
        .await?;
    let flow_record = fixture
        .provider
        .get_agent_flow_record(
            fixture.user_id,
            fixture.context_key.clone(),
            fixture.flow_id.clone(),
        )
        .await?;

    fixture.cleanup().await;

    let persisted = persisted.expect("flow memory should be persisted");
    let persisted_json = serde_json::to_string(&persisted)?;
    assert_eq!(persist_count.load(Ordering::SeqCst), 1);
    assert!(persisted_json.contains("beta snapshot"));
    assert!(!persisted_json.contains("alpha snapshot"));
    assert!(flow_record.is_none());
    Ok(())
}

#[tokio::test]
#[ignore = "Requires real R2 credentials"]
async fn forced_checkpoint_skips_identical_real_r2_writes() -> Result<()> {
    let fixture = FlowFixture::new().await?;
    fixture.verify_write_access().await?;
    let (persist_count, checkpoint) = fixture.checkpoint();
    let mut session = AgentSession::new(SessionId::from(fixture.user_id));
    session.set_memory_checkpoint(checkpoint);

    session.memory.upsert_topic_agents_md("stable snapshot");
    session.persist_memory_checkpoint().await?;
    session.persist_memory_checkpoint().await?;

    let persisted = fixture
        .provider
        .load_agent_memory_for_flow(
            fixture.user_id,
            fixture.context_key.clone(),
            fixture.flow_id.clone(),
        )
        .await?;

    fixture.cleanup().await;

    let persisted = persisted.expect("flow memory should be persisted");
    let persisted_json = serde_json::to_string(&persisted)?;
    assert_eq!(persist_count.load(Ordering::SeqCst), 1);
    assert!(persisted_json.contains("stable snapshot"));
    Ok(())
}
