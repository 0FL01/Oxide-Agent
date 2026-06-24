//! Derived Engram recall/index contracts.
//!
//! This module intentionally does not implement a live HTTP adapter. The live
//! upstream contract and legal path must be verified before any networked
//! backend is enabled. The contracts here keep Engram derived and replaceable:
//! projections originate from canonical Postgres outbox rows, and recall
//! candidates are dereferenced back into canonical Postgres memory.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::domain::{
    ActiveMemoryGeneration, LifeEngramOutboxRow, LifeMemoryItem, MemoryGenerationId, MemoryItemId,
    MemoryScope, MemorySensitivity, OutboxId, PrincipalUserId, TimestampMillis,
};
use crate::storage::{LifeStorageError, LifeStorageRepository, LifeStorageResult};

/// Result alias for derived Engram operations.
pub type EngramResult<T> = Result<T, EngramError>;

/// Derived Engram integration errors.
#[derive(Debug, Error)]
pub enum EngramError {
    /// Storage operation failed.
    #[error(transparent)]
    Storage(#[from] LifeStorageError),
    /// Backend operation failed.
    #[error("derived Engram backend error: {0}")]
    Backend(String),
    /// Outbox row has no canonical source memory id.
    #[error("Engram outbox row {outbox_id} has no source memory id")]
    MissingSourceMemory {
        /// Invalid outbox row id.
        outbox_id: OutboxId,
    },
}

/// Derived Engram namespace. It is scoped by principal and memory generation.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EngramNamespace {
    /// Principal owner.
    pub principal_user_id: PrincipalUserId,
    /// Generation namespace.
    pub memory_generation_id: MemoryGenerationId,
}

impl EngramNamespace {
    /// Creates a namespace for derived recall.
    #[must_use]
    pub const fn new(
        principal_user_id: PrincipalUserId,
        memory_generation_id: MemoryGenerationId,
    ) -> Self {
        Self {
            principal_user_id,
            memory_generation_id,
        }
    }

    /// Stable wire namespace for derived backends.
    #[must_use]
    pub fn as_wire_key(self) -> String {
        format!(
            "life:{}:gen:{}",
            self.principal_user_id, self.memory_generation_id
        )
    }
}

/// A canonical memory projection sent to a derived backend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngramMemoryProjection {
    /// Idempotency key owned by Oxide.
    pub idempotency_key: String,
    /// Canonical memory id used as the external id.
    pub memory_id: MemoryItemId,
    /// Backend payload derived from canonical Postgres memory.
    pub payload: Value,
}

/// Recall request against a derived generation namespace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecallRequest {
    /// User/task query.
    pub query: String,
    /// Max candidates requested from the backend.
    pub limit: usize,
}

/// Candidate returned by derived recall. Canonical data must be dereferenced in Postgres.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecallCandidate {
    /// Canonical memory id to dereference.
    pub memory_id: MemoryItemId,
    /// Derived relevance score.
    pub score: f32,
    /// Short explanation/evidence from the backend.
    pub rationale: Option<String>,
}

/// Recall candidate after canonical Postgres dereference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DereferencedRecallCandidate {
    /// Derived candidate metadata.
    pub candidate: RecallCandidate,
    /// Canonical memory row from Postgres source of truth.
    pub memory: LifeMemoryItem,
}

/// Replaceable derived long-term memory backend.
#[async_trait]
pub trait LifeLongTermMemoryBackend: Send + Sync {
    /// Appends/idempotently upserts a canonical memory projection.
    async fn append_memory_projection(
        &self,
        namespace: EngramNamespace,
        projection: EngramMemoryProjection,
    ) -> EngramResult<()>;

    /// Recalls candidate canonical memory ids from a generation namespace.
    async fn recall(
        &self,
        namespace: EngramNamespace,
        request: RecallRequest,
    ) -> EngramResult<Vec<RecallCandidate>>;
}

/// Deterministic in-memory backend for tests/bootstrap.
#[derive(Debug, Default, Clone)]
pub struct InMemoryLongTermMemoryBackend {
    projections: Arc<Mutex<Vec<(EngramNamespace, EngramMemoryProjection)>>>,
    recall: Arc<Mutex<HashMap<EngramNamespace, Vec<RecallCandidate>>>>,
}

impl InMemoryLongTermMemoryBackend {
    /// Returns stored projections for assertions.
    #[must_use]
    pub fn projections(&self) -> Vec<(EngramNamespace, EngramMemoryProjection)> {
        self.projections
            .lock()
            .expect("engram projections lock")
            .clone()
    }

    /// Seeds recall candidates for a namespace.
    pub fn seed_recall(&self, namespace: EngramNamespace, candidates: Vec<RecallCandidate>) {
        self.recall
            .lock()
            .expect("engram recall lock")
            .insert(namespace, candidates);
    }
}

#[async_trait]
impl LifeLongTermMemoryBackend for InMemoryLongTermMemoryBackend {
    async fn append_memory_projection(
        &self,
        namespace: EngramNamespace,
        projection: EngramMemoryProjection,
    ) -> EngramResult<()> {
        let mut projections = self.projections.lock().expect("engram projections lock");
        if !projections.iter().any(|(existing_namespace, existing)| {
            *existing_namespace == namespace
                && existing.idempotency_key == projection.idempotency_key
        }) {
            projections.push((namespace, projection));
        }
        Ok(())
    }

    async fn recall(
        &self,
        namespace: EngramNamespace,
        request: RecallRequest,
    ) -> EngramResult<Vec<RecallCandidate>> {
        let candidates = self
            .recall
            .lock()
            .expect("engram recall lock")
            .get(&namespace)
            .cloned()
            .unwrap_or_default();
        Ok(candidates.into_iter().take(request.limit).collect())
    }
}

/// Storage methods required by the outbox projector and recall dereferencer.
#[async_trait]
pub trait EngramProjectionStore: Send + Sync {
    /// Claims pending outbox rows.
    async fn claim_due_engram_outbox(
        &self,
        limit: i64,
        now: TimestampMillis,
    ) -> LifeStorageResult<Vec<LifeEngramOutboxRow>>;

    /// Marks a row flushed.
    async fn mark_engram_outbox_flushed(
        &self,
        outbox_id: OutboxId,
        now: TimestampMillis,
    ) -> LifeStorageResult<()>;

    /// Requeues a row for retry.
    async fn mark_engram_outbox_retry(
        &self,
        outbox_id: OutboxId,
        last_error: &str,
        next_attempt_at: TimestampMillis,
        now: TimestampMillis,
    ) -> LifeStorageResult<()>;

    /// Marks a row dead.
    async fn mark_engram_outbox_dead(
        &self,
        outbox_id: OutboxId,
        last_error: &str,
        now: TimestampMillis,
    ) -> LifeStorageResult<()>;

    /// Loads active memory rows by candidate ids from explicit active scope.
    async fn active_memory_items_by_ids(
        &self,
        scope: MemoryScope,
        memory_ids: &[MemoryItemId],
    ) -> LifeStorageResult<Vec<LifeMemoryItem>>;

    /// Loads active generation pointer.
    async fn active_generation(
        &self,
        principal_user_id: PrincipalUserId,
    ) -> LifeStorageResult<Option<ActiveMemoryGeneration>>;
}

#[async_trait]
impl<T> EngramProjectionStore for T
where
    T: LifeStorageRepository + Send + Sync,
{
    async fn claim_due_engram_outbox(
        &self,
        limit: i64,
        now: TimestampMillis,
    ) -> LifeStorageResult<Vec<LifeEngramOutboxRow>> {
        LifeStorageRepository::claim_due_engram_outbox(self, limit, now).await
    }

    async fn mark_engram_outbox_flushed(
        &self,
        outbox_id: OutboxId,
        now: TimestampMillis,
    ) -> LifeStorageResult<()> {
        LifeStorageRepository::mark_engram_outbox_flushed(self, outbox_id, now).await
    }

    async fn mark_engram_outbox_retry(
        &self,
        outbox_id: OutboxId,
        last_error: &str,
        next_attempt_at: TimestampMillis,
        now: TimestampMillis,
    ) -> LifeStorageResult<()> {
        LifeStorageRepository::mark_engram_outbox_retry(
            self,
            outbox_id,
            last_error,
            next_attempt_at,
            now,
        )
        .await
    }

    async fn mark_engram_outbox_dead(
        &self,
        outbox_id: OutboxId,
        last_error: &str,
        now: TimestampMillis,
    ) -> LifeStorageResult<()> {
        LifeStorageRepository::mark_engram_outbox_dead(self, outbox_id, last_error, now).await
    }

    async fn active_memory_items_by_ids(
        &self,
        scope: MemoryScope,
        memory_ids: &[MemoryItemId],
    ) -> LifeStorageResult<Vec<LifeMemoryItem>> {
        LifeStorageRepository::active_memory_items_by_ids(self, scope, memory_ids).await
    }

    async fn active_generation(
        &self,
        principal_user_id: PrincipalUserId,
    ) -> LifeStorageResult<Option<ActiveMemoryGeneration>> {
        LifeStorageRepository::active_generation(self, principal_user_id).await
    }
}

/// Clock seam for deterministic outbox tests.
pub trait EngramClock: Send + Sync {
    /// Current timestamp.
    fn now(&self) -> TimestampMillis;
}

/// Outbox projector configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngramOutboxProjectorConfig {
    /// Maximum rows claimed in one batch.
    pub batch_limit: i64,
    /// Attempts after which rows are marked dead.
    pub max_attempts: i32,
    /// Retry delay in milliseconds.
    pub retry_delay_millis: i64,
}

impl Default for EngramOutboxProjectorConfig {
    fn default() -> Self {
        Self {
            batch_limit: 32,
            max_attempts: 5,
            retry_delay_millis: 30_000,
        }
    }
}

/// Outbox flush report.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EngramOutboxFlushReport {
    /// Rows claimed for processing.
    pub claimed: usize,
    /// Rows successfully flushed.
    pub flushed: usize,
    /// Rows scheduled for retry.
    pub retried: usize,
    /// Rows marked permanently dead.
    pub dead: usize,
}

/// Projects canonical memory outbox rows to a derived backend.
pub struct EngramOutboxProjector<S, B, C> {
    store: S,
    backend: B,
    clock: C,
    config: EngramOutboxProjectorConfig,
}

impl<S, B, C> EngramOutboxProjector<S, B, C>
where
    S: EngramProjectionStore,
    B: LifeLongTermMemoryBackend,
    C: EngramClock,
{
    /// Creates a projector.
    #[must_use]
    pub const fn new(store: S, backend: B, clock: C, config: EngramOutboxProjectorConfig) -> Self {
        Self {
            store,
            backend,
            clock,
            config,
        }
    }

    /// Flushes due rows. Backend failures never mutate canonical memory.
    pub async fn flush_due(&self) -> EngramResult<EngramOutboxFlushReport> {
        let now = self.clock.now();
        let rows = self
            .store
            .claim_due_engram_outbox(self.config.batch_limit, now)
            .await?;
        let mut report = EngramOutboxFlushReport {
            claimed: rows.len(),
            ..EngramOutboxFlushReport::default()
        };

        for row in rows {
            let result = self.flush_row(&row).await;
            let now = self.clock.now();
            match result {
                Ok(()) => {
                    self.store
                        .mark_engram_outbox_flushed(row.outbox_id, now)
                        .await?;
                    report.flushed += 1;
                }
                Err(error) if row.attempts >= self.config.max_attempts => {
                    self.store
                        .mark_engram_outbox_dead(row.outbox_id, &error.to_string(), now)
                        .await?;
                    report.dead += 1;
                }
                Err(error) => {
                    let next_attempt_at = TimestampMillis::new(
                        now.get().saturating_add(self.config.retry_delay_millis),
                    );
                    self.store
                        .mark_engram_outbox_retry(
                            row.outbox_id,
                            &error.to_string(),
                            next_attempt_at,
                            now,
                        )
                        .await?;
                    report.retried += 1;
                }
            }
        }

        Ok(report)
    }

    async fn flush_row(&self, row: &LifeEngramOutboxRow) -> EngramResult<()> {
        let memory_id = row
            .source_memory_id
            .ok_or(EngramError::MissingSourceMemory {
                outbox_id: row.outbox_id,
            })?;
        let namespace = EngramNamespace::new(row.principal_user_id, row.memory_generation_id);
        let projection = EngramMemoryProjection {
            idempotency_key: row.idempotency_key.clone(),
            memory_id,
            payload: row.payload.clone(),
        };
        self.backend
            .append_memory_projection(namespace, projection)
            .await
    }
}

/// Derived recall service that always dereferences candidates in Postgres.
pub struct DerivedRecallService<S, B> {
    store: S,
    backend: B,
}

impl<S, B> DerivedRecallService<S, B>
where
    S: EngramProjectionStore,
    B: LifeLongTermMemoryBackend,
{
    /// Creates a recall service.
    #[must_use]
    pub const fn new(store: S, backend: B) -> Self {
        Self { store, backend }
    }

    /// Recalls derived candidates and returns only canonical active PG memory rows.
    pub async fn recall_context(
        &self,
        principal_user_id: PrincipalUserId,
        request: RecallRequest,
    ) -> EngramResult<Vec<DereferencedRecallCandidate>> {
        let active = self
            .store
            .active_generation(principal_user_id)
            .await?
            .ok_or(LifeStorageError::MissingActiveGeneration { principal_user_id })?;
        let namespace = EngramNamespace::new(principal_user_id, active.scope.memory_generation_id);
        let candidates = self.backend.recall(namespace, request).await?;
        let ids = candidates
            .iter()
            .map(|candidate| candidate.memory_id)
            .collect::<Vec<_>>();
        let memories = self
            .store
            .active_memory_items_by_ids(active.scope, &ids)
            .await?;
        let memories_by_id = memories
            .into_iter()
            .filter(|memory| memory.sensitivity != MemorySensitivity::SecretBlocked)
            .map(|memory| (memory.memory_id, memory))
            .collect::<HashMap<_, _>>();

        Ok(candidates
            .into_iter()
            .filter_map(|candidate| {
                memories_by_id
                    .get(&candidate.memory_id)
                    .cloned()
                    .map(|memory| DereferencedRecallCandidate { candidate, memory })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        ActiveMemoryGeneration, MemoryAuthority, MemoryItemKind, MemoryItemStatus, MemoryScope,
    };

    #[derive(Debug, Clone)]
    struct FixedClock(TimestampMillis);

    impl EngramClock for FixedClock {
        fn now(&self) -> TimestampMillis {
            self.0
        }
    }

    #[derive(Default)]
    struct FakeEngramStore {
        active: Mutex<Option<ActiveMemoryGeneration>>,
        claimed: Mutex<Vec<LifeEngramOutboxRow>>,
        flushed: Mutex<Vec<OutboxId>>,
        retried: Mutex<Vec<(OutboxId, String, TimestampMillis)>>,
        dead: Mutex<Vec<(OutboxId, String)>>,
        memories: Mutex<Vec<LifeMemoryItem>>,
    }

    #[async_trait]
    impl EngramProjectionStore for FakeEngramStore {
        async fn claim_due_engram_outbox(
            &self,
            _limit: i64,
            _now: TimestampMillis,
        ) -> LifeStorageResult<Vec<LifeEngramOutboxRow>> {
            Ok(std::mem::take(
                &mut *self.claimed.lock().expect("claimed lock"),
            ))
        }

        async fn mark_engram_outbox_flushed(
            &self,
            outbox_id: OutboxId,
            _now: TimestampMillis,
        ) -> LifeStorageResult<()> {
            self.flushed.lock().expect("flushed lock").push(outbox_id);
            Ok(())
        }

        async fn mark_engram_outbox_retry(
            &self,
            outbox_id: OutboxId,
            last_error: &str,
            next_attempt_at: TimestampMillis,
            _now: TimestampMillis,
        ) -> LifeStorageResult<()> {
            self.retried.lock().expect("retried lock").push((
                outbox_id,
                last_error.to_owned(),
                next_attempt_at,
            ));
            Ok(())
        }

        async fn mark_engram_outbox_dead(
            &self,
            outbox_id: OutboxId,
            last_error: &str,
            _now: TimestampMillis,
        ) -> LifeStorageResult<()> {
            self.dead
                .lock()
                .expect("dead lock")
                .push((outbox_id, last_error.to_owned()));
            Ok(())
        }

        async fn active_memory_items_by_ids(
            &self,
            scope: MemoryScope,
            memory_ids: &[MemoryItemId],
        ) -> LifeStorageResult<Vec<LifeMemoryItem>> {
            Ok(self
                .memories
                .lock()
                .expect("memories lock")
                .iter()
                .filter(|memory| {
                    memory.principal_user_id == scope.principal_user_id
                        && memory.memory_generation_id == scope.memory_generation_id
                        && memory_ids.contains(&memory.memory_id)
                        && memory.status == MemoryItemStatus::Active
                })
                .cloned()
                .collect())
        }

        async fn active_generation(
            &self,
            _principal_user_id: PrincipalUserId,
        ) -> LifeStorageResult<Option<ActiveMemoryGeneration>> {
            Ok(*self.active.lock().expect("active lock"))
        }
    }

    #[derive(Default)]
    struct FailingBackend;

    #[async_trait]
    impl LifeLongTermMemoryBackend for FailingBackend {
        async fn append_memory_projection(
            &self,
            _namespace: EngramNamespace,
            _projection: EngramMemoryProjection,
        ) -> EngramResult<()> {
            Err(EngramError::Backend("boom".to_owned()))
        }

        async fn recall(
            &self,
            _namespace: EngramNamespace,
            _request: RecallRequest,
        ) -> EngramResult<Vec<RecallCandidate>> {
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn outbox_projector_projects_to_generation_namespace_and_marks_flushed() {
        let principal = PrincipalUserId::new(7).expect("principal");
        let generation = MemoryGenerationId::new_v4();
        let memory_id = MemoryItemId::new_v4();
        let outbox_id = OutboxId::new_v4();
        let store = FakeEngramStore::default();
        store
            .claimed
            .lock()
            .expect("claimed lock")
            .push(outbox_row(principal, generation, memory_id, outbox_id, 1));
        let backend = InMemoryLongTermMemoryBackend::default();
        let projector = EngramOutboxProjector::new(
            store,
            backend.clone(),
            FixedClock(TimestampMillis::new(100)),
            EngramOutboxProjectorConfig::default(),
        );

        let report = projector.flush_due().await.expect("flush");

        assert_eq!(report.claimed, 1);
        assert_eq!(report.flushed, 1);
        let projection = backend.projections().pop().expect("projection");
        assert_eq!(projection.0, EngramNamespace::new(principal, generation));
        assert_eq!(projection.1.memory_id, memory_id);
        assert_eq!(
            projection.1.idempotency_key,
            format!("life:{principal}:gen:{generation}:memory:{memory_id}")
        );
        assert_eq!(
            projector
                .store
                .flushed
                .lock()
                .expect("flushed lock")
                .as_slice(),
            &[outbox_id]
        );
    }

    #[tokio::test]
    async fn outbox_projector_retries_then_marks_dead_after_max_attempts() {
        let principal = PrincipalUserId::new(8).expect("principal");
        let generation = MemoryGenerationId::new_v4();
        let memory_id = MemoryItemId::new_v4();
        let store = FakeEngramStore::default();
        store.claimed.lock().expect("claimed lock").push(outbox_row(
            principal,
            generation,
            memory_id,
            OutboxId::new_v4(),
            1,
        ));
        let projector = EngramOutboxProjector::new(
            store,
            FailingBackend,
            FixedClock(TimestampMillis::new(200)),
            EngramOutboxProjectorConfig {
                batch_limit: 8,
                max_attempts: 2,
                retry_delay_millis: 50,
            },
        );

        let report = projector.flush_due().await.expect("flush");

        assert_eq!(report.retried, 1);
        assert_eq!(
            projector.store.retried.lock().expect("retried lock")[0].2,
            TimestampMillis::new(250)
        );

        let outbox_id = OutboxId::new_v4();
        projector
            .store
            .claimed
            .lock()
            .expect("claimed lock")
            .push(outbox_row(principal, generation, memory_id, outbox_id, 2));

        let report = projector.flush_due().await.expect("flush");

        assert_eq!(report.dead, 1);
        assert_eq!(
            projector.store.dead.lock().expect("dead lock")[0].0,
            outbox_id
        );
    }

    #[tokio::test]
    async fn recall_dereferences_only_active_non_secret_postgres_memory() {
        let principal = PrincipalUserId::new(9).expect("principal");
        let active_generation = MemoryGenerationId::new_v4();
        let stale_generation = MemoryGenerationId::new_v4();
        let active_memory = memory_item(principal, active_generation, MemorySensitivity::Clean);
        let secret_memory = memory_item(
            principal,
            active_generation,
            MemorySensitivity::SecretBlocked,
        );
        let stale_memory = memory_item(principal, stale_generation, MemorySensitivity::Clean);
        let store = FakeEngramStore::default();
        *store.active.lock().expect("active lock") = Some(ActiveMemoryGeneration {
            scope: MemoryScope::new(principal, active_generation),
            activated_at: TimestampMillis::new(1),
        });
        store.memories.lock().expect("memories lock").extend([
            active_memory.clone(),
            secret_memory.clone(),
            stale_memory,
        ]);
        let backend = InMemoryLongTermMemoryBackend::default();
        backend.seed_recall(
            EngramNamespace::new(principal, active_generation),
            vec![
                RecallCandidate {
                    memory_id: secret_memory.memory_id,
                    score: 0.99,
                    rationale: Some("secret".to_owned()),
                },
                RecallCandidate {
                    memory_id: active_memory.memory_id,
                    score: 0.9,
                    rationale: Some("active".to_owned()),
                },
            ],
        );
        let recall = DerivedRecallService::new(store, backend);

        let result = recall
            .recall_context(
                principal,
                RecallRequest {
                    query: "architecture".to_owned(),
                    limit: 10,
                },
            )
            .await
            .expect("recall");

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].memory.memory_id, active_memory.memory_id);
        assert_eq!(result[0].candidate.score, 0.9);
    }

    fn outbox_row(
        principal_user_id: PrincipalUserId,
        memory_generation_id: MemoryGenerationId,
        memory_id: MemoryItemId,
        outbox_id: OutboxId,
        attempts: i32,
    ) -> LifeEngramOutboxRow {
        LifeEngramOutboxRow {
            outbox_id,
            principal_user_id,
            memory_generation_id,
            source_memory_id: Some(memory_id),
            idempotency_key: format!(
                "life:{principal_user_id}:gen:{memory_generation_id}:memory:{memory_id}"
            ),
            payload: serde_json::json!({"external_id": memory_id.to_string()}),
            status: crate::domain::LifeEngramOutboxStatus::Flushing,
            attempts,
            next_attempt_at: TimestampMillis::new(0),
            last_error: None,
            created_at: TimestampMillis::new(0),
            updated_at: TimestampMillis::new(0),
        }
    }

    fn memory_item(
        principal_user_id: PrincipalUserId,
        memory_generation_id: MemoryGenerationId,
        sensitivity: MemorySensitivity,
    ) -> LifeMemoryItem {
        LifeMemoryItem {
            memory_id: MemoryItemId::new_v4(),
            principal_user_id,
            memory_generation_id,
            kind: MemoryItemKind::ProjectPrinciple,
            authority: MemoryAuthority::UserAsserted,
            status: MemoryItemStatus::Active,
            text: "Prefer architecture-first fixes".to_owned(),
            structured: serde_json::json!({}),
            tags: vec!["oxide".to_owned()],
            evidence_turn_ids: Vec::new(),
            sensitivity,
            valid_from: None,
            valid_to: None,
            supersedes_memory_id: None,
            created_at: TimestampMillis::new(0),
            updated_at: TimestampMillis::new(0),
        }
    }
}
