//! Transport-neutral life gateway contracts and submit service.

use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::domain::{
    InputId, LifeDeliveryOutbox, LifeInput, LifeInputStatus, LifeTransportBinding, LifeTransportId,
    LifeTurn, LifeTurnRole, PrincipalUserId, RedactionState, RunId, TimestampMillis, TurnId,
};
use crate::errors::LifeDomainError;
use crate::storage::{LifeStorageError, LifeStorageRepository, LifeStorageResult};

/// Narrow submit contract used by Web/Telegram transports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LifeInputSubmission {
    /// Transport namespace.
    pub transport_id: LifeTransportId,
    /// Transport-local inbound address observed by the adapter.
    pub inbound_address: Value,
    /// Optional transport-local source reference, such as a message id.
    pub source_ref: Option<String>,
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
    /// The inbound transport address is not owner-approved.
    #[error("life transport binding is not configured for transport '{transport_id}'")]
    UnboundTransport {
        /// Transport namespace.
        transport_id: LifeTransportId,
    },
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
    /// Resolves an enabled owner-approved binding by inbound address.
    async fn resolve_transport_binding(
        &self,
        transport_id: &LifeTransportId,
        inbound_address: &Value,
    ) -> LifeStorageResult<Option<LifeTransportBinding>>;

    /// Atomically appends a user turn and enqueues cross-transport deliveries.
    async fn append_user_turn_and_enqueue_deliveries(
        &self,
        turn: &LifeTurn,
        now: TimestampMillis,
    ) -> LifeStorageResult<Vec<LifeDeliveryOutbox>>;

    /// Queues a life input.
    async fn enqueue_input(&self, input: &LifeInput) -> LifeStorageResult<()>;
}

#[async_trait]
impl<T> LifeGatewayStore for T
where
    T: LifeStorageRepository + Send + Sync,
{
    async fn resolve_transport_binding(
        &self,
        transport_id: &LifeTransportId,
        inbound_address: &Value,
    ) -> LifeStorageResult<Option<LifeTransportBinding>> {
        LifeStorageRepository::resolve_transport_binding(self, transport_id, inbound_address).await
    }

    async fn append_user_turn_and_enqueue_deliveries(
        &self,
        turn: &LifeTurn,
        now: TimestampMillis,
    ) -> LifeStorageResult<Vec<LifeDeliveryOutbox>> {
        LifeStorageRepository::append_user_turn_and_enqueue_deliveries(self, turn, now).await
    }

    async fn enqueue_input(&self, input: &LifeInput) -> LifeStorageResult<()> {
        LifeStorageRepository::enqueue_input(self, input).await
    }
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
pub struct LifeGateway<S, C = SystemLifeClock> {
    store: S,
    clock: C,
}

impl<S> LifeGateway<S, SystemLifeClock> {
    /// Creates a gateway using the system clock.
    #[must_use]
    pub const fn new(store: S) -> Self {
        Self {
            store,
            clock: SystemLifeClock,
        }
    }
}

impl<S, C> LifeGateway<S, C> {
    /// Creates a gateway with an explicit clock.
    #[must_use]
    pub const fn with_clock(store: S, clock: C) -> Self {
        Self { store, clock }
    }
}

impl<S, C> LifeGateway<S, C>
where
    S: LifeGatewayStore,
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
        let binding = self.resolve_binding(&submission).await?;
        let principal_user_id = binding.principal_user_id;
        let turn_id = TurnId::new_v4();
        let input_id = InputId::new_v4();

        let turn = LifeTurn {
            turn_id,
            principal_user_id,
            run_id: None,
            role: LifeTurnRole::User,
            source_transport: submission.transport_id.clone(),
            source_ref: submission.source_ref,
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
        self.store
            .append_user_turn_and_enqueue_deliveries(&turn, now)
            .await?;

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

    async fn resolve_binding(
        &self,
        submission: &LifeInputSubmission,
    ) -> LifeGatewayResult<LifeTransportBinding> {
        self.store
            .resolve_transport_binding(&submission.transport_id, &submission.inbound_address)
            .await?
            .ok_or_else(|| LifeGatewayError::UnboundTransport {
                transport_id: submission.transport_id.clone(),
            })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use crate::domain::{BindingId, TELEGRAM_TRANSPORT_ID, WEB_TRANSPORT_ID};
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn submit_uses_configured_binding_for_turn_and_input() {
        let store = FakeGatewayStore::default();
        let configured_principal = principal(100500);
        store.seed_binding(
            WEB_TRANSPORT_ID,
            json!({"user_id": 100500}),
            configured_principal,
        );
        let gateway = LifeGateway::with_clock(store.clone(), FixedClock(TimestampMillis::new(42)));

        let result = gateway
            .submit_life_input(submission(
                WEB_TRANSPORT_ID,
                json!({"user_id": 100500}),
                "start life mode",
            ))
            .await
            .expect("submit should succeed");

        assert_eq!(result.principal_user_id, configured_principal);
        assert!(result.run_id.is_none());

        let snapshot = store.snapshot();
        assert_eq!(snapshot.inputs.len(), 1);
        assert_eq!(snapshot.turns.len(), 1);
        assert_eq!(snapshot.turns[0].content, "start life mode");
        assert_eq!(snapshot.turns[0].source_ref.as_deref(), Some("test-ref"));
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
    async fn submit_enqueues_delivery_to_other_transports_only() {
        let store = FakeGatewayStore::default();
        let configured_principal = principal(100600);
        // Web binding — source transport
        store.seed_binding(
            WEB_TRANSPORT_ID,
            json!({"user_id": 100600}),
            configured_principal,
        );
        // Telegram binding — should receive the delivery
        store.seed_binding(
            TELEGRAM_TRANSPORT_ID,
            json!({"chat_id": 999}),
            configured_principal,
        );
        let gateway = LifeGateway::with_clock(store.clone(), FixedClock(TimestampMillis::new(77)));

        let result = gateway
            .submit_life_input(submission(
                WEB_TRANSPORT_ID,
                json!({"user_id": 100600}),
                "cross-transport message",
            ))
            .await
            .expect("submit should succeed");

        let snapshot = store.snapshot();
        // User turn persisted
        assert_eq!(snapshot.turns.len(), 1);
        assert_eq!(snapshot.turns[0].content, "cross-transport message");

        // Exactly one delivery — to Telegram, not back to Web
        assert_eq!(snapshot.deliveries.len(), 1);
        assert_eq!(
            snapshot.deliveries[0].transport_id.as_str(),
            TELEGRAM_TRANSPORT_ID
        );
        assert_eq!(snapshot.deliveries[0].turn_id, result.turn_id);
    }

    #[tokio::test]
    async fn submit_from_telegram_enqueues_delivery_to_web_only() {
        let store = FakeGatewayStore::default();
        let configured_principal = principal(100700);
        store.seed_binding(
            WEB_TRANSPORT_ID,
            json!({"user_id": 100700}),
            configured_principal,
        );
        store.seed_binding(
            TELEGRAM_TRANSPORT_ID,
            json!({"chat_id": 777}),
            configured_principal,
        );
        let gateway = LifeGateway::with_clock(store.clone(), FixedClock(TimestampMillis::new(88)));

        let result = gateway
            .submit_life_input(submission(
                TELEGRAM_TRANSPORT_ID,
                json!({"chat_id": 777}),
                "from telegram",
            ))
            .await
            .expect("submit should succeed");

        let snapshot = store.snapshot();
        assert_eq!(snapshot.deliveries.len(), 1);
        assert_eq!(
            snapshot.deliveries[0].transport_id.as_str(),
            WEB_TRANSPORT_ID
        );
        assert_eq!(snapshot.deliveries[0].turn_id, result.turn_id);
    }

    #[tokio::test]
    async fn submit_rejects_unknown_binding_before_persistence() {
        let store = FakeGatewayStore::default();
        let gateway = LifeGateway::with_clock(store.clone(), FixedClock(TimestampMillis::new(43)));

        let error = gateway
            .submit_life_input(submission(
                TELEGRAM_TRANSPORT_ID,
                json!({"chat_id": 424242}),
                "unknown chat",
            ))
            .await
            .expect_err("unknown inbound address must fail");

        assert!(matches!(error, LifeGatewayError::UnboundTransport { .. }));
        let snapshot = store.snapshot();
        assert!(snapshot.turns.is_empty());
        assert!(snapshot.inputs.is_empty());
    }

    #[tokio::test]
    async fn submit_accepts_future_transport_without_enum_or_schema_change() {
        let store = FakeGatewayStore::default();
        let existing_principal = principal(200601);
        store.seed_binding(
            "linux",
            json!({"instance_id": "desktop-1"}),
            existing_principal,
        );

        let gateway = LifeGateway::with_clock(store.clone(), FixedClock(TimestampMillis::new(43)));

        let result = gateway
            .submit_life_input(submission(
                "linux",
                json!({"instance_id": "desktop-1"}),
                "from linux",
            ))
            .await
            .expect("open transport id should be accepted");

        assert_eq!(result.principal_user_id, existing_principal);
        let snapshot = store.snapshot();
        assert_eq!(snapshot.turns[0].source_transport.as_str(), "linux");
    }

    #[tokio::test]
    async fn submit_rejects_empty_content_before_resolving_binding() {
        let gateway = LifeGateway::with_clock(
            FakeGatewayStore::default(),
            FixedClock(TimestampMillis::new(44)),
        );

        let error = gateway
            .submit_life_input(submission(WEB_TRANSPORT_ID, json!({"user_id": 2}), "   "))
            .await
            .expect_err("empty content must fail");

        assert!(matches!(error, LifeGatewayError::EmptyContent));
    }

    #[tokio::test]
    async fn submit_refuses_private_secret_before_principal_or_turn_persistence() {
        let store = FakeGatewayStore::default();
        let gateway = LifeGateway::with_clock(store.clone(), FixedClock(TimestampMillis::new(45)));
        let mut submission = submission(WEB_TRANSPORT_ID, json!({"user_id": 3}), "token raw");
        submission.sensitivity = LifeInputSensitivity::PrivateSecret;

        let error = gateway
            .submit_life_input(submission)
            .await
            .expect_err("private secrets must be refused");

        assert!(matches!(error, LifeGatewayError::PrivateSecretRefused));
        let snapshot = store.snapshot();
        assert!(snapshot.turns.is_empty());
        assert!(snapshot.inputs.is_empty());
    }

    #[tokio::test]
    async fn submit_preserves_redacted_transcript_state() {
        let store = FakeGatewayStore::default();
        let configured_principal = principal(300700);
        store.seed_binding(
            WEB_TRANSPORT_ID,
            json!({"user_id": 300700}),
            configured_principal,
        );
        let gateway = LifeGateway::with_clock(store.clone(), FixedClock(TimestampMillis::new(46)));
        let mut submission = submission(WEB_TRANSPORT_ID, json!({"user_id": 300700}), "[REDACTED]");
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
        bindings: Vec<LifeTransportBinding>,
        turns: Vec<LifeTurn>,
        inputs: Vec<LifeInput>,
        deliveries: Vec<LifeDeliveryOutbox>,
    }

    impl FakeGatewayStore {
        fn seed_binding(
            &self,
            transport_id: &str,
            inbound_address: Value,
            principal_user_id: PrincipalUserId,
        ) {
            let now = TimestampMillis::new(1);
            let transport_id = transport(transport_id);
            self.inner
                .lock()
                .expect("fake store lock")
                .bindings
                .push(LifeTransportBinding {
                    binding_id: BindingId::new_v4(),
                    principal_user_id,
                    transport_id,
                    inbound_address: inbound_address.clone(),
                    delivery_address: inbound_address,
                    enabled: true,
                    created_at: now,
                    updated_at: now,
                });
        }

        fn snapshot(&self) -> FakeState {
            self.inner.lock().expect("fake store lock").clone()
        }
    }

    #[async_trait]
    impl LifeGatewayStore for FakeGatewayStore {
        async fn resolve_transport_binding(
            &self,
            transport_id: &LifeTransportId,
            inbound_address: &Value,
        ) -> LifeStorageResult<Option<LifeTransportBinding>> {
            Ok(self
                .inner
                .lock()
                .expect("fake store lock")
                .bindings
                .iter()
                .find(|binding| {
                    binding.enabled
                        && &binding.transport_id == transport_id
                        && &binding.inbound_address == inbound_address
                })
                .cloned())
        }

        async fn append_user_turn_and_enqueue_deliveries(
            &self,
            turn: &LifeTurn,
            now: TimestampMillis,
        ) -> LifeStorageResult<Vec<LifeDeliveryOutbox>> {
            let mut state = self.inner.lock().expect("fake store lock");
            state.turns.push(turn.clone());
            let cross_bindings: Vec<LifeTransportBinding> = state
                .bindings
                .iter()
                .filter(|b| b.enabled && b.transport_id != turn.source_transport)
                .cloned()
                .collect();
            let mut deliveries = Vec::with_capacity(cross_bindings.len());
            for binding in cross_bindings {
                let delivery = LifeDeliveryOutbox {
                    delivery_id: crate::domain::DeliveryId::new_v4(),
                    turn_id: turn.turn_id,
                    binding_id: binding.binding_id,
                    principal_user_id: binding.principal_user_id,
                    transport_id: binding.transport_id.clone(),
                    delivery_address: binding.delivery_address.clone(),
                    status: crate::domain::LifeDeliveryStatus::Queued,
                    attempt_count: 0,
                    claimed_by: None,
                    claimed_at: None,
                    claim_expires_at: None,
                    next_attempt_at: now,
                    last_error: None,
                    created_at: now,
                    updated_at: now,
                };
                state.deliveries.push(delivery.clone());
                deliveries.push(delivery);
            }
            Ok(deliveries)
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
        inbound_address: Value,
        content: &str,
    ) -> LifeInputSubmission {
        LifeInputSubmission {
            transport_id: transport(transport_id),
            inbound_address,
            source_ref: Some("test-ref".to_owned()),
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
