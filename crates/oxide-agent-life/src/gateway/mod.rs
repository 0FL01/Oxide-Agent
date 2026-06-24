//! Transport-neutral life gateway contracts and submit service.

use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

use crate::domain::{
    ActiveMemoryGeneration, InputId, LifeIdentityLink, LifeIdentityProvider, LifeInput,
    LifeInputStatus, LifeMemoryGeneration, LifePrincipal, LifeSourceTransport, LifeTurn,
    LifeTurnRole, MemoryGenerationId, MemoryGenerationStatus, MemoryScope, PrincipalUserId,
    ProviderSubject, RedactionState, RunId, TimestampMillis, TurnId,
};
use crate::errors::LifeDomainError;
use crate::storage::{LifeStorageError, LifeStorageRepository, LifeStorageResult};

/// Narrow submit contract used by Web/Telegram transports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LifeInputSubmission {
    /// Provider namespace.
    pub provider: LifeIdentityProvider,
    /// Provider-local subject.
    pub provider_subject: ProviderSubject,
    /// User content.
    pub content: String,
    /// Attachment references.
    pub attachments: Value,
    /// Transport metadata.
    pub metadata: Value,
}

/// Submit result returned after canonical turn/input creation.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitLifeInputResult {
    /// Resolved principal.
    pub principal_user_id: PrincipalUserId,
    /// Active memory scope used for queue/run decisions.
    pub memory_scope: MemoryScope,
    /// Canonical user turn id.
    pub turn_id: TurnId,
    /// Queued input id.
    pub input_id: InputId,
    /// Attached or created run id.
    pub run_id: Option<RunId>,
}

/// Errors raised by the gateway boundary.
#[derive(Debug, Error)]
pub enum LifeGatewayError {
    /// User content is required for a life input.
    #[error("life input content must not be empty")]
    EmptyContent,
    /// System clock is before unix epoch.
    #[error("life gateway clock error: {0}")]
    Clock(String),
    /// Domain validation failed.
    #[error(transparent)]
    Domain(#[from] LifeDomainError),
    /// Storage failed.
    #[error(transparent)]
    Storage(#[from] LifeStorageError),
}

/// Gateway result alias.
pub type LifeGatewayResult<T> = Result<T, LifeGatewayError>;

/// Gateway-specific store boundary.
#[async_trait]
pub trait LifeGatewayStore: Send + Sync {
    /// Resolves a provider subject to a canonical principal.
    async fn resolve_identity(
        &self,
        provider: LifeIdentityProvider,
        provider_subject: &ProviderSubject,
    ) -> LifeStorageResult<Option<PrincipalUserId>>;

    /// Upserts a principal envelope.
    async fn upsert_principal(&self, principal: &LifePrincipal) -> LifeStorageResult<()>;

    /// Links a provider subject to a principal.
    async fn link_identity(&self, link: &LifeIdentityLink) -> LifeStorageResult<()>;

    /// Returns active memory generation pointer.
    async fn active_generation(
        &self,
        principal_user_id: PrincipalUserId,
    ) -> LifeStorageResult<Option<ActiveMemoryGeneration>>;

    /// Returns the next generation number for a principal.
    async fn next_memory_generation_number(
        &self,
        principal_user_id: PrincipalUserId,
    ) -> LifeStorageResult<i64>;

    /// Inserts a memory generation.
    async fn insert_memory_generation(
        &self,
        generation: &LifeMemoryGeneration,
    ) -> LifeStorageResult<()>;

    /// Activates a generation.
    async fn activate_memory_generation(
        &self,
        principal_user_id: PrincipalUserId,
        memory_generation_id: MemoryGenerationId,
        activated_at: TimestampMillis,
        activation_reason: &str,
    ) -> LifeStorageResult<ActiveMemoryGeneration>;

    /// Appends a canonical turn.
    async fn append_turn(&self, turn: &LifeTurn) -> LifeStorageResult<()>;

    /// Queues a life input.
    async fn enqueue_input(&self, input: &LifeInput) -> LifeStorageResult<()>;
}

#[async_trait]
impl<T> LifeGatewayStore for T
where
    T: LifeStorageRepository + Send + Sync,
{
    async fn resolve_identity(
        &self,
        provider: LifeIdentityProvider,
        provider_subject: &ProviderSubject,
    ) -> LifeStorageResult<Option<PrincipalUserId>> {
        LifeStorageRepository::resolve_identity(self, provider, provider_subject).await
    }

    async fn upsert_principal(&self, principal: &LifePrincipal) -> LifeStorageResult<()> {
        LifeStorageRepository::upsert_principal(self, principal).await
    }

    async fn link_identity(&self, link: &LifeIdentityLink) -> LifeStorageResult<()> {
        LifeStorageRepository::link_identity(self, link).await
    }

    async fn active_generation(
        &self,
        principal_user_id: PrincipalUserId,
    ) -> LifeStorageResult<Option<ActiveMemoryGeneration>> {
        LifeStorageRepository::active_generation(self, principal_user_id).await
    }

    async fn next_memory_generation_number(
        &self,
        principal_user_id: PrincipalUserId,
    ) -> LifeStorageResult<i64> {
        LifeStorageRepository::next_memory_generation_number(self, principal_user_id).await
    }

    async fn insert_memory_generation(
        &self,
        generation: &LifeMemoryGeneration,
    ) -> LifeStorageResult<()> {
        LifeStorageRepository::insert_memory_generation(self, generation).await
    }

    async fn activate_memory_generation(
        &self,
        principal_user_id: PrincipalUserId,
        memory_generation_id: MemoryGenerationId,
        activated_at: TimestampMillis,
        activation_reason: &str,
    ) -> LifeStorageResult<ActiveMemoryGeneration> {
        LifeStorageRepository::activate_memory_generation(
            self,
            principal_user_id,
            memory_generation_id,
            activated_at,
            activation_reason,
        )
        .await
    }

    async fn append_turn(&self, turn: &LifeTurn) -> LifeStorageResult<()> {
        LifeStorageRepository::append_turn(self, turn).await
    }

    async fn enqueue_input(&self, input: &LifeInput) -> LifeStorageResult<()> {
        LifeStorageRepository::enqueue_input(self, input).await
    }
}

/// Receiver-owned allocator for internal life principal ids.
#[async_trait]
pub trait LifePrincipalAllocator: Send + Sync {
    /// Allocates a new canonical principal id.
    async fn allocate_principal_user_id(&self) -> LifeGatewayResult<PrincipalUserId>;
}

/// Clock boundary for deterministic gateway tests.
pub trait LifeClock: Send + Sync {
    /// Returns the current timestamp.
    fn now(&self) -> LifeGatewayResult<TimestampMillis>;
}

/// System clock implementation.
#[derive(Debug, Copy, Clone, Default)]
pub struct SystemLifeClock;

impl LifeClock for SystemLifeClock {
    fn now(&self) -> LifeGatewayResult<TimestampMillis> {
        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| LifeGatewayError::Clock(error.to_string()))?;
        Ok(TimestampMillis::new(
            i64::try_from(duration.as_millis())
                .map_err(|error| LifeGatewayError::Clock(error.to_string()))?,
        ))
    }
}

/// Transport-neutral life gateway.
pub struct LifeGateway<S, A, C = SystemLifeClock> {
    store: S,
    allocator: A,
    clock: C,
}

impl<S, A> LifeGateway<S, A, SystemLifeClock> {
    /// Creates a gateway using the system clock.
    #[must_use]
    pub const fn new(store: S, allocator: A) -> Self {
        Self {
            store,
            allocator,
            clock: SystemLifeClock,
        }
    }
}

impl<S, A, C> LifeGateway<S, A, C> {
    /// Creates a gateway with an explicit clock.
    #[must_use]
    pub const fn with_clock(store: S, allocator: A, clock: C) -> Self {
        Self {
            store,
            allocator,
            clock,
        }
    }
}

impl<S, A, C> LifeGateway<S, A, C>
where
    S: LifeGatewayStore,
    A: LifePrincipalAllocator,
    C: LifeClock,
{
    /// Submits a life input through the narrow transport contract.
    pub async fn submit_life_input(
        &self,
        submission: LifeInputSubmission,
    ) -> LifeGatewayResult<SubmitLifeInputResult> {
        if submission.content.trim().is_empty() {
            return Err(LifeGatewayError::EmptyContent);
        }

        let now = self.clock.now()?;
        let principal_user_id = self.resolve_or_create_principal(&submission, now).await?;
        let active_generation = self
            .ensure_active_generation(principal_user_id, now)
            .await?;
        let turn_id = TurnId::new_v4();
        let input_id = InputId::new_v4();

        let turn = LifeTurn {
            turn_id,
            principal_user_id,
            run_id: None,
            role: LifeTurnRole::User,
            source_transport: source_transport_for_provider(submission.provider),
            source_ref: None,
            content: submission.content,
            attachments: submission.attachments,
            transport_metadata: submission.metadata,
            redaction_state: RedactionState::Clean,
            created_at: now,
        };
        self.store.append_turn(&turn).await?;

        let input = LifeInput {
            input_id,
            principal_user_id,
            turn_id,
            status: LifeInputStatus::Queued,
            claimed_by: None,
            claimed_at: None,
            created_at: now,
            updated_at: now,
        };
        self.store.enqueue_input(&input).await?;

        Ok(SubmitLifeInputResult {
            principal_user_id,
            memory_scope: active_generation.scope,
            turn_id,
            input_id,
            run_id: None,
        })
    }

    async fn resolve_or_create_principal(
        &self,
        submission: &LifeInputSubmission,
        now: TimestampMillis,
    ) -> LifeGatewayResult<PrincipalUserId> {
        if let Some(principal_user_id) = self
            .store
            .resolve_identity(submission.provider, &submission.provider_subject)
            .await?
        {
            return Ok(principal_user_id);
        }

        let principal_user_id = self.allocator.allocate_principal_user_id().await?;
        let principal = LifePrincipal {
            principal_user_id,
            profile_state: json!({}),
            operating_profile: json!({}),
            settings: json!({}),
            schema_version: 1,
            created_at: now,
            updated_at: now,
        };
        self.store.upsert_principal(&principal).await?;

        let link = LifeIdentityLink {
            provider: submission.provider,
            provider_subject: submission.provider_subject.clone(),
            principal_user_id,
            verified_at: Some(now),
            created_at: now,
            updated_at: now,
        };
        self.store.link_identity(&link).await?;
        Ok(principal_user_id)
    }

    async fn ensure_active_generation(
        &self,
        principal_user_id: PrincipalUserId,
        now: TimestampMillis,
    ) -> LifeGatewayResult<ActiveMemoryGeneration> {
        if let Some(active) = self.store.active_generation(principal_user_id).await? {
            return Ok(active);
        }

        let generation_number = self
            .store
            .next_memory_generation_number(principal_user_id)
            .await?;
        let generation = LifeMemoryGeneration {
            memory_generation_id: MemoryGenerationId::new_v4(),
            principal_user_id,
            generation_number,
            status: MemoryGenerationStatus::Building,
            source_generation_id: None,
            build_reason: "initial life input".to_owned(),
            build_policy: json!({"source": "life_gateway", "version": 1}),
            source_scope: json!({"kind": "initial_generation"}),
            comparison_report: json!({}),
            activated_at: None,
            created_at: now,
            updated_at: now,
        };
        self.store.insert_memory_generation(&generation).await?;
        Ok(self
            .store
            .activate_memory_generation(
                principal_user_id,
                generation.memory_generation_id,
                now,
                "initial life input",
            )
            .await?)
    }
}

const fn source_transport_for_provider(provider: LifeIdentityProvider) -> LifeSourceTransport {
    match provider {
        LifeIdentityProvider::Web => LifeSourceTransport::Web,
        LifeIdentityProvider::Telegram => LifeSourceTransport::Telegram,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;

    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn submit_creates_principal_generation_turn_and_input() {
        let store = FakeGatewayStore::default();
        let allocated_principal = principal(100500);
        let gateway = LifeGateway::with_clock(
            store.clone(),
            QueueAllocator::new(vec![allocated_principal]),
            FixedClock(TimestampMillis::new(42)),
        );

        let result = gateway
            .submit_life_input(submission(
                LifeIdentityProvider::Web,
                "web-user-1",
                "start life mode",
            ))
            .await
            .expect("submit should succeed");

        assert_eq!(result.principal_user_id, allocated_principal);
        assert_eq!(result.memory_scope.principal_user_id, allocated_principal);
        assert!(result.run_id.is_none());

        let snapshot = store.snapshot();
        assert!(snapshot.principals.contains_key(&allocated_principal));
        assert_eq!(snapshot.identities.len(), 1);
        assert_eq!(snapshot.generations.len(), 1);
        assert_eq!(snapshot.inputs.len(), 1);
        assert_eq!(snapshot.turns.len(), 1);
        assert_eq!(snapshot.turns[0].content, "start life mode");
        assert_eq!(snapshot.turns[0].source_transport, LifeSourceTransport::Web);
        assert_eq!(snapshot.turns[0].attachments, json!([{"kind": "document"}]));
        assert_eq!(
            snapshot.turns[0].transport_metadata,
            json!({"source": "test"})
        );
        assert_eq!(snapshot.inputs[0].status, LifeInputStatus::Queued);
        assert_eq!(snapshot.inputs[0].turn_id, result.turn_id);
    }

    #[tokio::test]
    async fn submit_reuses_existing_identity_and_active_generation() {
        let store = FakeGatewayStore::default();
        let existing_principal = principal(200600);
        let generation_id = MemoryGenerationId::new_v4();
        let scope = MemoryScope::new(existing_principal, generation_id);
        store.seed_identity(
            LifeIdentityProvider::Telegram,
            "telegram-user-1",
            existing_principal,
        );
        store.seed_active_generation(ActiveMemoryGeneration {
            scope,
            activated_at: TimestampMillis::new(7),
        });

        let gateway = LifeGateway::with_clock(
            store.clone(),
            PanicAllocator,
            FixedClock(TimestampMillis::new(43)),
        );

        let result = gateway
            .submit_life_input(submission(
                LifeIdentityProvider::Telegram,
                "telegram-user-1",
                "continue",
            ))
            .await
            .expect("submit should reuse existing identity");

        assert_eq!(result.principal_user_id, existing_principal);
        assert_eq!(result.memory_scope, scope);
        let snapshot = store.snapshot();
        assert_eq!(snapshot.generations.len(), 0);
        assert_eq!(snapshot.turns.len(), 1);
        assert_eq!(
            snapshot.turns[0].source_transport,
            LifeSourceTransport::Telegram
        );
    }

    #[tokio::test]
    async fn submit_rejects_empty_content_before_allocating_principal() {
        let gateway = LifeGateway::with_clock(
            FakeGatewayStore::default(),
            PanicAllocator,
            FixedClock(TimestampMillis::new(44)),
        );

        let error = gateway
            .submit_life_input(submission(LifeIdentityProvider::Web, "web-user-2", "   "))
            .await
            .expect_err("empty content must fail");

        assert!(matches!(error, LifeGatewayError::EmptyContent));
    }

    #[derive(Clone, Default)]
    struct FakeGatewayStore {
        inner: std::sync::Arc<Mutex<FakeState>>,
    }

    #[derive(Clone, Default)]
    struct FakeState {
        identities: HashMap<(LifeIdentityProvider, String), PrincipalUserId>,
        principals: HashMap<PrincipalUserId, LifePrincipal>,
        generations: Vec<LifeMemoryGeneration>,
        active: HashMap<PrincipalUserId, ActiveMemoryGeneration>,
        turns: Vec<LifeTurn>,
        inputs: Vec<LifeInput>,
    }

    impl FakeGatewayStore {
        fn seed_identity(
            &self,
            provider: LifeIdentityProvider,
            provider_subject: &str,
            principal_user_id: PrincipalUserId,
        ) {
            self.inner
                .lock()
                .expect("fake store lock")
                .identities
                .insert((provider, provider_subject.to_owned()), principal_user_id);
        }

        fn seed_active_generation(&self, active: ActiveMemoryGeneration) {
            self.inner
                .lock()
                .expect("fake store lock")
                .active
                .insert(active.scope.principal_user_id, active);
        }

        fn snapshot(&self) -> FakeState {
            self.inner.lock().expect("fake store lock").clone()
        }
    }

    #[async_trait]
    impl LifeGatewayStore for FakeGatewayStore {
        async fn resolve_identity(
            &self,
            provider: LifeIdentityProvider,
            provider_subject: &ProviderSubject,
        ) -> LifeStorageResult<Option<PrincipalUserId>> {
            Ok(self
                .inner
                .lock()
                .expect("fake store lock")
                .identities
                .get(&(provider, provider_subject.as_str().to_owned()))
                .copied())
        }

        async fn upsert_principal(&self, principal: &LifePrincipal) -> LifeStorageResult<()> {
            self.inner
                .lock()
                .expect("fake store lock")
                .principals
                .insert(principal.principal_user_id, principal.clone());
            Ok(())
        }

        async fn link_identity(&self, link: &LifeIdentityLink) -> LifeStorageResult<()> {
            self.inner
                .lock()
                .expect("fake store lock")
                .identities
                .insert(
                    (link.provider, link.provider_subject.as_str().to_owned()),
                    link.principal_user_id,
                );
            Ok(())
        }

        async fn active_generation(
            &self,
            principal_user_id: PrincipalUserId,
        ) -> LifeStorageResult<Option<ActiveMemoryGeneration>> {
            Ok(self
                .inner
                .lock()
                .expect("fake store lock")
                .active
                .get(&principal_user_id)
                .copied())
        }

        async fn next_memory_generation_number(
            &self,
            principal_user_id: PrincipalUserId,
        ) -> LifeStorageResult<i64> {
            let next = self
                .inner
                .lock()
                .expect("fake store lock")
                .generations
                .iter()
                .filter(|generation| generation.principal_user_id == principal_user_id)
                .map(|generation| generation.generation_number)
                .max()
                .unwrap_or(0)
                + 1;
            Ok(next)
        }

        async fn insert_memory_generation(
            &self,
            generation: &LifeMemoryGeneration,
        ) -> LifeStorageResult<()> {
            self.inner
                .lock()
                .expect("fake store lock")
                .generations
                .push(generation.clone());
            Ok(())
        }

        async fn activate_memory_generation(
            &self,
            principal_user_id: PrincipalUserId,
            memory_generation_id: MemoryGenerationId,
            activated_at: TimestampMillis,
            _activation_reason: &str,
        ) -> LifeStorageResult<ActiveMemoryGeneration> {
            let active = ActiveMemoryGeneration {
                scope: MemoryScope::new(principal_user_id, memory_generation_id),
                activated_at,
            };
            self.inner
                .lock()
                .expect("fake store lock")
                .active
                .insert(principal_user_id, active);
            Ok(active)
        }

        async fn append_turn(&self, turn: &LifeTurn) -> LifeStorageResult<()> {
            self.inner
                .lock()
                .expect("fake store lock")
                .turns
                .push(turn.clone());
            Ok(())
        }

        async fn enqueue_input(&self, input: &LifeInput) -> LifeStorageResult<()> {
            self.inner
                .lock()
                .expect("fake store lock")
                .inputs
                .push(input.clone());
            Ok(())
        }
    }

    struct QueueAllocator {
        principals: Mutex<VecDeque<PrincipalUserId>>,
    }

    impl QueueAllocator {
        fn new(principals: Vec<PrincipalUserId>) -> Self {
            Self {
                principals: Mutex::new(principals.into()),
            }
        }
    }

    #[async_trait]
    impl LifePrincipalAllocator for QueueAllocator {
        async fn allocate_principal_user_id(&self) -> LifeGatewayResult<PrincipalUserId> {
            self.principals
                .lock()
                .expect("allocator lock")
                .pop_front()
                .ok_or_else(|| LifeGatewayError::Clock("test allocator exhausted".to_owned()))
        }
    }

    struct PanicAllocator;

    #[async_trait]
    impl LifePrincipalAllocator for PanicAllocator {
        async fn allocate_principal_user_id(&self) -> LifeGatewayResult<PrincipalUserId> {
            panic!("allocator must not be called")
        }
    }

    #[derive(Debug, Copy, Clone)]
    struct FixedClock(TimestampMillis);

    impl LifeClock for FixedClock {
        fn now(&self) -> LifeGatewayResult<TimestampMillis> {
            Ok(self.0)
        }
    }

    fn principal(value: i64) -> PrincipalUserId {
        PrincipalUserId::new(value).expect("positive principal")
    }

    fn submission(
        provider: LifeIdentityProvider,
        provider_subject: &str,
        content: &str,
    ) -> LifeInputSubmission {
        LifeInputSubmission {
            provider,
            provider_subject: ProviderSubject::new(provider_subject).expect("provider subject"),
            content: content.to_owned(),
            attachments: json!([{"kind": "document"}]),
            metadata: json!({"source": "test"}),
        }
    }
}
