//! Transport-neutral life gateway contracts and submit service.

use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

use crate::domain::{
    InputId, LifeIdentityLink, LifeInput, LifeInputStatus, LifePrincipal, LifeTransportId,
    LifeTurn, LifeTurnRole, PrincipalUserId, ProviderSubject, RedactionState, RunId,
    TimestampMillis, TurnId,
};
use crate::errors::LifeDomainError;
use crate::storage::{LifeStorageError, LifeStorageRepository, LifeStorageResult};

/// Narrow submit contract used by Web/Telegram transports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LifeInputSubmission {
    /// Transport namespace.
    pub transport_id: LifeTransportId,
    /// Transport-local subject.
    pub provider_subject: ProviderSubject,
    /// User content.
    pub content: String,
    /// Attachment references.
    pub attachments: Value,
    /// Transport metadata.
    pub metadata: Value,
    /// Caller-declared sensitivity. Private secrets are refused before transcript persistence.
    #[serde(default)]
    pub sensitivity: LifeInputSensitivity,
}

/// Explicit sensitivity contract at the transport -> gateway boundary.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LifeInputSensitivity {
    /// Normal user input, stored as clean transcript.
    #[default]
    Normal,
    /// Content has already been redacted by the caller and may be stored as redacted transcript.
    Redacted,
    /// Raw private secret material; refused by life memory instead of persisted.
    PrivateSecret,
}

/// Submit result returned after canonical turn/input creation.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitLifeInputResult {
    /// Resolved principal.
    pub principal_user_id: PrincipalUserId,
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
    /// Raw private secrets must not enter transcript/memory/outbox.
    #[error("private secrets must be stored in the private secret store, not life memory")]
    PrivateSecretRefused,
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
    /// Resolves a transport subject to a canonical principal.
    async fn resolve_identity(
        &self,
        transport_id: &LifeTransportId,
        provider_subject: &ProviderSubject,
    ) -> LifeStorageResult<Option<PrincipalUserId>>;

    /// Upserts a principal envelope.
    async fn upsert_principal(&self, principal: &LifePrincipal) -> LifeStorageResult<()>;

    /// Links a transport subject to a principal.
    async fn link_identity(&self, link: &LifeIdentityLink) -> LifeStorageResult<()>;

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
        transport_id: &LifeTransportId,
        provider_subject: &ProviderSubject,
    ) -> LifeStorageResult<Option<PrincipalUserId>> {
        LifeStorageRepository::resolve_identity(self, transport_id, provider_subject).await
    }

    async fn upsert_principal(&self, principal: &LifePrincipal) -> LifeStorageResult<()> {
        LifeStorageRepository::upsert_principal(self, principal).await
    }

    async fn link_identity(&self, link: &LifeIdentityLink) -> LifeStorageResult<()> {
        LifeStorageRepository::link_identity(self, link).await
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
        if submission.sensitivity == LifeInputSensitivity::PrivateSecret {
            return Err(LifeGatewayError::PrivateSecretRefused);
        }

        let now = self.clock.now()?;
        let principal_user_id = self.resolve_or_create_principal(&submission, now).await?;
        let turn_id = TurnId::new_v4();
        let input_id = InputId::new_v4();

        let turn = LifeTurn {
            turn_id,
            principal_user_id,
            run_id: None,
            role: LifeTurnRole::User,
            source_transport: submission.transport_id.clone(),
            source_ref: None,
            content: submission.content,
            attachments: submission.attachments,
            transport_metadata: submission.metadata,
            redaction_state: match submission.sensitivity {
                LifeInputSensitivity::Normal => RedactionState::Clean,
                LifeInputSensitivity::Redacted => RedactionState::Redacted,
                LifeInputSensitivity::PrivateSecret => unreachable!("private secret refused above"),
            },
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
            .resolve_identity(&submission.transport_id, &submission.provider_subject)
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
            transport_id: submission.transport_id.clone(),
            provider_subject: submission.provider_subject.clone(),
            principal_user_id,
            verified_at: Some(now),
            created_at: now,
            updated_at: now,
        };
        self.store.link_identity(&link).await?;
        Ok(principal_user_id)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;

    use crate::domain::{TELEGRAM_TRANSPORT_ID, WEB_TRANSPORT_ID};
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn submit_creates_principal_turn_and_input() {
        let store = FakeGatewayStore::default();
        let allocated_principal = principal(100500);
        let gateway = LifeGateway::with_clock(
            store.clone(),
            QueueAllocator::new(vec![allocated_principal]),
            FixedClock(TimestampMillis::new(42)),
        );

        let result = gateway
            .submit_life_input(submission(
                WEB_TRANSPORT_ID,
                "web-user-1",
                "start life mode",
            ))
            .await
            .expect("submit should succeed");

        assert_eq!(result.principal_user_id, allocated_principal);
        assert!(result.run_id.is_none());

        let snapshot = store.snapshot();
        assert!(snapshot.principals.contains_key(&allocated_principal));
        assert_eq!(snapshot.identities.len(), 1);
        assert_eq!(snapshot.inputs.len(), 1);
        assert_eq!(snapshot.turns.len(), 1);
        assert_eq!(snapshot.turns[0].content, "start life mode");
        assert_eq!(
            snapshot.turns[0].source_transport.as_str(),
            WEB_TRANSPORT_ID
        );
        assert_eq!(snapshot.turns[0].attachments, json!([{"kind": "document"}]));
        assert_eq!(
            snapshot.turns[0].transport_metadata,
            json!({"source": "test"})
        );
        assert_eq!(snapshot.inputs[0].status, LifeInputStatus::Queued);
        assert_eq!(snapshot.inputs[0].turn_id, result.turn_id);
    }

    #[tokio::test]
    async fn submit_reuses_existing_identity() {
        let store = FakeGatewayStore::default();
        let existing_principal = principal(200600);
        store.seed_identity(TELEGRAM_TRANSPORT_ID, "telegram-user-1", existing_principal);

        let gateway = LifeGateway::with_clock(
            store.clone(),
            PanicAllocator,
            FixedClock(TimestampMillis::new(43)),
        );

        let result = gateway
            .submit_life_input(submission(
                TELEGRAM_TRANSPORT_ID,
                "telegram-user-1",
                "continue",
            ))
            .await
            .expect("submit should reuse existing identity");

        assert_eq!(result.principal_user_id, existing_principal);
        let snapshot = store.snapshot();
        assert_eq!(snapshot.turns.len(), 1);
        assert_eq!(
            snapshot.turns[0].source_transport.as_str(),
            TELEGRAM_TRANSPORT_ID
        );
    }

    #[tokio::test]
    async fn submit_accepts_future_transport_without_enum_or_schema_change() {
        let store = FakeGatewayStore::default();
        let existing_principal = principal(200601);
        store.seed_identity("linux", "machine-local-user", existing_principal);

        let gateway = LifeGateway::with_clock(
            store.clone(),
            PanicAllocator,
            FixedClock(TimestampMillis::new(43)),
        );

        let result = gateway
            .submit_life_input(submission("linux", "machine-local-user", "from linux"))
            .await
            .expect("open transport id should be accepted");

        assert_eq!(result.principal_user_id, existing_principal);
        let snapshot = store.snapshot();
        assert_eq!(snapshot.turns[0].source_transport.as_str(), "linux");
    }

    #[tokio::test]
    async fn submit_rejects_empty_content_before_allocating_principal() {
        let gateway = LifeGateway::with_clock(
            FakeGatewayStore::default(),
            PanicAllocator,
            FixedClock(TimestampMillis::new(44)),
        );

        let error = gateway
            .submit_life_input(submission(WEB_TRANSPORT_ID, "web-user-2", "   "))
            .await
            .expect_err("empty content must fail");

        assert!(matches!(error, LifeGatewayError::EmptyContent));
    }

    #[tokio::test]
    async fn submit_refuses_private_secret_before_principal_or_turn_persistence() {
        let store = FakeGatewayStore::default();
        let gateway = LifeGateway::with_clock(
            store.clone(),
            PanicAllocator,
            FixedClock(TimestampMillis::new(45)),
        );
        let mut submission = submission(WEB_TRANSPORT_ID, "web-user-secret", "token raw");
        submission.sensitivity = LifeInputSensitivity::PrivateSecret;

        let error = gateway
            .submit_life_input(submission)
            .await
            .expect_err("private secrets must be refused");

        assert!(matches!(error, LifeGatewayError::PrivateSecretRefused));
        let snapshot = store.snapshot();
        assert!(snapshot.principals.is_empty());
        assert!(snapshot.turns.is_empty());
        assert!(snapshot.inputs.is_empty());
    }

    #[tokio::test]
    async fn submit_preserves_redacted_transcript_state() {
        let store = FakeGatewayStore::default();
        let allocated_principal = principal(300700);
        let gateway = LifeGateway::with_clock(
            store.clone(),
            QueueAllocator::new(vec![allocated_principal]),
            FixedClock(TimestampMillis::new(46)),
        );
        let mut submission = submission(WEB_TRANSPORT_ID, "web-user-redacted", "[REDACTED]");
        submission.sensitivity = LifeInputSensitivity::Redacted;

        gateway
            .submit_life_input(submission)
            .await
            .expect("redacted input should persist");

        let snapshot = store.snapshot();
        assert_eq!(snapshot.turns[0].redaction_state, RedactionState::Redacted);
        assert_eq!(snapshot.turns[0].content, "[REDACTED]");
    }

    #[derive(Clone, Default)]
    struct FakeGatewayStore {
        inner: std::sync::Arc<Mutex<FakeState>>,
    }

    #[derive(Clone, Default)]
    struct FakeState {
        identities: HashMap<(LifeTransportId, String), PrincipalUserId>,
        principals: HashMap<PrincipalUserId, LifePrincipal>,
        turns: Vec<LifeTurn>,
        inputs: Vec<LifeInput>,
    }

    impl FakeGatewayStore {
        fn seed_identity(
            &self,
            transport_id: &str,
            provider_subject: &str,
            principal_user_id: PrincipalUserId,
        ) {
            let transport_id = transport(transport_id);
            self.inner
                .lock()
                .expect("fake store lock")
                .identities
                .insert(
                    (transport_id, provider_subject.to_owned()),
                    principal_user_id,
                );
        }

        fn snapshot(&self) -> FakeState {
            self.inner.lock().expect("fake store lock").clone()
        }
    }

    #[async_trait]
    impl LifeGatewayStore for FakeGatewayStore {
        async fn resolve_identity(
            &self,
            transport_id: &LifeTransportId,
            provider_subject: &ProviderSubject,
        ) -> LifeStorageResult<Option<PrincipalUserId>> {
            Ok(self
                .inner
                .lock()
                .expect("fake store lock")
                .identities
                .get(&(transport_id.clone(), provider_subject.as_str().to_owned()))
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
                    (
                        link.transport_id.clone(),
                        link.provider_subject.as_str().to_owned(),
                    ),
                    link.principal_user_id,
                );
            Ok(())
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
        transport_id: &str,
        provider_subject: &str,
        content: &str,
    ) -> LifeInputSubmission {
        LifeInputSubmission {
            transport_id: transport(transport_id),
            provider_subject: ProviderSubject::new(provider_subject).expect("provider subject"),
            content: content.to_owned(),
            attachments: json!([{"kind": "document"}]),
            metadata: json!({"source": "test"}),
            sensitivity: LifeInputSensitivity::Normal,
        }
    }

    fn transport(value: &str) -> LifeTransportId {
        LifeTransportId::new(value).expect("transport id")
    }
}
