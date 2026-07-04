//! Storage repository contracts.

use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;

use crate::domain::{
    ClaimedLifeDelivery, DeliveryId, LifeDeliveryOutbox, LifeEvent, LifeIdentityLink, LifeInput,
    LifePrincipal, LifeRun, LifeRunStatus, LifeTransportBinding, LifeTransportId, LifeTurn,
    PrincipalUserId, ProviderSubject, RunId, TimestampMillis,
};

/// Durable running-run lease duration.
///
/// Claims and heartbeats extend a run by this amount from the storage-observed
/// timestamp. Expired leases are reaped before any new claim for the same
/// principal, so a crashed worker cannot block the queue indefinitely.
pub const LIFE_RUN_LEASE_MILLIS: i64 = 15 * 60 * 1000;

/// Delivery claim visibility timeout.
///
/// Expired claimed rows are eligible for another worker claim, so a delivery
/// worker crash cannot permanently strand assistant responses.
pub const LIFE_DELIVERY_CLAIM_MILLIS: i64 = 5 * 60 * 1000;

/// Result alias for life storage operations.
pub type LifeStorageResult<T> = Result<T, LifeStorageError>;

/// Storage-layer errors for life repositories.
#[derive(Debug, Error)]
pub enum LifeStorageError {
    /// Database operation failed.
    #[error("life storage database error: {0}")]
    Database(String),
    /// Migration discovery or execution failed.
    #[error("life storage migration error: {0}")]
    Migration(String),
    /// Domain validation failed while mapping storage rows.
    #[error(transparent)]
    Domain(#[from] crate::errors::LifeDomainError),
    /// JSON serialization failed.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// A repository method was called with an invalid domain operation.
    #[error("invalid life storage operation: {0}")]
    InvalidOperation(String),
    /// A provider subject is already linked to another principal.
    #[error("life identity link conflict for {transport_id}:{provider_subject}")]
    IdentityLinkConflict {
        /// Transport namespace.
        transport_id: LifeTransportId,
        /// Transport-local subject.
        provider_subject: ProviderSubject,
    },
    /// A closed enum contained an unknown stored value.
    #[error("unknown life storage enum {type_name} value '{value}'")]
    UnknownEnumValue {
        /// Enum type name.
        type_name: &'static str,
        /// Stored value.
        value: String,
    },
    /// A paging cursor was malformed or could not be parsed.
    #[error("invalid life paging cursor: {0}")]
    InvalidCursor(String),
}

/// Claimed input plus the persisted run created for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedLifeInputRun {
    /// Claimed queue input.
    pub input: LifeInput,
    /// Persisted running run.
    pub run: LifeRun,
    /// User turn content loaded from `life_turns` at claim time.
    /// Avoids a separate round-trip when the executor needs the user message.
    pub user_content: String,
}

/// Durable cancellation transition result for a life run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelLifeRunOutcome {
    /// The running run was transitioned to `cancelled` by this call.
    Cancelled,
    /// The run already existed but was no longer running.
    AlreadyTerminal {
        /// Current terminal status.
        status: LifeRunStatus,
    },
    /// No run with this id belongs to the principal.
    NotFound,
}

/// Async Postgres repository boundary for source-of-truth life state.
#[async_trait]
pub trait LifeStorageRepository: Send + Sync {
    /// Upserts the canonical principal envelope and ensures the referenced `users` row exists.
    async fn upsert_principal(&self, principal: &LifePrincipal) -> LifeStorageResult<()>;

    /// Loads the canonical principal envelope.
    async fn principal(
        &self,
        principal_user_id: PrincipalUserId,
    ) -> LifeStorageResult<Option<LifePrincipal>>;

    /// Stores a transport-subject link to a canonical principal.
    async fn link_identity(&self, link: &LifeIdentityLink) -> LifeStorageResult<()>;

    /// Resolves a transport-local subject to a canonical principal.
    async fn resolve_identity(
        &self,
        transport_id: &LifeTransportId,
        provider_subject: &ProviderSubject,
    ) -> LifeStorageResult<Option<PrincipalUserId>>;

    /// Stores an owner-approved transport binding.
    async fn upsert_transport_binding(
        &self,
        binding: &LifeTransportBinding,
    ) -> LifeStorageResult<()>;

    /// Resolves an enabled owner-approved transport binding by inbound address.
    async fn resolve_transport_binding(
        &self,
        transport_id: &LifeTransportId,
        inbound_address: &Value,
    ) -> LifeStorageResult<Option<LifeTransportBinding>>;

    /// Appends a canonical transcript turn.
    async fn append_turn(&self, turn: &LifeTurn) -> LifeStorageResult<()>;

    /// Atomically appends an assistant turn and enqueues delivery rows for all enabled bindings.
    async fn append_assistant_turn_and_enqueue_deliveries(
        &self,
        turn: &LifeTurn,
        now: TimestampMillis,
    ) -> LifeStorageResult<Vec<LifeDeliveryOutbox>>;

    /// Atomically appends a user turn and enqueues delivery rows for enabled
    /// bindings **other than the source transport** — the user already sees
    /// their own message on the transport they typed it from.
    async fn append_user_turn_and_enqueue_deliveries(
        &self,
        turn: &LifeTurn,
        now: TimestampMillis,
    ) -> LifeStorageResult<Vec<LifeDeliveryOutbox>>;

    /// Enqueues a canonical user input for future worker processing.
    async fn enqueue_input(&self, input: &LifeInput) -> LifeStorageResult<()>;

    /// Synchronously persists the stable life hot-memory checkpoint.
    async fn save_life_memory_checkpoint(
        &self,
        principal_user_id: PrincipalUserId,
        context_key: &str,
        flow_id: &str,
        memory: &Value,
        schema_version: i32,
        now: TimestampMillis,
    ) -> LifeStorageResult<()>;

    /// Atomically claims a queued input and starts a running life run.
    async fn claim_input_and_start_run(
        &self,
        principal_user_id: PrincipalUserId,
        input_id: crate::domain::InputId,
        run_id: RunId,
        worker_id: &str,
        now: TimestampMillis,
    ) -> LifeStorageResult<Option<ClaimedLifeInputRun>>;

    /// Marks a claimed input as consumed after it has been incorporated into a run.
    async fn mark_input_consumed(
        &self,
        input_id: crate::domain::InputId,
        now: TimestampMillis,
    ) -> LifeStorageResult<()>;

    /// Atomically claims the oldest queued input for a principal and starts a running life run.
    async fn claim_next_queued_input_and_start_run(
        &self,
        principal_user_id: PrincipalUserId,
        run_id: RunId,
        worker_id: &str,
        now: TimestampMillis,
    ) -> LifeStorageResult<Option<ClaimedLifeInputRun>>;

    /// Returns distinct principals that have at least one queued input.
    /// Used by the background polling worker to discover inputs submitted
    /// by transports other than the web UI (e.g. Telegram).
    async fn find_principals_with_queued_inputs(&self) -> LifeStorageResult<Vec<PrincipalUserId>>;

    /// Extends the lease for a running run owned by `worker_id`.
    async fn heartbeat_run_lease(
        &self,
        run_id: RunId,
        worker_id: &str,
        now: TimestampMillis,
    ) -> LifeStorageResult<bool>;

    /// Finds the currently running run for a principal, if any.
    async fn find_active_run(
        &self,
        principal_user_id: PrincipalUserId,
    ) -> LifeStorageResult<Option<LifeRun>>;

    /// Links a transcript turn to a run by setting `life_turns.run_id`.
    async fn link_turn_to_run(
        &self,
        turn_id: crate::domain::TurnId,
        run_id: RunId,
    ) -> LifeStorageResult<()>;

    /// Appends a transport-neutral run event.
    async fn append_event(&self, event: &LifeEvent) -> LifeStorageResult<()>;

    /// Returns the next event sequence number for a run.
    async fn next_event_seq(&self, run_id: RunId) -> LifeStorageResult<i64>;

    /// Marks a running run as completed.
    ///
    /// Returns `true` when this call won the terminal transition. Returns
    /// `false` when the run was already moved out of `running` by another
    /// owner-visible transition such as cancellation.
    async fn complete_run(
        &self,
        run_id: RunId,
        finished_at: TimestampMillis,
        last_checkpoint_at: TimestampMillis,
    ) -> LifeStorageResult<bool>;

    /// Marks a running run as failed.
    ///
    /// Returns `true` when this call won the terminal transition. Returns
    /// `false` when the run was already moved out of `running`.
    async fn fail_run(
        &self,
        run_id: RunId,
        finished_at: TimestampMillis,
        error_text: &str,
    ) -> LifeStorageResult<bool>;

    /// Cancels a running run owned by a principal.
    ///
    /// Cancellation is durable and transport-neutral. It also marks claimed
    /// inputs whose originating turns are linked to the run as consumed, so a
    /// user-requested stop cannot strand the permanent input queue.
    async fn cancel_run(
        &self,
        principal_user_id: PrincipalUserId,
        run_id: RunId,
        cancelled_at: TimestampMillis,
    ) -> LifeStorageResult<CancelLifeRunOutcome>;

    /// Loads the current status for a principal-owned run.
    async fn run_status(
        &self,
        principal_user_id: PrincipalUserId,
        run_id: RunId,
    ) -> LifeStorageResult<Option<LifeRunStatus>>;

    /// Claims the next due delivery row for a transport.
    async fn claim_next_delivery(
        &self,
        transport_id: &LifeTransportId,
        worker_id: &str,
        now: TimestampMillis,
    ) -> LifeStorageResult<Option<ClaimedLifeDelivery>>;

    /// Marks a claimed delivery row delivered.
    async fn mark_delivery_delivered(
        &self,
        delivery_id: DeliveryId,
        now: TimestampMillis,
    ) -> LifeStorageResult<()>;

    /// Marks a claimed delivery row retryable after a later attempt time.
    async fn mark_delivery_failed(
        &self,
        delivery_id: DeliveryId,
        error_text: &str,
        next_attempt_at: TimestampMillis,
        now: TimestampMillis,
    ) -> LifeStorageResult<()>;

    /// Marks a claimed delivery row permanently dead.
    async fn mark_delivery_dead(
        &self,
        delivery_id: DeliveryId,
        error_text: &str,
        now: TimestampMillis,
    ) -> LifeStorageResult<()>;
}
