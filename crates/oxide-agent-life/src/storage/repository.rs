//! Storage repository contracts.

use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;

use crate::domain::{
    LifeEvent, LifeIdentityLink, LifeInput, LifePrincipal, LifeRun, LifeTransportId, LifeTurn,
    PrincipalUserId, ProviderSubject, RunId, TimestampMillis,
};

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

    /// Appends a canonical transcript turn.
    async fn append_turn(&self, turn: &LifeTurn) -> LifeStorageResult<()>;

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

    /// Drains queued inputs for a principal at a runtime safe boundary.
    async fn drain_queued_inputs_for_run(
        &self,
        principal_user_id: PrincipalUserId,
        worker_id: &str,
        now: TimestampMillis,
    ) -> LifeStorageResult<Vec<LifeInput>>;

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
    async fn complete_run(
        &self,
        run_id: RunId,
        finished_at: TimestampMillis,
        last_checkpoint_at: TimestampMillis,
    ) -> LifeStorageResult<()>;

    /// Marks a running run as failed.
    async fn fail_run(
        &self,
        run_id: RunId,
        finished_at: TimestampMillis,
        error_text: &str,
    ) -> LifeStorageResult<()>;
}
