//! Storage repository contracts.

use async_trait::async_trait;
use thiserror::Error;

use crate::domain::{
    ActiveMemoryGeneration, LifeFrictionPattern, LifeIdentityLink, LifeIdentityProvider, LifeInput,
    LifeMemoryGeneration, LifeMemoryItem, LifePrincipal, LifeSupportProtocol, LifeTaskState,
    LifeTurn, MemoryGenerationId, MemoryScope, PrincipalUserId, ProviderSubject, TimestampMillis,
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
    /// A generation operation targeted a missing or wrong-principal generation.
    #[error(
        "life memory generation {generation_id} does not belong to principal {principal_user_id}"
    )]
    GenerationNotOwned {
        /// Expected principal.
        principal_user_id: PrincipalUserId,
        /// Target generation.
        generation_id: MemoryGenerationId,
    },
    /// A provider subject is already linked to another principal.
    #[error("life identity link conflict for {provider}:{provider_subject}")]
    IdentityLinkConflict {
        /// Provider namespace.
        provider: LifeIdentityProvider,
        /// Provider-local subject.
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
}

/// Minimal repository boundary shared by future storage services.
pub trait LifeGenerationReader {
    /// Returns the active memory generation for a principal.
    fn active_generation(
        &self,
        principal_user_id: PrincipalUserId,
    ) -> Option<ActiveMemoryGeneration>;

    /// Converts a principal id to the mandatory active memory scope.
    fn active_memory_scope(&self, principal_user_id: PrincipalUserId) -> Option<MemoryScope> {
        self.active_generation(principal_user_id)
            .map(|active| active.scope)
    }
}

/// Async Postgres repository boundary for source-of-truth life state.
#[async_trait]
pub trait LifeStorageRepository: Send + Sync {
    /// Upserts the canonical principal envelope and ensures the referenced `users` row exists.
    async fn upsert_principal(&self, principal: &LifePrincipal) -> LifeStorageResult<()>;

    /// Stores a provider-subject link to a canonical principal.
    async fn link_identity(&self, link: &LifeIdentityLink) -> LifeStorageResult<()>;

    /// Resolves a provider-local subject to a canonical principal.
    async fn resolve_identity(
        &self,
        provider: LifeIdentityProvider,
        provider_subject: &ProviderSubject,
    ) -> LifeStorageResult<Option<PrincipalUserId>>;

    /// Inserts a memory generation row.
    async fn insert_memory_generation(
        &self,
        generation: &LifeMemoryGeneration,
    ) -> LifeStorageResult<()>;

    /// Atomically activates a generation and archives the previously active generation, if any.
    async fn activate_memory_generation(
        &self,
        principal_user_id: PrincipalUserId,
        memory_generation_id: MemoryGenerationId,
        activated_at: TimestampMillis,
        activation_reason: &str,
    ) -> LifeStorageResult<ActiveMemoryGeneration>;

    /// Returns the active generation pointer for a principal.
    async fn active_generation(
        &self,
        principal_user_id: PrincipalUserId,
    ) -> LifeStorageResult<Option<ActiveMemoryGeneration>>;

    /// Returns the next generation number for a principal.
    async fn next_memory_generation_number(
        &self,
        principal_user_id: PrincipalUserId,
    ) -> LifeStorageResult<i64>;

    /// Appends a canonical transcript turn.
    async fn append_turn(&self, turn: &LifeTurn) -> LifeStorageResult<()>;

    /// Enqueues a canonical user input for future worker processing.
    async fn enqueue_input(&self, input: &LifeInput) -> LifeStorageResult<()>;

    /// Inserts or updates a canonical memory item.
    async fn upsert_memory_item(&self, item: &LifeMemoryItem) -> LifeStorageResult<()>;

    /// Lists active canonical memory items from the explicit active scope.
    async fn active_memory_items(
        &self,
        scope: MemoryScope,
    ) -> LifeStorageResult<Vec<LifeMemoryItem>>;

    /// Inserts or updates a task resume packet.
    async fn upsert_task_state(&self, task_state: &LifeTaskState) -> LifeStorageResult<()>;

    /// Lists active task resume packets from the explicit active scope.
    async fn active_task_states(&self, scope: MemoryScope)
    -> LifeStorageResult<Vec<LifeTaskState>>;

    /// Inserts or updates a friction pattern.
    async fn upsert_friction_pattern(&self, pattern: &LifeFrictionPattern)
    -> LifeStorageResult<()>;

    /// Lists active friction patterns from the explicit active scope.
    async fn active_friction_patterns(
        &self,
        scope: MemoryScope,
    ) -> LifeStorageResult<Vec<LifeFrictionPattern>>;

    /// Inserts or updates a support protocol.
    async fn upsert_support_protocol(
        &self,
        protocol: &LifeSupportProtocol,
    ) -> LifeStorageResult<()>;

    /// Lists active support protocols from the explicit active scope.
    async fn active_support_protocols(
        &self,
        scope: MemoryScope,
    ) -> LifeStorageResult<Vec<LifeSupportProtocol>>;
}
